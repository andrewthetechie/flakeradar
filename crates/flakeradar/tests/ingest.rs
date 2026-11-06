//! Ingest: validate an upload, queue a pending Report, answer 202.
//!
//! The behaviour is `backend/tests/test_ingest_api.py` and
//! `test_pipeline_ingest_api.py`: what the queue holds after an upload, what a
//! retried upload answers, and every refusal `POST /api/ingest` gives.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when ingest answers something else"
)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, StatusCode};
use chrono::{DateTime, FixedOffset, TimeZone as _, Utc};
use flakeradar_core::ReportStatus;
use flakeradar_db::entities::{projects, reports, repos};
use flakeradar_test_support::{
    FIXTURE_INSTANT, FixedClock, Reply, TestDb, app_with, get, make_junit, make_project, post,
    request, send, settings,
};

use sea_orm::{
    ColumnTrait, ConnectionTrait as _, DbErr, EntityTrait, PaginatorTrait as _, QueryFilter,
    QueryOrder,
};
use serde_json::{Value, json};

/// The token the tests present.
const TOKEN: &str = "ingest-token";

/// The settings every test runs with: a known token and nothing else.
fn test_settings() -> flakeradar::Settings {
    settings(&[("FLAKERADAR_API_TOKEN", TOKEN)])
}

/// An app over `db` with the test's settings.
fn test_app(db: &TestDb) -> Router {
    app_with(db, test_settings())
}

/// The app over `db`, on a clock that always reads `instant`.
fn app_at(db: &TestDb, instant: DateTime<FixedOffset>) -> Router {
    let state = flakeradar::AppState::new(
        db.connection().clone(),
        std::sync::Arc::new(test_settings()),
    )
    .expect("the app state builds")
    .with_clock(std::sync::Arc::new(FixedClock::new(instant)));
    flakeradar::build_app(state)
}

/// The headers an authorised upload sends.
fn auth() -> [(&'static str, &'static str); 1] {
    [("x-api-key", TOKEN)]
}

/// One ingest request, before its body and headers are chosen.
fn ingest_uri(params: &[(&str, &str)]) -> String {
    let mut base: Vec<(&str, &str)> = vec![("repo", "Acme/App"), ("commit_sha", "sha1")];
    for (key, value) in params {
        base.retain(|(existing, _)| existing != key);
        base.push((key, value));
    }
    let query: Vec<String> = base
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                form_urlencoded::byte_serialize(key.as_bytes()).collect::<String>(),
                form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>()
            )
        })
        .collect();
    format!("/api/ingest?{}", query.join("&"))
}

/// Uploads `body` to `/api/ingest` with the test's token.
async fn ingest(app: &Router, params: &[(&str, &str)], body: Vec<u8>) -> Reply {
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri(ingest_uri(params))
        .header("x-api-key", TOKEN)
        .body(Body::from(body))
        .expect("the fixture request builds");
    send(app, request).await
}

/// One uploaded report: a single passing Test.
fn passing_report() -> Vec<u8> {
    make_junit(&[("t1", "passed")])
}

/// Every Report the database holds, oldest first.
async fn stored_reports(db: &TestDb) -> Vec<reports::Model> {
    reports::Entity::find()
        .filter(reports::Column::Id.gte(0))
        .order_by_asc(reports::Column::Id)
        .all(db.connection())
        .await
        .expect("the Report rows")
}

#[tokio::test]
async fn ingest_requires_token() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = post(&test_app(&db), &ingest_uri(&[]), &[], passing_report()).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        reply.json(),
        json!({"detail": "Invalid or missing X-API-Key"})
    );
    assert_eq!(stored_reports(&db).await.len(), 0);
}

/// The instant the clock stops at, which no wall clock would agree with.
fn frozen_instant() -> DateTime<FixedOffset> {
    Utc.with_ymd_and_hms(2001, 2, 3, 4, 5, 6)
        .unwrap()
        .fixed_offset()
}

#[tokio::test]
async fn a_report_is_stamped_with_the_states_clock() {
    let db = TestDb::new().await.expect("a test database is available");
    let instant = frozen_instant();
    let reply = ingest(&app_at(&db, instant), &[], passing_report()).await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());

    let stored = stored_reports(&db).await;
    let report = stored.first().expect("one Report");
    assert_eq!(
        report.created_at, instant,
        "the Report was stamped with the state's clock, not the wall clock"
    );
}

#[tokio::test]
async fn a_pipeline_report_is_stamped_with_the_states_clock() {
    let db = TestDb::new().await.expect("a test database is available");
    let instant = frozen_instant();
    let reply = post(
        &app_at(&db, instant),
        "/api/ingest/pipeline",
        &[("x-api-key", TOKEN), ("content-type", "application/json")],
        pipeline_report_body(&json!({})),
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());

    let stored = stored_reports(&db).await;
    let report = stored.first().expect("one Report");
    assert_eq!(report.created_at, instant);
}

