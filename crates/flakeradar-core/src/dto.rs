//! The request and response DTOs, and the lax readers that validate a body.
//!
//! Each struct is a field-for-field mirror of the JSON this API answers and
//! accepts, because the frontend, the CI actions and Fabro parse it. A field that
//! has no value serializes as `null` rather than being skipped, except where a
//! request may leave it out. Timestamps go through [`timestamp`] so every one
//! follows the timestamp rule.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::classify::FailureCategory;
use crate::identity::{IdentityError, normalize_provider, normalize_repo};
use crate::lax;
use crate::model::{
    ExecutionStatus, FailedJobOutcome, JobOutcome, JobStatus, ReportKind, ReportStatus, Tier, Trend,
};
use crate::py::strip_wire_whitespace;
use crate::types::{CiRunId, Fingerprint, Score};

/// The timestamp shape every response uses.
///
/// A timestamp is `YYYY-MM-DDTHH:MM:SS`, then `.` and six fractional digits,
/// then `Z`. Fabro rejects anything else, so the fraction is always written,
/// even when it is zero.
pub mod timestamp {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    /// The `strftime` pattern every timestamp is written with.
    pub const FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.6fZ";

    /// Writes a timestamp in the one accepted shape.
    ///
    /// # Errors
    ///
    /// Returns the serializer's error when the sink rejects the string.
    pub fn serialize<S: Serializer>(
        value: &DateTime<Utc>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.format(FORMAT).to_string())
    }

    /// Reads a timestamp written as RFC 3339.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not an RFC 3339 timestamp.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<DateTime<Utc>, D::Error> {
        let text = String::deserialize(deserializer)?;
        DateTime::parse_from_rfc3339(&text)
            .map(|parsed| parsed.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)
    }
}

/// The timestamp helpers for an optional field.
pub mod optional_timestamp {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    /// Writes an optional timestamp in the one accepted shape.
    ///
    /// # Errors
    ///
    /// Returns the serializer's error when the sink rejects the string.
    pub fn serialize<S: Serializer>(
        value: &Option<DateTime<Utc>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => super::timestamp::serialize(value, serializer),
            None => serializer.serialize_none(),
        }
    }

    /// Reads an optional RFC 3339 timestamp.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not an RFC 3339 timestamp.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<DateTime<Utc>>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| {
                DateTime::parse_from_rfc3339(&text)
                    .map(|parsed| parsed.with_timezone(&Utc))
                    .map_err(serde::de::Error::custom)
            })
            .transpose()
    }
}

/// The lax boolean a query or a body may spell, for the quarantine toggle.
///
/// Accepted: JSON `true`/`false`, the numbers `1`/`0` (as integers or as the
/// floats `1.0`/`0.0`) and the strings `on`/`off`, `t`/`f`, `true`/`false`,
/// `y`/`n` and `yes`/`no` in any case. Everything else is refused with one of two
/// kinds: `bool_type` for a value of another JSON type, and `bool_parsing` for a
/// number or string of the right type that spells no boolean. Both kinds reach
/// the client, so both are part of the wire: `{"quarantined": "yes"}` is read as
/// `true` and the answer is the route's 404 for a missing Test, while
/// `{"quarantined": []}` is a 422 of the type kind.
pub mod lax_bool {
    use serde::{Deserialize, Deserializer};
    use serde_json::{Number, Value};

    /// The message a boolean of the right type but the wrong spelling is refused
    /// with, which the request extractors report as `bool_parsing`.
    pub const UNPARSABLE_MESSAGE: &str =
        "Input should be a valid boolean, unable to interpret input";

    /// The message a value of another JSON type is refused with, which the
    /// request extractors report as `bool_type`.
    pub const WRONG_TYPE_MESSAGE: &str = "Input should be a valid boolean";

    /// The float that reads as `false`.
    const ZERO: f64 = 0.0;

    /// The float that reads as `true`.
    const ONE: f64 = 1.0;

    /// The lower bound, exclusive, of the range the float-to-integer conversion
    /// accepts: `i64::MIN` as a `f64`.
    const INTEGER_FLOOR: f64 = -9.223_372_036_854_776e18;

    /// The upper bound, exclusive, of that range: `i64::MAX` plus one as a `f64`.
    const INTEGER_CEILING: f64 = 9.223_372_036_854_776e18;

    /// Reads a required lax boolean.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is not one of the accepted spellings.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
        interpret(&Value::deserialize(deserializer)?).map_err(refusal)
    }

    /// Reads an optional lax boolean, where `null` or an absent value is `None`.
    ///
    /// # Errors
    ///
    /// Returns an error when a present value is not one of the accepted spellings.
    pub fn optional<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::Null => Ok(None),
            value => interpret(&value).map(Some).map_err(refusal),
        }
    }

    /// The `serde` error a refused value becomes, carrying the message so the
    /// request extractors report the matching kind.
    fn refusal<E: serde::de::Error>(refusal: Refusal) -> E {
        match refusal {
            Refusal::WrongType => E::custom(WRONG_TYPE_MESSAGE),
            Refusal::Unparsable => E::custom(UNPARSABLE_MESSAGE),
        }
    }

    /// What the lax reading makes of one JSON value.
    ///
    /// # Errors
    ///
    /// Returns [`Refusal::WrongType`] for a value of another JSON type and
    /// [`Refusal::Unparsable`] for a number or a string with no boolean in it.
    pub fn read(value: &Value) -> Result<bool, Refusal> {
        interpret(value)
    }

    /// Why a value was refused, which is also the error kind the 422 reports.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Refusal {
        /// A value of another JSON type: the `bool_type` kind.
        WrongType,
        /// A number or a string with no boolean in it: the `bool_parsing` kind.
        Unparsable,
    }

    /// What the lax reading makes of one JSON value.
    fn interpret(value: &Value) -> Result<bool, Refusal> {
        match value {
            Value::Bool(flag) => Ok(*flag),
            Value::Number(number) => interpret_number(number),
            Value::String(text) => match text.to_ascii_lowercase().as_str() {
                "0" | "off" | "f" | "false" | "n" | "no" => Ok(false),
                "1" | "on" | "t" | "true" | "y" | "yes" => Ok(true),
                _ => Err(Refusal::Unparsable),
            },
            _ => Err(Refusal::WrongType),
        }
    }

    /// What the lax reading makes of one JSON number.
    ///
    /// A whole number goes through the integer path, which reads `0` and `1` and
    /// refuses the rest as unparsable. A float reads `0.0` and `1.0`, and takes
    /// the integer path when it is whole and fits the conversion the integer rule
    /// makes, so `2.0` is unparsable; every other float, including `1.5`, `NaN`
    /// and an infinity, is a value of the wrong type.
    #[expect(
        clippy::float_cmp_const,
        reason = "the rule reads exactly these floats as booleans: a tolerance would accept a \
                  value it refuses"
    )]
    fn interpret_number(number: &Number) -> Result<bool, Refusal> {
        if let Some(integer) = number.as_i64() {
            return match integer {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err(Refusal::Unparsable),
            };
        }
        if number.as_u64().is_some() {
            // A whole number past `i64::MAX` fits no integer the rule reads.
            return Err(Refusal::WrongType);
        }
        let Some(float) = number.as_f64() else {
            return Err(Refusal::WrongType);
        };
        if float == ZERO {
            Ok(false)
        } else if float == ONE {
            Ok(true)
        } else if float.fract() == ZERO && float > INTEGER_FLOOR && float < INTEGER_CEILING {
            Err(Refusal::Unparsable)
        } else {
            Err(Refusal::WrongType)
        }
    }
}

/// The lax integer a query parameter may spell.
///
/// A query string carries text, and an integer there is read in the spellings
/// this API accepts: `"1_0"` is 10, `"2.0"` is 2, `" 2 "` is 2, and `"true"` or
/// `"0x10"` are none of them. `serde`'s own number reader takes only digits, so
/// every query field that is an integer goes through here.
pub mod lax_int {
    use serde::{Deserialize, Deserializer};

    use crate::lax::{INT_PARSING_MESSAGE, text_int};

    /// Reads a required lax integer.
    ///
    /// # Errors
    ///
    /// Returns the `int_parsing` message for a text that is no integer.
    pub fn value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
        let text = String::deserialize(deserializer)?;
        text_int(&text).ok_or_else(|| serde::de::Error::custom(INT_PARSING_MESSAGE))
    }

    /// Reads an optional lax integer, where an absent parameter is `None`.
    ///
    /// # Errors
    ///
    /// Returns the `int_parsing` message for a text that is no integer.
    pub fn optional<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| {
                text_int(&text).ok_or_else(|| serde::de::Error::custom(INT_PARSING_MESSAGE))
            })
            .transpose()
    }
}

