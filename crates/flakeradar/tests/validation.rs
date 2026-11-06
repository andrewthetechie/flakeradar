//! The request validation Pydantic applied, case by case.
//!
//! Pydantic validated a JSON body in one pass with its lax rules and reported
//! every field it refused; the port parses with `serde`, which stops at the
//! first failure and accepts no other spelling of a value. Each test here pins
//! one row of what the reference at `86c98804` answered, recorded by sending the
//! same request to it, so the port answers what CI already sends.

#![expect(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test fails loudly when a route answers something else"
)]

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

/// The `(type, loc)` of every issue a 422 carries, which is what Pydantic's
/// `detail` list names and what the parity replay compares.
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
async fn stored_pipeline(db: &TestDb) -> (Option<i32>, Value) {
    use flakeradar_db::entities::reports;
    use sea_orm::{EntityTrait, QueryOrder};

    let row = reports::Entity::find()
        .order_by_desc(reports::Column::Id)
        .one(db.connection())
        .await
        .expect("the Report query")
        .expect("a Report is stored");
    (
        row.ci_run_attempt,
        serde_json::from_slice(&row.body).expect("the stored payload is JSON"),
    )
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
    let project = make_project()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("a Project");
    make_test_case(&project)
        .name("test_total")
        .classname("tests.test_cart")
        .build(db.connection())
        .await
        .expect("a Test")
        .id
}

#[tokio::test]
async fn the_pipeline_body_lists_every_missing_field() {
    // The reference's `{}` answer: all six required fields, in field order.
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
    // Two bad Jobs, one label each: the reference listed all three, and a label's
    // `loc` names its Job and its own position.
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
    // One row per shape: the reference's own type name, not `value_error`.
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
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY, "{over}");
        let (kind, loc) = expected;
        assert_eq!(issues(&reply), [(kind.to_owned(), loc)], "{over}");
    }
}

#[tokio::test]
async fn the_pipeline_body_reads_the_attempt_as_pydantic_did() {
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
        assert_eq!(
            reply.status,
            StatusCode::ACCEPTED,
            "{value}: {}",
            reply.text()
        );
        assert_eq!(stored_pipeline(&db).await.0, Some(attempt), "{value}");
    }
    // Refused spellings, with the reference's own kind.
    for (value, kind) in [
        (json!(false), "greater_than_equal"),
        (json!(0), "greater_than_equal"),
        (json!("2.5"), "int_parsing"),
        (json!(2.5), "int_from_float"),
        (json!(null), "int_type"),
        (json!(""), "int_parsing"),
    ] {
        let reply = pipeline(&app, &json!({"ci_run_attempt": value})).await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{value}: {}",
            reply.text()
        );
        assert_eq!(
            issues(&reply),
            [(kind.to_owned(), json!(["body", "ci_run_attempt"]))],
            "{value}"
        );
    }
}

#[tokio::test]
async fn a_pipeline_attempt_past_the_column_is_a_bad_request() {
    // Python's `int` had no ceiling and the driver raised, answering 500 after
    // the Repo row was created. The approved answer is the 400 ingest gives.
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

#[tokio::test]
async fn the_pipeline_body_reads_a_timestamp_the_way_pydantic_did() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    // Every spelling the reference accepted, and the instant it means.
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
        (json!(1_767_225_600_i64), "2026-01-01T00:00:00.000000Z"),
        (json!(1_767_225_600.5), "2026-01-01T00:00:00.500000Z"),
    ] {
        let reply = pipeline(
            &app,
            &json!({"jobs": [{"ci_job_id": "1", "name": "n", "status": "passed",
                              "started_at": value}]}),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::ACCEPTED,
            "{value}: {}",
            reply.text()
        );
        assert_eq!(
            stored_pipeline(&db).await.1["jobs"][0]["started_at"],
            json!(instant),
            "{value}"
        );
    }
    // Refused spellings.
    for (value, kind) in [
        (json!("20260101T000000Z"), "datetime_from_date_parsing"),
        (json!(""), "datetime_from_date_parsing"),
        (json!(true), "datetime_type"),
        (json!([]), "datetime_type"),
    ] {
        let reply = pipeline(
            &app,
            &json!({"jobs": [{"ci_job_id": "1", "name": "n", "status": "passed",
                              "started_at": value}]}),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{value}: {}",
            reply.text()
        );
        assert_eq!(
            issues(&reply),
            [(kind.to_owned(), json!(["body", "jobs", 0, "started_at"]))],
            "{value}"
        );
    }
}

#[tokio::test]
async fn the_pipeline_body_reads_a_bool_as_pydantic_did() {
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
        let reply = pipeline(
            &app,
            &json!({"jobs": [{"ci_job_id": "1", "name": "n", "status": "passed",
                              "summary": value}]}),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::ACCEPTED,
            "{value}: {}",
            reply.text()
        );
        assert_eq!(
            stored_pipeline(&db).await.1["jobs"][0]["summary"],
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
        let reply = pipeline(
            &app,
            &json!({"jobs": [{"ci_job_id": "1", "name": "n", "status": "passed",
                              "summary": value}]}),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{value}: {}",
            reply.text()
        );
        assert_eq!(
            issues(&reply),
            [(kind.to_owned(), json!(["body", "jobs", 0, "summary"]))],
            "{value}"
        );
    }
}

