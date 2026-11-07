//! The lax reading of one JSON request body, field by field.
//!
//! A body is validated in one pass and every field it gets wrong is answered at
//! once, in the spellings this API accepts: `"2"` and `2.0` for an integer,
//! `"yes"` and `1` for a boolean, a date or an epoch number for a timestamp.
//! `serde` stops at the first failure and takes one spelling per type, so the
//! reader here applies the coercion itself and builds the issue each refused
//! value answers with.

// Rust guideline compliant 2026-07-21

use chrono::{DateTime, Datelike as _, NaiveDate, NaiveDateTime, Utc};
use serde_json::{Map, Number, Value};

use crate::dto::lax_bool::{self, Refusal};
use crate::dto::{LocSegment, ValidationIssue};
use crate::py::strip_wire_whitespace;

/// `2^63`, the first whole number above `i64::MAX`.
///
/// It is a power of two, so the float holds it exactly; `i64::MAX` itself is not
/// a float, which is why the bound is this one. A float at or past it in
/// magnitude names no `i64`, and such a field is refused as `int_parsing_size`;
/// the comparison is on the float because the cast to an integer saturates
/// instead.
#[expect(
    clippy::lossy_float_literal,
    reason = "2^63 is exact in a float; the lint counts digits rather than the value"
)]
const INTEGER_RANGE: f64 = 9_223_372_036_854_775_808.0;

/// The milliseconds in one second, which a millisecond count is split by.
const MILLIS_PER_SECOND: i64 = 1_000;

/// [`MILLIS_PER_SECOND`] as the float the magnitude is compared in.
const MILLIS_PER_SECOND_F: f64 = 1_000.0;

/// The microseconds in one second, the resolution a datetime keeps.
///
/// A datetime holds microseconds, so a fraction is rounded to them:
/// `1767225600000.123456789` is `.000124`, not `.000123`.
const MICROS_PER_SECOND: i64 = 1_000_000;

/// [`MICROS_PER_SECOND`] as the float a fraction below a second is scaled by.
const MICROS_PER_SECOND_F: f64 = 1_000_000.0;

/// The nanoseconds in one microsecond, which `chrono` counts a fraction in.
const NANOS_PER_MICROSECOND: u32 = 1_000;

/// The magnitude past which a number names milliseconds rather than seconds.
///
/// A magnitude past this bound is a millisecond count, which is the shape
/// `Date.now()` gives: `1767225600000` is 2026-01-01, not the year 57971.
const EPOCH_MILLISECONDS: f64 = 20_000_000_000.0;

/// The last year a datetime names.
const LAST_YEAR: i32 = 9999;

/// The sentence a timestamp past year 9999 is refused with.
const AFTER_LAST_YEAR: &str = "dates after 9999 are not supported as unix timestamps";

/// The sentence a timestamp before year 0 is refused with.
const BEFORE_FIRST_YEAR: &str = "dates before 0000 are not supported as unix timestamps";

/// The sentence the year between the first years is refused with.
const YEAR_ZERO: &str = "year 0 is out of range";

/// The message a string that spells no integer is refused with, which the
/// request extractors report as `int_parsing`.
pub const INT_PARSING_MESSAGE: &str =
    "Input should be a valid integer, unable to parse string as an integer";

/// One JSON object being read, and where its fields sit in the request body.
///
/// `at` is the `loc` prefix every field of the object extends, so a Job's fields
/// report `["body", "jobs", 0, "name"]` without the caller repeating the prefix.
#[derive(Debug, Clone)]
pub struct Fields<'a> {
    /// The object the request body held.
    object: &'a Map<String, Value>,
    /// The `loc` prefix the object's fields sit under.
    at: Vec<LocSegment>,
}

impl<'a> Fields<'a> {
    /// Reads the fields of `object`, whose own position in the body is `at`.
    #[must_use]
    pub fn new(object: &'a Map<String, Value>, at: Vec<LocSegment>) -> Self {
        Self { object, at }
    }

