//! Request extractors that answer a bad request the way this API answers one.
//!
//! A bad query parameter, path parameter or body is a **422** whose `detail` is a
//! list of issues, each naming the request part and the field. Axum's own
//! extractors answer 400 with a sentence and no field name, so the three types
//! here wrap them and rebuild the published shape: [`ApiQuery`] and [`ApiJson`]
//! parse with the same `serde` machinery Axum uses, keeping the path to the
//! failing field, and [`ApiPath`] reads the field name from Axum's typed path
//! rejection.
//!
//! A value's own range and length rules go in [`Validate`], so a route's limits
//! report in the same shape and with the same wording as a field's own rules.

// Rust guideline compliant 2026-07-21

use std::future::ready;

use axum::body::Bytes;
use axum::extract::{FromRequest, FromRequestParts, Path, Request};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use flakeradar_core::dto::lax_bool;
use flakeradar_core::lax;
use flakeradar_core::{
    dto::BodyRefusal, dto::LocSegment, dto::ValidationIssue, dto::ci_run_attempt_too_large,
};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use serde_path_to_error::{Path as ErrorPath, Segment};

use crate::error::{ApiError, ApiResult, Detail};

impl From<BodyRefusal> for ApiError {
    fn from(refusal: BodyRefusal) -> Self {
        match refusal {
            BodyRefusal::Issues(issues) => Self::unprocessable(issues),
            BodyRefusal::AttemptPastColumn => {
                Self::new(StatusCode::BAD_REQUEST, ci_run_attempt_too_large())
            }
        }
    }
}

/// The range and length rules a request value checks after it is parsed.
///
/// A field's type check happens while the value is parsed; a rule such as
/// "at least 1" or "at most 64 characters" can only run once the field holds a
/// value. The trait exists so those rules answer exactly like the parse failures
/// do: 422 with an issue naming the field.
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
        let pairs = last_value_per_name(parts.uri.query().unwrap_or_default());
        ready(parse_query::<T>(&pairs).map(Self))
    }
}

/// The path parameters of a request.
///
/// Rejects with 422 and `loc: ["path", <field>]` when a parameter does not fit
/// its type. No path parameter has a range or length rule, so there is no
/// [`Validate`] step.
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

/// A JSON request body a model reads its own fields from.
///
/// The reading rules a field has — the spellings of a boolean, a number's range,
/// a string's length — belong to the model, so the extractor only gets the body
/// to it as the object the model reads.
pub trait FromJsonObject: Sized {
    /// Reads the value from the parsed object.
    ///
    /// # Errors
    ///
    /// Returns the 422 the model's rules failed with.
    fn from_object(object: &Map<String, Value>) -> ApiResult<Self>;
}

/// A JSON request body, validated.
///
/// A body is read by its `Content-Type`: anything that is not JSON is left as the
/// raw bytes, which the model then rejects as not an object. The same two steps
/// happen here, so a request that sends no `Content-Type` gets a 422 rather than
/// Axum's 415.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: FromJsonObject + Send + 'static,
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
        // A Pipeline body can be 20 MiB, and reading it as JSON and validating
        // every Job it names are both CPU work proportional to the body, so both
        // run on a blocking thread: the `Bytes` handle moves for free, and the
        // parsed object is dropped there rather than on an async worker.
        let parsed = off_thread("The JSON body parser", move || {
            let object = parse_json_object(&bytes, content_type.as_deref())?;
            T::from_object(&object)
        })
        .await?;
        Ok(Self(parsed))
    }
}

impl FromJsonObject for flakeradar_core::dto::QuarantineIn {
    fn from_object(object: &Map<String, Value>) -> ApiResult<Self> {
        Self::from_value(object).map_err(ApiError::unprocessable)
    }
}

impl FromJsonObject for flakeradar_core::dto::IssueLinkIn {
    fn from_object(object: &Map<String, Value>) -> ApiResult<Self> {
        Self::from_value(object).map_err(ApiError::unprocessable)
    }
}

impl FromJsonObject for flakeradar_core::dto::PipelineReportIn {
    fn from_object(object: &Map<String, Value>) -> ApiResult<Self> {
        Self::from_value(object).map_err(ApiError::from)
    }
}

/// Runs `work` on a blocking thread, answering the 500 a panic inside it gives.
///
/// A request body can be 20 MiB, and reading, validating or serialising it is CPU
/// work proportional to what the client sent: it runs here rather than on an async
/// worker that the other requests share. `what` names the work in the error.
///
/// # Errors
///
/// Answers 500 naming `what` when the blocking task did not finish, which only a
/// panic inside it causes, and `work`'s own error otherwise.
pub(crate) async fn off_thread<T, F>(what: &str, work: F) -> ApiResult<T>
where
    F: FnOnce() -> ApiResult<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work).await.map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{what} did not finish: {error}"),
        )
    })?
}