#[tokio::test]
async fn ingest_queues_report() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(
        &test_app(&db),
        &[
            ("project", "Backend"),
            ("root", "./server/"),
            ("branch", "feat"),
            ("ci_run_id", "42-1"),
        ],
        passing_report(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
    assert_eq!(reply.json()["status"], json!("pending"));
    assert_eq!(reply.json()["duplicate"], json!(false));

    let stored = stored_reports(&db).await;
    assert_eq!(stored.len(), 1);
    let report = stored.first().expect("one Report");
    assert_eq!(report.status, ReportStatus::Pending.as_str());
    assert_eq!(report.commit_sha, "sha1");
    assert_eq!(report.branch, "feat");
    assert_eq!(report.ci_run_id, "42-1");
    assert_eq!(report.root.as_deref(), Some("server"));
    assert_eq!(report.body, passing_report());
    assert_eq!(report.kind, "junit");
    assert_eq!(reply.json()["report_id"], json!(report.id));

    let project = projects::Entity::find_by_id(report.project_id.expect("a Project id"))
        .one(db.connection())
        .await
        .expect("the Project query")
        .expect("the Project");
    let repo = repos::Entity::find_by_id(project.repo_id)
        .one(db.connection())
        .await
        .expect("the Repo query")
        .expect("the Repo");
    assert_eq!(
        (repo.name.as_str(), project.name.as_str()),
        ("acme/app", "backend")
    );
}

#[tokio::test]
async fn ingest_defaults_project_to_default() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(&test_app(&db), &[], passing_report()).await;
    assert_eq!(reply.status, StatusCode::ACCEPTED);

    let names: Vec<String> = projects::Entity::find()
        .all(db.connection())
        .await
        .expect("the Project rows")
        .into_iter()
        .map(|project| project.name)
        .collect();
    assert_eq!(names, vec!["default".to_owned()]);
}

#[tokio::test]
async fn ingest_stores_and_trims_default_branch() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(
        &test_app(&db),
        &[("default_branch", "  main  ")],
        passing_report(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED);
    let stored = stored_reports(&db).await;
    assert_eq!(
        stored
            .first()
            .expect("one Report")
            .default_branch
            .as_deref(),
        Some("main")
    );
}

#[tokio::test]
async fn ingest_empty_default_branch_becomes_none() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(
        &test_app(&db),
        &[("default_branch", "   ")],
        passing_report(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED);
    let stored = stored_reports(&db).await;
    assert_eq!(stored.first().expect("one Report").default_branch, None);
}

#[tokio::test]
async fn ingest_attribution_params_round_trip() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(
        &test_app(&db),
        &[
            ("ci_job_id", "  42 "),
            ("ci_run_attempt", "2"),
            ("pipeline", ".github/workflows/ci.yml"),
        ],
        passing_report(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED);
    let stored = stored_reports(&db).await;
    let report = stored.first().expect("one Report");
    assert_eq!(report.ci_job_id.as_deref(), Some("42"));
    assert_eq!(report.ci_run_attempt, Some(2));
    assert_eq!(report.pipeline.as_deref(), Some(".github/workflows/ci.yml"));
}

#[tokio::test]
async fn ingest_empty_attribution_params_become_none() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(
        &test_app(&db),
        &[("ci_job_id", "   "), ("pipeline", "   ")],
        passing_report(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED);
    let report = stored_reports(&db).await;
    let report = report.first().expect("one Report");
    assert_eq!(report.ci_job_id, None);
    assert_eq!(report.pipeline, None);
    assert_eq!(report.ci_run_attempt, None);
}