/// One segment of a validation error's location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum LocSegment {
    /// A field name, or the request part (`body`, `query`, `path`).
    Field(String),
    /// A position in a list.
    Index(usize),
}

/// One entry of the `detail` list a 422 response carries.
#[expect(
    clippy::derive_partial_eq_without_eq,
    reason = "serde_json::Value holds floats, so the issue cannot be Eq"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ValidationIssue {
    /// The error kind, such as `string_too_long` or `value_error`.
    pub r#type: String,
    /// Where the bad value was.
    pub loc: Vec<LocSegment>,
    /// The message, as the client reads it.
    pub msg: String,
    /// The rejected input, on the kinds that carry one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    /// The constraint that failed, on the kinds that name one.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub ctx: Option<Value>,
}

/// Every issue one request's checks found, in the order they found them.
///
/// A validator pushes what it finds and ends with [`FinishIssues::finish`], so
/// "nothing found is an accepted request" is a rule stated once for both front
/// doors rather than an `is_empty` test at each site.
pub type Issues = Vec<ValidationIssue>;

/// How a request's checks reach their end.
///
/// Every validator on either front door collects an [`Issues`] list and ends with
/// this method, so the rule that an empty list is an accepted request is stated
/// once.
pub trait FinishIssues {
    /// Whether the checks found anything.
    ///
    /// The issues come back rather than a refusal because what a refusal costs
    /// belongs to the surface: a route answers its 422 body, a request model
    /// answers its own refusal kind.
    ///
    /// # Errors
    ///
    /// Returns the whole list when it is not empty, and `Ok(())` when it is.
    fn finish(self) -> Result<(), Issues>;
}

impl FinishIssues for Issues {
    fn finish(self) -> Result<(), Issues> {
        if self.is_empty() { Ok(()) } else { Err(self) }
    }
}

impl ValidationIssue {
    /// A string shorter than its minimum length.
    #[must_use]
    pub fn string_too_short(loc: Vec<LocSegment>, input: Value, limit: usize) -> Self {
        Self::length(
            "string_too_short",
            format!(
                "String should have at least {limit} character{}",
                plural(limit)
            ),
            "min_length",
            loc,
            input,
            limit,
        )
    }

    /// A string longer than its maximum length.
    #[must_use]
    pub fn string_too_long(loc: Vec<LocSegment>, input: Value, limit: usize) -> Self {
        Self::length(
            "string_too_long",
            format!(
                "String should have at most {limit} character{}",
                plural(limit)
            ),
            "max_length",
            loc,
            input,
            limit,
        )
    }

    /// A list shorter than its minimum length.
    #[must_use]
    pub fn list_too_short(loc: Vec<LocSegment>, input: Value, limit: usize) -> Self {
        let actual = input.as_u64().unwrap_or_default();
        Self::length(
            "too_short",
            format!(
                "List should have at least {limit} item{} after validation, not {actual}",
                plural(limit)
            ),
            "min_length",
            loc,
            input,
            limit,
        )
    }

    /// A list longer than its maximum length.
    #[must_use]
    pub fn list_too_long(loc: Vec<LocSegment>, input: Value, limit: usize, actual: usize) -> Self {
        Self::length(
            "too_long",
            format!(
                "List should have at most {limit} item{} after validation, not {actual}",
                plural(limit)
            ),
            "max_length",
            loc,
            input,
            limit,
        )
    }

    /// A length constraint that failed.
    fn length(
        error_type: &str,
        msg: String,
        key: &str,
        loc: Vec<LocSegment>,
        input: Value,
        limit: usize,
    ) -> Self {
        let mut ctx = serde_json::Map::new();
        let _ = ctx.insert(key.to_owned(), Value::from(limit));
        Self {
            r#type: error_type.to_owned(),
            loc,
            msg,
            input: Some(input),
            ctx: Some(Value::Object(ctx)),
        }
    }

    /// A value below its minimum.
    #[must_use]
    pub fn greater_than_equal(loc: Vec<LocSegment>, input: Value, limit: i64) -> Self {
        let mut ctx = serde_json::Map::new();
        let _ = ctx.insert("ge".to_owned(), Value::from(limit));
        Self {
            r#type: "greater_than_equal".to_owned(),
            loc,
            msg: format!("Input should be greater than or equal to {limit}"),
            input: Some(input),
            ctx: Some(Value::Object(ctx)),
        }
    }

    /// A value above its maximum.
    #[must_use]
    pub fn less_than_equal(loc: Vec<LocSegment>, input: Value, limit: i64) -> Self {
        let mut ctx = serde_json::Map::new();
        let _ = ctx.insert("le".to_owned(), Value::from(limit));
        Self {
            r#type: "less_than_equal".to_owned(),
            loc,
            msg: format!("Input should be less than or equal to {limit}"),
            input: Some(input),
            ctx: Some(Value::Object(ctx)),
        }
    }

    /// A value that is not an integer.
    #[must_use]
    pub fn int_parsing(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing("int_parsing", lax::INT_PARSING_MESSAGE, loc, input)
    }

    /// A value that is not a number.
    #[must_use]
    pub fn float_parsing(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing(
            "float_parsing",
            "Input should be a valid number, unable to parse string as a number",
            loc,
            input,
        )
    }

    /// A value that is not a boolean.
    #[must_use]
    pub fn bool_parsing(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing(
            "bool_parsing",
            "Input should be a valid boolean, unable to interpret input",
            loc,
            input,
        )
    }

    /// A value of another JSON type, where a boolean was expected.
    #[must_use]
    pub fn bool_type(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing("bool_type", "Input should be a valid boolean", loc, input)
    }

    /// A value of another JSON type, where a string was expected.
    #[must_use]
    pub fn string_type(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing("string_type", "Input should be a valid string", loc, input)
    }

    /// A value of another JSON type, where a list was expected.
    #[must_use]
    pub fn list_type(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing("list_type", "Input should be a valid list", loc, input)
    }

    /// A value of another JSON type, where an integer was expected.
    #[must_use]
    pub fn int_type(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing("int_type", "Input should be a valid integer", loc, input)
    }

    /// A float with a fractional part, where an integer was expected.
    #[must_use]
    pub fn int_from_float(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing(
            "int_from_float",
            "Input should be a valid integer, got a number with a fractional part",
            loc,
            input,
        )
    }

    /// A number past the range an integer is held in.
    #[must_use]
    pub fn int_parsing_size(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing(
            "int_parsing_size",
            "Unable to parse input string as an integer, exceeded maximum size",
            loc,
            input,
        )
    }

    /// A value of another JSON type, where a timestamp was expected.
    #[must_use]
    pub fn datetime_type(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing(
            "datetime_type",
            "Input should be a valid datetime",
            loc,
            input,
        )
    }

    /// A string that is no date and no datetime, with the reader's reason.
    #[must_use]
    pub fn datetime_from_date_parsing(loc: Vec<LocSegment>, input: Value, error: &str) -> Self {
        let mut ctx = serde_json::Map::new();
        let _ = ctx.insert("error".to_owned(), Value::from(error));
        Self {
            r#type: "datetime_from_date_parsing".to_owned(),
            loc,
            msg: format!("Input should be a valid datetime or date, {error}"),
            input: Some(input),
            ctx: Some(Value::Object(ctx)),
        }
    }

    /// A timestamp outside the years 1 to 9999, with the reader's reason.
    ///
    /// A Unix timestamp is a number of seconds or milliseconds, and the instant
    /// it names has to be one a datetime holds; any other instant is refused with
    /// this kind, the sentence in its `msg` and the same sentence again in its
    /// `ctx.error`.
    #[must_use]
    pub fn datetime_parsing(loc: Vec<LocSegment>, input: Value, error: &str) -> Self {
        let mut ctx = serde_json::Map::new();
        let _ = ctx.insert("error".to_owned(), Value::from(error));
        Self {
            r#type: "datetime_parsing".to_owned(),
            loc,
            msg: format!("Input should be a valid datetime, {error}"),
            input: Some(input),
            ctx: Some(Value::Object(ctx)),
        }
    }

