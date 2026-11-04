//! Request extractors that reject the way FastAPI rejected.
//!
//! FastAPI answers a bad query parameter, path parameter or body with **422**
//! and a `detail` list of Pydantic issues, each naming the request part and the
//! field. Axum's own extractors answer 400 with a sentence and no field name, so
//! the three types here wrap them and rebuild the FastAPI shape: [`ApiQuery`]
//! and [`ApiJson`] parse with the same `serde` machinery Axum uses, keeping the
//! path to the failing field, and [`ApiPath`] reads the field name from Axum's
//! typed path rejection.
//!
//! A value's own range and length rules go in [`Validate`], so a route's limits
//! report in the same shape and with Pydantic's wording.

// Rust guideline compliant 2026-07-21

use std::future::ready;

use axum::body::Bytes;
use axum::extract::{FromRequest, FromRequestParts, Path, Request};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use flakeradar_core::{LocSegment, ValidationIssue};
use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_path_to_error::{Path as ErrorPath, Segment};

use crate::error::{ApiError, ApiResult};

/// The range and length rules a request value checks after it is parsed.
///
/// A field's type check happens while the value is parsed; a rule such as
/// "at least 1" or "at most 64 characters" can only run once the field holds a
/// value. The trait exists so those rules answer exactly like the parse failures
/// do: 422 with a Pydantic issue naming the field.
pub trait Validate {
    /// Checks the value's own rules.
    ///
    /// # Errors
    ///
    /// Returns the 422 [`ApiError::unprocessable`] describes when a rule fails.
    fn validate(&self) -> ApiResult<()>;
}

/// The query parameters of a request, validated.
///
/// Rejects with 422 and `loc: ["query", <field>]` when the query string does not
/// fit `T`, and with [`Validate`]'s error afterwards.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned + Validate + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    // Reading a query string never waits, so the extractor answers with a future
    // that is already resolved instead of an `async fn` whose body has no await.
    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let parsed = parse_query::<T>(parts.uri.query().unwrap_or_default()).and_then(|value| {
            value.validate()?;
            Ok(Self(value))
        });
        ready(parsed)
    }
}

/// The path parameters of a request.
///
/// Rejects with 422 and `loc: ["path", <field>]` when a parameter does not fit
/// its type. The frozen server places no range or length rule on a path
/// parameter, so there is no [`Validate`] step.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(path_error)
    }
}

/// A JSON request body, validated.
///
/// FastAPI reads a body by its `Content-Type`: anything that is not JSON is left
/// as the raw bytes, which a model then rejects as not an object. The same two
/// steps happen here, so a request that sends no `Content-Type` gets the 422
/// FastAPI gave it rather than Axum's 415.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned + Validate + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let content_type = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|rejection| body_error(&rejection))?;
        let value: T = parse_json(&bytes, content_type.as_deref())?;
        value.validate()?;
        Ok(Self(value))
    }
}

/// Parses `query` into `T`, reporting the failing field the way FastAPI did.
fn parse_query<T: DeserializeOwned>(query: &str) -> ApiResult<T> {
    let deserializer =
        serde_urlencoded::Deserializer::new(form_urlencoded::parse(query.as_bytes()));
    serde_path_to_error::deserialize(deserializer).map_err(|error| {
        let issue = issue_for("query", error.path(), &error.inner().to_string());
        ApiError::unprocessable(vec![issue])
    })
}

/// Parses a request body into `T` by the rule FastAPI applied.
fn parse_json<T: DeserializeOwned>(bytes: &[u8], content_type: Option<&str>) -> ApiResult<T> {
    if bytes.is_empty() {
        return Err(ApiError::unprocessable(vec![ValidationIssue::missing(
            scope_loc("body"),
        )]));
    }
    if !is_json_content_type(content_type) {
        // A body that is not JSON is handed to the model as raw text, which
        // Pydantic reports as not being a dictionary or object.
        let text = String::from_utf8_lossy(bytes);
        return Err(ApiError::unprocessable(vec![
            ValidationIssue::model_attributes_type(
                scope_loc("body"),
                Value::String(text.to_string()),
            ),
        ]));
    }
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(error) => {
            let mut loc = scope_loc("body");
            loc.push(LocSegment::Index(json_error_position(bytes, &error)));
            return Err(ApiError::unprocessable(vec![
                ValidationIssue::json_invalid(loc, &error.to_string()),
            ]));
        }
    };
    if !value.is_object() {
        return Err(ApiError::unprocessable(vec![
            ValidationIssue::model_attributes_type(scope_loc("body"), value),
        ]));
    }
    serde_path_to_error::deserialize(value).map_err(|error| {
        let issue = issue_for("body", error.path(), &error.inner().to_string());
        ApiError::unprocessable(vec![issue])
    })
}

