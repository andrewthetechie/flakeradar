//! The request validation this API applies, case by case.
//!
//! A JSON body is validated in one pass with the lax rules and every field it
//! refuses is reported; `serde` alone stops at the first failure and accepts no
//! other spelling of a value, so the readers are this crate's own. Each test
//! pins one recorded answer, so the server answers what CI already sends, and
//! states it through [`assert_refused`] or [`assert_accepted`]: a table of cases
//! reads as the table, with the assertions out of the way.

#![expect(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test fails loudly when a route answers something else"
)]

use std::fmt::Write as _;

use axum::http::StatusCode;
use flakeradar_test_support::{
    Reply, TestDb, app_with, get, make_project, make_test_case, post, send, settings,
};

use serde_json::{Value, json};

/// The token the tests present.
const TOKEN: &str = "validation-token";

/// An app over `db` with the test's token.
fn test_app(db: &TestDb) -> axum::Router {
    app_with(db, settings(&[("FLAKERADAR_API_TOKEN", TOKEN)]))
}

/// The `(type, loc)` of every issue a 422 carries, which is what the `detail`
/// list names and what the parity replay compares.
fn issues(reply: &Reply) -> Vec<(String, Value)> {
    reply.json()["detail"]
        .as_array()
        .unwrap_or_else(|| panic!("a 422 carries a detail list: {}", reply.text()))
        .iter()
        .map(|issue| {
            (
                issue["type"]
                    .as_str()
                    .expect("every issue names its type")
                    .to_owned(),
                issue["loc"].clone(),
            )
        })
        .collect()
}

/// Asserts that `reply` is the 422 whose only issue is `kind` at `loc`.
#[track_caller]
fn assert_refused(reply: &Reply, kind: &str, loc: Value, label: impl std::fmt::Display) {
    assert_eq!(
        reply.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{label}: {}",
        reply.text()
    );
    assert_eq!(issues(reply), [(kind.to_owned(), loc)], "{label}");
}

/// Asserts that `reply` took the body it was sent.
#[track_caller]
fn assert_accepted(reply: &Reply, label: impl std::fmt::Display) {
    assert_eq!(
        reply.status,
        StatusCode::ACCEPTED,
        "{label}: {}",
        reply.text()
    );
}

/// One job of a Pipeline report body, with `field` set to `value`.
fn one_job(field: &str, value: &Value) -> Value {
    json!({"ci_job_id": "1", "name": "n", "status": "passed", field: value})
}

/// Sends `body` as JSON with the token, as `method`, to `uri`.
async fn send_json(
    app: &axum::Router,
    method: axum::http::Method,
    uri: &str,
    body: &Value,
) -> Reply {
    send(
        app,
        axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("x-api-key", TOKEN)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .expect("the fixture request builds"),
    )
    .await
}

/// A Pipeline report body, with `over` replacing its top-level fields.
fn pipeline_payload(over: &Value) -> Vec<u8> {
    let mut body = json!({
        "repo": "acme/app",
        "provider": "github",
        "pipeline": ".github/workflows/ci.yml",
        "commit_sha": "deadbeef",
        "branch": "main",
        "ci_run_id": "42",
        "ci_run_attempt": 1,
        "jobs": [{"ci_job_id": "100", "name": "test", "status": "passed"}]
    });
    if let (Some(target), Some(source)) = (body.as_object_mut(), over.as_object()) {
        for (key, value) in source {
            let _ = target.insert(key.clone(), value.clone());
        }
    }
    serde_json::to_vec(&body).expect("the fixture serializes")
}

/// Posts one Pipeline report body to the route, `over` replacing its fields.
async fn pipeline(app: &axum::Router, over: &Value) -> Reply {
    pipeline_body(app, &pipeline_payload(over)).await
}

/// Posts the exact Pipeline report body `body`.
async fn pipeline_body(app: &axum::Router, body: &[u8]) -> Reply {
    post(
        app,
        "/api/ingest/pipeline",
        &[("x-api-key", TOKEN), ("content-type", "application/json")],
        body.to_vec(),
    )
    .await
}

