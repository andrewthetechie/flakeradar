//! The three extractors' rejections: status, and the `type` and `loc` of every
//! issue, which is what the parity replay compares.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when an extractor answers something else"
)]

mod common;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::routing::{get, post};
use common::Reply;
use flakeradar::{ApiError, ApiJson, ApiPath, ApiQuery, Validate, body_loc, query_loc};
use flakeradar_core::ValidationIssue;
use serde::Deserialize;
use serde_json::{Value, json};

/// A query DTO with one bounded integer, one boolean and one literal.
#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "the extractor must parse it; these tests read other fields"
    )]
    include_stable: bool,
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "the extractor must parse it; these tests read other fields"
    )]
    sort: Sort,
    #[serde(default)]
    name: Option<String>,
}

/// The default page size, as the API's own query DTOs have.
fn default_limit() -> i64 {
    50
}

/// The fixed set a query parameter may take.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Sort {
    #[default]
    Score,
    LastSeen,
    Proven,
}

impl Validate for ListQuery {
    fn validate(&self) -> Result<(), ApiError> {
        let mut issues = Vec::new();
        if self.limit < 1 {
            issues.push(ValidationIssue::greater_than_equal(
                query_loc("limit"),
                json!(self.limit),
                1,
            ));
        }
        if self.limit > 500 {
            issues.push(ValidationIssue::less_than_equal(
                query_loc("limit"),
                json!(self.limit),
                500,
            ));
        }
        if let Some(name) = &self.name
            && name.chars().count() > 10
        {
            issues.push(ValidationIssue::string_too_long(
                query_loc("name"),
                json!(name),
                10,
            ));
        }
        if issues.is_empty() {
            return Ok(());
        }
        Err(ApiError::unprocessable(issues))
    }
}

/// A path DTO naming the resource.
#[derive(Debug, Deserialize)]
struct TestPath {
    test_id: i64,
}

/// A body DTO with an integer, a boolean and a bounded string.
#[derive(Debug, Deserialize)]
struct UpdateBody {
    number: i64,
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "the extractor must parse it; these tests read other fields"
    )]
    quarantined: bool,
    #[serde(default)]
    reason: Option<String>,
}

impl Validate for UpdateBody {
    fn validate(&self) -> Result<(), ApiError> {
        match &self.reason {
            Some(reason) if reason.chars().count() > 5 => Err(ApiError::unprocessable(vec![
                ValidationIssue::string_too_long(body_loc("reason"), json!(reason), 5),
            ])),
            _ => Ok(()),
        }
    }
}

/// A DTO the path deserializer cannot fill.
#[derive(Debug, Deserialize)]
struct NestedPath {
    #[expect(
        dead_code,
        reason = "the field only has to make the path deserializer give up"
    )]
    fields: std::collections::HashMap<String, String>,
}

/// An app holding one route per extractor.
fn extractor_app() -> Router {
    Router::new()
        .route(
            "/list",
            get(|ApiQuery(query): ApiQuery<ListQuery>| async move { format!("{:?}", query.limit) }),
        )
        .route(
            "/test/{test_id}",
            get(|ApiPath(path): ApiPath<TestPath>| async move { path.test_id.to_string() }),
        )
        .route(
            "/update",
            post(|ApiJson(body): ApiJson<UpdateBody>| async move { body.number.to_string() }),
        )
        .route(
            "/nested/{fields}",
            get(|ApiPath(_): ApiPath<NestedPath>| async { "reached" }),
        )
        .route(
            "/plain/{id}",
            get(|ApiPath(id): ApiPath<i64>| async move { id.to_string() }),
        )
        .route(
            "/pair/{left}/{right}",
            get(|ApiPath(pair): ApiPath<(i64, i64)>| async move { format!("{pair:?}") }),
        )
        .layer(DefaultBodyLimit::max(flakeradar::JSON_BODY_LIMIT))
}