    /// A text holding a NUL character, which a `text` column cannot store.
    #[must_use]
    pub fn nul_character(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing(
            "value_error",
            "Value error, the text contains a NUL character, which a text column cannot store",
            loc,
            input,
        )
    }

    /// A value that is not a dictionary or object, where a model was expected.
    #[must_use]
    pub fn model_attributes_type(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing(
            "model_attributes_type",
            "Input should be a valid dictionary or object to extract fields from",
            loc,
            input,
        )
    }

    /// A required field that was not there.
    #[must_use]
    pub fn missing(loc: Vec<LocSegment>) -> Self {
        Self {
            r#type: "missing".to_owned(),
            loc,
            msg: "Field required".to_owned(),
            input: Some(Value::Null),
            ctx: None,
        }
    }

    /// A value that is not one of a fixed set.
    #[must_use]
    pub fn literal_error(loc: Vec<LocSegment>, input: Value, expected: &str) -> Self {
        let mut ctx = serde_json::Map::new();
        let _ = ctx.insert("expected".to_owned(), Value::from(expected));
        Self {
            r#type: "literal_error".to_owned(),
            loc,
            msg: format!("Input should be {expected}"),
            input: Some(input),
            ctx: Some(Value::Object(ctx)),
        }
    }

    /// A request body that is not valid JSON.
    #[must_use]
    pub fn json_invalid(loc: Vec<LocSegment>, error: &str) -> Self {
        let mut ctx = serde_json::Map::new();
        let _ = ctx.insert("error".to_owned(), Value::from(error));
        Self {
            r#type: "json_invalid".to_owned(),
            loc,
            msg: "JSON decode error".to_owned(),
            input: Some(Value::Object(serde_json::Map::new())),
            ctx: Some(Value::Object(ctx)),
        }
    }

    /// A value a field type could not parse, with the message the client reads.
    fn parsing(error_type: &str, msg: &str, loc: Vec<LocSegment>, input: Value) -> Self {
        Self {
            r#type: error_type.to_owned(),
            loc,
            msg: msg.to_owned(),
            input: Some(input),
            ctx: None,
        }
    }

    /// A value a field validator rejected.
    #[must_use]
    pub fn value_error(error: &IdentityError, loc: Vec<LocSegment>, input: Value) -> Self {
        Self {
            r#type: "value_error".to_owned(),
            loc,
            msg: format!("Value error, {error}"),
            input: Some(input),
            ctx: None,
        }
    }

    /// A model-level rule that failed, such as duplicate job ids.
    #[must_use]
    pub fn model_error(message: &str) -> Self {
        Self {
            r#type: "value_error".to_owned(),
            loc: body_loc_root(),
            msg: format!("Value error, {message}"),
            input: None,
            ctx: None,
        }
    }
}

/// Where a model-level error points: the body the model was built from.
fn body_loc_root() -> Vec<LocSegment> {
    vec![LocSegment::Field("body".to_owned())]
}

/// The singular or plural noun a limit's message ends with.
fn plural(limit: usize) -> &'static str {
    if limit == 1 { "" } else { "s" }
}

/// The response to an accepted upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IngestAccepted {
    /// The new Report's id, or the stored duplicate's.
    pub report_id: i64,
    /// The Report's status: pending for a new one, else the stored status.
    pub status: ReportStatus,
    /// Whether the upload repeated a stored Report.
    pub duplicate: bool,
}

/// One Job's result inside a Pipeline report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct JobResultIn {
    /// The CI provider's id for the Job.
    pub ci_job_id: String,
    /// The Job's name within its Pipeline.
    pub name: String,
    /// What the Job did.
    pub status: JobStatus,
    /// The Job's CI url, when the reporter sent one.
    #[serde(default)]
    pub url: String,
    /// The runner's name, when the reporter sent one.
    #[serde(default)]
    pub runner_name: String,
    /// The runner's labels, such as `ubuntu-latest`.
    #[serde(default)]
    pub runner_labels: Vec<String>,
    /// When the Job started.
    #[serde(default, with = "optional_timestamp")]
    pub started_at: Option<DateTime<Utc>>,
    /// When the Job finished.
    #[serde(default, with = "optional_timestamp")]
    pub completed_at: Option<DateTime<Utc>>,
    /// Whether the Job is a Summary Job.
    #[serde(default)]
    pub summary: bool,
}

/// The longest name a Report may give a Repo, a Project, a branch, a
/// `ci_job_id`, a `ci_run_id` or a runner label, in code points.
///
/// This is the bound this API publishes: a longer value is refused with the 422
/// `string_too_long` naming the field, which `tests/parity` replays against the
/// recorded answer. Raising it would accept a Report no consumer has ever sent
/// and store a name in a `text` column that the published contract says is too
/// long; lowering it would refuse an `owner/repo` or a matrix Job name a CI
/// provider really reports.
pub const NAME_MAX: usize = 255;

/// The longest name a Pipeline's Job may carry, in code points.
///
/// A matrix Job's name spells its whole matrix in one string, such as
/// `test (ubuntu-24.04, 3.12)`, and several of those run long, so a Job name gets
/// twice the room an identity does. The bound is part of the published 422 and
/// `tests/parity` replays it.
pub const JOB_NAME_MAX: usize = 512;

/// The longest Report `pipeline`, in code points.
///
/// A `pipeline` is a workflow path such as
/// `.github/workflows/integration-tests.yml`, which is why it shares the Job
/// name's bound rather than the shorter identity one.
pub const PIPELINE_MAX: usize = JOB_NAME_MAX;

/// The longest commit SHA a Report may name, in code points.
///
/// Both SHA-1 (40 hex characters) and SHA-256 (64) Git object names fit, and
/// nothing longer is an object name. The column is `text`, so this bound is the
/// contract's rather than storage's.
pub const COMMIT_SHA_MAX: usize = 64;

/// The longest Provider name a Report may name, in code points.
///
/// Providers are a short fixed vocabulary (`github`, `gitlab`, `other`), so
/// anything past a short word is not one. The published 422 names the field when
/// the value is longer.
pub const PROVIDER_MAX: usize = 32;

/// The longest Project name a Report or a query may name, in code points.
///
/// A Project is a test suite's name (`frontend`, `backend`, `e2e`), so a short
/// word is normal and 100 is generous. The bound is published on both the ingest
/// query and the read filters, and `tests/parity` replays the 422 for a longer
/// one. The column is `text`, so nothing in the database enforces it.
pub const PROJECT_MAX: usize = 100;

/// The longest path a Report or a query may name, in code points.
///
/// A Project root and a Test's `file` are paths inside a repository, and a
/// deeply nested monorepo path runs long; 1024 is the usual filesystem ceiling
/// and far past any path a repository holds. Both the ingest query and the Test
/// list filter publish the same bound, and the column is `text`.
pub const PATH_MAX: usize = 1024;

/// The longest URL a Report or an Issue link may carry, in code points.
///
/// A GitHub issue URL with a long Repo name and a query string fits well inside
/// 2048, which is the bound browsers and GitHub's own API work to. The column is
/// `text`; the bound is the contract's, and `tests/parity` replays the 422.
pub const URL_MAX: usize = 2048;

/// The longest `runner_labels` list a Job may carry.
///
/// A GitHub Actions matrix leg reports one label per dimension plus its own
/// (`ubuntu-24.04`, `3.12`, `gpu` and so on), so a handful is normal and 50 is
/// far past any real matrix. A longer list is refused with the recorded 422
/// rather than stored; the list is stored as one row's text, so no index or
/// column width bounds it.
const RUNNER_LABELS_MAX: usize = 50;

/// The longest one runner label may be.
///
/// Labels are runner names and container image tags, the longest of which fit in
/// a short line. The bound is part of the same published 422 as the list's size.
const RUNNER_LABEL_MAX: usize = NAME_MAX;