    /// The value of `field`, when the body carries one.
    #[must_use]
    pub fn field(&self, field: &str) -> Option<&'a Value> {
        self.object.get(field)
    }

    /// The value of `field`, recording the `missing` issue when absent.
    pub fn required(&self, field: &str, issues: &mut Vec<ValidationIssue>) -> Option<&'a Value> {
        let value = self.field(field);
        if value.is_none() {
            issues.push(ValidationIssue::missing(self.loc(field)));
        }
        value
    }

    /// Where `field` of this object sits.
    #[must_use]
    pub fn loc(&self, field: &str) -> Vec<LocSegment> {
        let mut loc = self.at.clone();
        loc.push(LocSegment::Field(field.to_owned()));
        loc
    }

    /// Where the item at `index` of the list `field` sits.
    #[must_use]
    pub fn item_loc(&self, field: &str, index: usize) -> Vec<LocSegment> {
        let mut loc = self.loc(field);
        loc.push(LocSegment::Index(index));
        loc
    }
}

/// The string `value` holds, when it holds one.
///
/// Any other JSON type is refused with `string_type`: a number is never coerced
/// into text, so a body that sends `{"name": 5}` is told what is wrong instead of
/// having a name invented for it.
///
/// # Errors
///
/// Returns the `string_type` issue for a value of another type.
pub fn string(value: &Value, loc: Vec<LocSegment>) -> Result<String, Box<ValidationIssue>> {
    match value {
        Value::String(text) => Ok(text.clone()),
        _ => Err(Box::new(ValidationIssue::string_type(loc, value.clone()))),
    }
}

/// The integer `value` names, in any spelling this API accepts.
///
/// A JSON `true` or `false` is `1` or `0`; a whole float is its integer, and a
/// float with a fractional part is `int_from_float`; a string is read as a
/// decimal number, with underscores allowed between digits and a fractional part
/// only when it is all zeros. A value of another type is `int_type`, and a
/// number past `i64` is `int_parsing_size`.
///
/// # Errors
///
/// Returns the issue a value that is no integer answers with: `int_type`,
/// `int_from_float`, `int_parsing` or `int_parsing_size`.
pub fn integer(value: &Value, loc: Vec<LocSegment>) -> Result<i64, Box<ValidationIssue>> {
    match value {
        Value::Bool(flag) => Ok(i64::from(*flag)),
        Value::Number(number) => integer_from_number(number, value, loc),
        Value::String(text) => {
            text_int(text).ok_or_else(|| Box::new(ValidationIssue::int_parsing(loc, value.clone())))
        }
        _ => Err(Box::new(ValidationIssue::int_type(loc, value.clone()))),
    }
}

/// The integer one JSON number names.
fn integer_from_number(
    number: &Number,
    value: &Value,
    loc: Vec<LocSegment>,
) -> Result<i64, Box<ValidationIssue>> {
    if let Some(integer) = number.as_i64() {
        return Ok(integer);
    }
    if number.as_u64().is_some() {
        return Err(Box::new(ValidationIssue::int_parsing_size(
            loc,
            value.clone(),
        )));
    }
    let Some(float) = number.as_f64() else {
        return Err(Box::new(ValidationIssue::int_type(loc, value.clone())));
    };
    if !float.is_finite() {
        return Err(Box::new(ValidationIssue::int_type(loc, value.clone())));
    }
    if float.fract() != 0.0 {
        return Err(Box::new(ValidationIssue::int_from_float(
            loc,
            value.clone(),
        )));
    }
    if float.abs() >= INTEGER_RANGE {
        return Err(Box::new(ValidationIssue::int_parsing_size(
            loc,
            value.clone(),
        )));
    }
    Ok(whole_integer(float))
}

/// The integer one whole `f64` names.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the caller checked that the float is whole and inside i64's range"
)]
fn whole_integer(float: f64) -> i64 {
    float as i64
}