/// Whether a `Content-Type` names JSON, the way FastAPI's check did.
fn is_json_content_type(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let essence = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    match essence.split_once('/') {
        Some((main, subtype)) => {
            main == "application" && (subtype == "json" || subtype.ends_with("+json"))
        }
        None => false,
    }
}

/// The `loc` a 422 reports for a bad query parameter named `field`.
#[must_use]
pub fn query_loc(field: &str) -> Vec<LocSegment> {
    field_loc("query", field)
}

/// The `loc` a 422 reports for a bad body field named `field`.
#[must_use]
pub fn body_loc(field: &str) -> Vec<LocSegment> {
    field_loc("body", field)
}

/// The `loc` of a bad field of one request part.
fn field_loc(scope: &str, field: &str) -> Vec<LocSegment> {
    let mut loc = scope_loc(scope);
    loc.push(LocSegment::Field(field.to_owned()));
    loc
}

/// The `loc` of a request part itself, before any field or position.
fn scope_loc(scope: &str) -> Vec<LocSegment> {
    vec![LocSegment::Field(scope.to_owned())]
}

/// The character offset `serde_json` reports an error at, as Python counted it.
///
/// Python's `json` module reports a position as an index into the decoded text,
/// so a multi-byte character counts once and a newline counts as one. `serde_json`
/// reports a 1-based line and column instead, which this turns back into an index.
fn json_error_position(bytes: &[u8], error: &serde_json::Error) -> usize {
    let text = String::from_utf8_lossy(bytes);
    let mut offset = 0;
    for _ in 1..error.line() {
        let Some(newline) = text.get(offset..).and_then(|rest| rest.find('\n')) else {
            break;
        };
        offset += newline + 1;
    }
    offset = offset.saturating_add(error.column().saturating_sub(1));
    text.get(..offset)
        .map_or(offset, |prefix| prefix.chars().count())
}

/// Builds the issue for a `serde` failure at `path`.
fn issue_for(scope: &str, path: &ErrorPath, message: &str) -> ValidationIssue {
    let kind = kind_for(message);
    let mut loc = scope_loc(scope);
    loc.extend(path.iter().filter_map(|segment| match segment {
        Segment::Map { key } => Some(LocSegment::Field(key.clone())),
        Segment::Seq { index } => Some(LocSegment::Index(*index)),
        Segment::Enum { .. } | Segment::Unknown => None,
    }));
    if kind == "missing"
        && let Some(field) = missing_field(message)
    {
        loc.push(LocSegment::Field(field));
    }
    ValidationIssue {
        r#type: kind.to_owned(),
        loc,
        msg: message.to_owned(),
        input: None,
        ctx: None,
    }
}

/// Pydantic's error kind for a `serde` failure, from the parser's own wording.
///
/// `serde` reports one sentence per failure and exposes no kind, so the kind is
/// read from that sentence. Each mapping is pinned by a test, and an unfamiliar
/// message falls back to `value_error` rather than guessing a narrower kind.
fn kind_for(message: &str) -> &'static str {
    let lowered = message.to_lowercase();
    if lowered.contains("expected a boolean") || lowered.contains("was not `true` or `false`") {
        "bool_parsing"
    } else if lowered.contains("unknown variant") {
        "literal_error"
    } else if lowered.contains("invalid float")
        || lowered.contains("expected f64")
        || lowered.contains("expected f32")
    {
        "float_parsing"
    } else if is_integer_failure(&lowered) {
        "int_parsing"
    } else if lowered.contains("missing field") {
        "missing"
    } else {
        "value_error"
    }
}

