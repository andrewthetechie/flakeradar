//! The Report queue's read side: status, listing, summary, retry and delete.
//!
//! The behaviour is `backend/tests/test_reports_api.py`, on a queue seeded
//! directly rather than through ingest.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a Report route answers something else"
)]

mod common;

use axum::http::{Method, StatusCode};
use common::{Reply, app, get, request};
use flakeradar_core::ReportStatus;
use flakeradar_db::entities::reports;
use flakeradar_test_support::{TestDb, make_project, make_report};
use sea_orm::{EntityTrait, PaginatorTrait as _};
use serde_json::json;

/// The token the tests present.
const TOKEN: &str = "reports-token";

/// An app over `db` with the test's token.
fn test_app(db: &TestDb) -> axum::Router {
    app(db, &[("FLAKERADAR_API_TOKEN", TOKEN)])
}

/// The headers an authorised write sends.
fn auth() -> [(&'static str, &'static str); 1] {
    [("x-api-key", TOKEN)]
}

/// One Report per status, plus a Pipeline Report with no Project.
///
/// Returns the ids in the order they were written, which is also ascending.
async fn seed(db: &TestDb) -> [i32; 4] {
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let pending = make_report(&project, project.repo_id)
        .commit_sha("a")
        .build(db.connection())
        .await
        .expect("a pending Report");
    let failed = make_report(&project, project.repo_id)
        .commit_sha("b")
        .status("failed")
        .error("ParseError: boom")
        .retry_count(3)
        .build(db.connection())
        .await
        .expect("a failed Report");
    let done = make_report(&project, project.repo_id)
        .commit_sha("c")
        .status("processed")
        .build(db.connection())
        .await
        .expect("a processed Report");
    let pipeline = make_report(&project, project.repo_id)
        .kind("pipeline")
        .commit_sha("p")
        .status("processed")
        .ci_run_attempt(2)
        .build(db.connection())
        .await
        .expect("a Pipeline Report");
    [pending.id, failed.id, done.id, pipeline.id]
}

/// The ids of a list answer, in the order it returned them.
fn ids(reply: &Reply) -> Vec<i32> {
    reply
        .json()
        .as_array()
        .expect("a list of Reports")
        .iter()
        .map(|report| i32::try_from(report["id"].as_i64().expect("an id")).expect("an id fits"))
        .collect()
}