/// The `ci_run_attempt` of the newest stored Report, and its stored payload.
async fn stored_pipeline(db: &TestDb) -> StoredPipeline {
    use flakeradar_db::entities::reports;
    use sea_orm::{EntityTrait, QueryOrder};

    let row = reports::Entity::find()
        .order_by_desc(reports::Column::Id)
        .one(db.connection())
        .await
        .expect("the Report query")
        .expect("a Report is stored");
    StoredPipeline {
        attempt: row.ci_run_attempt,
        body: serde_json::from_slice(&row.body).expect("the stored payload is JSON"),
    }
}

/// The newest stored Pipeline Report: the attempt it kept and its body.
struct StoredPipeline {
    attempt: Option<i32>,
    body: Value,
}

/// The `ci_run_attempt` of the newest stored Report.
async fn newest_report_attempt(db: &TestDb) -> Option<i32> {
    use flakeradar_db::entities::reports;
    use sea_orm::{EntityTrait as _, QueryOrder};

    reports::Entity::find()
        .order_by_desc(reports::Column::Id)
        .one(db.connection())
        .await
        .expect("the Report query")
        .expect("a Report is stored")
        .ci_run_attempt
}

/// A `GET` with the token, for a route behind it.
async fn authorised_get(app: &axum::Router, uri: &str) -> Reply {
    send(
        app,
        axum::http::Request::builder()
            .method(axum::http::Method::GET)
            .uri(uri)
            .header("x-api-key", TOKEN)
            .body(axum::body::Body::empty())
            .expect("the fixture request builds"),
    )
    .await
}

/// One Test, so a write route reaches its own rules instead of a 404.
async fn one_test(db: &TestDb) -> i32 {
    let project = make_project().repo("acme/app").create(db).await;
    make_test_case(&project)
        .name("test_total")
        .classname("tests.test_cart")
        .create(db)
        .await
        .id
}

#[tokio::test]
async fn the_pipeline_body_lists_every_missing_field() {
    // An empty body is one issue per required field, in field order.
    let db = TestDb::new().await.expect("a test database is available");
    let reply = pipeline_body(&test_app(&db), b"{}").await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        issues(&reply),
        [
            ("missing".to_owned(), json!(["body", "repo"])),
            ("missing".to_owned(), json!(["body", "provider"])),
            ("missing".to_owned(), json!(["body", "pipeline"])),
            ("missing".to_owned(), json!(["body", "commit_sha"])),
            ("missing".to_owned(), json!(["body", "branch"])),
            ("missing".to_owned(), json!(["body", "jobs"])),
        ]
    );
}

