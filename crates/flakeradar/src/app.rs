//! The Axum app: state, routes, middleware and the fallback.
//!
//! [`build_app`] is the whole server minus the socket, so a test drives it with
//! `tower::ServiceExt::oneshot` instead of binding a port. Later tasks add their
//! routes to [`routes::router`], and every one of them runs under the middleware
//! [`with_common_layers`] applies.

// Rust guideline compliant 2026-07-21

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use sea_orm::DatabaseConnection;

use crate::error::ApiError;
use crate::middleware::{cors_layer, request_log_layer};
use crate::routes;
use crate::settings::Settings;

/// The largest JSON request body the app accepts unless a route says otherwise.
///
/// Axum's own default is 2 MB, which is under a Pipeline report for a large
/// matrix, so it is raised to the 20 MiB `POST /api/ingest` already allows. The
/// ingest route lifts the limit entirely and enforces its own while reading, so
/// an oversized upload is cut off rather than buffered.
pub const JSON_BODY_LIMIT: usize = 20 * 1024 * 1024;

/// What every handler, middleware and extractor can reach.
#[derive(Debug, Clone)]
pub struct AppState {
    pool: DatabaseConnection,
    settings: Arc<Settings>,
}

impl AppState {
    /// Builds the state from the connection pool and the settings.
    #[must_use]
    pub fn new(pool: DatabaseConnection, settings: Arc<Settings>) -> Self {
        Self { pool, settings }
    }

    /// The connection pool every query borrows from.
    #[must_use]
    pub fn pool(&self) -> &DatabaseConnection {
        &self.pool
    }

    /// The configuration the process runs with.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
}

/// Builds the server's router over `state`.
pub fn build_app(state: AppState) -> Router {
    let settings = Arc::clone(&state.settings);
    let router = Router::new()
        .merge(routes::router())
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed);
    with_common_layers(router, &settings).with_state(state)
}

/// Applies the middleware every route runs under.
///
/// Axum applies a layer only to the routes that exist when it is applied, so
/// [`build_app`] calls this last. A test that registers its own route and wants
/// the app's own body limit, CORS policy and request log calls it too.
pub fn with_common_layers(router: Router<AppState>, settings: &Settings) -> Router<AppState> {
    router
        // Inner to outer: the body limit applies to whichever extractor reads the
        // body, CORS answers a browser's preflight before it reaches a route, and
        // the request log wraps both so every answer is logged once.
        .layer(DefaultBodyLimit::max(JSON_BODY_LIMIT))
        .layer(cors_layer(settings))
        .layer(request_log_layer())
}

/// The response for a path that exists but does not answer this method.
///
/// The frozen server answered with Starlette's `HTTPException`, which FastAPI
/// renders as the usual `{"detail": ...}` document; Axum's own 405 has an empty
/// body. Both send an `Allow` header naming the methods the path does answer.
async fn method_not_allowed() -> Response {
    ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed").into_response()
}

/// The response for a path no route matched.
///
/// The frozen server ran Starlette, which redirects a path with a trailing slash
/// to the same path without it; every API path is registered without the slash,
/// so `GET /api/tests/` answered 307 and the client retried. The fallback keeps
/// that, and answers 404 only when there is no slash to remove. Redirecting a
/// path that then 404s is intended: the client asked for a different resource.
async fn not_found(uri: Uri) -> Response {
    if let Some(target) = without_trailing_slash(&uri) {
        return Redirect::temporary(&target).into_response();
    }
    ApiError::new(StatusCode::NOT_FOUND, "Not Found").into_response()
}

/// The request path with one trailing slash removed, query string kept.
fn without_trailing_slash(uri: &Uri) -> Option<String> {
    let path = uri.path();
    if path == "/" {
        return None;
    }
    let trimmed = path.strip_suffix('/')?;
    let mut target = trimmed.to_owned();
    if let Some(query) = uri.query() {
        target.push('?');
        target.push_str(query);
    }
    Some(target)
}
