//! Proof for H0/S3 through the route, rewritten after the fix: a deeply nested
//! upload is refused with the 422 a bad report gets, and the server keeps serving.
//!
//! At `f9330fd` this test died with `thread 'tokio-rt-worker' has overflowed its
//! stack / fatal runtime error: stack overflow, aborting`.

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

fn nested(depth: usize) -> String {
    let mut xml = String::new();
    for _ in 0..depth {
        xml.push_str("<testsuite>");
    }
    xml.push_str(r#"<testcase name="t"/>"#);
    for _ in 0..depth {
        xml.push_str("</testsuite>");
    }
    xml
}

#[tokio::test]
async fn a_deeply_nested_report_is_refused_and_the_server_stays_up() {
    let db = TestDb::new().await.expect("db");
    for depth in [5_000, 200_000] {
        let xml = nested(depth);
        let request = Request::post("/api/ingest?repo=a/b&commit_sha=abc")
            .header("x-api-key", "changeme")
            .body(Body::from(xml))
            .expect("the request builds");
        let (code, body) = answer(app(&db), request).await;
        println!("depth {depth} -> {code} {body}");
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    }

    // The process survived both uploads: health, a garbage upload and a good one.
    let (code, body) = answer(
        app(&db),
        Request::get("/api/health").body(Body::empty()).expect("built"),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");

    let good = r#"<testsuite name="s"><testcase name="t"/></testsuite>"#;
    let request = Request::post("/api/ingest?repo=a/b&commit_sha=abc")
        .header("x-api-key", "changeme")
        .body(Body::from(good))
        .expect("the request builds");
    let (code, body) = answer(app(&db), request).await;
    println!("good upload -> {code} {body}");
    assert_eq!(code, StatusCode::ACCEPTED);
}
