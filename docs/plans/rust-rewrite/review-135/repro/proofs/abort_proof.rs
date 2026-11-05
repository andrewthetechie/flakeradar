use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use flakeradar::{AppState, Settings, build_app};
use flakeradar_test_support::TestDb;
use tower::ServiceExt as _;

#[tokio::test]
async fn nested_suites_abort_the_process_through_the_route() {
    let db = TestDb::new().await.expect("db");
    let app = build_app(AppState::new(db.connection().clone(), Arc::new(Settings::default())));
    let depth = 5_000;
    let mut xml = String::new();
    for _ in 0..depth { xml.push_str("<testsuite>"); }
    xml.push_str(r#"<testcase name="t"/>"#);
    for _ in 0..depth { xml.push_str("</testsuite>"); }
    println!("uploading {} bytes", xml.len());
    let request = Request::post("/api/ingest?repo=a/b&commit_sha=abc")
        .header("x-api-key", "changeme")
        .body(Body::from(xml))
        .unwrap();
    let response = app.oneshot(request).await.expect("the router answers");
    println!("answered {}", response.status());
}
