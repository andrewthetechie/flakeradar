//! The app shell: health, the 404 body, the trailing-slash redirect, CORS, the
//! request log and the documentation routes.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the app answers something else"
)]

use axum::http::{Method, StatusCode};
use flakeradar::app::AppState;
use flakeradar::app::build_app;
use flakeradar_test_support::{TestDb, app, get, post, request, send, settings};

use serde_json::json;

#[tokio::test]
async fn health_answers_ok() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = get(&app(&db, &[]), "/api/health").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json(), json!({"status": "ok"}));
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("application/json")
    );
}

#[tokio::test]
async fn an_unknown_path_is_a_404_with_the_detail_body() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = app(&db, &[]);
    let reply = get(&app, "/nowhere").await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.json(), json!({"detail": "Not Found"}));
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("application/json")
    );

    // The root is not redirected to an empty path.
    let reply = get(&app, "/").await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.json(), json!({"detail": "Not Found"}));
}

#[tokio::test]
async fn a_trailing_slash_redirects_to_the_path_without_it() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = app(&db, &[]);
    let reply = get(&app, "/api/health/").await;
    assert_eq!(reply.status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(reply.header("location").as_deref(), Some("/api/health"));
    // Starlette answered a redirect with no body and no content type, and the
    // recorded goldens are compared on both.
    assert_eq!(reply.body, Vec::<u8>::new());
    assert_eq!(reply.header("content-type"), None);

    let reply = get(&app, "/api/health/?repo=o%2Fr").await;
    assert_eq!(reply.status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        reply.header("location").as_deref(),
        Some("/api/health?repo=o%2Fr")
    );

    // The redirect does not check whether the target exists, as Starlette's
    // redirect_slashes did not: the client asked for a different resource.
    let reply = get(&app, "/api/tests/").await;
    assert_eq!(reply.status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(reply.header("location").as_deref(), Some("/api/tests"));
}

#[tokio::test]
async fn a_method_a_path_does_not_answer_is_a_405_with_fastapis_body() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = app(&db, &[]);
    let reply = request(&app, Method::POST, "/api/health").await;
    assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(reply.json(), json!({"detail": "Method Not Allowed"}));
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("application/json")
    );
    // Starlette names the methods the path does answer, and so does Axum.
    let allow = reply.header("allow").expect("the answer names its methods");
    assert!(allow.contains("GET"), "{allow}");
}

#[tokio::test]
async fn a_route_that_needs_the_token_refuses_a_missing_or_wrong_one() {
    // No route requires the token until task 06, so the middleware's own contract
    // is pinned here: a missing or wrong `X-API-Key` is 401 with the frozen
    // server's message, and the configured one is let through.
    let db = TestDb::new().await.expect("a test database is available");
    let state = AppState::new(
        db.connection().clone(),
        std::sync::Arc::new(settings(&[("FLAKERADAR_API_TOKEN", "right-token")])),
    )
    .expect("the app state builds");
    let app = axum::Router::new()
        .route("/guarded", axum::routing::get(|| async { "opened" }))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            flakeradar::middleware::require_token,
        ))
        .with_state(state);

    let reply = get(&app, "/guarded").await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        reply.json(),
        json!({"detail": "Invalid or missing X-API-Key"})
    );
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("application/json")
    );

    for presented in ["wrong-token", "right-tokeN", "right-token "] {
        let request = axum::http::Request::builder()
            .uri("/guarded")
            .header("x-api-key", presented)
            .body(axum::body::Body::empty())
            .expect("the fixture request builds");
        let reply = send(&app, request).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{presented:?}");
    }

    let request = axum::http::Request::builder()
        .uri("/guarded")
        .header("x-api-key", "right-token")
        .body(axum::body::Body::empty())
        .expect("the fixture request builds");
    let reply = send(&app, request).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.text(), "opened");
}

#[tokio::test]
async fn the_openapi_document_lists_every_route() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = get(&app(&db, &[]), "/openapi.json").await;
    assert_eq!(reply.status, StatusCode::OK);
    let document = reply.json();
    assert_eq!(document["info"]["title"], json!("FlakeRadar"));
    assert_eq!(
        document["info"]["version"],
        json!(env!("CARGO_PKG_VERSION"))
    );
    for path in ["/api/health", "/api/health/ready", "/metrics"] {
        assert!(
            document["paths"].get(path).is_some(),
            "{path} is missing from the document"
        );
    }
}

#[tokio::test]
async fn the_docs_page_renders_the_document_with_a_local_bundle() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = app(&db, &[]);
    let reply = get(&app, "/docs").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("text/html; charset=utf-8")
    );
    let html = reply.text();
    assert!(html.contains("/docs/redoc.standalone.js"), "{html}");
    assert!(html.contains("/openapi.json"), "{html}");
    assert!(!html.contains("http://"), "{html}");
    assert!(!html.contains("https://"), "{html}");

    let bundle = get(&app, "/docs/redoc.standalone.js").await;
    assert_eq!(bundle.status, StatusCode::OK);
    assert_eq!(
        bundle.header("content-type").as_deref(),
        Some("text/javascript; charset=utf-8")
    );
    assert!(bundle.text().contains("Redoc"));
}