#[tokio::test]
async fn the_pipeline_body_lists_every_job_issue() {
    // Two bad Jobs, one label each: every issue is listed, and a label's `loc`
    // names its Job and its own position.
    let db = TestDb::new().await.expect("a test database is available");
    let reply = pipeline(
        &test_app(&db),
        &json!({
            "jobs": [
                {"ci_job_id": "", "name": "", "status": "passed"},
                {"ci_job_id": "b", "name": "x", "status": "passed",
                 "runner_labels": ["ok", "z".repeat(256)]}
            ]
        }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        issues(&reply),
        [
            (
                "string_too_short".to_owned(),
                json!(["body", "jobs", 0, "ci_job_id"])
            ),
            (
                "string_too_short".to_owned(),
                json!(["body", "jobs", 0, "name"])
            ),
            (
                "string_too_long".to_owned(),
                json!(["body", "jobs", 1, "runner_labels", 1])
            ),
        ]
    );
}

#[tokio::test]
async fn the_pipeline_body_names_a_field_of_the_wrong_type() {
    // One row per shape: each carries the issue type of the value it read, not
    // a single `value_error`.
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    let cases: [(Value, (&str, Value)); 11] = [
        (json!({"repo": 5}), ("string_type", json!(["body", "repo"]))),
        (
            json!({"pipeline": null}),
            ("string_type", json!(["body", "pipeline"])),
        ),
        (json!({"jobs": "x"}), ("list_type", json!(["body", "jobs"]))),
        (json!({"jobs": []}), ("too_short", json!(["body", "jobs"]))),
        (
            json!({"jobs": [5]}),
            ("model_attributes_type", json!(["body", "jobs", 0])),
        ),
        (
            json!({"jobs": [{"ci_job_id": "1", "name": "n", "status": 5}]}),
            ("literal_error", json!(["body", "jobs", 0, "status"])),
        ),
        (
            json!({"jobs": [{"ci_job_id": 5, "name": "n", "status": "passed"}]}),
            ("string_type", json!(["body", "jobs", 0, "ci_job_id"])),
        ),
        (
            json!({"jobs": [{"ci_job_id": "1", "name": "n", "status": "passed",
                             "runner_labels": "x"}]}),
            ("list_type", json!(["body", "jobs", 0, "runner_labels"])),
        ),
        (
            json!({"ci_run_id": 5}),
            ("string_type", json!(["body", "ci_run_id"])),
        ),
        (
            json!({"default_branch": 5}),
            ("string_type", json!(["body", "default_branch"])),
        ),
        (
            json!({"jobs": [{"ci_job_id": "1", "name": "n", "status": "passed",
                             "runner_labels": [5]}]}),
            (
                "string_type",
                json!(["body", "jobs", 0, "runner_labels", 0]),
            ),
        ),
    ];
    for (over, expected) in cases {
        let reply = pipeline(&app, &over).await;
        let (kind, loc) = expected;
        assert_refused(&reply, kind, loc, &over);
    }
}

#[tokio::test]
async fn the_pipeline_body_reads_the_attempt_as_documented() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    // Accepted spellings, and the attempt each one means.
    for (value, attempt) in [
        (json!("2"), 2),
        (json!(2.0), 2),
        (json!(true), 1),
        (json!(" 2 "), 2),
        (json!("1_0"), 10),
    ] {
        let reply = pipeline(&app, &json!({"ci_run_attempt": value})).await;
        assert_accepted(&reply, &value);
        assert_eq!(stored_pipeline(&db).await.attempt, Some(attempt), "{value}");
    }
    // Refused spellings, each with the kind that names what was read.
    for (value, kind) in [
        (json!(false), "greater_than_equal"),
        (json!(0), "greater_than_equal"),
        (json!("2.5"), "int_parsing"),
        (json!(2.5), "int_from_float"),
        (json!(null), "int_type"),
        (json!(""), "int_parsing"),
        // A float past `i64::MAX` names no integer at all, so it is refused for
        // its size rather than its range.
        (json!(1e19), "int_parsing_size"),
    ] {
        let reply = pipeline(&app, &json!({"ci_run_attempt": value})).await;
        assert_refused(&reply, kind, json!(["body", "ci_run_attempt"]), &value);
    }
}