#[tokio::test]
async fn ingest_rejects_ci_run_attempt_zero() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(&test_app(&db), &[("ci_run_attempt", "0")], passing_report()).await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        reply.json()["detail"][0]["type"],
        json!("greater_than_equal")
    );
    assert_eq!(
        reply.json()["detail"][0]["loc"],
        json!(["query", "ci_run_attempt"])
    );
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn ingest_rejects_a_ci_run_attempt_past_the_column() {
    // The column is `int4`: Python's `ge=1` let a bigger number through and the
    // driver raised, which answered 500 after creating rows. A value the column
    // cannot hold is refused with 400 and queues nothing, as an unparsable
    // `time` is.
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(
        &test_app(&db),
        &[("ci_run_attempt", "2147483648")],
        passing_report(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.text());
    assert_eq!(
        reply.json(),
        json!({"detail": "ci_run_attempt must be at most 2147483647"})
    );
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn ingest_rejects_a_ci_run_attempt_past_i64() {
    // Python holds arbitrary-precision ints, so this reached the driver and
    // answered 500. It is past the column by any reading, so it is a 400.
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(
        &test_app(&db),
        &[("ci_run_attempt", "9223372036854775808")],
        passing_report(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.text());
    assert_eq!(
        reply.json(),
        json!({"detail": "ci_run_attempt must be at most 2147483647"})
    );
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn ingest_stores_the_largest_ci_run_attempt() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(
        &test_app(&db),
        &[("ci_run_attempt", "2147483647")],
        passing_report(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
    assert_eq!(
        stored_reports(&db)
            .await
            .first()
            .expect("one Report")
            .ci_run_attempt,
        Some(i32::MAX)
    );
}

#[tokio::test]
async fn ingest_rejects_an_empty_ci_run_attempt() {
    // Pydantic reads an empty string as no integer at all, not as no value.
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(&test_app(&db), &[("ci_run_attempt", "")], passing_report()).await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reply.json()["detail"][0]["type"], json!("int_parsing"));
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn ingest_rejects_a_ci_run_attempt_that_is_a_word() {
    // Pydantic's `int_parsing`, which is what keeps a word apart from a number
    // past the column's range.
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(
        &test_app(&db),
        &[("ci_run_attempt", "first")],
        passing_report(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reply.json()["detail"][0]["type"], json!("int_parsing"));
    assert_eq!(
        reply.json()["detail"][0]["loc"],
        json!(["query", "ci_run_attempt"])
    );
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn ingest_defaults_branch_and_ci_run_id() {
    // `build_junit_report`'s defaults, which the route supplies now that the
    // builder is a struct: an upload that names no attribution is on `main`
    // with an empty CI run id, pending, and without a Job or attempt.
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    let reply = ingest(&app, &[], passing_report()).await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());

    let stored = stored_reports(&db).await;
    let report = stored.first().expect("one Report");
    assert_eq!(
        (report.branch.as_str(), report.ci_run_id.as_str()),
        ("main", "")
    );
    assert_eq!(report.status, ReportStatus::Pending.as_str());
    assert_eq!(
        (report.ci_job_id.as_deref(), report.ci_run_attempt),
        (None, None)
    );

    // The served Report carries the same two defaults.
    let served = get(&app, &format!("/api/reports/{}", report.id)).await;
    assert_eq!(served.status, StatusCode::OK);
    assert_eq!(served.json()["branch"], json!("main"));
    assert_eq!(served.json()["ci_run_id"], json!(""));
    assert_eq!(served.json()["status"], json!("pending"));
}

#[tokio::test]
async fn a_failed_ingest_leaves_no_repo_or_project() {
    // Python committed the Repo, Project and Report once, at the end, so a
    // failure after the get-or-create left none of them. Here the insert is what
    // fails: a CHECK the row cannot satisfy refuses it after the Repo and
    // Project are in the transaction.
    let db = TestDb::new().await.expect("a test database is available");
    db.connection()
        .execute_unprepared("ALTER TABLE reports ADD CONSTRAINT ingest_always_fails CHECK (false)")
        .await
        .expect("the fixture constraint is installed");
    let reply = ingest(&test_app(&db), &[], passing_report()).await;
    assert_eq!(
        reply.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{}",
        reply.text()
    );
    assert_eq!(stored_reports(&db).await.len(), 0);
    assert_eq!(
        repos::Entity::find()
            .count(db.connection())
            .await
            .expect("the Repo count"),
        0
    );
    assert_eq!(
        projects::Entity::find()
            .count(db.connection())
            .await
            .expect("the Project count"),
        0
    );
}

#[tokio::test]
async fn a_duplicate_upload_creates_no_extra_rows() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    let content = passing_report();
    let first = ingest(&app, &[("ci_run_id", "9")], content.clone()).await;
    let second = ingest(&app, &[("ci_run_id", "9")], content).await;
    assert_eq!(first.json()["duplicate"], json!(false));
    assert_eq!(second.json()["duplicate"], json!(true));
    assert_eq!(stored_reports(&db).await.len(), 1);
    assert_eq!(
        repos::Entity::find()
            .count(db.connection())
            .await
            .expect("the Repo count"),
        1
    );
    assert_eq!(
        projects::Entity::find()
            .count(db.connection())
            .await
            .expect("the Project count"),
        1
    );
}

#[tokio::test]
async fn ingest_rejects_line_base_two() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(&test_app(&db), &[("line_base", "2")], passing_report()).await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reply.json()["detail"][0]["type"], json!("less_than_equal"));
    assert_eq!(
        reply.json()["detail"][0]["loc"],
        json!(["query", "line_base"])
    );
}

#[tokio::test]
async fn ingest_rejects_line_base_below_zero() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(&test_app(&db), &[("line_base", "-1")], passing_report()).await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        reply.json()["detail"][0]["type"],
        json!("greater_than_equal")
    );
}

#[tokio::test]
async fn ingest_stores_line_base() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(&test_app(&db), &[("line_base", "0")], passing_report()).await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
    assert_eq!(
        stored_reports(&db)
            .await
            .first()
            .expect("one Report")
            .line_base,
        Some(0)
    );
}

#[tokio::test]
async fn ingest_line_base_defaults_to_one() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(&test_app(&db), &[], passing_report()).await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
    assert_eq!(
        stored_reports(&db)
            .await
            .first()
            .expect("one Report")
            .line_base,
        Some(1)
    );
}