/// The integer a string spells, in the form a query or a body may send.
///
/// The spelling is an optional sign, then ASCII digits with single underscores
/// allowed between them, then either nothing or a `.` and a run of digits that
/// are all zeros: `"1_0"` is 10, `"10.000"` is 10, and `"1e3"`, `"5."` and
/// `"1_0.5"` are no integer. A run past `i64`'s range is no integer either, even
/// though a wider integer type could hold it: a value past the column's range is
/// refused rather than truncated.
#[must_use]
pub fn text_int(text: &str) -> Option<i64> {
    let text = strip_wire_whitespace(text);
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (whole, fraction) = match rest.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (rest, None),
    };
    let mut value: i64 = 0;
    for digit in digits_of(whole)? {
        value = value.checked_mul(10)?.checked_add(i64::from(digit))?;
    }
    if let Some(fraction) = fraction
        && (fraction.is_empty() || !fraction.bytes().all(|byte| byte == b'0'))
    {
        return None;
    }
    Some(if negative { -value } else { value })
}

/// The values of the ASCII digits of `text`, which may be separated by single
/// underscores.
///
/// An underscore must sit between two digits, so `"_1"`, `"1_"` and `"1__0"` are
/// no number.
fn digits_of(text: &str) -> Option<impl Iterator<Item = u8> + '_> {
    if text.is_empty() || text.starts_with('_') || text.ends_with('_') || text.contains("__") {
        return None;
    }
    if !text
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'_')
    {
        return None;
    }
    Some(
        text.bytes()
            .filter(|byte| *byte != b'_')
            .map(|byte| byte - b'0'),
    )
}

/// The boolean `value` names, in any spelling [`lax_bool`] accepts.
///
/// # Errors
///
/// Returns `bool_type` for a value of another JSON type and `bool_parsing` for a
/// number or a string it could not read.
pub fn boolean(value: &Value, loc: Vec<LocSegment>) -> Result<bool, Box<ValidationIssue>> {
    match lax_bool::read(value) {
        Ok(flag) => Ok(flag),
        Err(Refusal::WrongType) => Err(Box::new(ValidationIssue::bool_type(loc, value.clone()))),
        Err(Refusal::Unparsable) => {
            Err(Box::new(ValidationIssue::bool_parsing(loc, value.clone())))
        }
    }
}

/// The instant `value` names, in any spelling this API accepts.
///
/// A string is any ISO 8601 form the reader takes: a datetime with or without
/// seconds, a space or `T` between the date and the time, a `.` or `,` before a
/// fraction, and `Z`, `z` or an offset with or without its colon; or a date
/// alone, which is that day's midnight. A string holding nothing but a decimal
/// number, and a number of any shape, are both a Unix timestamp: seconds when
/// its magnitude is at most `2e10`, and milliseconds past that, which is the
/// shape `Date.now()` gives. A naive timestamp is read as UTC.
///
/// # Errors
///
/// Returns `datetime_type` for a value of another type,
/// `datetime_from_date_parsing` for a string that names no instant, and
/// `datetime_parsing` for a timestamp outside the years 1 to 9999, which no
/// datetime holds.
pub fn timestamp(
    value: &Value,
    loc: Vec<LocSegment>,
) -> Result<DateTime<Utc>, Box<ValidationIssue>> {
    match value {
        Value::String(text) => parse_timestamp(text).map_err(|refusal| {
            Box::new(match refusal {
                TimestampRefusal::NotADate => ValidationIssue::datetime_from_date_parsing(
                    loc,
                    value.clone(),
                    timestamp_detail(text),
                ),
                TimestampRefusal::OutOfYears(sentence) => {
                    ValidationIssue::datetime_from_date_parsing(loc, value.clone(), sentence)
                }
            })
        }),
        Value::Number(number) => epoch_instant(number).map_err(|sentence| {
            Box::new(ValidationIssue::datetime_parsing(
                loc,
                value.clone(),
                sentence,
            ))
        }),
        _ => Err(Box::new(ValidationIssue::datetime_type(loc, value.clone()))),
    }
}