#[tokio::test]
async fn a_pipeline_attempt_past_the_column_is_a_bad_request() {
    // An integer type with no ceiling let this reach the driver, which raised and
    // answered 500 after the Repo row was created. The approved answer is the 400
    // ingest gives.
    let db = TestDb::new().await.expect("a test database is available");
    let reply = pipeline(
        &test_app(&db),
        &json!({"ci_run_attempt": 3_000_000_000_i64}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.text());
    assert_eq!(
        reply.json(),
        json!({"detail": "ci_run_attempt must be at most 2147483647"})
    );
}

/// Posts `value` as one Job's `field` and answers the value the body stored.
async fn stored_timestamp(app: &axum::Router, db: &TestDb, field: &str, value: &Value) -> Value {
    let reply = pipeline(app, &json!({"jobs": [one_job(field, value)]})).await;
    assert_accepted(&reply, value);
    stored_pipeline(db).await.body["jobs"][0][field].clone()
}

/// Posts `value` as one Job's `field` and answers the reply.
async fn refused_timestamp(app: &axum::Router, field: &str, value: &Value) -> Reply {
    pipeline(app, &json!({"jobs": [one_job(field, value)]})).await
}

#[tokio::test]
async fn the_pipeline_body_reads_a_timestamp_the_way_the_wire_does() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    // Every spelling this reader takes, and the instant it means, with the
    // reference's own answer for each. The epoch rows are measured from it
    // rather than inferred; the command is in `STATUS.md`'s note for this task.
    for (value, instant) in [
        (json!("2026-01-01T00:00:00Z"), "2026-01-01T00:00:00.000000Z"),
        (
            json!("2026-01-01T00:00:00+02:00"),
            "2025-12-31T22:00:00.000000Z",
        ),
        (json!("2026-01-01T00:00:00"), "2026-01-01T00:00:00.000000Z"),
        (json!("2026-01-01 00:00:00"), "2026-01-01T00:00:00.000000Z"),
        (json!("2026-01-01T00:00"), "2026-01-01T00:00:00.000000Z"),
        (json!("2026-01-01"), "2026-01-01T00:00:00.000000Z"),
        (
            json!("2026-01-01T00:00:00,5"),
            "2026-01-01T00:00:00.500000Z",
        ),
        (json!("2026-01-01T00:00:00z"), "2026-01-01T00:00:00.000000Z"),
        // A number at or below 2e10 in magnitude is seconds, and past it is
        // milliseconds: the value `Date.now()` gives is 2026, not the year 57971.
        (json!(1_767_225_600_i64), "2026-01-01T00:00:00.000000Z"),
        (json!(1_767_225_600.5), "2026-01-01T00:00:00.500000Z"),
        (json!(20_000_000_000_i64), "2603-10-11T11:33:20.000000Z"),
        (json!(20_000_000_001_i64), "1970-08-20T11:33:20.001000Z"),
        (json!(-20_000_000_001_i64), "1969-05-14T12:26:39.999000Z"),
        (json!(1_767_225_600_000_i64), "2026-01-01T00:00:00.000000Z"),
        (json!(1_767_225_600_000.5), "2026-01-01T00:00:00.000500Z"),
        (json!(1_767_225_600_123_i64), "2026-01-01T00:00:00.123000Z"),
        (json!(253_402_300_799_i64), "1978-01-11T21:31:40.799000Z"),
        (json!(253_402_300_800_i64), "1978-01-11T21:31:40.800000Z"),
        (json!(1e14), "5138-11-16T09:46:40.000000Z"),
        // The exact edges of the years a datetime holds: the last millisecond of
        // 9999 is accepted and the first of the year 10000 is not, as is the
        // first instant of year 1.
        (
            json!(253_402_300_799_999_i64),
            "9999-12-31T23:59:59.999000Z",
        ),
        (
            json!(-62_135_596_800_000_i64),
            "0001-01-01T00:00:00.000000Z",
        ),
        // A string holding a plain decimal number is the same value.
        (json!("1767225600"), "2026-01-01T00:00:00.000000Z"),
        (json!("1767225600.5"), "2026-01-01T00:00:00.500000Z"),
        (json!("1767225600000"), "2026-01-01T00:00:00.000000Z"),
        (json!("1700000000"), "2023-11-14T22:13:20.000000Z"),
        (json!("0"), "1970-01-01T00:00:00.000000Z"),
        (json!("-5"), "1969-12-31T23:59:55.000000Z"),
        // A fraction that rounds up to the whole microsecond carries into the
        // next second.
        (
            json!(1_767_225_600.0 + 0.999_999_9),
            "2026-01-01T00:00:01.000000Z",
        ),
    ] {
        assert_eq!(
            stored_timestamp(&app, &db, "started_at", &value).await,
            json!(instant),
            "{value}"
        );
    }
    // Refused spellings. `1e15` is past the year 9999, and a string holding a
    // plain decimal number is refused the way a string is.
    for (value, kind) in [
        (json!("20260101T000000Z"), "datetime_from_date_parsing"),
        (json!(""), "datetime_from_date_parsing"),
        (json!("1e3"), "datetime_from_date_parsing"),
        (json!("1_000"), "datetime_from_date_parsing"),
        (json!(" 1767225600"), "datetime_from_date_parsing"),
        (json!("253402300800000"), "datetime_from_date_parsing"),
        (json!(1e15), "datetime_parsing"),
        (json!(253_402_300_800_000_i64), "datetime_parsing"),
        // Past the year 9999, and past the year 0 the millisecond reading can
        // name: each answers the sentence its own side of the range has.
        (json!(1e16), "datetime_parsing"),
        (json!(-1e16), "datetime_parsing"),
        (json!(-1e30), "datetime_parsing"),
        (json!(-62_135_596_800_001_i64), "datetime_parsing"),
        (json!(-62_200_000_000_000_i64), "datetime_parsing"),
        (json!(true), "datetime_type"),
        (json!([]), "datetime_type"),
    ] {
        let reply = refused_timestamp(&app, "started_at", &value).await;
        assert_refused(
            &reply,
            kind,
            json!(["body", "jobs", 0, "started_at"]),
            &value,
        );
    }
    // `completed_at` goes through the same reader, so it earns the same answer.
    for (value, instant) in [
        (json!(1_767_225_600_000_i64), "2026-01-01T00:00:00.000000Z"),
        (json!("1767225600"), "2026-01-01T00:00:00.000000Z"),
    ] {
        assert_eq!(
            stored_timestamp(&app, &db, "completed_at", &value).await,
            json!(instant),
            "{value}"
        );
    }
    let past_the_range = json!(1e15);
    let reply = refused_timestamp(&app, "completed_at", &past_the_range).await;
    assert_refused(
        &reply,
        "datetime_parsing",
        json!(["body", "jobs", 0, "completed_at"]),
        &past_the_range,
    );
}