#[tokio::test]
async fn ingest_multipart_upload() {
    let db = TestDb::new().await.expect("a test database is available");
    let boundary = "----flakeradar-ingest-test";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"report\"; filename=\"junit.xml\"\r\n\
          Content-Type: text/xml\r\n\r\n",
    );
    body.extend_from_slice(&passing_report());
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri(ingest_uri(&[]))
        .header("x-api-key", TOKEN)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .expect("the fixture request builds");
    let reply = send(&test_app(&db), request).await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
    assert_eq!(
        stored_reports(&db).await.first().expect("one Report").body,
        passing_report()
    );
}

#[tokio::test]
async fn ingest_multipart_without_the_file_field_is_an_empty_report() {
    let db = TestDb::new().await.expect("a test database is available");
    let boundary = "----flakeradar-ingest-test";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"note\"\r\n\r\nhello\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri(ingest_uri(&[]))
        .header("x-api-key", TOKEN)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .expect("the fixture request builds");
    let reply = send(&test_app(&db), request).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.json(), json!({"detail": "Empty report body"}));
}

#[tokio::test]
async fn ingest_multipart_with_a_garbage_header_is_a_bad_request() {
    // `python_multipart` refuses a header line that is not `name: value` (its
    // field name may hold no spaces), and Starlette rounds that up to
    // `MultiPartException("Invalid multipart data.")`, which FastAPI answers as
    // 400. The reference sent exactly this answer and queued nothing.
    let db = TestDb::new().await.expect("a test database is available");
    let boundary = "----flakeradar-ingest-test";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"This is not a header\r\n\
          Content-Disposition: form-data; name=\"report\"; filename=\"junit.xml\"\r\n\
          Content-Type: text/xml\r\n\r\n",
    );
    body.extend_from_slice(&passing_report());
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri(ingest_uri(&[]))
        .header("x-api-key", TOKEN)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .expect("the fixture request builds");
    let reply = send(&test_app(&db), request).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.text());
    assert_eq!(reply.json(), json!({"detail": "Invalid multipart data."}));
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn ingest_raw_body_any_content_type() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    for content_type in [
        "application/x-www-form-urlencoded",
        "application/xml",
        "text/plain",
    ] {
        let request = axum::http::Request::builder()
            .method(Method::POST)
            .uri(ingest_uri(&[("commit_sha", content_type)]))
            .header("x-api-key", TOKEN)
            .header("content-type", content_type)
            .body(Body::from(passing_report()))
            .expect("the fixture request builds");
        let reply = send(&app, request).await;
        assert_eq!(
            reply.status,
            StatusCode::ACCEPTED,
            "{content_type}: {}",
            reply.text()
        );
    }
}

#[tokio::test]
async fn ingest_rejects_missing_repo() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = post(
        &test_app(&db),
        "/api/ingest?commit_sha=s",
        &auth(),
        passing_report(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reply.json()["detail"][0]["type"], json!("missing"));
    assert_eq!(reply.json()["detail"][0]["loc"], json!(["query", "repo"]));
}

#[tokio::test]
async fn ingest_rejects_bad_names() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    let cases: [(&[(&str, &str)], &str); 4] = [
        (
            &[("repo", "writers-app")],
            "repo must look like 'owner/name', got 'writers-app'",
        ),
        (
            &[("project", "front end")],
            "project must be 1-100 chars of a-z, 0-9, '.', '_' or '-', got 'front end'",
        ),
        (
            &[("root", "../x")],
            "root must not contain '..', got '../x'",
        ),
        (
            &[("repo", "o/r"), ("project", "Bad Project")],
            "project must be 1-100 chars of a-z, 0-9, '.', '_' or '-', got 'Bad Project'",
        ),
    ];
    for (params, detail) in cases {
        let reply = ingest(&app, params, passing_report()).await;
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY, "{params:?}");
        assert_eq!(reply.json(), json!({"detail": detail}), "{params:?}");
    }
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn ingest_rejects_empty_and_garbage() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    let empty = post(&app, &ingest_uri(&[]), &auth(), Body::empty()).await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);
    assert_eq!(empty.json(), json!({"detail": "Empty report body"}));

    let garbage = ingest(&app, &[], b"not xml at all".to_vec()).await;
    assert_eq!(garbage.status, StatusCode::UNPROCESSABLE_ENTITY);
    let detail = garbage.json()["detail"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        detail.starts_with("Not a valid JUnit XML report: "),
        "{detail}"
    );
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn a_deeply_nested_report_is_refused_and_the_server_stays_up() {
    // The reference's parser recursed per level, and 5,000 nested suites aborted
    // the Rust process (H0). Both depths answer the 422 a bad report gets, and
    // the next upload is served, so the parser neither overflowed nor stopped.
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    for depth in [5_000_usize, 200_000] {
        let mut body = Vec::new();
        for _ in 0..depth {
            body.extend_from_slice(b"<testsuite name=\"s\">");
        }
        body.extend_from_slice(b"<testcase classname=\"c\" name=\"t\"/>");
        for _ in 0..depth {
            body.extend_from_slice(b"</testsuite>");
        }
        let reply = ingest(&app, &[], body).await;
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY, "{depth}");
        let detail = reply.json()["detail"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert!(
            detail.starts_with("Not a valid JUnit XML report: "),
            "{depth}: {detail}"
        );
        // The server still answers health, and the next upload is still judged.
        let healthy = get(&app, "/api/health").await;
        assert_eq!(healthy.status, StatusCode::OK, "{depth}");
        let refused = ingest(&app, &[], b"not xml at all".to_vec()).await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{depth}");
    }
    // Nothing was queued for a refused parse, and a good upload still is.
    assert_eq!(stored_reports(&db).await.len(), 0);
    let queued = ingest(&app, &[], passing_report()).await;
    assert_eq!(queued.status, StatusCode::ACCEPTED);
    assert_eq!(stored_reports(&db).await.len(), 1);
}