/// Why a string names no instant.
enum TimestampRefusal {
    /// The text is no date and no number.
    NotADate,
    /// The text is a number, and the instant it names is outside the years a
    /// datetime holds, in the sentence that refuses the instant's side.
    OutOfYears(&'static str),
}

/// The instant `text` names, when it names one.
fn parse_timestamp(text: &str) -> Result<DateTime<Utc>, TimestampRefusal> {
    if let Some(seconds) = numeric_seconds(text) {
        return epoch_of(seconds).map_err(TimestampRefusal::OutOfYears);
    }
    // `chrono` reads a date whose month and day are not zero-padded, and this
    // reader does not: `2026-1-1` is refused as too short.
    if !starts_like_a_date(text) {
        return Err(TimestampRefusal::NotADate);
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
        return Ok(parsed.with_timezone(&Utc));
    }
    let normalised = normalised_timestamp(text);
    if let Ok(parsed) = DateTime::parse_from_rfc3339(&normalised) {
        return Ok(parsed.with_timezone(&Utc));
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(&normalised, format) {
            return Ok(DateTime::from_naive_utc_and_offset(naive, Utc));
        }
    }
    NaiveDate::parse_from_str(&normalised, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|naive| DateTime::from_naive_utc_and_offset(naive, Utc))
        .ok_or(TimestampRefusal::NotADate)
}

/// The number `text` spells, when it is nothing but a plain decimal number.
///
/// A sign, digits, and a fraction when it has one: `"1767225600"`, `"0"`,
/// `"-5"` and `"1767225600.5"` are numbers, while `"1e3"`, `"1_000"`,
/// `" 1767225600"` and `"5.5.5"` are not, so they stay strings that name no
/// date and are refused as one.
fn numeric_seconds(text: &str) -> Option<f64> {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (unsigned, None),
    };
    let digits = |run: &str| run.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(whole) || !fraction.is_none_or(digits) {
        return None;
    }
    // A lone sign or a lone dot names nothing.
    if whole.is_empty() && fraction.is_none_or(str::is_empty) {
        return None;
    }
    text.parse().ok()
}

/// Whether `text` starts with a zero-padded ISO date: `YYYY-MM-DD`.
fn starts_like_a_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        bytes
            .get(range)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
    };
    digits(0..4)
        && bytes.get(4) == Some(&b'-')
        && digits(5..7)
        && bytes.get(7) == Some(&b'-')
        && digits(8..10)
}

/// `text` with the separators this API accepts put in the shape `chrono` reads.
///
/// A comma before the fraction becomes a dot, a lowercase `z` becomes `Z`, and an
/// offset written without its colon (`+0200`) gets one.
fn normalised_timestamp(text: &str) -> String {
    let text = text.replace(',', ".");
    let text = match text.strip_suffix('z') {
        Some(rest) => format!("{rest}Z"),
        None => text,
    };
    let offset_at = text.len().saturating_sub(5);
    let with_colon = match text.get(offset_at..) {
        Some(tail) if tail.len() == 5 => {
            let sign = tail.chars().next();
            let digits = tail.get(1..).unwrap_or_default();
            match sign {
                Some('+' | '-') if digits.chars().all(|char| char.is_ascii_digit()) => {
                    let head = text.get(..offset_at).unwrap_or_default();
                    let (hours, minutes) = digits.split_at(2);
                    Some(format!(
                        "{head}{}{hours}:{minutes}",
                        sign.unwrap_or_default()
                    ))
                }
                _ => None,
            }
        }
        _ => None,
    };
    with_colon.unwrap_or(text)
}

/// The wording a string that names no instant is refused with.
fn timestamp_detail(text: &str) -> &'static str {
    if text.chars().count() < 10 {
        "input is too short"
    } else {
        "invalid date"
    }
}

/// The instant one Unix timestamp names.
fn epoch_instant(number: &Number) -> Result<DateTime<Utc>, &'static str> {
    match number.as_f64() {
        Some(value) => epoch_of(value),
        None => Err(AFTER_LAST_YEAR),
    }
}