#[tokio::test]
async fn the_pipeline_body_reads_a_bool_as_the_wire_does() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    for (value, summary) in [
        (json!("true"), true),
        (json!("yes"), true),
        (json!("on"), true),
        (json!("T"), true),
        (json!(1), true),
        (json!(0), false),
        (json!(1.0), true),
    ] {
        let reply = pipeline(&app, &json!({"jobs": [one_job("summary", &value)]})).await;
        assert_accepted(&reply, &value);
        assert_eq!(
            stored_pipeline(&db).await.body["jobs"][0]["summary"],
            json!(summary),
            "{value}"
        );
    }
    for (value, kind) in [
        (json!("maybe"), "bool_parsing"),
        (json!(2), "bool_parsing"),
        (json!([]), "bool_type"),
        (json!(null), "bool_type"),
    ] {
        let reply = pipeline(&app, &json!({"jobs": [one_job("summary", &value)]})).await;
        assert_refused(&reply, kind, json!(["body", "jobs", 0, "summary"]), &value);
    }
}

#[tokio::test]
async fn nul_in_a_request_body_is_refused() {
    // PostgreSQL `text` cannot hold a NUL, so the insert answered 500 and stored
    // nothing. The approved answer is the 4xx the route already gives bad text.
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    for field in ["branch", "pipeline", "ci_run_id", "commit_sha"] {
        let reply = pipeline(&app, &json!({field: "a\u{0}b"})).await;
        assert_refused(&reply, "value_error", json!(["body", field]), field);
    }
}

#[tokio::test]
async fn nul_in_a_query_is_refused() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    for uri in [
        "/api/tests?q=a%00b",
        "/api/jobs?q=a%00b",
        "/api/tests?file=a%00b",
        "/api/jobs?pipeline=a%00b",
    ] {
        let reply = get(&app, uri).await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{uri}: {}",
            reply.text()
        );
    }
    let reply = post(
        &app,
        "/api/ingest?repo=acme/app&commit_sha=abc%00",
        &[("x-api-key", TOKEN), ("content-type", "application/xml")],
        "<testsuites><testsuite name=\"s\" tests=\"1\"><testcase classname=\"c\" name=\"t\"/></testsuite></testsuites>",
    )
    .await;
    assert_refused(
        &reply,
        "value_error",
        json!(["query", "commit_sha"]),
        "a NUL in the query",
    );
}

#[tokio::test]
async fn a_query_integer_takes_the_documented_forms() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    for (uri, page, page_size) in [
        ("/api/tests?page=1.0", 1, 50),
        ("/api/tests?page=+2", 2, 50),
        ("/api/tests?page=%201%20", 1, 50),
        ("/api/tests?page=1_0", 10, 50),
        ("/api/tests?page_size=1_0", 1, 10),
        ("/api/tests?page_size=1.0", 1, 1),
        ("/api/tests?page_size=%2050", 1, 50),
        ("/api/jobs?page=2.0", 2, 50),
        ("/api/tests?new_within_days=7.0", 1, 50),
    ] {
        let reply = get(&app, uri).await;
        assert_eq!(reply.status, StatusCode::OK, "{uri}: {}", reply.text());
        assert_eq!(reply.json()["page"], json!(page), "{uri}");
        assert_eq!(reply.json()["page_size"], json!(page_size), "{uri}");
    }
    assert_eq!(
        authorised_get(&app, "/api/reports?limit=1.0").await.status,
        StatusCode::OK
    );
    assert_eq!(
        authorised_get(&app, "/api/reports?limit=1_0").await.status,
        StatusCode::OK
    );
    // `true`, `0x10` and nothing at all are no integers either.
    for uri in [
        "/api/tests?page=true",
        "/api/tests?page=0x10",
        "/api/tests?page=",
        "/api/reports?limit=true",
    ] {
        let reply = if uri.starts_with("/api/reports") {
            authorised_get(&app, uri).await
        } else {
            get(&app, uri).await
        };
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{uri}: {}",
            reply.text()
        );
        assert_eq!(issues(&reply)[0].0, "int_parsing", "{uri}");
    }
}