#[tokio::test]
async fn get_report() {
    let db = TestDb::new().await.expect("a test database is available");
    let [pending, ..] = seed(&db).await;
    let app = test_app(&db);
    let body = get(&app, &format!("/api/reports/{pending}")).await.json();
    assert_eq!(body["repo"], json!("acme/app"));
    assert_eq!(body["project"], json!("default"));
    assert_eq!(body["kind"], json!("junit"));
    assert_eq!(body["status"], json!("pending"));
    assert_eq!(body["counts"], json!(null));
    assert!(body.get("body").is_none(), "{body}");
    let missing = get(&app, "/api/reports/9999").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_and_summary() {
    let db = TestDb::new().await.expect("a test database is available");
    let [pending, failed, done, pipe] = seed(&db).await;
    let app = test_app(&db);

    let all = get(&app, "/api/reports").await;
    assert_eq!(all.status, StatusCode::OK);
    assert_eq!(ids(&all), vec![pipe, done, failed, pending]);

    let only_failed = get(&app, "/api/reports?status=failed").await;
    let errors: Vec<serde_json::Value> = only_failed
        .json()
        .as_array()
        .expect("a list of Reports")
        .iter()
        .map(|report| report["error"].clone())
        .collect();
    assert_eq!(errors, vec![json!("ParseError: boom")]);

    let bogus = get(&app, "/api/reports?status=bogus").await;
    assert_eq!(bogus.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        bogus.json(),
        json!({"detail": "status must be one of pending, processed, failed"})
    );

    assert_eq!(
        get(&app, "/api/reports/summary").await.json(),
        json!({"pending": 1, "failed": 1})
    );
}

#[tokio::test]
async fn a_list_limit_is_bounded() {
    let db = TestDb::new().await.expect("a test database is available");
    seed(&db).await;
    let app = test_app(&db);
    assert_eq!(ids(&get(&app, "/api/reports?limit=2").await).len(), 2);
    for limit in ["0", "201"] {
        let reply = get(&app, &format!("/api/reports?limit={limit}")).await;
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY, "{limit}");
        assert_eq!(reply.json()["detail"][0]["loc"], json!(["query", "limit"]));
    }
    let not_a_number = get(&app, "/api/reports?limit=abc").await;
    assert_eq!(
        not_a_number.json()["detail"][0]["type"],
        json!("int_parsing")
    );
    // The default limit is 50.
    assert_eq!(
        get(&app, "/api/reports?limit=50").await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn pipeline_report_listed_with_null_project() {
    let db = TestDb::new().await.expect("a test database is available");
    let [_, _, _, pipe] = seed(&db).await;
    let body = get(&test_app(&db), &format!("/api/reports/{pipe}"))
        .await
        .json();
    assert_eq!(body["kind"], json!("pipeline"));
    assert_eq!(body["project"], json!(null));
    assert_eq!(body["repo"], json!("acme/app"));
}

#[tokio::test]
async fn retry() {
    let db = TestDb::new().await.expect("a test database is available");
    let [pending, failed, ..] = seed(&db).await;
    let app = test_app(&db);

    let unauthorised = request(&app, Method::POST, &format!("/api/reports/{failed}/retry")).await;
    assert_eq!(unauthorised.status, StatusCode::UNAUTHORIZED);

    let retried = common::post(&app, &format!("/api/reports/{failed}/retry"), &auth(), "").await;
    assert_eq!(retried.status, StatusCode::OK);
    assert_eq!(retried.json()["status"], json!("pending"));
    assert_eq!(retried.json()["error"], json!(null));
    assert_eq!(retried.json()["retry_count"], json!(0));

    let conflict = common::post(&app, &format!("/api/reports/{pending}/retry"), &auth(), "").await;
    assert_eq!(conflict.status, StatusCode::CONFLICT);
    assert_eq!(
        conflict.json(),
        json!({"detail": "Only failed reports can be retried"})
    );

    let missing = common::post(&app, "/api/reports/999999/retry", &auth(), "").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.json(), json!({"detail": "Report not found"}));

    let stored = reports::Entity::find_by_id(failed)
        .one(db.connection())
        .await
        .expect("the Report query")
        .expect("the Report");
    assert_eq!(stored.status, ReportStatus::Pending.as_str());
    assert_eq!(stored.retry_count, 0);
    assert_eq!(stored.error, None);
}

#[tokio::test]
async fn retry_resets_the_budget_at_the_cap() {
    // A manual retry is a human decision: it works even when the automatic
    // retries have spent the whole budget, and the budget starts again.
    let db = TestDb::new().await.expect("a test database is available");
    let [_, failed, ..] = seed(&db).await;
    let reply = common::post(
        &test_app(&db),
        &format!("/api/reports/{failed}/retry"),
        &auth(),
        "",
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["retry_count"], json!(0));
}

#[tokio::test]
async fn delete() {
    let db = TestDb::new().await.expect("a test database is available");
    let [pending, failed, done, _] = seed(&db).await;
    let app = test_app(&db);

    let unauthorised = request(&app, Method::DELETE, &format!("/api/reports/{failed}")).await;
    assert_eq!(unauthorised.status, StatusCode::UNAUTHORIZED);

    let deleted = request(&app, Method::DELETE, &format!("/api/reports/{failed}")).await;
    assert_eq!(deleted.status, StatusCode::UNAUTHORIZED);
    let deleted = common::request_with_headers(
        &app,
        Method::DELETE,
        &format!("/api/reports/{failed}"),
        &auth(),
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(deleted.body, Vec::<u8>::new());
    assert_eq!(
        get(&app, &format!("/api/reports/{failed}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        reports::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        3
    );

    for other in [pending, done] {
        let conflict = common::request_with_headers(
            &app,
            Method::DELETE,
            &format!("/api/reports/{other}"),
            &auth(),
        )
        .await;
        assert_eq!(conflict.status, StatusCode::CONFLICT);
        assert_eq!(
            conflict.json(),
            json!({"detail": "Only failed reports can be deleted"})
        );
    }
    let missing =
        common::request_with_headers(&app, Method::DELETE, "/api/reports/9999", &auth()).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.json(), json!({"detail": "Report not found"}));
}

#[tokio::test]
async fn a_report_path_that_is_not_an_id_is_a_422() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = get(&test_app(&db), "/api/reports/abc").await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reply.json()["detail"][0]["type"], json!("int_parsing"));
    assert_eq!(
        reply.json()["detail"][0]["loc"],
        json!(["path", "report_id"])
    );
}

#[tokio::test]
async fn an_id_no_row_can_hold_is_not_found() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = get(&test_app(&db), "/api/reports/9999999999").await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.json(), json!({"detail": "Report not found"}));
}

#[tokio::test]
async fn a_report_whose_stored_status_this_version_never_wrote_is_a_server_error() {
    // Only this server writes a `reports` row, so an unknown status means
    // another writer made it: the route reports that rather than serve a value
    // its own queue filter and counter would ignore.
    let db = TestDb::new().await.expect("a test database is available");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let report = make_report(&project, project.repo_id)
        .commit_sha("a")
        .status("retrying")
        .build(db.connection())
        .await
        .expect("a Report");
    let reply = get(&test_app(&db), &format!("/api/reports/{}", report.id)).await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(reply.text(), "Internal Server Error");
}

#[tokio::test]
async fn a_report_query_that_fails_is_a_server_error() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = test_app(&db);
    db.connection()
        .clone()
        .close()
        .await
        .expect("the pool closes");
    for reply in [
        get(&app, "/api/reports").await,
        get(&app, "/api/reports/summary").await,
        get(&app, "/api/reports/1").await,
    ] {
        assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(reply.text(), "Internal Server Error");
    }
}

#[tokio::test]
async fn the_summary_counts_each_status_once() {
    let db = TestDb::new().await.expect("a test database is available");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    for sha in ["a", "b"] {
        make_report(&project, project.repo_id)
            .commit_sha(sha)
            .build(db.connection())
            .await
            .expect("a pending Report");
    }
    make_report(&project, project.repo_id)
        .commit_sha("c")
        .status("failed")
        .build(db.connection())
        .await
        .expect("a failed Report");
    assert_eq!(
        get(&test_app(&db), "/api/reports/summary").await.json(),
        json!({"pending": 2, "failed": 1})
    );
}
