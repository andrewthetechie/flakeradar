//! The embedded frontend: the page at `/`, the files beside it, and the paths
//! that must never reach either.
//!
//! Every test serves files it supplies, so what the routes answer does not
//! depend on whether this machine has run `npm run build`. The one test that
//! does look at the real build is `the_embedded_frontend_is_in_the_binary`, and
//! it only exists in a build that embedded one.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the app answers something else"
)]

use axum::http::{Method, StatusCode};
use flakeradar_test_support::{
    Reply, TestDb, app, app_serving, get, request, request_with_headers, settings,
};

use serde_json::json;

/// The files every test serves unless it says otherwise.
///
/// The names and shapes are the ones Vite writes: a page, hashed files under
/// `assets/`, and one directory with its own `index.html`.
const FILES: &[(&str, &[u8], &str)] = &[
    (
        "index.html",
        b"<!doctype html><div id=\"root\"></div>",
        "\"index-1\"",
    ),
    ("assets/index-BmIU4Kio.js", b"console.log(1);", "\"js-1\""),
    ("assets/index-ClGSo3eO.css", b"body{}", "\"css-1\""),
    ("assets/logo.svg", b"<svg/>", "\"svg-1\""),
    ("assets/ibm-plex.woff2", b"\x00\x01", "\"woff-1\""),
    ("sub/index.html", b"<p>sub</p>", "\"sub-1\""),
];

/// The page's own bytes, which a 404 must not be.
const PAGE: &[u8] = b"<!doctype html><div id=\"root\"></div>";

/// Asserts the reply is the API's 404 document and not the page.
fn assert_not_found(reply: &Reply, what: &str) {
    assert_eq!(reply.status, StatusCode::NOT_FOUND, "{what}");
    assert_eq!(reply.json(), json!({"detail": "Not Found"}), "{what}");
    assert_ne!(reply.body, PAGE, "{what} served the page");
}

#[tokio::test]
async fn the_root_serves_the_dashboard() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = get(&app_serving(&db, FILES), "/").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body, PAGE);
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(reply.header("cache-control").as_deref(), Some("no-cache"));
    assert_eq!(reply.header("etag").as_deref(), Some("\"index-1\""));
}

#[tokio::test]
async fn a_directory_serves_its_index_html() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = app_serving(&db, FILES);
    for path in ["/sub/", "/sub"] {
        let reply = get(&app, path).await;
        assert_eq!(reply.status, StatusCode::OK, "{path}");
        assert_eq!(reply.body, b"<p>sub</p>", "{path}");
        assert_eq!(
            reply.header("content-type").as_deref(),
            Some("text/html; charset=utf-8"),
            "{path}"
        );
    }
}

#[tokio::test]
async fn a_hashed_asset_is_served_as_is_with_a_long_cache() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = get(&app_serving(&db, FILES), "/assets/index-BmIU4Kio.js").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body, b"console.log(1);");
    assert_eq!(reply.header("etag").as_deref(), Some("\"js-1\""));
    // Vite puts a content hash in the name, so the bytes behind it never change.
    assert_eq!(
        reply.header("cache-control").as_deref(),
        Some("public, max-age=31536000, immutable")
    );
}

#[tokio::test]
async fn every_asset_gets_the_content_type_its_extension_names() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = app_serving(&db, FILES);
    let expected = [
        (
            "/assets/index-BmIU4Kio.js",
            "text/javascript; charset=utf-8",
        ),
        ("/assets/index-ClGSo3eO.css", "text/css; charset=utf-8"),
        ("/assets/logo.svg", "image/svg+xml"),
        ("/assets/ibm-plex.woff2", "font/woff2"),
    ];
    for (path, content_type) in expected {
        let reply = get(&app, path).await;
        assert_eq!(reply.status, StatusCode::OK, "{path}");
        assert_eq!(
            reply.header("content-type").as_deref(),
            Some(content_type),
            "{path}"
        );
    }
}

#[tokio::test]
async fn a_percent_encoded_path_names_the_same_file() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = get(&app_serving(&db, FILES), "/assets/index%2DBmIU4Kio.js").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body, b"console.log(1);");

    // An escape that is not two hexadecimal digits is part of the name, so the
    // path names no file rather than an unreadable one.
    let reply = get(&app_serving(&db, FILES), "/assets/index%zz.js").await;
    assert_not_found(&reply, "/assets/index%zz.js");
}

#[tokio::test]
async fn an_unknown_path_is_the_api_404_rather_than_the_page() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = app_serving(&db, FILES);
    // The dashboard keeps its state in the query string, so a path the frontend
    // does not hold is a 404 and never the page: no single-page fallback.
    for path in ["/nowhere", "/assets", "/assets/", "/index.htm"] {
        let reply = get(&app, path).await;
        if path == "/assets/" {
            // A trailing slash keeps Starlette's redirect, which lands on the
            // same 404 the path without it gets.
            assert_eq!(reply.status, StatusCode::TEMPORARY_REDIRECT, "{path}");
            assert_eq!(
                reply.header("location").as_deref(),
                Some("/assets"),
                "{path}"
            );
            continue;
        }
        assert_not_found(&reply, path);
    }
}

#[tokio::test]
async fn an_api_path_never_reaches_the_assets() {
    let db = TestDb::new().await.expect("a test database is available");
    // The frontend holds files under the API's prefixes, and still none of them
    // is served: a missing API route is the JSON error its clients read.
    let files: &[(&str, &[u8], &str)] = &[
        ("index.html", PAGE, "\"index-1\""),
        ("api/index.html", b"page", "\"api-index-1\""),
        ("api/nope", b"page", "\"api-1\""),
        ("mcp/nope", b"page", "\"mcp-1\""),
        ("metrics/nope", b"page", "\"metrics-1\""),
    ];
    let app = app_serving(&db, files);
    // `/api` itself is a prefix with nothing after it, and is refused like the
    // rest: its own `index.html` is not a page anyone asked for.
    for path in ["/api", "/api/nope", "/mcp/nope", "/metrics/nope"] {
        assert_not_found(&get(&app, path).await, path);
    }
}