impl JobResultIn {
    /// Reads one Job of a Pipeline report with the lax rules this API applies.
    ///
    /// `at` is where the Job sits in the body, so each issue's `loc` names the
    /// Job and the field a client has to fix.
    ///
    /// # Errors
    ///
    /// Returns every issue the Job has, in field order.
    pub fn from_value(
        object: &serde_json::Map<String, Value>,
        at: Vec<LocSegment>,
    ) -> Result<Self, Vec<ValidationIssue>> {
        let fields = lax::Fields::new(object, at);
        let mut issues = Vec::new();

        let ci_job_id =
            read_string(&fields, "ci_job_id", 1, NAME_MAX, &mut issues).unwrap_or_default();
        let name = read_string(&fields, "name", 1, JOB_NAME_MAX, &mut issues).unwrap_or_default();
        let status = read_status(&fields, &mut issues).unwrap_or(JobStatus::Passed);
        let url = read_defaulted_string(&fields, "url", URL_MAX, &mut issues).unwrap_or_default();
        let runner_name = read_defaulted_string(&fields, "runner_name", NAME_MAX, &mut issues)
            .unwrap_or_default();
        let runner_labels = read_runner_labels(&fields, &mut issues);
        let started_at = read_optional_timestamp(&fields, "started_at", &mut issues);
        let completed_at = read_optional_timestamp(&fields, "completed_at", &mut issues);
        let summary = read_defaulted_bool(&fields, "summary", &mut issues).unwrap_or(false);

        issues.finish()?;
        Ok(Self {
            ci_job_id,
            name,
            status,
            url,
            runner_name,
            runner_labels,
            started_at,
            completed_at,
            summary,
        })
    }
}

/// The required string `field` of `fields` holds, stripped and checked.
fn read_string(
    fields: &lax::Fields<'_>,
    field: &str,
    min: usize,
    max: usize,
    issues: &mut Vec<ValidationIssue>,
) -> Option<String> {
    let value = fields.required(field, issues)?;
    let text = lax::recorded(lax::string(value, fields.loc(field)), issues)?;
    Some(check_string(&text, min, max, fields.loc(field), issues))
}

/// The string `field` of `fields` holds, stripped and checked, when it is there.
///
/// The field has a default, so an absent value leaves it; a `null` is a value of
/// the wrong type, which is refused exactly as a number of the wrong type is.
fn read_defaulted_string(
    fields: &lax::Fields<'_>,
    field: &str,
    max: usize,
    issues: &mut Vec<ValidationIssue>,
) -> Option<String> {
    let value = fields.field(field)?;
    let text = lax::recorded(lax::string(value, fields.loc(field)), issues)?;
    Some(check_string(&text, 0, max, fields.loc(field), issues))
}

/// The `status` of a Job, read against the literals a Job status may be.
fn read_status(fields: &lax::Fields<'_>, issues: &mut Vec<ValidationIssue>) -> Option<JobStatus> {
    let value = fields.required("status", issues)?;
    let options: Vec<&str> = JobStatus::ALL
        .iter()
        .map(|status| status.as_str())
        .collect();
    lax::recorded(lax::literal(value, &options, fields.loc("status")), issues)
        .and_then(JobStatus::parse)
}

/// The `runner_labels` of a Job: a list of short strings, at most 50 of them.
fn read_runner_labels(fields: &lax::Fields<'_>, issues: &mut Vec<ValidationIssue>) -> Vec<String> {
    let Some(value) = fields.field("runner_labels") else {
        return Vec::new();
    };
    let Some(labels) = lax::recorded(lax::list_of(value, fields.loc("runner_labels")), issues)
    else {
        return Vec::new();
    };
    // A list past the bound is refused whole, before any item is read: one issue
    // names the list's length rather than one issue per label.
    if labels.len() > RUNNER_LABELS_MAX {
        issues.push(ValidationIssue::list_too_long(
            fields.loc("runner_labels"),
            Value::from(labels.len()),
            RUNNER_LABELS_MAX,
            labels.len(),
        ));
        return Vec::new();
    }
    let mut out = Vec::with_capacity(labels.len());
    for (index, label) in labels.iter().enumerate() {
        let loc = fields.item_loc("runner_labels", index);
        let Some(label) = lax::recorded(lax::string(label, loc.clone()), issues) else {
            continue;
        };
        let label = stripped(&label);
        lax::refuse_nul(&label, loc.clone(), issues);
        if label.chars().count() > RUNNER_LABEL_MAX {
            issues.push(ValidationIssue::string_too_long(
                loc,
                Value::from(label.clone()),
                RUNNER_LABEL_MAX,
            ));
        }
        out.push(label);
    }
    out
}

/// The optional timestamp `field` of `fields` holds.
fn read_optional_timestamp(
    fields: &lax::Fields<'_>,
    field: &str,
    issues: &mut Vec<ValidationIssue>,
) -> Option<DateTime<Utc>> {
    let value = fields.field(field).filter(|value| !value.is_null())?;
    lax::recorded(lax::timestamp(value, fields.loc(field)), issues)
}

/// The boolean `field` of `fields` holds, when it is there.
///
/// The field has a default, so an absent value leaves it; a `null` is a value of
/// the wrong type, which is refused the same way.
fn read_defaulted_bool(
    fields: &lax::Fields<'_>,
    field: &str,
    issues: &mut Vec<ValidationIssue>,
) -> Option<bool> {
    let value = fields.field(field)?;
    lax::recorded(lax::boolean(value, fields.loc(field)), issues)
}

/// Trims text on both ends past every whitespace character.
fn stripped(text: &str) -> String {
    strip_wire_whitespace(text).to_owned()
}

/// Checks a string's NUL and length bounds, returning the stripped value.
fn check_string(
    text: &str,
    min: usize,
    max: usize,
    loc: Vec<LocSegment>,
    issues: &mut Vec<ValidationIssue>,
) -> String {
    let text = stripped(text);
    lax::refuse_nul(&text, loc.clone(), issues);
    let len = text.chars().count();
    if len < min {
        issues.push(ValidationIssue::string_too_short(
            loc.clone(),
            Value::from(text.clone()),
            min,
        ));
    }
    if len > max {
        issues.push(ValidationIssue::string_too_long(
            loc,
            Value::from(text.clone()),
            max,
        ));
    }
    text
}

/// The most Jobs one Pipeline report may carry.
///
/// One Pipeline run reports its whole Job matrix in one upload, and a large
/// repository's is dozens, not hundreds. 1000 is far past any real matrix and is
/// what the published 422 states; a longer list is refused before anything is
/// stored, so the upload cannot fill `job_executions` on its own.
const JOBS_MAX: usize = 1000;

/// The largest `ci_run_attempt` the `int4` column holds.
pub const ATTEMPT_MAX: i64 = i32::MAX as i64;

/// Why a Pipeline report body was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum BodyRefusal {
    /// Every issue the body has, which the 422's `detail` list carries.
    Issues(Vec<ValidationIssue>),
    /// A `ci_run_attempt` past the `int4` column, which both ingest routes answer
    /// 400 for rather than rewriting the value CI sent.
    AttemptPastColumn,
}

/// What one `ci_run_attempt` value says, before a route answers it.
#[expect(
    clippy::derive_partial_eq_without_eq,
    reason = "the reading keeps the JSON value the caller sent, which holds floats"
)]
#[derive(Debug, Clone, PartialEq)]
pub enum AttemptIn {
    /// The attempt the value names, inside the column's range.
    Number(i32),
    /// A number below 1, as the caller sent it.
    BelowMinimum(Value),
    /// A number past the `int4` column, however it is spelled.
    PastColumn,
    /// A value that is no integer, in the terms the 422 reports it.
    NotAnInteger {
        /// The error kind, such as `int_type`.
        kind: String,
        /// The message for that kind.
        msg: String,
        /// The value as the caller sent it.
        input: Value,
    },
}

impl AttemptIn {
    /// The issue this reading reports at `loc`, or `None` when it reports none.
    ///
    /// The two ingest routes place the same value at a query and at a body
    /// field, so the reading carries what was said and the route says where.
    #[must_use]
    pub fn issue(&self, loc: Vec<LocSegment>) -> Option<ValidationIssue> {
        match self {
            Self::Number(_) | Self::PastColumn => None,
            Self::BelowMinimum(input) => {
                Some(ValidationIssue::greater_than_equal(loc, input.clone(), 1))
            }
            Self::NotAnInteger { kind, msg, input } => Some(ValidationIssue {
                r#type: kind.clone(),
                loc,
                msg: msg.clone(),
                input: Some(input.clone()),
                ctx: None,
            }),
        }
    }
}