/// Parses `query` into `T` and checks its own rules, reporting the failing field
/// the way this API does.
///
/// A parameter sent more than once keeps its last value, which is the one this
/// reader validates. A refused value's issue echoes the text the request sent,
/// including when the value parsed and broke a declared range.
fn parse_query<T: DeserializeOwned + Validate>(pairs: &[(String, String)]) -> ApiResult<T> {
    let collapsed = serialised(pairs);
    let deserializer =
        serde_urlencoded::Deserializer::new(form_urlencoded::parse(collapsed.as_bytes()));
    let value: T = serde_path_to_error::deserialize(deserializer).map_err(|error| {
        let mut issue = issue_for("query", error.path(), &error.inner().to_string());
        attach_query_input(&mut issue, pairs);
        ApiError::unprocessable(vec![issue])
    })?;
    value
        .validate()
        .map_err(|error| with_query_input(error, pairs))?;
    Ok(value)
}

/// `error` with the text each issue's parameter carried.
fn with_query_input(error: ApiError, pairs: &[(String, String)]) -> ApiError {
    let Detail::Issues(issues) = error.detail() else {
        return error;
    };
    let mut issues = issues.clone();
    for issue in &mut issues {
        attach_query_input(issue, pairs);
    }
    ApiError::unprocessable(issues)
}

/// The query with one pair per name, each keeping the last value the query gave it.
fn last_value_per_name(query: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (name, value) in form_urlencoded::parse(query.as_bytes()) {
        match pairs.iter_mut().find(|(seen, _)| *seen == name) {
            Some((_, kept)) => *kept = value.into_owned(),
            None => pairs.push((name.into_owned(), value.into_owned())),
        }
    }
    pairs
}

/// The collapsed query, re-encoded for the deserializer.
fn serialised(pairs: &[(String, String)]) -> String {
    let mut serialiser = form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        let _ = serialiser.append_pair(name, value);
    }
    serialiser.finish()
}

/// Puts the text the request sent into `issue`'s `input`.
///
/// A refused query value reports the raw text, including one that broke a
/// declared range, so the issue carries the string even where the rule saw the
/// parsed number.
fn attach_query_input(issue: &mut ValidationIssue, pairs: &[(String, String)]) {
    if issue.r#type == "missing" {
        return;
    }
    let Some(LocSegment::Field(name)) = issue.loc.last() else {
        return;
    };
    if let Some((_, value)) = pairs.iter().find(|(seen, _)| seen == name) {
        issue.input = Some(Value::String(value.clone()));
    }
}

/// Parses a request body into the JSON object a model reads its fields from.
///
/// A body that is refused before any field is validated is refused here: an empty
/// body is a missing field, a body that is not JSON is no body at all, and one
/// that is not an object is not a map a model can read fields from.
pub(crate) fn parse_json_object(
    bytes: &[u8],
    content_type: Option<&str>,
) -> ApiResult<Map<String, Value>> {
    if bytes.is_empty() {
        return Err(ApiError::unprocessable(vec![ValidationIssue::missing(
            scope_loc("body"),
        )]));
    }
    if !is_json_content_type(content_type) {
        // A body that is not JSON is handed to the model as raw text, which
        // is reported as not being a dictionary or object.
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
    match value {
        Value::Object(object) => Ok(object),
        other => Err(ApiError::unprocessable(vec![
            ValidationIssue::model_attributes_type(scope_loc("body"), other),
        ])),
    }
}

/// Whether a `Content-Type` names JSON.
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

/// The character offset `serde_json` reports an error at, as an index into the
/// decoded text.
///
/// The published position is an index into the text, so a multi-byte character
/// counts once and a newline counts as one. `serde_json` reports a 1-based line
/// and column instead, which this turns back into an index.
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
    if kind == "missing" {
        return ValidationIssue::missing(loc);
    }
    ValidationIssue {
        r#type: kind.to_owned(),
        loc,
        msg: message.to_owned(),
        input: None,
        ctx: None,
    }
}

/// The error kind a `serde` failure is reported as, from the parser's wording.
///
/// `serde` reports one sentence per failure and exposes no kind, so the kind is
/// read from that sentence. Each mapping is pinned by a test, and an unfamiliar
/// message falls back to `value_error` rather than guessing a narrower kind.
fn kind_for(message: &str) -> &'static str {
    if message == lax::INT_PARSING_MESSAGE {
        return "int_parsing";
    }
    if message == lax_bool::UNPARSABLE_MESSAGE {
        return "bool_parsing";
    }
    if message == lax_bool::WRONG_TYPE_MESSAGE {
        return "bool_type";
    }
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

