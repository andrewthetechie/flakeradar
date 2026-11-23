//! The refusal vocabulary a 422 answer carries: the issue shape, the finish rule, and
//! the reading a `ci_run_attempt` value gets.
//!
//! These items live here and are re-exported from the dto root, so the paths callers
//! write today still resolve. `body_loc_root` is the one crate-visible item, because
//! the request models outside this module point their model-level errors at the same
//! place.

// Rust guideline compliant 2026-07-21

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::identity::IdentityError;
use crate::lax;
use crate::py::strip_wire_whitespace;

use super::request::ATTEMPT_MAX;

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
pub(crate) fn body_loc_root() -> Vec<LocSegment> {
    vec![LocSegment::Field("body".to_owned())]
}

/// The singular or plural noun a limit's message ends with.
fn plural(limit: usize) -> &'static str {
    if limit == 1 { "" } else { "s" }
}

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
