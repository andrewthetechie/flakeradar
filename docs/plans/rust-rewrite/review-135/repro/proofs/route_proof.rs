//! Proofs for A4/H2 and H19a through the routes, rewritten after the fix: an
//! out-of-range page and a NUL in any request text answer the 4xx the route
//! already uses, instead of the 500 (or the wrapped page) the reference gave.
//!
//! At `f9330fd` `?page=9223372036854775807` was a 500, a page whose offset wrapped
//! answered page one's rows under that number, and every NUL below was a 500.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use flakeradar::app::{AppState, build_app};
use flakeradar::settings::Settings;
use flakeradar_test_support::TestDb;
use std::sync::Arc;
use tower::ServiceExt as _;

fn app(db: &TestDb) -> axum::Router {
    build_app(
        AppState::new(db.connection().clone(), Arc::new(Settings::default()))
            .expect("the app state builds"),
    )
}

async fn answer(app: axum::Router, request: Request<Body>) -> (StatusCode, String) {
    let response = app.oneshot(request).await.expect("the router answers");
    let code = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("the body reads");
    (code, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn a_page_whose_offset_does_not_fit_is_refused() {
    let db = TestDb::new().await.expect("db");
    let wrap = (1_i64 << 62) + 1; // (page - 1) * 4 == 2^64, which used to wrap to 0
    for path in [
        "/api/tests?page=9223372036854775807&page_size=100".to_owned(),
        "/api/jobs?page=9223372036854775807&page_size=100".to_owned(),
        format!("/api/tests?page={wrap}&page_size=4"),
    ] {
        let (code, body) = answer(
            app(&db),
            Request::get(&path).body(Body::empty()).expect("the request builds"),
        )
        .await;
        println!("GET {path} -> {code} {body}");
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "{path}");
    }
}

#[tokio::test]
async fn nul_in_a_query_parameter_and_a_json_string_is_refused() {
    let db = TestDb::new().await.expect("db");
    let xml = r#"<testsuite name="s"><testcase name="t"/></testsuite>"#;
    let request = Request::post("/api/ingest?repo=a/b&commit_sha=abc%00")
        .header("x-api-key", "changeme")
        .body(Body::from(xml))
        .expect("the request builds");
    let (code, body) = answer(app(&db), request).await;
    println!("POST /api/ingest commit_sha=abc%00 -> {code} {body}");
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);

    let json = r#"{"repo":"a/b","provider":"github","pipeline":"ci","commit_sha":"abc","branch":"m\u0000n","jobs":[{"ci_job_id":"1","name":"j","status":"passed"}]}"#;
    let request = Request::post("/api/ingest/pipeline")
        .header("x-api-key", "changeme")
        .header("content-type", "application/json")
        .body(Body::from(json))
        .expect("the request builds");
    let (code, body) = answer(app(&db), request).await;
    println!("POST /api/ingest/pipeline branch=m\\u0000n -> {code} {body}");
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);

    let (code, body) = answer(
        app(&db),
        Request::get("/api/tests?q=a%00b")
            .body(Body::empty())
            .expect("the request builds"),
    )
    .await;
    println!("GET /api/tests?q=a%00b -> {code} {body}");
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
}