#[tokio::test]
async fn a_configured_origin_may_read_the_api_and_another_may_not() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = app(
        &db,
        &[(
            "FLAKERADAR_CORS_ORIGINS",
            "http://localhost:5173,https://app.example.com",
        )],
    );
    let request = axum::http::Request::builder()
        .method(Method::OPTIONS)
        .uri("/api/health")
        .header("origin", "https://app.example.com")
        .header("access-control-request-method", "GET")
        .body(axum::body::Body::empty())
        .expect("the fixture request builds");
    let reply = send(&app, request).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.header("access-control-allow-origin").as_deref(),
        Some("https://app.example.com")
    );
    assert_eq!(reply.header("access-control-allow-credentials"), None);

    let request = axum::http::Request::builder()
        .method(Method::OPTIONS)
        .uri("/api/health")
        .header("origin", "https://elsewhere.example.com")
        .header("access-control-request-method", "GET")
        .body(axum::body::Body::empty())
        .expect("the fixture request builds");
    let reply = send(&app, request).await;
    assert_eq!(reply.header("access-control-allow-origin"), None);
}

#[tokio::test]
async fn the_request_log_names_the_path_and_never_the_query() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = app(&db, &[]);
    let captured = CapturedLog::install();
    let reply = get(&app, "/api/health?repo=o%2Fr&X-API-Key=secret").await;
    assert_eq!(reply.status, StatusCode::OK);

    let logged = captured.lines();
    assert!(logged.contains("path=/api/health"), "{logged}");
    assert!(!logged.contains("secret"), "{logged}");
    assert!(!logged.contains("repo="), "{logged}");
    assert!(logged.contains("status=200"), "{logged}");
}

#[tokio::test]
async fn the_json_body_limit_is_twenty_megabytes() {
    let db = TestDb::new().await.expect("a test database is available");
    // The body limit is app middleware, so a route that reads a body is
    // registered before it, exactly as `build_app` does with its own routes.
    let settings = settings(&[]);
    let state = AppState::new(
        db.connection().clone(),
        std::sync::Arc::new(settings.clone()),
    )
    .expect("the app state builds");
    let app = flakeradar::app::with_common_layers(
        axum::Router::new().route(
            "/echo",
            axum::routing::post(|body: axum::body::Bytes| async move { body.len().to_string() }),
        ),
        &settings,
    )
    .with_state(state);

    let inside = "a".repeat(flakeradar::app::JSON_BODY_LIMIT);
    let reply = post(
        &app,
        "/echo",
        &[("content-type", "application/json")],
        inside,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);

    let over = "a".repeat(flakeradar::app::JSON_BODY_LIMIT + 1);
    let reply = post(&app, "/echo", &[("content-type", "application/json")], over).await;
    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn the_app_is_built_over_the_state_it_is_given() {
    let db = TestDb::new().await.expect("a test database is available");
    // The token reaches the app through the state, so this also proves the state
    // is attached before the first request rather than after.
    let state = AppState::new(
        db.connection().clone(),
        std::sync::Arc::new(settings(&[("FLAKERADAR_API_TOKEN", "t")])),
    )
    .expect("the app state builds");
    let app = build_app(state);
    let reply = get(&app, "/api/health").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json(), json!({"status": "ok"}));
}

#[tokio::test]
async fn the_app_state_debug_names_no_database_password() {
    let db = TestDb::new().await.expect("a test database is available");
    let state = AppState::new(
        db.connection().clone(),
        std::sync::Arc::new(settings(&[
            ("FLAKERADAR_API_TOKEN", "real-token"),
            (
                "FLAKERADAR_DATABASE_URL",
                "postgresql://flakeradar:s3cret-password@db:5432/flakeradar",
            ),
            (
                "FLAKERADAR_WEBHOOK_URLS",
                "https://hooks.example.com/T000/B000/xyz",
            ),
        ])),
    )
    .expect("the app state builds");
    let printed = format!("{state:?}");
    // The state derives `Debug` over the settings, so every credential in them
    // reaches any log line that prints it unless the field redacts itself.
    assert!(!printed.contains("s3cret-password"), "{printed}");
    assert!(!printed.contains("real-token"), "{printed}");
    assert!(!printed.contains("hooks.example.com"), "{printed}");
    assert!(printed.contains("Secret(..)"), "{printed}");
}

/// A log sink a test can read back.
///
/// The subscriber is installed for the current thread only, so it sees the
/// requests this test makes and no others.
struct CapturedLog {
    lines: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    _guard: tracing::subscriber::DefaultGuard,
}

impl CapturedLog {
    /// Starts capturing this thread's log.
    fn install() -> Self {
        use std::sync::{Arc, Mutex};

        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || SharedWriter(Arc::clone(&sink)))
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        Self {
            lines,
            _guard: guard,
        }
    }

    /// Everything logged so far.
    fn lines(&self) -> String {
        let guard = self.lines.lock().expect("the log sink is not poisoned");
        String::from_utf8_lossy(&guard).into_owned()
    }
}

/// A `io::Write` that appends to the shared buffer.
struct SharedWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_error| std::io::Error::other("the log sink is poisoned"))?;
        guard.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