#[tokio::test]
async fn ingest_refuses_a_report_whose_identities_exceed_the_budget() {
    // H3: one long suite name copied once per case is the quadratic case.
    let db = TestDb::new().await.expect("a test database is available");
    let mut body = b"<testsuite name=\"".to_vec();
    body.extend(std::iter::repeat_n(b'n', 10 * 1024 * 1024));
    body.extend_from_slice(b"\">");
    for _ in 0..10 {
        body.extend_from_slice(b"<testcase name=\"a\"/>");
    }
    body.extend_from_slice(b"</testsuite>");
    let reply = ingest(&test_app(&db), &[], body).await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    let detail = reply.json()["detail"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(detail.contains("byte budget"), "{detail}");
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn ingest_rejects_a_report_with_no_test_cases() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(
        &test_app(&db),
        &[],
        br#"<testsuites name="empty"/>"#.to_vec(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        reply.json(),
        json!({"detail": "Report parsed but contained no test cases."})
    );
}

#[tokio::test]
async fn ingest_rejects_a_testcase_time_that_is_not_a_number() {
    // The user approved this: Python's unhandled ValueError answered 500, and
    // the Rust server validates the attribute and answers 400.
    let db = TestDb::new().await.expect("a test database is available");
    let body = br#"<testsuite><testcase classname="c" name="t" time="abc"/></testsuite>"#;
    let reply = ingest(&test_app(&db), &[], body.to_vec()).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        reply.json(),
        json!({"detail": "Not a valid JUnit XML report: testcase time 'abc' is not a number"})
    );
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn ingest_rejects_oversized_report() {
    let db = TestDb::new().await.expect("a test database is available");
    // The limit cannot be lowered from a test, so the body is one byte over it.
    let body = vec![b'a'; flakeradar::routes::reports::MAX_REPORT_BYTES + 1];
    let reply = ingest(&test_app(&db), &[], body).await;
    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(reply.json()["detail"], json!("Report exceeds 20 MB limit"));
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn ingest_accepts_exactly_the_limit() {
    let db = TestDb::new().await.expect("a test database is available");
    // Exactly 20 MiB of XML is accepted, judged and refused for its content, not
    // for its size: the size check is the boundary the contract names.
    let suffix = b"--></testsuite>";
    let mut body = b"<testsuite name=\"big\"><testcase classname=\"c\" name=\"t\"/><!--".to_vec();
    body.resize(
        flakeradar::routes::reports::MAX_REPORT_BYTES - suffix.len(),
        b'x',
    );
    body.extend_from_slice(suffix);
    assert_eq!(body.len(), flakeradar::routes::reports::MAX_REPORT_BYTES);
    let reply = ingest(&test_app(&db), &[], body).await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
    assert_eq!(stored_reports(&db).await.len(), 1);
}

#[tokio::test]
async fn ingest_cuts_off_a_body_far_past_the_limit() {
    let db = TestDb::new().await.expect("a test database is available");
    let body = vec![b'a'; flakeradar::routes::reports::INGEST_BODY_LIMIT + 1];
    let reply = ingest(&test_app(&db), &[], body).await;
    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(reply.json()["detail"], json!("Report exceeds 20 MB limit"));
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn ingest_retried_upload_is_a_duplicate() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    let params = [
        ("ci_run_id", "55"),
        ("ci_job_id", "7"),
        ("ci_run_attempt", "1"),
    ];
    let content = make_junit(&[("t1", "failed")]);
    let first = ingest(&app, &params, content.clone()).await;
    let second = ingest(&app, &params, content).await;
    assert_eq!(first.status, StatusCode::ACCEPTED);
    assert_eq!(second.status, StatusCode::ACCEPTED);
    assert_eq!(first.json()["report_id"], second.json()["report_id"]);
    assert_eq!(first.json()["duplicate"], json!(false));
    assert_eq!(second.json()["duplicate"], json!(true));
    assert_eq!(stored_reports(&db).await.len(), 1);
}

#[tokio::test]
async fn ingest_duplicate_check_keys_on_job_and_project() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    let content = passing_report();
    let responses = [
        ingest(&app, &[("ci_job_id", "1")], content.clone()).await,
        ingest(&app, &[("ci_job_id", "2")], content.clone()).await,
        ingest(
            &app,
            &[("ci_job_id", "1"), ("project", "other")],
            content.clone(),
        )
        .await,
    ];
    for reply in &responses {
        assert_eq!(reply.status, StatusCode::ACCEPTED);
        assert_eq!(reply.json()["duplicate"], json!(false));
    }
    let ids: Vec<Value> = responses
        .iter()
        .map(|reply| reply.json()["report_id"].clone())
        .collect();
    let unique: std::collections::BTreeSet<String> = ids.iter().map(ToString::to_string).collect();
    assert_eq!(unique.len(), 3, "{ids:?}");

    // The very same upload again is the retry of the first one.
    let retry = ingest(&app, &[("ci_job_id", "1")], content).await;
    assert_eq!(retry.json()["duplicate"], json!(true));
    assert_eq!(retry.json()["report_id"], responses[0].json()["report_id"]);
    assert_eq!(stored_reports(&db).await.len(), 3);
}

#[tokio::test]
async fn ingest_duplicate_check_keys_on_line_base() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    let content = make_junit(&[("t1", "failed")]);
    let default = ingest(&app, &[("ci_run_id", "61")], content.clone()).await;
    let corrected = ingest(
        &app,
        &[("ci_run_id", "61"), ("line_base", "0")],
        content.clone(),
    )
    .await;
    assert_eq!(default.status, StatusCode::ACCEPTED);
    assert_eq!(corrected.status, StatusCode::ACCEPTED);
    assert_eq!(corrected.json()["duplicate"], json!(false));
    assert_ne!(corrected.json()["report_id"], default.json()["report_id"]);

    let retry = ingest(&app, &[("ci_run_id", "61"), ("line_base", "0")], content).await;
    assert_eq!(retry.json()["duplicate"], json!(true));
    assert_eq!(retry.json()["report_id"], corrected.json()["report_id"]);
    assert_eq!(stored_reports(&db).await.len(), 2);
}