/// The error kind for a type name `serde` or Axum printed.
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
pub(crate) fn body_error(rejection: &axum::extract::rejection::BytesRejection) -> ApiError {
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

#[cfg(test)]
mod tests {
    //! The error-shape mapping, on messages the parsers really produce.

    use flakeradar_core::dto::LocSegment;
    use serde_json::Value;

    use flakeradar_core::dto::lax_bool;

    use super::{
        INTEGER_TYPES, is_integer_failure, is_json_content_type, json_error_position, kind_for,
        kind_for_type_name, missing_field, off_thread, path_issue,
    };

    #[tokio::test]
    async fn cpu_work_leaves_the_thread_that_awaits_it() {
        // A current-thread runtime, so the awaiting thread is the one the work
        // would run on if it were inline. The closure reports where it really ran.
        let here = std::thread::current().id();
        let there = off_thread("The test's work", move || Ok(std::thread::current().id()))
            .await
            .expect("the work finishes");
        assert_ne!(here, there, "the work ran on the thread that awaited it");
    }

    #[test]
    fn only_a_json_media_type_is_parsed_as_json() {
        assert!(is_json_content_type(Some("application/json")));
        assert!(is_json_content_type(Some(
            "Application/JSON; charset=utf-8"
        )));
        assert!(is_json_content_type(Some("application/vnd.api+json")));
        assert!(!is_json_content_type(Some("text/plain")));
        assert!(!is_json_content_type(Some("application/xml")));
        assert!(!is_json_content_type(Some("json")));
        assert!(!is_json_content_type(None));
    }

    #[test]
    fn a_serde_message_maps_to_its_kind() {
        assert_eq!(kind_for(lax_bool::WRONG_TYPE_MESSAGE), "bool_type");
        assert_eq!(kind_for(lax_bool::UNPARSABLE_MESSAGE), "bool_parsing");
        assert_eq!(
            kind_for("invalid type: string, expected a boolean"),
            "bool_parsing"
        );
        assert_eq!(
            kind_for("provided string was not `true` or `false`"),
            "bool_parsing"
        );
        assert_eq!(
            kind_for("unknown variant `bogus`, expected one of `score`"),
            "literal_error"
        );
        assert_eq!(kind_for("invalid float literal"), "float_parsing");
        assert_eq!(
            kind_for("invalid type: string, expected f64"),
            "float_parsing"
        );
        assert_eq!(
            kind_for("invalid type: string, expected f32"),
            "float_parsing"
        );
        assert_eq!(
            kind_for("invalid type: string, expected i64"),
            "int_parsing"
        );
        assert_eq!(
            kind_for("invalid type: string, expected usize"),
            "int_parsing"
        );
        assert_eq!(kind_for("invalid digit found in string"), "int_parsing");
        assert_eq!(
            kind_for("number too large to fit in target type"),
            "int_parsing"
        );
        assert_eq!(kind_for("missing field `number`"), "missing");
        assert_eq!(kind_for("something else entirely"), "value_error");
    }

    #[test]
    fn a_type_name_maps_to_its_kind() {
        assert_eq!(kind_for_type_name("bool"), "bool_parsing");
        for name in ["f32", "f64"] {
            assert_eq!(kind_for_type_name(name), "float_parsing");
        }
        assert_eq!(kind_for_type_name("i64"), "int_parsing");
        assert_eq!(kind_for_type_name("u128"), "int_parsing");
        assert_eq!(kind_for_type_name("char"), "value_error");
    }

    #[test]
    fn every_integer_width_serde_prints_is_an_integer_failure() {
        for name in INTEGER_TYPES {
            assert!(is_integer_failure(name), "{name}");
        }
        assert!(is_integer_failure("number too small to fit in target type"));
        assert!(!is_integer_failure("invalid float literal"));
    }

    #[test]
    fn a_missing_field_names_the_field() {
        assert_eq!(
            missing_field("missing field `number` at line 1 column 2"),
            Some("number".to_owned())
        );
        assert_eq!(missing_field("missing field"), None);
        assert_eq!(missing_field("missing field `number"), None);
    }

    #[test]
    fn a_path_issue_names_the_type_it_could_not_read() {
        let loc = vec![
            LocSegment::Field("path".to_owned()),
            LocSegment::Field("id".to_owned()),
        ];
        let issue = path_issue(loc[1..].to_vec(), "abc", "i64");
        assert_eq!(issue.r#type, "int_parsing");
        assert_eq!(issue.loc, loc);
        let issue = path_issue(loc[1..].to_vec(), "abc", "f64");
        assert_eq!(issue.r#type, "float_parsing");
        let issue = path_issue(loc[1..].to_vec(), "abc", "bool");
        assert_eq!(issue.r#type, "bool_parsing");
        let issue = path_issue(loc[1..].to_vec(), "abc", "char");
        assert_eq!(issue.r#type, "value_error");
        assert_eq!(issue.msg, "Input should be a valid char, got 'abc'");
        assert_eq!(issue.input, Some(Value::from("abc")));
    }

    #[test]
    fn an_error_position_counts_characters_from_the_start() {
        // The position is an index into the decoded text: a newline counts one and
        // a multi-byte character counts one, whatever its UTF-8 length.
        let cases: [(&str, usize); 4] = [
            ("{\"number\": }", 11),
            ("{\n  \"number\": }", 14),
            ("{\"númber\": }", 11),
            ("{\n  \"númber\": }", 14),
        ];
        for (body, expected) in cases {
            let error = serde_json::from_str::<Value>(body).expect_err("the fixture is malformed");
            assert_eq!(
                json_error_position(body.as_bytes(), &error),
                expected,
                "{body:?}"
            );
        }
    }
}