/// Pydantic's error kind for a type name `serde` or Axum printed.
fn kind_for_type_name(name: &str) -> &'static str {
    match name {
        "bool" => "bool_parsing",
        "f32" | "f64" => "float_parsing",
        other if is_integer_failure(&format!("expected {other}")) => "int_parsing",
        _ => "value_error",
    }
}

/// The integer type names `serde` prints when a value is not one.
const INTEGER_TYPES: [&str; 12] = [
    "expected i8",
    "expected i16",
    "expected i32",
    "expected i64",
    "expected i128",
    "expected isize",
    "expected u8",
    "expected u16",
    "expected u32",
    "expected u64",
    "expected u128",
    "expected usize",
];

/// Whether a `serde` failure is a value that is not an integer.
fn is_integer_failure(message: &str) -> bool {
    message.contains("invalid digit found in string")
        || message.contains("number too large")
        || message.contains("number too small")
        || INTEGER_TYPES.iter().any(|name| message.contains(name))
}

/// The field name in serde's own "missing field" message.
fn missing_field(message: &str) -> Option<String> {
    let rest = message.split_once("missing field `")?.1;
    rest.split_once('`').map(|(field, _)| field.to_owned())
}

/// The 422 for a path parameter that does not fit its type.
fn path_error(rejection: axum::extract::rejection::PathRejection) -> ApiError {
    use axum::extract::path::ErrorKind;
    use axum::extract::rejection::PathRejection;

    let PathRejection::FailedToDeserializePathParams(error) = rejection else {
        return ApiError::new(StatusCode::NOT_FOUND, "Not Found");
    };
    match error.kind() {
        ErrorKind::ParseErrorAtKey {
            key,
            value,
            expected_type,
        } => ApiError::unprocessable(vec![path_issue(
            vec![LocSegment::Field(key.clone())],
            value,
            expected_type,
        )]),
        ErrorKind::ParseError {
            value,
            expected_type,
        } => ApiError::unprocessable(vec![path_issue(Vec::new(), value, expected_type)]),
        ErrorKind::ParseErrorAtIndex {
            index,
            value,
            expected_type,
        } => ApiError::unprocessable(vec![path_issue(
            vec![LocSegment::Index(*index)],
            value,
            expected_type,
        )]),
        ErrorKind::InvalidUtf8InPathParam { key } => {
            let loc = field_loc("path", key);
            ApiError::unprocessable(vec![ValidationIssue {
                r#type: "string_unicode".to_owned(),
                loc,
                msg: format!("Input should be a valid string, `{key}` is not valid UTF-8"),
                input: None,
                ctx: None,
            }])
        }
        // A route asking for a type the path deserializer cannot fill, or a path
        // that does not carry the expected number of parameters, is a mistake in
        // the router rather than in the request.
        other => ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Path parameters cannot be read: {other}"),
        ),
    }
}

/// One path parameter that does not fit its type, in the shape a query or body
/// parameter rejects with.
fn path_issue(tail: Vec<LocSegment>, value: &str, expected_type: &str) -> ValidationIssue {
    let mut loc = scope_loc("path");
    loc.extend(tail);
    match kind_for_type_name(expected_type) {
        "int_parsing" => ValidationIssue::int_parsing(loc, Value::from(value)),
        "float_parsing" => ValidationIssue::float_parsing(loc, Value::from(value)),
        "bool_parsing" => ValidationIssue::bool_parsing(loc, Value::from(value)),
        other => ValidationIssue {
            r#type: other.to_owned(),
            loc,
            msg: format!("Input should be a valid {expected_type}, got '{value}'"),
            input: Some(Value::from(value)),
            ctx: None,
        },
    }
}

/// The 413 or 500 for a body that could not be read.
fn body_error(rejection: &axum::extract::rejection::BytesRejection) -> ApiError {
    use axum::extract::rejection::{BytesRejection, FailedToBufferBody};

    match rejection {
        BytesRejection::FailedToBufferBody(FailedToBufferBody::LengthLimitError(_)) => {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Request body exceeds the size limit",
            )
        }
        _ => ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Reading the request body failed: {rejection}"),
        ),
    }
}