#[tokio::test]
async fn ingest_answers_a_database_failure_as_a_server_error() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    let reply = {
        db.connection()
            .clone()
            .close()
            .await
            .map_err(|error| DbErr::Custom(error.to_string()))
            .expect("the pool closes");
        ingest(&app, &[], passing_report()).await
    };
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(reply.text(), "Internal Server Error");
}

#[tokio::test]
async fn a_valid_pipeline_report_is_queued() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = post(
        &test_app(&db),
        "/api/ingest/pipeline",
        &[("x-api-key", TOKEN), ("content-type", "application/json")],
        pipeline_report_body(&json!({})),
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
    assert_eq!(reply.json()["status"], json!("pending"));

    let stored = stored_reports(&db).await;
    let report = stored.first().expect("one Report");
    assert_eq!(report.kind, "pipeline");
    assert_eq!(report.project_id, None);
    assert_eq!(report.pipeline.as_deref(), Some(".github/workflows/ci.yml"));
    assert_eq!(report.ci_run_attempt, Some(2));
    assert_eq!(report.body_sha256, None);
    let repo = repos::Entity::find_by_id(report.repo_id)
        .one(db.connection())
        .await
        .expect("the Repo query")
        .expect("the Repo");
    assert_eq!(repo.name, "acme/app");

    // The stored body is the normalized Report.
    let body: Value = serde_json::from_slice(&report.body).expect("the stored Report body is JSON");
    assert_eq!(body["repo"], json!("acme/app"));
    assert_eq!(body["provider"], json!("github"));
}

/// A Pipeline Report body, with `over` overriding its fields.
fn pipeline_report_body(over: &Value) -> Vec<u8> {
    let mut body = json!({
        "repo": "Acme/App",
        "provider": "github",
        "pipeline": ".github/workflows/ci.yml",
        "commit_sha": "deadbeef",
        "branch": "main",
        "default_branch": "main",
        "ci_run_id": "42",
        "ci_run_attempt": 2,
        "jobs": [{
            "ci_job_id": "100",
            "name": "test (ubuntu, 3.12)",
            "status": "passed",
            "url": "https://github.com/acme/app/actions/runs/42/jobs/100",
            "runner_name": "GitHub Actions 4",
            "runner_labels": ["ubuntu-latest", "3.12"]
        }]
    });
    if let (Some(target), Some(source)) = (body.as_object_mut(), over.as_object()) {
        for (key, value) in source {
            let _ = target.insert(key.clone(), value.clone());
        }
    }
    serde_json::to_vec(&body).expect("the fixture serializes")
}

/// Posts a Pipeline report body to the route.
async fn pipeline(app: &Router, over: &Value) -> Reply {
    post(
        app,
        "/api/ingest/pipeline",
        &[("x-api-key", TOKEN), ("content-type", "application/json")],
        pipeline_report_body(over),
    )
    .await
}