#[tokio::test]
async fn nul_in_a_request_body_is_refused() {
    // PostgreSQL `text` cannot hold a NUL, so Python answered 500 and stored
    // nothing. The approved answer is the 4xx the route already gives bad text.
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    for field in ["branch", "pipeline", "ci_run_id", "commit_sha"] {
        let reply = pipeline(&app, &json!({field: "a\u{0}b"})).await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{field}: {}",
            reply.text()
        );
        assert_eq!(
            issues(&reply),
            [("value_error".to_owned(), json!(["body", field]))],
            "{field}"
        );
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
    assert_eq!(
        reply.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        reply.text()
    );
    assert_eq!(
        issues(&reply),
        [("value_error".to_owned(), json!(["query", "commit_sha"]))]
    );
}

#[tokio::test]
async fn a_query_integer_takes_the_forms_pydantic_took() {
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
    // `true`, `0x10` and nothing at all are not integers to Pydantic either.
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
async fn a_query_issue_carries_the_value_pydantic_echoed() {
    // Measured from the reference at 86c98804: a refused query value's issue
    // echoes the raw text Pydantic read (`input`), a value outside a declared
    // range carries the constraint it broke (`ctx`), and a required parameter
    // that is absent is Pydantic's own `missing` issue, `input: null`.
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
}

#[tokio::test]
async fn a_page_whose_offset_does_not_fit_is_refused() {
    // Python's `page` had only `ge=1`, so the product overflowed and PostgreSQL
    // refused the offset. Both leaderboards answer the 4xx, whichever way the
    // product would have wrapped.
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    for uri in [
        "/api/tests?page=9223372036854775807",
        "/api/tests?page=4611686018427387905&page_size=4",
        "/api/jobs?page=9223372036854775807",
        "/api/jobs?page=4611686018427387905&page_size=4",
    ] {
        let reply = get(&app, uri).await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{uri}: {}",
            reply.text()
        );
        assert_eq!(
            issues(&reply),
            [("less_than_equal".to_owned(), json!(["query", "page"]))],
            "{uri}"
        );
    }
    // The same page size at the largest accepted page is served.
    let reply = get(&app, "/api/tests?page=1000000&page_size=100").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
}

#[tokio::test]
async fn the_ingest_attempt_takes_the_forms_pydantic_took() {
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
        assert_eq!(
            reply.status,
            StatusCode::ACCEPTED,
            "{value}: {}",
            reply.text()
        );
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
async fn the_quarantine_body_reads_its_numbers_as_pydantic_did() {
    let db = TestDb::new().await.expect("a test database is available");
    let id = one_test(&db).await;
    let app = test_app(&db);
    for body in [
        json!({"quarantined": "yes", "expires_in_days": "5"}),
        json!({"quarantined": true, "expires_in_days": "5.0"}),
        json!({"quarantined": true, "expires_in_days": " 5"}),
    ] {
        let reply = post(
            &app,
            &format!("/api/tests/{id}/quarantine"),
            &[("x-api-key", TOKEN), ("content-type", "application/json")],
            body.to_string(),
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
    ] {
        let reply = post(
            &app,
            &format!("/api/tests/{id}/quarantine"),
            &[("x-api-key", TOKEN), ("content-type", "application/json")],
            body.to_string(),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}: {}",
            reply.text()
        );
        assert_eq!(issues(&reply), [(kind.to_owned(), loc)], "{body}");
    }
}

#[tokio::test]
async fn the_issue_body_reads_its_number_as_pydantic_did() {
    let db = TestDb::new().await.expect("a test database is available");
    let id = one_test(&db).await;
    let app = test_app(&db);
    for body in [json!({"number": "5"}), json!({"number": 5.0})] {
        let reply = send(
            &app,
            axum::http::Request::builder()
                .method(axum::http::Method::PUT)
                .uri(format!("/api/tests/{id}/issue"))
                .header("x-api-key", TOKEN)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .expect("the fixture request builds"),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{body}: {}", reply.text());
        assert_eq!(reply.json()["github_issue_number"], json!(5), "{body}");
    }
    let reply = send(
        &app,
        axum::http::Request::builder()
            .method(axum::http::Method::PUT)
            .uri(format!("/api/tests/{id}/issue"))
            .header("x-api-key", TOKEN)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                json!({"number": 5, "url": 5}).to_string(),
            ))
            .expect("the fixture request builds"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        issues(&reply),
        [("string_type".to_owned(), json!(["body", "url"]))]
    );
}

#[tokio::test]
async fn the_quarantine_route_describes_its_body() {
    // The route's own topics: a body that is not an object, and one with no
    // `quarantined`, keep the shapes the reference gave them.
    let db = TestDb::new().await.expect("a test database is available");
    let id = one_test(&db).await;
    let app = test_app(&db);
    let reply = post(
        &app,
        &format!("/api/tests/{id}/quarantine"),
        &[("x-api-key", TOKEN), ("content-type", "application/json")],
        json!({}).to_string(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        issues(&reply),
        [("missing".to_owned(), json!(["body", "quarantined"]))]
    );
}