/// Reads a `ci_run_attempt` from the value a request carried.
///
/// One rule serves both ingest routes: the lax integer, at least 1, and inside
/// the `int4` column. The issue's `loc` is the route's to place, so the reading
/// is built without one.
#[must_use]
pub fn read_ci_run_attempt(value: &Value) -> AttemptIn {
    // A run of digits past `i64` is a number past the column whatever it spells,
    // where any other unreadable value is the `int_parsing` kind.
    if let Value::String(text) = value {
        let trimmed = strip_wire_whitespace(text);
        if lax::text_int(text).is_none()
            && !trimmed.is_empty()
            && trimmed.bytes().all(|byte| byte.is_ascii_digit())
        {
            return AttemptIn::PastColumn;
        }
    }
    match lax::integer(value, Vec::new()) {
        Ok(number) if number < 1 => AttemptIn::BelowMinimum(value.clone()),
        Ok(number) => i32::try_from(number).map_or(AttemptIn::PastColumn, AttemptIn::Number),
        Err(issue) => AttemptIn::NotAnInteger {
            kind: issue.r#type,
            msg: issue.msg,
            input: value.clone(),
        },
    }
}

/// The `detail` both ingest routes answer for an attempt past the column.
#[must_use]
pub fn ci_run_attempt_too_large() -> String {
    format!("ci_run_attempt must be at most {ATTEMPT_MAX}")
}

/// One attempt of one Pipeline run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PipelineReportIn {
    /// The Repo's `owner/name`, normalized on the way in.
    pub repo: String,
    /// The CI provider's label, such as `github`.
    pub provider: String,
    /// The Pipeline's name, such as `.github/workflows/ci.yml`.
    pub pipeline: String,
    /// The commit the attempt ran.
    pub commit_sha: String,
    /// The branch the commit is on.
    pub branch: String,
    /// The Repo's Default branch, when the reporter knew it.
    #[serde(default)]
    pub default_branch: Option<String>,
    /// The CI provider's run id.
    #[serde(default)]
    pub ci_run_id: CiRunId,
    /// Which attempt of the CI run this is, numbered from 1.
    #[serde(default = "default_attempt")]
    pub ci_run_attempt: i64,
    /// The Jobs this attempt ran.
    pub jobs: Vec<JobResultIn>,
}

/// The attempt number a Report that names none counts as.
fn default_attempt() -> i64 {
    1
}

impl PipelineReportIn {
    /// Reads a Pipeline report body with the lax rules this API applies.
    ///
    /// # Errors
    ///
    /// Returns every issue the body has, in field order, or
    /// [`BodyRefusal::AttemptPastColumn`] for an attempt the `int4` column cannot
    /// hold.
    pub fn from_value(object: &serde_json::Map<String, Value>) -> Result<Self, BodyRefusal> {
        let fields = lax::Fields::new(object, body_loc_root());
        let mut issues = Vec::new();

        let repo = read_name(&fields, "repo", NAME_MAX, &mut issues, normalize_repo);
        let provider = read_name(
            &fields,
            "provider",
            PROVIDER_MAX,
            &mut issues,
            normalize_provider,
        );
        let pipeline =
            read_string(&fields, "pipeline", 1, PIPELINE_MAX, &mut issues).unwrap_or_default();
        let commit_sha =
            read_string(&fields, "commit_sha", 1, COMMIT_SHA_MAX, &mut issues).unwrap_or_default();
        let branch = read_string(&fields, "branch", 1, NAME_MAX, &mut issues).unwrap_or_default();
        let default_branch = read_default_branch(&fields, &mut issues);
        // A refused id is the reason the whole body is refused, so the fallback
        // value never reaches a row.
        let ci_run_id = read_defaulted_string(&fields, "ci_run_id", NAME_MAX, &mut issues)
            .and_then(|text| CiRunId::new(&text).ok())
            .unwrap_or_else(CiRunId::empty);
        let mut ci_run_attempt = default_attempt();
        let mut attempt_past_column = false;
        if let Some(value) = fields.field("ci_run_attempt") {
            let reading = read_ci_run_attempt(value);
            if let Some(issue) = reading.issue(fields.loc("ci_run_attempt")) {
                issues.push(issue);
            }
            match reading {
                AttemptIn::Number(number) => ci_run_attempt = i64::from(number),
                AttemptIn::PastColumn => attempt_past_column = true,
                _ => {}
            }
        }
        let jobs = read_jobs(&fields, &mut issues);

        if issues.is_empty() {
            // A model-level rule runs only once every field validated, so a body
            // with two faults reports the field's issue and not the model's.
            let mut seen = std::collections::BTreeSet::new();
            if !jobs.iter().all(|job| seen.insert(job.ci_job_id.clone())) {
                issues.push(ValidationIssue::model_error("duplicate ci_job_id in jobs"));
            }
        }
        issues.finish().map_err(BodyRefusal::Issues)?;
        if attempt_past_column {
            return Err(BodyRefusal::AttemptPastColumn);
        }
        Ok(Self {
            repo,
            provider,
            pipeline,
            commit_sha,
            branch,
            default_branch,
            ci_run_id,
            ci_run_attempt,
            jobs,
        })
    }
}

/// The required name `field` of `fields` holds, normalized by `normalize`.
fn read_name(
    fields: &lax::Fields<'_>,
    field: &str,
    max: usize,
    issues: &mut Vec<ValidationIssue>,
    normalize: fn(&str) -> Result<String, IdentityError>,
) -> String {
    let Some(value) = fields.required(field, issues) else {
        return String::new();
    };
    let Some(text) = lax::recorded(lax::string(value, fields.loc(field)), issues) else {
        return String::new();
    };
    normalized_name(&text, max, field, issues, normalize)
}

/// The `default_branch` of a Pipeline report: a short string, blank for none.
fn read_default_branch(
    fields: &lax::Fields<'_>,
    issues: &mut Vec<ValidationIssue>,
) -> Option<String> {
    let value = fields
        .field("default_branch")
        .filter(|value| !value.is_null())?;
    let text = lax::recorded(lax::string(value, fields.loc("default_branch")), issues)?;
    let text = stripped(&text);
    lax::refuse_nul(&text, fields.loc("default_branch"), issues);
    if text.is_empty() {
        return None;
    }
    if text.chars().count() > NAME_MAX {
        issues.push(ValidationIssue::string_too_long(
            fields.loc("default_branch"),
            Value::from(text),
            NAME_MAX,
        ));
        return None;
    }
    Some(text)
}

/// The `jobs` of a Pipeline report: one or more Jobs, each read field by field.
fn read_jobs(fields: &lax::Fields<'_>, issues: &mut Vec<ValidationIssue>) -> Vec<JobResultIn> {
    let Some(value) = fields.required("jobs", issues) else {
        return Vec::new();
    };
    let Some(items) = lax::recorded(lax::list_of(value, fields.loc("jobs")), issues) else {
        return Vec::new();
    };
    if items.is_empty() {
        issues.push(ValidationIssue::list_too_short(
            fields.loc("jobs"),
            Value::from(0),
            1,
        ));
        return Vec::new();
    }
    if items.len() > JOBS_MAX {
        issues.push(ValidationIssue::list_too_long(
            fields.loc("jobs"),
            Value::from(items.len()),
            JOBS_MAX,
            items.len(),
        ));
        return Vec::new();
    }
    let mut jobs = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let at = fields.item_loc("jobs", index);
        let Some(object) = item.as_object() else {
            issues.push(ValidationIssue::model_attributes_type(at, item.clone()));
            continue;
        };
        match JobResultIn::from_value(object, at) {
            Ok(job) => jobs.push(job),
            Err(job_issues) => issues.extend(job_issues),
        }
    }
    jobs
}

/// The stored body of a Pipeline Report: the validated Report, canonically
/// serialized.
///
/// A Pipeline Report keeps its body so the processor reads back exactly what was
/// validated, so this and `PipelineReportIn`'s deserializer are inverses.
///
/// # Panics
///
/// Panics only if the body cannot be serialized, which a Report that
/// deserialized from JSON cannot do: every field is a string, a bool, an
/// integer, a list or an optional value.
#[must_use]
#[expect(
    clippy::expect_used,
    reason = "a Report that deserialized from JSON always serializes back: every field is a \
              string, a bool, an integer, a list or an optional value, none of which can fail"
)]
pub fn pipeline_body(report: &PipelineReportIn) -> Vec<u8> {
    serde_json::to_vec(report).expect("a validated Pipeline Report always serializes")
}

/// Where one field of the Pipeline report's body sits.
fn body_loc(field: &str) -> Vec<LocSegment> {
    vec![
        LocSegment::Field("body".to_owned()),
        LocSegment::Field(field.to_owned()),
    ]
}