/// Sends one request to the extractor app.
async fn call(
    method: axum::http::Method,
    uri: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Reply {
    let request = {
        let mut builder = axum::http::Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder
            .body(axum::body::Body::from(body.to_owned()))
            .expect("the fixture request builds")
    };
    common::send(&extractor_app(), request).await
}

/// Asserts the reply is one 422 carrying exactly `(type, loc)`.
fn assert_single_issue(reply: &Reply, kind: &str, loc: &Value) {
    assert_eq!(
        reply.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        reply.text()
    );
    let body = reply.json();
    let issues = body
        .get("detail")
        .and_then(Value::as_array)
        .expect("the detail is a list of issues");
    assert_eq!(issues.len(), 1, "{}", reply.text());
    let issue = issues.first().expect("the list holds one issue");
    assert_eq!(issue.get("type"), Some(&json!(kind)), "{}", reply.text());
    assert_eq!(issue.get("loc"), Some(loc), "{}", reply.text());
    assert!(
        issue
            .get("msg")
            .and_then(Value::as_str)
            .is_some_and(|msg| !msg.is_empty()),
        "{}",
        reply.text()
    );
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("application/json"),
        "{}",
        reply.text()
    );
}

#[tokio::test]
async fn a_query_value_of_the_wrong_type_names_the_parameter() {
    let reply = call(axum::http::Method::GET, "/list?limit=abc", &[], "").await;
    assert_single_issue(&reply, "int_parsing", &json!(["query", "limit"]));

    let reply = call(
        axum::http::Method::GET,
        "/list?include_stable=maybe",
        &[],
        "",
    )
    .await;
    assert_single_issue(&reply, "bool_parsing", &json!(["query", "include_stable"]));

    let reply = call(axum::http::Method::GET, "/list?sort=bogus", &[], "").await;
    assert_single_issue(&reply, "literal_error", &json!(["query", "sort"]));
}

#[tokio::test]
async fn a_query_parameter_with_a_rule_of_its_own_reports_it() {
    let reply = call(axum::http::Method::GET, "/list?limit=0", &[], "").await;
    assert_single_issue(&reply, "greater_than_equal", &json!(["query", "limit"]));
    assert_eq!(reply.json()["detail"][0]["ctx"], json!({"ge": 1}));

    let reply = call(axum::http::Method::GET, "/list?limit=501", &[], "").await;
    assert_single_issue(&reply, "less_than_equal", &json!(["query", "limit"]));
    assert_eq!(reply.json()["detail"][0]["ctx"], json!({"le": 500}));

    let reply = call(
        axum::http::Method::GET,
        "/list?name=waytoolongname",
        &[],
        "",
    )
    .await;
    assert_single_issue(&reply, "string_too_long", &json!(["query", "name"]));

    let reply = call(axum::http::Method::GET, "/list?limit=5", &[], "").await;
    assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test]
async fn a_path_parameter_of_the_wrong_type_names_the_parameter() {
    let reply = call(axum::http::Method::GET, "/test/abc", &[], "").await;
    assert_single_issue(&reply, "int_parsing", &json!(["path", "test_id"]));

    let reply = call(axum::http::Method::GET, "/test/7", &[], "").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.text(), "7");
}

#[tokio::test]
async fn a_path_parameter_the_router_cannot_fill_is_a_server_error() {
    let reply = call(axum::http::Method::GET, "/nested/anything", &[], "").await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        reply.json()["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("Path parameters cannot be read")),
        "{}",
        reply.text()
    );
}

#[tokio::test]
async fn a_single_path_parameter_of_the_wrong_type_names_the_path_only() {
    // A route whose whole path is one value has no key to name, as FastAPI's
    // `loc: ["path"]` had none.
    let reply = call(axum::http::Method::GET, "/plain/abc", &[], "").await;
    assert_single_issue(&reply, "int_parsing", &json!(["path"]));

    let reply = call(axum::http::Method::GET, "/plain/9", &[], "").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.text(), "9");
}

#[tokio::test]
async fn a_tuple_path_parameter_of_the_wrong_type_names_its_position() {
    let reply = call(axum::http::Method::GET, "/pair/1/two", &[], "").await;
    assert_single_issue(&reply, "int_parsing", &json!(["path", 1]));

    let reply = call(axum::http::Method::GET, "/pair/1/2", &[], "").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.text(), "(1, 2)");
}

#[tokio::test]
async fn a_path_parameter_that_is_not_utf8_is_a_unicode_issue() {
    // Percent-decoding runs before the handler, so a raw byte that is not UTF-8
    // reaches the extractor as a rejection rather than as a value.
    let reply = call(axum::http::Method::GET, "/test/%FF", &[], "").await;
    assert_single_issue(&reply, "string_unicode", &json!(["path", "test_id"]));
}