#[tokio::test]
async fn an_api_route_wins_over_an_asset_of_the_same_path() {
    let db = TestDb::new().await.expect("a test database is available");
    // The reference mounted the frontend after every API route, so a route
    // answered first even when the built frontend held a file of that name.
    let files: &[(&str, &[u8], &str)] = &[
        ("index.html", PAGE, "\"index-1\""),
        ("api/health", b"page", "\"health-1\""),
    ];
    let reply = get(&app_serving(&db, files), "/api/health").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json(), json!({"status": "ok"}));
}

#[tokio::test]
async fn an_etag_the_client_already_holds_is_not_modified() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = app_serving(&db, FILES);
    for held in ["\"js-1\"", "*", "W/\"js-1\"", "\"other\", \"js-1\""] {
        let reply = request_with_headers(
            &app,
            Method::GET,
            "/assets/index-BmIU4Kio.js",
            &[("if-none-match", held)],
        )
        .await;
        assert_eq!(reply.status, StatusCode::NOT_MODIFIED, "{held}");
        assert_eq!(reply.body, Vec::<u8>::new(), "{held}");
        assert_eq!(reply.header("etag").as_deref(), Some("\"js-1\""), "{held}");
    }

    // A tag the client does not hold gets the bytes again.
    let reply = request_with_headers(
        &app,
        Method::GET,
        "/assets/index-BmIU4Kio.js",
        &[("if-none-match", "\"other\"")],
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body, b"console.log(1);");
}

#[tokio::test]
async fn a_head_request_carries_the_headers_and_no_body() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = request(&app_serving(&db, FILES), Method::HEAD, "/").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body, Vec::<u8>::new());
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(reply.header("etag").as_deref(), Some("\"index-1\""));
    assert_eq!(reply.header("cache-control").as_deref(), Some("no-cache"));
    // The length is the file's, not the empty body's: Starlette's FileResponse
    // announced the file it would have sent, and a client asks HEAD to learn it.
    assert_eq!(
        reply.header("content-length").as_deref(),
        Some(PAGE.len().to_string().as_str())
    );
}

#[tokio::test]
async fn a_path_that_climbs_out_of_the_root_is_refused() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = app_serving(&db, FILES);
    // Every one of these resolves to `index.html`, or to a file outside the
    // root, if the path is taken at face value. None of them is served.
    for path in [
        "/assets/../index.html",
        "/assets/%2e%2e/index.html",
        "/assets/%2E%2E/index.html",
        // Encoded twice. One decode leaves `%2e%2e` as literal text, which names
        // no file; a second decode would turn it back into `..` and escape, so
        // this pins the single decode the route performs.
        "/assets/%252e%252e/index.html",
        "/..%2findex.html",
        "/..%252findex.html",
        "/%2e%2e/%2e%2e/etc/passwd",
        "/%252e%252e/%252e%252e/etc/passwd",
        "/assets/..%5cindex.html",
        "/assets/%5c..%5cindex.html",
        "/a%00.html",
        "/%ff.html",
    ] {
        assert_not_found(&get(&app, path).await, path);
    }
}

#[tokio::test]
async fn a_post_to_a_served_path_is_a_405() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = request(
        &app_serving(&db, FILES),
        Method::POST,
        "/assets/index-BmIU4Kio.js",
    )
    .await;
    assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(reply.json(), json!({"detail": "Method Not Allowed"}));
    assert_eq!(reply.header("allow").as_deref(), Some("GET, HEAD"));

    // A path the frontend does not hold is the API's 404, exactly as it was
    // before a frontend was embedded.
    let reply = request(&app_serving(&db, FILES), Method::POST, "/nowhere").await;
    assert_not_found(&reply, "POST /nowhere");
}

#[tokio::test]
async fn a_build_with_no_frontend_serves_no_page() {
    let db = TestDb::new().await.expect("a test database is available");
    // `app` is the state a build with no `frontend/dist` has: it embeds nothing,
    // so the fallback answers the API's 404 for the root and for every path.
    let app = app(&db, &[]);
    for path in ["/", "/index.html", "/assets/index-BmIU4Kio.js"] {
        assert_not_found(&get(&app, path).await, path);
    }
    // The API is untouched by a frontend being absent or present.
    let reply = get(&app, "/api/health").await;
    assert_eq!(reply.status, StatusCode::OK);
}

#[cfg(has_frontend)]
#[tokio::test]
async fn the_embedded_frontend_is_in_the_binary() {
    use std::sync::Arc;

    use flakeradar::assets::embedded;
    use flakeradar::{app::AppState, app::build_app};

    let db = TestDb::new().await.expect("a test database is available");
    let state = AppState::new(db.connection().clone(), Arc::new(settings(&[])))
        .expect("the app state builds")
        .with_assets(embedded());
    let app = build_app(state);
    let reply = get(&app, "/").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("text/html; charset=utf-8")
    );
    let page = reply.text();
    assert!(page.contains("<div id=\"root\"></div>"), "{page}");
    // The page names its hashed script absolutely, and it is in the binary too.
    let script = page
        .split("src=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the built page names its script");
    let reply = get(&app, script).await;
    assert_eq!(reply.status, StatusCode::OK, "{script}");
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("text/javascript; charset=utf-8"),
        "{script}"
    );
    assert!(!reply.body.is_empty(), "{script}");
}