/// Normalizes a required name, recording the length or NUL issue it finds.
fn normalized_name(
    raw: &str,
    max: usize,
    field: &str,
    issues: &mut Vec<ValidationIssue>,
    normalize: fn(&str) -> Result<String, IdentityError>,
) -> String {
    let raw = stripped(raw);
    let loc = body_loc(field);
    lax::refuse_nul(&raw, loc.clone(), issues);
    if raw.chars().count() > max {
        issues.push(ValidationIssue::string_too_long(
            loc,
            Value::from(raw.clone()),
            max,
        ));
        return raw;
    }
    match normalize(&raw) {
        Ok(normalized) => normalized,
        Err(error) => {
            issues.push(ValidationIssue::value_error(
                &error,
                loc,
                Value::from(raw.clone()),
            ));
            raw
        }
    }
}

/// The response describing one Report.
#[expect(
    clippy::derive_partial_eq_without_eq,
    reason = "the counts payload is JSON, which holds floats, so the report cannot be Eq"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ReportOut {
    /// The Report's id.
    pub id: i64,
    /// `junit` or `pipeline`.
    pub kind: ReportKind,
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The Project's name, or `None` for a Pipeline report.
    pub project: Option<String>,
    /// The commit the Report covers.
    pub commit_sha: String,
    /// The branch the commit is on.
    pub branch: String,
    /// The CI provider's run id, or the empty id an upload gave no run.
    pub ci_run_id: CiRunId,
    /// `pending`, `processed` or `failed`.
    pub status: ReportStatus,
    /// Why processing failed, when it did.
    pub error: Option<String>,
    /// The Executions the Report produced, when it was processed.
    #[schema(value_type = Option<Object>)]
    pub counts: Option<Value>,
    /// The Run the Report became, when it was processed.
    pub run_id: Option<i64>,
    /// Automatic retries spent, capped by the maximum report retries.
    pub retry_count: i64,
    /// When the Report arrived.
    #[serde(with = "timestamp")]
    pub created_at: DateTime<Utc>,
    /// When processing finished, or failed.
    #[serde(with = "optional_timestamp")]
    pub processed_at: Option<DateTime<Utc>>,
}

/// Pending and failed Report counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReportSummaryOut {
    /// Reports waiting to be processed.
    pub pending: i64,
    /// Reports that failed permanently.
    pub failed: i64,
}

/// The processor's readiness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ReadinessOut {
    /// Whether the database answers.
    pub database: bool,
    /// Whether this process holds the processor lock.
    pub processor_leader: bool,
    /// Reports waiting to be processed.
    pub pending_reports: i64,
    /// How long the oldest pending Report has waited, in seconds.
    pub oldest_pending_age_seconds: f64,
}

/// One Project inside a Repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProjectOut {
    /// The Project's name.
    pub name: String,
    /// The Project root, repo-relative; `""` at the Repo root.
    pub root: String,
}

/// One Repo and its Projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RepoOut {
    /// The Repo's `owner/name`.
    pub name: String,
    /// Every Project in the Repo.
    pub projects: Vec<ProjectOut>,
}

/// How many rows a delete removed, counted before the cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeletedCountsOut {
    /// Tests removed.
    pub tests: i64,
    /// Executions removed.
    pub executions: i64,
    /// Jobs removed.
    pub jobs: i64,
    /// Reports removed.
    pub reports: i64,
}

/// The result of deleting a Repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RepoDeleteOut {
    /// The Repo that was deleted.
    pub repo: String,
    /// How much went with it.
    pub deleted: DeletedCountsOut,
    /// Every GitHub issue number the removed rows stored.
    pub github_issue_numbers: Vec<i64>,
}

/// The result of deleting a Project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProjectDeleteOut {
    /// The Project's Repo.
    pub repo: String,
    /// The Project that was deleted.
    pub project: String,
    /// How much went with it.
    pub deleted: DeletedCountsOut,
    /// Every GitHub issue number the removed Tests stored.
    pub github_issue_numbers: Vec<i64>,
}

/// One Test as the leaderboard serves it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TestOut {
    /// The Test's id.
    pub id: i64,
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The Project's name.
    pub project: String,
    /// The Test's stable identity hash.
    pub fingerprint: Fingerprint,
    /// The suite the runner reported.
    pub suite: String,
    /// The classname the runner reported.
    pub classname: String,
    /// The Test's name within its classname.
    pub name: String,
    /// The file the Test is defined in, when reported.
    pub file: Option<String>,
    /// The line the Test is defined on, when reported.
    pub line: Option<i64>,
    /// The Flakiness score.
    pub flakiness_score: Score,
    /// The tier the score falls in.
    pub tier: Tier,
    /// How many commit SHAs recorded both outcomes.
    pub confirmed_flake_count: i64,
    /// The most common Failure category among recent failures.
    pub failure_category: Option<FailureCategory>,
    /// Non-skipped Executions since the newest failure.
    pub clean_streak: i64,
    /// Consecutive failures, newest first.
    pub fail_streak: i64,
    /// Whether the Fail streak reached the broken threshold.
    pub broken: bool,
    /// The score's direction over the trend window.
    #[serde(default)]
    pub trend: Option<Trend>,
    /// The newest Execution's status.
    pub last_status: ExecutionStatus,
    /// When the Test last ran.
    #[serde(with = "timestamp")]
    pub last_seen_at: DateTime<Utc>,
    /// Whether the Test is quarantined.
    pub quarantined: bool,
    /// When the quarantine started.
    #[serde(with = "optional_timestamp")]
    pub quarantined_at: Option<DateTime<Utc>>,
    /// Why the Test was quarantined.
    pub quarantine_reason: Option<String>,
    /// When the quarantine lapses.
    #[serde(with = "optional_timestamp")]
    pub quarantine_expires_at: Option<DateTime<Utc>>,
    /// The GitHub issue the Test is linked to.
    pub github_issue_number: Option<i64>,
    /// The linked issue's url.
    pub github_issue_url: Option<String>,
    /// Whether the linked issue was closed as not planned.
    pub github_issue_closed: bool,
}

/// One page of Tests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TestPage {
    /// The Tests on this page.
    pub items: Vec<TestOut>,
    /// How many Tests the filter matched.
    pub total: i64,
    /// The page number, numbered from 1.
    pub page: i64,
    /// How many items a page holds.
    pub page_size: i64,
}

/// The dashboard's headline counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SummaryOut {
    /// How many Tests exist.
    pub total_tests: i64,
    /// How many are at or above the Flake threshold.
    pub flaky_tests: i64,
    /// How many are above zero but below the threshold.
    pub suspect_tests: i64,
    /// How many have at least one Proven flake.
    pub confirmed_flaky_tests: i64,
    /// How many became flaky in the last seven days.
    pub new_flaky_7d: i64,
    /// How many Runs exist.
    pub total_runs: i64,
    /// Executions in the last thirty days.
    pub executions_30d: i64,
    /// Failures in the last thirty days.
    pub failures_30d: i64,
    /// The configured Flake threshold.
    pub flake_threshold: f64,
    /// Failing Executions per Failure category.
    pub category_counts: BTreeMap<FailureCategory, i64>,
}

/// One Execution in a Test's history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ExecutionOut {
    /// The Execution's id.
    pub id: i64,
    /// What the Execution did.
    pub status: ExecutionStatus,
    /// How long it took, in seconds.
    pub duration: f64,
    /// The Failure message.
    pub message: String,
    /// The Failure details.
    pub details: String,
    /// When the Execution was recorded.
    #[serde(with = "timestamp")]
    pub created_at: DateTime<Utc>,
    /// The commit the Execution ran.
    pub commit_sha: String,
    /// The branch the commit is on.
    pub branch: String,
    /// The CI provider's run id.
    pub ci_run_id: CiRunId,
    /// Which retry inside the Run this is, numbered from 0.
    pub attempt: i64,
    /// The Failure category, when the Execution failed.
    pub failure_category: Option<FailureCategory>,
}

/// Where a Test is defined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LocationOut {
    /// The file joined onto the Project root, repo-relative.
    pub path: String,
    /// The line the Test is defined on, when reported.
    pub line: Option<i64>,
    /// A GitHub permalink at the last failing commit, when one exists.
    pub url: Option<String>,
}

/// One Job a Test has run in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TestJobLinkOut {
    /// The Job's id.
    pub job_id: i64,
    /// The Job's Pipeline.
    pub pipeline: String,
    /// The Job's name.
    pub name: String,
}