#[tokio::test]
async fn a_query_issue_carries_the_value_the_request_sent() {
    // A refused query value's issue echoes the raw text the request sent
    // (`input`), a value outside a declared range carries the constraint it broke
    // (`ctx`), and a required parameter that is absent is the `missing` issue with
    // `input: null`.
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    for (uri, expected) in [
        (
            "/api/tests?include_stable=maybe",
            json!([{
                "type": "bool_parsing",
                "loc": ["query", "include_stable"],
                "msg": "Input should be a valid boolean, unable to interpret input",
                "input": "maybe",
            }]),
        ),
        (
            "/api/tests?page=abc",
            json!([{
                "type": "int_parsing",
                "loc": ["query", "page"],
                "msg": "Input should be a valid integer, unable to parse string as an integer",
                "input": "abc",
            }]),
        ),
        (
            "/api/tests?page=0",
            json!([{
                "type": "greater_than_equal",
                "loc": ["query", "page"],
                "msg": "Input should be greater than or equal to 1",
                "input": "0",
                "ctx": {"ge": 1},
            }]),
        ),
        (
            "/api/tests?page_size=101",
            json!([{
                "type": "less_than_equal",
                "loc": ["query", "page_size"],
                "msg": "Input should be less than or equal to 100",
                "input": "101",
                "ctx": {"le": 100},
            }]),
        ),
        (
            "/api/tests?page=%201%20&page_size=x",
            json!([{
                "type": "int_parsing",
                "loc": ["query", "page_size"],
                "msg": "Input should be a valid integer, unable to parse string as an integer",
                "input": "x",
            }]),
        ),
    ] {
        let reply = get(&app, uri).await;
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY, "{uri}");
        assert_eq!(reply.json()["detail"], expected, "{uri}");
    }
    let reply = authorised_get(&app, "/api/quarantine").await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        reply.json()["detail"],
        json!([{
            "type": "missing",
            "loc": ["query", "repo"],
            "msg": "Field required",
            "input": null,
        }])
    );
}

#[tokio::test]
async fn a_repeated_query_parameter_takes_the_last_value() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    let reply = get(&app, "/api/tests?page=1&page=2").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.json()["page"], json!(2));
    let reply = get(&app, "/api/jobs?page=1&page=2&page=3").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.json()["page"], json!(3));
    let reply = get(&app, "/api/tests?include_stable=false&include_stable=true").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    // A name that comes back between other names keeps its first position and its
    // last value, and a query holding thousands of names behaves the same: that
    // is the shape the name lookup is built for, not a timing claim.
    let reply = get(&app, "/api/tests?page=1&page_size=10&page=2").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.json()["page"], json!(2));
    assert_eq!(reply.json()["page_size"], json!(10));
    let mut query = String::from("page=1");
    for name in 0..1_024 {
        write!(query, "&k{name}={name}").expect("the query builds");
    }
    query.push_str("&page=3");
    let reply = get(&app, &format!("/api/tests?{query}")).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.json()["page"], json!(3));
}

#[tokio::test]
async fn a_page_whose_offset_does_not_fit_is_refused() {
    // Bounding only `ge=1` let the product overflow, and PostgreSQL refused the
    // offset. Both leaderboards answer the 4xx, whichever way the product would
    // have wrapped.
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    for uri in [
        "/api/tests?page=9223372036854775807",
        "/api/tests?page=4611686018427387905&page_size=4",
        "/api/jobs?page=9223372036854775807",
        "/api/jobs?page=4611686018427387905&page_size=4",
    ] {
        let reply = get(&app, uri).await;
        assert_refused(&reply, "less_than_equal", json!(["query", "page"]), uri);
    }
    // The same page size at the largest accepted page is served.
    let reply = get(&app, "/api/tests?page=1000000&page_size=100").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
}