#[tokio::test]
async fn pipeline_ingest_strips_names() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = pipeline(
        &test_app(&db),
        &json!({
            "pipeline": "  ci.yml ",
            "branch": " feat ",
            "default_branch": "  ",
            "jobs": [{"ci_job_id": "100", "name": " build ", "status": "passed"}]
        }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
    let stored = stored_reports(&db).await;
    let report = stored.first().expect("one Report");
    let body: Value = serde_json::from_slice(&report.body).expect("the stored Report body is JSON");
    assert_eq!(body["pipeline"], json!("ci.yml"));
    assert_eq!(body["branch"], json!("feat"));
    assert_eq!(body["default_branch"], Value::Null);
    assert_eq!(body["jobs"][0]["name"], json!("build"));
    assert_eq!(report.pipeline.as_deref(), Some("ci.yml"));
    assert_eq!(report.branch, "feat");
    assert_eq!(report.default_branch, None);
}

#[tokio::test]
async fn pipeline_ingest_requires_token() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = post(
        &test_app(&db),
        "/api/ingest/pipeline",
        &[("content-type", "application/json")],
        pipeline_report_body(&json!({})),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn pipeline_ingest_validation() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    let cases = [
        json!({"repo": "bad"}),
        json!({"provider": "has space"}),
        json!({"jobs": []}),
        json!({"jobs": [{"ci_job_id": "1", "name": "n", "status": "bogus"}]}),
        json!({"pipeline": "   "}),
        json!({"branch": " "}),
        json!({"jobs": [{"ci_job_id": "1", "name": "  ", "status": "passed"}]}),
        json!({"jobs": [{"ci_job_id": " ", "name": "n", "status": "passed"}]}),
    ];
    for over in cases {
        let reply = pipeline(&app, &over).await;
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY, "{over}");
    }
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn pipeline_ingest_rejects_duplicate_ci_job_id() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = pipeline(
        &test_app(&db),
        &json!({
            "jobs": [
                {"ci_job_id": "9", "name": "a", "status": "passed"},
                {"ci_job_id": "9", "name": "b", "status": "passed"}
            ]
        }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        reply.text().contains("duplicate ci_job_id in jobs"),
        "{}",
        reply.text()
    );
}

#[tokio::test]
async fn pipeline_report_auto_creates_repo() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = pipeline(&test_app(&db), &json!({"repo": "brand/NewRepo"})).await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
    let names: Vec<String> = repos::Entity::find()
        .all(db.connection())
        .await
        .expect("the Repo rows")
        .into_iter()
        .map(|repo| repo.name)
        .collect();
    assert_eq!(names, vec!["brand/newrepo".to_owned()]);
}

#[tokio::test]
async fn pipeline_ingest_rejects_a_body_that_is_not_json() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = post(
        &test_app(&db),
        "/api/ingest/pipeline",
        &[("x-api-key", TOKEN)],
        Body::from("not json"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        reply.json()["detail"][0]["type"],
        json!("model_attributes_type")
    );
    assert_eq!(reply.json()["detail"][0]["loc"], json!(["body"]));
}

#[tokio::test]
async fn ingest_rejects_a_commit_sha_outside_its_lengths() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    let short = ingest(&app, &[("commit_sha", "")], passing_report()).await;
    assert_eq!(short.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(short.json()["detail"][0]["type"], json!("string_too_short"));
    assert_eq!(
        short.json()["detail"][0]["loc"],
        json!(["query", "commit_sha"])
    );

    let long = ingest(&app, &[("commit_sha", &"x".repeat(65))], passing_report()).await;
    assert_eq!(long.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(long.json()["detail"][0]["type"], json!("string_too_long"));
}

#[tokio::test]
async fn ingest_rejects_a_root_that_climbs_out() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = ingest(&test_app(&db), &[("root", "../etc")], passing_report()).await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        reply.json(),
        json!({"detail": "root must not contain '..', got '../etc'"})
    );
}

#[tokio::test]
async fn ingest_rejects_a_query_that_is_not_an_integer() {
    // Measured from the reference at 86c98804: `line_base` is a Pydantic `int`
    // in the query, so `abc` and an empty value are `int_parsing` with Pydantic's
    // sentence, and `1.0` is the integer 1 rather than a refusal.
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    for value in ["abc", ""] {
        let reply = ingest(&app, &[("line_base", value)], passing_report()).await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "line_base={value}"
        );
        assert_eq!(
            reply.json()["detail"],
            json!([{
                "type": "int_parsing",
                "loc": ["query", "line_base"],
                "msg": "Input should be a valid integer, unable to parse string as an integer",
                "input": value,
            }]),
            "line_base={value}"
        );
    }
    let reply = ingest(&app, &[("line_base", "1.0")], passing_report()).await;
    assert_eq!(
        reply.status,
        StatusCode::ACCEPTED,
        "line_base=1.0: {}",
        reply.text()
    );
    assert_eq!(reply.json()["status"], json!("pending"));
}