/// The instant a Unix timestamp names, when a datetime holds it.
///
/// A magnitude past [`EPOCH_MILLISECONDS`] is a millisecond count, which is the
/// shape `Date.now()` gives; anything smaller is seconds. The whole units are
/// split from the fraction before either is converted, so an integer keeps the
/// instant it names to the microsecond:
/// dividing `253402300799999` by 1000 as a float would round its last
/// microsecond away. An instant outside the years 1 to 9999 names no datetime,
/// and is refused by the sentence that fits its side.
fn epoch_of(value: f64) -> Result<DateTime<Utc>, &'static str> {
    let units = value.trunc();
    if units.abs() >= INTEGER_RANGE {
        return Err(side_of(units));
    }
    let whole = whole_units(units);
    let fraction = value - units;
    let (seconds, micros) = if units.abs() > EPOCH_MILLISECONDS {
        let millis = whole.rem_euclid(MILLIS_PER_SECOND);
        (
            whole.div_euclid(MILLIS_PER_SECOND),
            millis * 1_000 + round_micros(fraction * MILLIS_PER_SECOND_F),
        )
    } else {
        (whole, round_micros(fraction * MICROS_PER_SECOND_F))
    };
    // A fraction that rounds up to a whole second carries into it, so the
    // microseconds below the second always stay in `0..MICROS_PER_SECOND`. The
    // wide integer is what a second of year 5138 needs to hold its micros.
    let scale = i128::from(MICROS_PER_SECOND);
    let micros = i128::from(seconds) * scale + i128::from(micros);
    let seconds = micros.div_euclid(scale);
    let below = micros.rem_euclid(scale);
    let whole = i64::try_from(seconds).map_err(|_overflow| side_of(units))?;
    let below = u32::try_from(below).unwrap_or_default();
    let instant = DateTime::from_timestamp(whole, below * NANOS_PER_MICROSECOND)
        .ok_or_else(|| side_of(units))?;
    match instant.year() {
        year if year > LAST_YEAR => Err(AFTER_LAST_YEAR),
        0 => Err(YEAR_ZERO),
        year if year < 0 => Err(BEFORE_FIRST_YEAR),
        _ => Ok(instant),
    }
}

/// `seconds` as the whole units a timestamp is counted in.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the caller checked the magnitude is inside i64's range"
)]
fn whole_units(seconds: f64) -> i64 {
    seconds as i64
}

/// The microseconds `micros` names, rounded to the nearest one.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the caller scaled a fraction below one second, so it is under a million"
)]
fn round_micros(micros: f64) -> i64 {
    micros.round() as i64
}

/// The sentence a timestamp at `seconds` is refused with: one side of the range
/// or the other, which is all that separates the two.
fn side_of(seconds: f64) -> &'static str {
    if seconds.is_sign_negative() {
        BEFORE_FIRST_YEAR
    } else {
        AFTER_LAST_YEAR
    }
}

/// The items of the JSON list `value` holds.
///
/// # Errors
///
/// Returns the `list_type` issue for a value of another JSON type.
pub fn list_of(value: &Value, loc: Vec<LocSegment>) -> Result<&[Value], Box<ValidationIssue>> {
    match value {
        Value::Array(items) => Ok(items),
        _ => Err(Box::new(ValidationIssue::list_type(loc, value.clone()))),
    }
}

/// The option of `options` that `value` names.
///
/// # Errors
///
/// Returns the `literal_error` issue, whose `expected` lists every option, for a
/// value that names none.
pub fn literal<'v>(
    value: &'v Value,
    options: &[&str],
    loc: Vec<LocSegment>,
) -> Result<&'v str, Box<ValidationIssue>> {
    match value.as_str() {
        Some(text) if options.contains(&text) => Ok(text),
        _ => Err(Box::new(ValidationIssue::literal_error(
            loc,
            value.clone(),
            &expected_list(options),
        ))),
    }
}

/// `options` as the 422 spells them: each quoted, with `or` before the last.
#[must_use]
pub fn expected_list(options: &[&str]) -> String {
    let quoted: Vec<String> = options.iter().map(|option| format!("'{option}'")).collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// Records the issue a text holding a NUL character gets.
///
/// A PostgreSQL `text` column cannot store a NUL, so every text field of a
/// request refuses one here rather than letting it reach a statement.
pub fn refuse_nul(text: &str, loc: Vec<LocSegment>, issues: &mut Vec<ValidationIssue>) {
    if text.contains('\0') {
        issues.push(ValidationIssue::nul_character(loc, Value::from(text)));
    }
}
