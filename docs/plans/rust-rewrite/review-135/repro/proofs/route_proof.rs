use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use flakeradar::{AppState, Settings, build_app};
use flakeradar_test_support::TestDb;
use tower::ServiceExt as _;

fn app(db: &TestDb) -> axum::Router {
    build_app(AppState::new(db.connection().clone(), Arc::new(Settings::default())))
}

async fn status(app: axum::Router, request: Request<Body>) -> (StatusCode, String) {
    let response = app.oneshot(request).await.expect("the router answers");
    let code = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (code, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn page_past_i64_overflow() {
    let db = TestDb::new().await.expect("db");
    for path in [
        "/api/tests?page=9223372036854775807&page_size=100",
        "/api/jobs?page=9223372036854775807&page_size=100",
    ] {
        let (code, body) = status(app(&db), Request::get(path).body(Body::empty()).unwrap()).await;
        println!("GET {path} -> {code} {body}");
    }
    // A page that wraps to a small non-negative offset answers a real page.
    let page = (1_i64 << 62) + 1; // (page - 1) * 4 == 2^64 -> wraps to 0
    let path = format!("/api/tests?page={page}&page_size=4");
    let (code, body) = status(app(&db), Request::get(&path).body(Body::empty()).unwrap()).await;
    println!("GET {path} -> {code} {body}");
}

#[tokio::test]
async fn nul_in_a_query_parameter_and_a_json_string() {
    let db = TestDb::new().await.expect("db");
    let xml = r#"<testsuite name="s"><testcase name="t"/></testsuite>"#;
    let request = Request::post("/api/ingest?repo=a/b&commit_sha=abc%00")
        .header("x-api-key", "changeme")
        .body(Body::from(xml))
        .unwrap();
    let (code, body) = status(app(&db), request).await;
    println!("POST /api/ingest commit_sha=abc%00 -> {code} {body}");

    let json = r#"{"repo":"a/b","provider":"github","pipeline":"ci","commit_sha":"abc","branch":"m\u0000n","jobs":[{"ci_job_id":"1","name":"j","status":"passed"}]}"#;
    let request = Request::post("/api/ingest/pipeline")
        .header("x-api-key", "changeme")
        .header("content-type", "application/json")
        .body(Body::from(json))
        .unwrap();
    let (code, body) = status(app(&db), request).await;
    println!("POST /api/ingest/pipeline branch=m\\u0000n -> {code} {body}");

    let (code, body) = status(
        app(&db),
        Request::get("/api/tests?q=a%00b").body(Body::empty()).unwrap(),
    )
    .await;
    println!("GET /api/tests?q=a%00b -> {code} {body}");
}