#[tokio::test]
async fn the_writers_app_request_shape_is_accepted() {
    // actions/report/report.sh: a raw XML body typed application/xml, with the
    // attribution its workflow knows.
    let db = TestDb::new().await.expect("a test database is available");
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri(
            "/api/ingest?repo=andrewthetechie%2Fwriters-app&project=backend&\
             commit_sha=9f8e7d6&branch=main&root=backend&\
             pipeline=.github%2Fworkflows%2Fci.yml&ci_run_id=12345&\
             ci_run_attempt=1&ci_job_id=100&default_branch=main",
        )
        .header("x-api-key", TOKEN)
        .header("content-type", "application/xml")
        .body(Body::from(passing_report()))
        .expect("the fixture request builds");
    let reply = send(&test_app(&db), request).await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
    let stored = stored_reports(&db).await;
    let report = stored.first().expect("one Report");
    assert_eq!(report.root.as_deref(), Some("backend"));
    assert_eq!(report.ci_job_id.as_deref(), Some("100"));
    assert_eq!(report.default_branch.as_deref(), Some("main"));
}

#[tokio::test]
async fn the_reports_routes_answer_an_empty_queue() {
    // The summary route is registered before the id route, so "summary" is never
    // read as an id.
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    let summary = get(&app, "/api/reports/summary").await;
    assert_eq!(summary.status, StatusCode::OK);
    assert_eq!(summary.json(), json!({"pending": 0, "failed": 0}));
    let list = get(&app, "/api/reports").await;
    assert_eq!(list.json(), json!([]));
    let missing = get(&app, "/api/reports/999").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.json(), json!({"detail": "Report not found"}));
    let request_reply = request(&app, Method::DELETE, "/api/reports/999").await;
    assert_eq!(request_reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn pipeline_ingest_cuts_off_an_oversized_body() {
    // The route keeps the app's JSON body limit, so a body past it is refused
    // while it is read rather than buffered.
    let db = TestDb::new().await.expect("a test database is available");
    let body = vec![b'a'; flakeradar::JSON_BODY_LIMIT + 1];
    let reply = post(
        &test_app(&db),
        "/api/ingest/pipeline",
        &[("x-api-key", TOKEN), ("content-type", "application/json")],
        Body::from(body),
    )
    .await;
    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        reply.json()["detail"],
        json!("Request body exceeds the size limit")
    );
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn ingest_answers_a_body_that_fails_while_it_is_read() {
    // A client that goes away mid-upload is not a size problem, so it must not be
    // reported as one.
    let db = TestDb::new().await.expect("a test database is available");
    let stream = futures_util::stream::iter(vec![
        Ok(axum::body::Bytes::from_static(b"<testsuite>")),
        Err(std::io::Error::other("the client went away")),
    ]);
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri(ingest_uri(&[]))
        .header("x-api-key", TOKEN)
        .body(Body::from_stream(stream))
        .expect("the fixture request builds");
    let reply = send(&test_app(&db), request).await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        reply.text().contains("Reading the report body failed"),
        "{}",
        reply.text()
    );
    assert_eq!(stored_reports(&db).await.len(), 0);
}

#[tokio::test]
async fn pipeline_ingest_answers_a_body_that_fails_while_it_is_read() {
    let db = TestDb::new().await.expect("a test database is available");
    let stream = futures_util::stream::iter(vec![
        Ok(axum::body::Bytes::from_static(b"{\"repo\":")),
        Err(std::io::Error::other("the client went away")),
    ]);
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/ingest/pipeline")
        .header("x-api-key", TOKEN)
        .header("content-type", "application/json")
        .body(Body::from_stream(stream))
        .expect("the fixture request builds");
    let reply = send(&test_app(&db), request).await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        reply.text().contains("Reading the request body failed"),
        "{}",
        reply.text()
    );
}

#[tokio::test]
async fn a_reports_list_counts_only_the_rows_it_holds() {
    let db = TestDb::new().await.expect("a test database is available");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    reports::Entity::insert(
        flakeradar_db::NewJunitReport {
            project_id: project.id,
            repo_id: project.repo_id,
            body: b"<testsuite/>".to_vec(),
            commit_sha: "abc".to_owned(),
            branch: "main".to_owned(),
            ci_run_id: String::new(),
            root: None,
            line_base: Some(1),
            ci_job_id: None,
            ci_run_attempt: None,
            pipeline: None,
            default_branch: None,
        }
        .into_active_model(FIXTURE_INSTANT.fixed_offset()),
    )
    .exec(db.connection())
    .await
    .expect("a Report");
    let summary = get(&test_app(&db), "/api/reports/summary").await;
    assert_eq!(summary.json(), json!({"pending": 1, "failed": 0}));
    assert_eq!(
        reports::Entity::find()
            .filter(reports::Column::Status.eq("pending"))
            .count(db.connection())
            .await
            .expect("a count"),
        1
    );
}