/// One day of a Test's or Job's Score history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScorePointOut {
    /// The day the snapshot was taken.
    pub day: NaiveDate,
    /// The Flakiness score at the end of that day.
    pub flakiness_score: Score,
    /// The Proven flake count at the end of that day.
    pub confirmed_flake_count: i64,
    /// Executions that day.
    pub executions: i64,
    /// Failures that day.
    pub failures: i64,
}

/// A Test's detail page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HistoryOut {
    /// The Test itself.
    pub test: TestOut,
    /// Where the Test is defined, when the runner reported a file.
    pub location: Option<LocationOut>,
    /// The commit of the newest failing Execution.
    pub last_failing_sha: Option<String>,
    /// The branch of the newest failing Execution.
    pub last_failing_branch: Option<String>,
    /// The newest Executions first.
    pub executions: Vec<ExecutionOut>,
    /// The Jobs whose Executions came from these Runs.
    pub jobs: Vec<TestJobLinkOut>,
    /// The last ninety days, oldest first.
    pub score_history: Vec<ScorePointOut>,
}

/// One Job as the leaderboard serves it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JobOut {
    /// The Job's id.
    pub id: i64,
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The CI provider's label.
    pub provider: String,
    /// The Job's Pipeline.
    pub pipeline: String,
    /// The Job's name.
    pub name: String,
    /// The Flakiness score.
    pub flakiness_score: Score,
    /// The tier the score falls in.
    pub tier: Tier,
    /// How many commit SHAs recorded both outcomes.
    pub confirmed_flake_count: i64,
    /// Non-skipped Executions since the newest failure.
    pub clean_streak: i64,
    /// The score's direction over the trend window.
    #[serde(default)]
    pub trend: Option<Trend>,
    /// The newest Job execution's status, as the column stores it.
    ///
    /// A `JobStatus`, except that the column is a `varchar(16)` with no `CHECK` and
    /// a hand-written row can hold a word the three variants do not name. Serving
    /// the stored word is the behaviour, so the field stays a `String` and only
    /// [`JobOutcome`] is folded — see [`JobStatus::from_stored`](crate::model::JobStatus::from_stored).
    pub last_status: String,
    /// When the Job last ran.
    #[serde(with = "timestamp")]
    pub last_seen_at: DateTime<Utc>,
    /// Whether the Job is a Summary Job.
    pub summary: bool,
    /// The GitHub issue the Job is linked to.
    pub github_issue_number: Option<i64>,
    /// The linked issue's url, when the Pipeline is on GitHub.
    pub github_issue_url: Option<String>,
    /// Whether the linked issue was closed as not planned.
    pub github_issue_closed: bool,
}

/// One page of Jobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JobPage {
    /// The Jobs on this page.
    pub items: Vec<JobOut>,
    /// How many Jobs the filter matched.
    pub total: i64,
    /// The page number, numbered from 1.
    pub page: i64,
    /// How many items a page holds.
    pub page_size: i64,
}

/// One Test execution that explains a failed Job execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ExplainingTestOut {
    /// The Test's id.
    pub test_id: i64,
    /// The Test's Project.
    pub project: String,
    /// The Test's classname.
    pub classname: String,
    /// The Test's name.
    pub name: String,
    /// The newest Execution's status in this Run.
    pub status: ExecutionStatus,
}

/// One sibling Job that explains a Summary Job's failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ExplainingJobOut {
    /// The sibling Job's id.
    pub job_id: i64,
    /// The sibling Job's Pipeline.
    pub pipeline: String,
    /// The sibling Job's name.
    pub name: String,
    /// The sibling Execution's CI url.
    pub url: String,
}

/// One Job execution in a Job's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct JobExecutionOut {
    /// The Job execution's id.
    pub id: i64,
    /// The stored status, as the column stores it; see [`JobOut::last_status`] for
    /// why this is not a `JobStatus`.
    pub status: String,
    /// What the execution counts as once Explained failures are attributed.
    pub outcome: JobOutcome,
    /// The commit the execution ran.
    pub commit_sha: String,
    /// The branch the commit is on.
    pub branch: String,
    /// The CI provider's run id.
    pub ci_run_id: CiRunId,
    /// Which attempt of the CI run this was.
    pub ci_run_attempt: i64,
    /// The CI provider's Job id.
    pub ci_job_id: String,
    /// The Job's CI url.
    pub url: String,
    /// The runner's name.
    pub runner_name: String,
    /// The runner's labels.
    pub runner_labels: Vec<String>,
    /// When the Job started.
    #[serde(with = "optional_timestamp")]
    pub started_at: Option<DateTime<Utc>>,
    /// When the Job finished.
    #[serde(with = "optional_timestamp")]
    pub completed_at: Option<DateTime<Utc>>,
    /// When the execution was recorded.
    #[serde(with = "timestamp")]
    pub created_at: DateTime<Utc>,
    /// The failing Tests that explain it; empty otherwise.
    pub explained_by: Vec<ExplainingTestOut>,
    /// The failed sibling Jobs that explain a Summary Job; empty otherwise.
    pub explained_by_jobs: Vec<ExplainingJobOut>,
}

/// One runner (or label set) a Job ran on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RunnerStatOut {
    /// The sorted label set.
    pub labels: Vec<String>,
    /// The runner's name, when it ran more than once.
    pub runner_name: Option<String>,
    /// How many executions.
    pub executions: i64,
    /// How many failures nothing explained.
    pub unexplained_failures: i64,
}

/// A Job's detail page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JobHistoryOut {
    /// The Job itself.
    pub job: JobOut,
    /// Unexplained failures among the returned executions.
    pub unexplained_failures: i64,
    /// Explained failures among the returned executions.
    pub explained_failures: i64,
    /// The newest executions first.
    pub executions: Vec<JobExecutionOut>,
    /// Per-runner counts over the newest score window, worst first.
    pub runners: Vec<RunnerStatOut>,
    /// The last ninety days, oldest first.
    pub score_history: Vec<ScorePointOut>,
}

/// The Jobs dashboard's headline counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JobSummaryOut {
    /// How many Jobs exist.
    pub total_jobs: i64,
    /// How many are at or above the Flake threshold.
    pub flaky_jobs: i64,
    /// How many are above zero but below the threshold.
    pub suspect_jobs: i64,
    /// How many have at least one Proven flake.
    pub confirmed_flaky_jobs: i64,
    /// How many became flaky in the last seven days.
    pub new_flaky_7d: i64,
    /// Job executions in the last thirty days.
    pub job_executions_30d: i64,
    /// Job failures in the last thirty days.
    pub job_failures_30d: i64,
    /// The configured Flake threshold.
    pub flake_threshold: f64,
    /// Every Pipeline that has a Job.
    pub pipelines: Vec<String>,
}

/// A human's quarantine decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct QuarantineIn {
    /// Whether the Test is quarantined, in any spelling [`lax_bool`] accepts.
    #[serde(deserialize_with = "lax_bool::deserialize")]
    pub quarantined: bool,
    /// Why the human quarantined it.
    #[serde(default)]
    pub reason: Option<String>,
    /// How many days the quarantine lasts.
    #[serde(default)]
    pub expires_in_days: Option<i64>,
}

impl QuarantineIn {
    /// Reads a quarantine body with the lax rules this API applies.
    ///
    /// # Errors
    ///
    /// Returns every issue the body has, in field order, which the 422's `detail`
    /// list carries.
    pub fn from_value(
        object: &serde_json::Map<String, Value>,
    ) -> Result<Self, Vec<ValidationIssue>> {
        let fields = lax::Fields::new(object, body_loc_root());
        let mut issues = Vec::new();

        let quarantined = fields
            .required("quarantined", &mut issues)
            .and_then(|value| {
                lax::recorded(lax::boolean(value, fields.loc("quarantined")), &mut issues)
            })
            .unwrap_or_default();
        let reason = if let Some(text) = fields
            .field("reason")
            .filter(|value| !value.is_null())
            .and_then(|value| lax::recorded(lax::string(value, fields.loc("reason")), &mut issues))
        {
            lax::refuse_nul(&text, fields.loc("reason"), &mut issues);
            if text.chars().count() > QUARANTINE_REASON_MAX {
                issues.push(ValidationIssue::string_too_long(
                    fields.loc("reason"),
                    Value::from(text.clone()),
                    QUARANTINE_REASON_MAX,
                ));
            }
            Some(text)
        } else {
            None
        };
        let mut expires_in_days = None;
        if let Some(value) = fields
            .field("expires_in_days")
            .filter(|value| !value.is_null())
        {
            match lax::recorded(
                lax::integer(value, fields.loc("expires_in_days")),
                &mut issues,
            ) {
                Some(days) if days < 1 => issues.push(ValidationIssue::greater_than_equal(
                    fields.loc("expires_in_days"),
                    value.clone(),
                    1,
                )),
                Some(days) if days > QUARANTINE_DAYS_MAX => {
                    issues.push(ValidationIssue::less_than_equal(
                        fields.loc("expires_in_days"),
                        value.clone(),
                        QUARANTINE_DAYS_MAX,
                    ));
                }
                Some(days) => expires_in_days = Some(days),
                // A value that is no integer has recorded its own issue.
                None => {}
            }
        }

        issues.finish().map(|()| Self {
            quarantined,
            reason,
            expires_in_days,
        })
    }
}