#[tokio::test]
async fn a_body_field_of_the_wrong_type_names_the_field() {
    let reply = call(
        axum::http::Method::POST,
        "/update",
        &[("content-type", "application/json")],
        r#"{"number": "twelve"}"#,
    )
    .await;
    assert_single_issue(&reply, "int_parsing", &json!(["body", "number"]));

    let reply = call(
        axum::http::Method::POST,
        "/update",
        &[("content-type", "application/json")],
        r#"{"number": 1, "quarantined": "yes"}"#,
    )
    .await;
    assert_single_issue(&reply, "bool_parsing", &json!(["body", "quarantined"]));
}

#[tokio::test]
async fn a_body_with_no_field_is_a_missing_field() {
    let reply = call(
        axum::http::Method::POST,
        "/update",
        &[("content-type", "application/json")],
        "{}",
    )
    .await;
    assert_single_issue(&reply, "missing", &json!(["body", "number"]));
}

#[tokio::test]
async fn an_empty_body_is_a_missing_body() {
    let reply = call(
        axum::http::Method::POST,
        "/update",
        &[("content-type", "application/json")],
        "",
    )
    .await;
    assert_single_issue(&reply, "missing", &json!(["body"]));
}

#[tokio::test]
async fn malformed_json_reports_the_position_of_the_error() {
    let reply = call(
        axum::http::Method::POST,
        "/update",
        &[("content-type", "application/json")],
        r#"{"number": }"#,
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    let issue = &reply.json()["detail"][0];
    assert_eq!(issue["type"], json!("json_invalid"));
    assert_eq!(issue["loc"][0], json!("body"));
    // Python reports the character index of the error, which is the offset of
    // the offending token.
    assert_eq!(issue["loc"][1], json!(11));
}

#[tokio::test]
async fn a_body_that_is_not_an_object_is_not_a_model() {
    for body in ["[1, 2]", "\"text\"", "17"] {
        let reply = call(
            axum::http::Method::POST,
            "/update",
            &[("content-type", "application/json")],
            body,
        )
        .await;
        assert_single_issue(&reply, "model_attributes_type", &json!(["body"]));
    }
}

#[tokio::test]
async fn a_body_without_a_json_content_type_is_raw_text_not_a_model() {
    // FastAPI left a body whose Content-Type is not JSON as bytes, so the model
    // rejected it rather than the framework refusing the content type.
    for headers in [
        &[][..],
        &[("content-type", "text/plain")][..],
        &[("content-type", "application/xml")][..],
    ] {
        let reply = call(
            axum::http::Method::POST,
            "/update",
            headers,
            r#"{"number": 1}"#,
        )
        .await;
        assert_single_issue(&reply, "model_attributes_type", &json!(["body"]));
        assert_eq!(
            reply.json()["detail"][0]["input"],
            json!(r#"{"number": 1}"#)
        );
    }
    // A JSON content type with parameters, and a structured suffix, both count.
    for content_type in [
        "application/json; charset=utf-8",
        "application/vnd.api+json",
    ] {
        let reply = call(
            axum::http::Method::POST,
            "/update",
            &[("content-type", content_type)],
            r#"{"number": 1}"#,
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{content_type}: {}",
            reply.text()
        );
    }
}

#[tokio::test]
async fn a_body_field_with_a_rule_of_its_own_reports_it() {
    let reply = call(
        axum::http::Method::POST,
        "/update",
        &[("content-type", "application/json")],
        r#"{"number": 1, "reason": "far too long"}"#,
    )
    .await;
    assert_single_issue(&reply, "string_too_long", &json!(["body", "reason"]));
    assert_eq!(reply.json()["detail"][0]["ctx"], json!({"max_length": 5}));

    let reply = call(
        axum::http::Method::POST,
        "/update",
        &[("content-type", "application/json")],
        r#"{"number": 1, "reason": "ok"}"#,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test]
async fn a_json_body_over_the_limit_is_refused() {
    let body = format!(
        "{{\"number\": 1, \"reason\": \"{}\"}}",
        "a".repeat(flakeradar::JSON_BODY_LIMIT)
    );
    let reply = call(
        axum::http::Method::POST,
        "/update",
        &[("content-type", "application/json")],
        &body,
    )
    .await;
    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
}