#[tokio::test]
async fn the_ingest_attempt_takes_the_documented_forms() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    let report = "<testsuites><testsuite name=\"s\" tests=\"1\">\
                  <testcase classname=\"c\" name=\"t\"/></testsuite></testsuites>";
    for (value, attempt) in [("2", 2), ("2.0", 2), ("%203", 3), ("1_0", 10)] {
        let uri = format!("/api/ingest?repo=acme/app&commit_sha=abc&ci_run_attempt={value}");
        let reply = post(
            &app,
            &uri,
            &[("x-api-key", TOKEN), ("content-type", "application/xml")],
            report,
        )
        .await;
        assert_accepted(&reply, value);
        assert_eq!(newest_report_attempt(&db).await, Some(attempt), "{value}");
    }
    // A number past the column answers the shared 400, on this route too.
    let reply = post(
        &app,
        "/api/ingest?repo=acme/app&commit_sha=abc&ci_run_attempt=3000000000",
        &[("x-api-key", TOKEN), ("content-type", "application/xml")],
        report,
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.text());
    assert_eq!(
        reply.json(),
        json!({"detail": "ci_run_attempt must be at most 2147483647"})
    );
}

#[tokio::test]
async fn the_quarantine_body_reads_its_numbers_as_documented() {
    let db = TestDb::new().await.expect("a test database is available");
    let id = one_test(&db).await;
    let app = test_app(&db);
    for body in [
        json!({"quarantined": "yes", "expires_in_days": "5"}),
        json!({"quarantined": true, "expires_in_days": "5.0"}),
        json!({"quarantined": true, "expires_in_days": " 5"}),
    ] {
        let reply = send_json(
            &app,
            axum::http::Method::POST,
            &format!("/api/tests/{id}/quarantine"),
            &body,
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{body}: {}", reply.text());
        assert_eq!(reply.json()["quarantined"], json!(true), "{body}");
    }
    for (body, kind, loc) in [
        (
            json!({"quarantined": true, "reason": 5}),
            "string_type",
            json!(["body", "reason"]),
        ),
        (
            json!({"quarantined": true, "expires_in_days": [1]}),
            "int_type",
            json!(["body", "expires_in_days"]),
        ),
        (
            json!({"quarantined": true, "expires_in_days": 1e19}),
            "int_parsing_size",
            json!(["body", "expires_in_days"]),
        ),
    ] {
        let reply = send_json(
            &app,
            axum::http::Method::POST,
            &format!("/api/tests/{id}/quarantine"),
            &body,
        )
        .await;
        assert_refused(&reply, kind, loc, &body);
    }
}

#[tokio::test]
async fn the_issue_body_reads_its_number_as_documented() {
    let db = TestDb::new().await.expect("a test database is available");
    let id = one_test(&db).await;
    let app = test_app(&db);
    for body in [json!({"number": "5"}), json!({"number": 5.0})] {
        let reply = send_json(
            &app,
            axum::http::Method::PUT,
            &format!("/api/tests/{id}/issue"),
            &body,
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{body}: {}", reply.text());
        assert_eq!(reply.json()["github_issue_number"], json!(5), "{body}");
    }
    let bad_url = json!({"number": 5, "url": 5});
    let reply = send_json(
        &app,
        axum::http::Method::PUT,
        &format!("/api/tests/{id}/issue"),
        &bad_url,
    )
    .await;
    assert_refused(&reply, "string_type", json!(["body", "url"]), &bad_url);
    // A float past `i64::MAX` names no integer, so it is the size refusal and
    // not the ceiling the stored column has.
    let past_i64 = json!({"number": 1e19});
    let reply = send_json(
        &app,
        axum::http::Method::PUT,
        &format!("/api/tests/{id}/issue"),
        &past_i64,
    )
    .await;
    assert_refused(
        &reply,
        "int_parsing_size",
        json!(["body", "number"]),
        &past_i64,
    );
}

#[tokio::test]
async fn the_quarantine_route_describes_its_body() {
    // The route's own topics: a body that is not an object, and one with no
    // `quarantined`, each keep the shape their rules answer.
    let db = TestDb::new().await.expect("a test database is available");
    let id = one_test(&db).await;
    let app = test_app(&db);
    let empty = json!({});
    let reply = send_json(
        &app,
        axum::http::Method::POST,
        &format!("/api/tests/{id}/quarantine"),
        &empty,
    )
    .await;
    assert_refused(&reply, "missing", json!(["body", "quarantined"]), &empty);
}
