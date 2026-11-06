//! Pydantic's lax reading of one JSON request body, field by field.
//!
//! The reference validated a body in one pass and answered every field it
//! refused, accepting the spellings Pydantic 2's lax mode takes: `"2"` and
//! `2.0` for an integer, `"yes"` and `1` for a boolean, a date or an epoch
//! number for a timestamp. `serde` stops at the first failure and takes one
//! spelling per type, so a body is read here with the coercion Pydantic applied
//! and the issue it reported for each value it refused.

// Rust guideline compliant 2026-07-21

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde_json::{Map, Number, Value};

use crate::dto::lax_bool::{self, Refusal};
use crate::dto::{LocSegment, ValidationIssue};
use crate::py::py_strip;

/// `2^63`, the first whole number above `i64::MAX`. Pydantic's float-to-integer
/// conversion accepts a float at or above the negative bound and below this one,
/// and reports `int_parsing_size` outside that range.
const INTEGER_RANGE: f64 = 9_223_372_036_854_776_000_000_000_000_000.0;

/// The milliseconds in one second, as Pydantic's timestamps count them.
const NANOS_PER_SECOND: f64 = 1_000_000_000.0;

/// Pydantic's message for a string it could not read as an integer, which the
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

    /// The value of `field`, recording Pydantic's `missing` issue when absent.
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

/// The string `value` holds, as Pydantic read a `str`.
///
/// Pydantic 2 refuses every other JSON type for a string field, reporting
/// `string_type`; it never coerces a number into text, which is the change from
/// Pydantic 1 that this port keeps.
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

/// The integer `value` names, as Pydantic's lax mode read one.
///
/// A JSON `true` or `false` is `1` or `0`; a whole float is its integer, and a
/// float with a fractional part is `int_from_float`; a string is read as a
/// decimal number, with underscores allowed between digits and a fractional part
/// only when it is all zeros. A value of another type is `int_type`, and a
/// number past `i64` is `int_parsing_size`.
///
/// # Errors
///
/// Returns the issue Pydantic reported for a value it could not read as an
/// integer: `int_type`, `int_from_float`, `int_parsing` or `int_parsing_size`.
pub fn integer(value: &Value, loc: Vec<LocSegment>) -> Result<i64, Box<ValidationIssue>> {
    match value {
        Value::Bool(flag) => Ok(i64::from(*flag)),
        Value::Number(number) => integer_from_number(number, value, loc),
        Value::String(text) => python_int(text)
            .ok_or_else(|| Box::new(ValidationIssue::int_parsing(loc, value.clone()))),
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
    if !(-INTEGER_RANGE..INTEGER_RANGE).contains(&float) {
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

/// The integer a string spells, as Pydantic's lax mode read one.
///
/// The spelling is an optional sign, then ASCII digits with single underscores
/// allowed between them, then either nothing or a `.` and a run of digits that
/// are all zeros: `"1_0"` is 10, `"10.000"` is 10, and `"1e3"`, `"5."` and
/// `"1_0.5"` are no integer. A run past `i64`'s range is no integer here either,
/// although Python's `int` holds it.
#[must_use]
pub fn python_int(text: &str) -> Option<i64> {
    let text = py_strip(text);
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
/// An underscore must sit between two digits, as Python's own literal rules have
/// it, so `"_1"`, `"1_"` and `"1__0"` are no number.
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

/// The boolean `value` names, as Pydantic's lax mode read one.
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

/// The instant `value` names, as Pydantic's lax mode read one.
///
/// A string is any ISO 8601 form Pydantic took: a datetime with or without
/// seconds, a space or `T` between the date and the time, a `.` or `,` before a
/// fraction, and `Z`, `z` or an offset with or without its colon; or a date
/// alone, which is that day's midnight. A number is a Unix timestamp in seconds,
/// with its fraction when it has one. A naive timestamp is read as UTC.
///
/// # Errors
///
/// Returns `datetime_type` for a value of another type and
/// `datetime_from_date_parsing` for a string that names no instant.
pub fn timestamp(
    value: &Value,
    loc: Vec<LocSegment>,
) -> Result<DateTime<Utc>, Box<ValidationIssue>> {
    match value {
        Value::String(text) => parse_timestamp(text).ok_or_else(|| {
            Box::new(ValidationIssue::datetime_from_date_parsing(
                loc,
                value.clone(),
                timestamp_detail(text),
            ))
        }),
        Value::Number(number) => epoch_instant(number)
            .ok_or_else(|| Box::new(ValidationIssue::int_parsing_size(loc, value.clone()))),
        _ => Err(Box::new(ValidationIssue::datetime_type(loc, value.clone()))),
    }
}

/// The instant `text` names, when it names one.
fn parse_timestamp(text: &str) -> Option<DateTime<Utc>> {
    // `chrono` reads a date whose month and day are not zero-padded, and
    // Pydantic does not: it reports `2026-1-1` as too short.
    if !starts_like_a_date(text) {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
        return Some(parsed.with_timezone(&Utc));
    }
    let normalised = normalised_timestamp(text);
    if let Ok(parsed) = DateTime::parse_from_rfc3339(&normalised) {
        return Some(parsed.with_timezone(&Utc));
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(&normalised, format) {
            return Some(DateTime::from_naive_utc_and_offset(naive, Utc));
        }
    }
    NaiveDate::parse_from_str(&normalised, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|naive| DateTime::from_naive_utc_and_offset(naive, Utc))
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

/// `text` with the separators Pydantic also took put in the shape `chrono` reads.
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

/// Pydantic's own wording for a string it could not read as a date or datetime.
fn timestamp_detail(text: &str) -> &'static str {
    if text.chars().count() < 10 {
        "input is too short"
    } else {
        "invalid date"
    }
}

/// The instant one Unix timestamp names, in seconds with its fraction.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the whole seconds are a finite float inside i64's range, which is checked"
)]
#[expect(
    clippy::cast_sign_loss,
    reason = "the nanosecond count is made positive before it is converted"
)]
fn epoch_instant(number: &Number) -> Option<DateTime<Utc>> {
    if let Some(seconds) = number.as_i64() {
        return DateTime::from_timestamp(seconds, 0);
    }
    let seconds = number.as_f64()?;
    if !seconds.is_finite() {
        return None;
    }
    let whole = seconds.trunc();
    if !(-INTEGER_RANGE..INTEGER_RANGE).contains(&whole) {
        return None;
    }
    let fraction = seconds - whole;
    let (seconds, nanos) = if fraction < 0.0 {
        (
            whole as i64 - 1,
            ((1.0 + fraction) * NANOS_PER_SECOND).round().abs(),
        )
    } else {
        (whole as i64, (fraction * NANOS_PER_SECOND).round().abs())
    };
    if nanos >= NANOS_PER_SECOND {
        return DateTime::from_timestamp(seconds + 1, 0);
    }
    DateTime::from_timestamp(seconds, nanos as u32)
}

/// The items of the JSON list `value` holds.
///
/// # Errors
///
/// Returns Pydantic's `list_type` issue for a value of another JSON type.
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
/// Returns Pydantic's `literal_error` issue, whose `expected` lists every option
/// as Pydantic printed it, for a value that names none.
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

/// `options` as Pydantic printed them: each quoted, with `or` before the last.
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
/// A PostgreSQL `text` column cannot store a NUL, so the reference answered 500
/// and stored nothing; every text field of a request refuses one here instead.
pub fn refuse_nul(text: &str, loc: Vec<LocSegment>, issues: &mut Vec<ValidationIssue>) {
    if text.contains('\0') {
        issues.push(ValidationIssue::nul_character(loc, Value::from(text)));
    }
}