/// The longest quarantine reason a human may record, in code points.
///
/// A reason is a sentence or two, such as "fails on CI's Windows runner, see
/// issue 42"; 500 characters holds that with room to spare. The value travels to
/// the frontend's quarantine form, which publishes the same bound, and a longer
/// one is refused with the recorded 422.
const QUARANTINE_REASON_MAX: usize = 500;

/// The most days a quarantine may last.
///
/// A quarantine expires by itself when its day count runs out, so this bound is
/// how far ahead one person can switch a Test off without coming back to it. A
/// year is the longest a fix could plausibly take; a longer one would be a way to
/// forget a Test forever, so it is refused with the recorded 422.
const QUARANTINE_DAYS_MAX: i64 = 365;

/// One quarantined Test in the quarantine list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct QuarantineItem {
    /// The suite the runner reported.
    pub suite: String,
    /// The classname the runner reported.
    pub classname: String,
    /// The Test's name.
    pub name: String,
    /// The Test's identity hash.
    pub fingerprint: Fingerprint,
    /// The file the Test is defined in.
    pub file: Option<String>,
    /// The line the Test is defined on.
    pub line: Option<i64>,
    /// When the quarantine started.
    #[serde(with = "optional_timestamp")]
    pub quarantined_at: Option<DateTime<Utc>>,
    /// Why the Test was quarantined.
    pub reason: Option<String>,
    /// When the quarantine lapses.
    #[serde(with = "optional_timestamp")]
    pub expires_at: Option<DateTime<Utc>>,
}

/// Report counts for one CI run attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReportCountsOut {
    /// Reports still waiting.
    pub pending: i64,
    /// Reports processed.
    pub processed: i64,
    /// Reports that failed.
    pub failed: i64,
}

/// One failing Test in a CI run attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CiRunTestFailureOut {
    /// The Test's id.
    pub test_id: i64,
    /// The Test's Project.
    pub project: String,
    /// The suite the runner reported.
    pub suite: String,
    /// The classname the runner reported.
    pub classname: String,
    /// The Test's name.
    pub name: String,
    /// The Run the failure came from.
    pub run_id: i64,
    /// The CI provider's Job id.
    pub ci_job_id: Option<String>,
    /// The Test's newest status in this Run.
    pub status: ExecutionStatus,
    /// How many failing Executions this Run had.
    pub failed_attempts: i64,
    /// The last failing Execution's category.
    pub failure_category: Option<FailureCategory>,
    /// The last failing Execution's message, first 1024 characters.
    pub message: String,
    /// The tier the score falls in.
    pub tier: Tier,
    /// The Flakiness score.
    pub flakiness_score: Score,
    /// How many commit SHAs recorded both outcomes.
    pub confirmed_flake_count: i64,
    /// Non-skipped Executions since the newest failure.
    pub clean_streak: i64,
    /// Consecutive failures, newest first.
    pub fail_streak: i64,
    /// Whether the Fail streak reached the broken threshold.
    pub broken: bool,
    /// Whether the Test is quarantined.
    pub quarantined: bool,
    /// The GitHub issue the Test is linked to.
    pub github_issue_number: Option<i64>,
    /// The linked issue's url.
    pub github_issue_url: Option<String>,
    /// Whether the linked issue was closed as not planned.
    pub github_issue_closed: bool,
}

/// One failed Job in a CI run attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CiRunFailedJobOut {
    /// The Job's id.
    pub job_id: i64,
    /// The Job's Pipeline.
    pub pipeline: String,
    /// The Job's name.
    pub name: String,
    /// The CI provider's Job id.
    pub ci_job_id: String,
    /// The Job's CI url.
    pub url: String,
    /// Whether something explains the failure.
    pub outcome: FailedJobOutcome,
    /// Whether the Job is a Summary Job.
    pub summary: bool,
    /// The tier the score falls in.
    pub tier: Tier,
    /// The Flakiness score.
    pub flakiness_score: Score,
    /// How many commit SHAs recorded both outcomes.
    pub confirmed_flake_count: i64,
    /// Non-skipped Executions since the newest failure.
    pub clean_streak: i64,
    /// The GitHub issue the Job is linked to.
    pub github_issue_number: Option<i64>,
    /// The linked issue's url, when the Pipeline is on GitHub.
    pub github_issue_url: Option<String>,
    /// Whether the linked issue was closed as not planned.
    pub github_issue_closed: bool,
}

/// Every failure in one CI run attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CiRunFailuresOut {
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The CI provider's run id.
    pub ci_run_id: CiRunId,
    /// Which attempt this is.
    pub ci_run_attempt: i64,
    /// The configured Flake threshold.
    pub flake_threshold: f64,
    /// Report counts for both kinds, this attempt only.
    pub reports: ReportCountsOut,
    /// How many (Test, Run) pairs there were before the cap.
    pub total_failures: i64,
    /// Whether the failure list was capped.
    pub truncated: bool,
    /// The failing Tests.
    pub failures: Vec<CiRunTestFailureOut>,
    /// The failed Jobs.
    pub failed_jobs: Vec<CiRunFailedJobOut>,
}

/// The largest GitHub issue number a link can hold.
///
/// `github_issue_number` is a PostgreSQL `INTEGER`.
pub const MAX_ISSUE_NUMBER: i64 = 2_147_483_647;

/// A human's Issue link decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IssueLinkIn {
    /// The GitHub issue number.
    pub number: i64,
    /// The issue's url, when the caller has one.
    #[serde(default)]
    pub url: Option<String>,
}

impl IssueLinkIn {
    /// Reads an Issue link body with the lax rules this API applies.
    ///
    /// # Errors
    ///
    /// Returns every issue the body has, in field order, which the 422's `detail`
    /// list carries.
    pub fn from_value(
        object: &serde_json::Map<String, Value>,
    ) -> Result<Self, Vec<ValidationIssue>> {
        let fields = lax::Fields::new(object, body_loc_root());
        let mut issues = Vec::new();

        let mut number = 0;
        if let Some(value) = fields.required("number", &mut issues) {
            match lax::recorded(lax::integer(value, fields.loc("number")), &mut issues) {
                Some(value_number) if value_number < 1 => issues.push(
                    ValidationIssue::greater_than_equal(fields.loc("number"), value.clone(), 1),
                ),
                Some(value_number) if value_number > MAX_ISSUE_NUMBER => {
                    issues.push(ValidationIssue::less_than_equal(
                        fields.loc("number"),
                        value.clone(),
                        MAX_ISSUE_NUMBER,
                    ));
                }
                Some(value_number) => number = value_number,
                // A value that is no integer has recorded its own issue.
                None => {}
            }
        }
        let url = if let Some(text) = fields
            .field("url")
            .filter(|value| !value.is_null())
            .and_then(|value| lax::recorded(lax::string(value, fields.loc("url")), &mut issues))
        {
            lax::refuse_nul(&text, fields.loc("url"), &mut issues);
            if text.chars().count() > ISSUE_URL_MAX {
                issues.push(ValidationIssue::string_too_long(
                    fields.loc("url"),
                    Value::from(text.clone()),
                    ISSUE_URL_MAX,
                ));
            }
            Some(text)
        } else {
            None
        };

        issues.finish().map(|()| Self { number, url })
    }
}

/// The longest issue url an Issue link body may carry, in code points.
///
/// It is [`URL_MAX`]: a GitHub issue URL with a long Repo name, a query string
/// and a fragment fits inside it, and the published 422 names the field when one
/// does not. The URL is stored as `text`, so the bound is the contract's.
const ISSUE_URL_MAX: usize = URL_MAX;
