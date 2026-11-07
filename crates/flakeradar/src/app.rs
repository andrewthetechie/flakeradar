//! The Axum app: state, routes, middleware and the fallback.
//!
//! [`build_app`] is the whole server minus the socket, so a test drives it with
//! `tower::ServiceExt::oneshot` instead of binding a port. Later tasks add their
//! routes to [`routes::router`], and every one of them runs under the middleware
//! [`with_common_layers`] applies.

// Rust guideline compliant 2026-07-21

use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::header::{self, HeaderValue};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use flakeradar_db::{clock::SharedClock, clock::SystemClock, reads::ReadSettings};
use sea_orm::DatabaseConnection;

use crate::assets::{Asset, AssetSource, NoAssets};
use crate::error::ApiError;
use crate::github::Api;
use crate::middleware::{cors_layer, request_log_layer};
use crate::routes;
use crate::settings::Settings;

/// The largest JSON request body the app accepts unless a route says otherwise.
///
/// Axum's own default is 2 MB, which is under a Pipeline report for a large
/// matrix, so it is raised to the 20 MiB `POST /api/ingest` already allows. The
/// ingest route replaces it with its own limit for the length of one request, so
/// an oversized upload is cut off rather than buffered whole.
pub const JSON_BODY_LIMIT: usize = 20 * 1024 * 1024;

/// How long a browser may keep a file under `assets/` without asking again.
///
/// Vite puts a content hash in every such name, so the bytes behind one name
/// never change and a year is as good as forever. A year is what every bundler's
/// own documentation recommends; a longer one buys nothing, and the maximum a
/// cache is allowed to honour is a year for the `max-age` directive.
const HASHED_ASSET_CACHE: &str = "public, max-age=31536000, immutable";

/// What a browser may do with a page it has already fetched.
///
/// `index.html` is the one file whose name says nothing about its content: a
/// rebuild rewrites it in place, so a cached copy would keep pointing at the
/// previous build's hashed files. `no-cache` still lets it be stored, and makes
/// the browser ask before using it, which the `ETag` turns into a 304.
const PAGE_CACHE: &str = "no-cache";

/// The path prefixes whose 404 is a JSON error rather than a page.
const API_PREFIXES: [&str; 3] = ["/api", "/mcp", "/metrics"];

/// What every handler, middleware and extractor can reach.
#[derive(Debug, Clone)]
pub struct AppState {
    pool: DatabaseConnection,
    settings: Arc<Settings>,
    github: Api,
    assets: Arc<dyn AssetSource>,
    clock: SharedClock,
}

impl AppState {
    /// Builds the state from the connection pool and the settings.
    ///
    /// The GitHub client talks to the production REST API; a test that needs a
    /// different base URL calls [`AppState::with_github`]. Nothing is served
    /// from memory until [`AppState::with_assets`] names a source, which is how
    /// [`crate::server::Server`] hands the state the frontend it was built with. A test
    /// that has to know which instant a request wrote calls
    /// [`AppState::with_clock`].
    ///
    /// # Errors
    ///
    /// Returns an error when the GitHub client cannot be built; see
    /// [`Api::new`].
    pub fn new(pool: DatabaseConnection, settings: Arc<Settings>) -> anyhow::Result<Self> {
        let github = Api::production(settings.github_token.clone())?;
        Ok(Self {
            pool,
            settings,
            github,
            assets: Arc::new(NoAssets),
            clock: Arc::new(SystemClock),
        })
    }

    /// Replaces the clock every handler and worker reads, so a test can say
    /// which instant a request wrote.
    #[must_use]
    pub fn with_clock(mut self, clock: SharedClock) -> Self {
        self.clock = clock;
        self
    }

    /// The clock every handler reads the current instant from.
    #[must_use]
    pub fn clock(&self) -> &SharedClock {
        &self.clock
    }

    /// The read layer's view of these settings, on this state's clock.
    #[must_use]
    pub fn reads(&self) -> ReadSettings {
        self.settings.reads(Arc::clone(&self.clock))
    }

    /// The GitHub client every outward call goes through.
    #[must_use]
    pub fn github(&self) -> &Api {
        &self.github
    }

    /// Replaces the GitHub client, so a test can point it at a stub server.
    #[must_use]
    pub fn with_github(mut self, github: Api) -> Self {
        self.github = github;
        self
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

    /// The source the fallback serves static files from.
    #[must_use]
    pub fn assets(&self) -> &dyn AssetSource {
        self.assets.as_ref()
    }

    /// Replaces the asset source, so a test serves the files it supplies.
    #[must_use]
    pub fn with_assets(mut self, assets: Arc<dyn AssetSource>) -> Self {
        self.assets = assets;
        self
    }
}

/// Builds the server's router over `state`.
pub fn build_app(state: AppState) -> Router {
    let settings = Arc::clone(&state.settings);
    let router = Router::new()
        .merge(routes::router(state.clone()))
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
/// The answer is the usual `{"detail": "Method Not Allowed"}` document with an
/// `Allow` header naming the methods the path does answer; Axum's own 405 has an
/// empty body and no document, which is not the shape this API answers.
async fn method_not_allowed() -> Response {
    method_not_allowed_body(None)
}

/// The 405 document, with `allow` when the path serves a known set of methods.
fn method_not_allowed_body(allow: Option<&'static str>) -> Response {
    let mut response =
        ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed").into_response();
    if let Some(allow) = allow {
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static(allow));
    }
    response
}

/// The response for a path no route matched.
///
/// A path with a trailing slash is redirected to the same path without it; every
/// API path is registered without the slash, so `GET /api/tests/` answers 307 and
/// the client retries. The fallback keeps that, and answers 404 only when there is
/// no slash to remove. Redirecting a path that then 404s is intended: the client
/// asked for a different resource.
///
/// The built frontend is mounted at `/`, after every API route, so a path no route
/// matched is a file when there is one and this 404 when there is not.
/// [`static_response`] is that mount; it answers first, and a path it does not
/// serve reaches the redirect and the 404.
async fn not_found(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    if !is_api_path(uri.path())
        && let Some(response) = static_response(state.assets(), &method, &uri, &headers)
    {
        return response;
    }
    if let Some(target) = without_trailing_slash(&uri) {
        return Redirect::temporary(&target).into_response();
    }
    ApiError::new(StatusCode::NOT_FOUND, "Not Found").into_response()
}

/// Whether a path belongs to the API, the MCP endpoint or the metrics scrape.
///
/// A missing API route is a JSON error the consumers read, never a page, so
/// these prefixes never reach the assets.
fn is_api_path(path: &str) -> bool {
    API_PREFIXES.into_iter().any(|prefix| {
        path.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

/// The static answer for `uri`, or `None` when the frontend does not serve it.
///
/// `None` sends the request on to the trailing-slash redirect and the 404, which
/// is what a path the frontend has no file for gets.
fn static_response(
    source: &dyn AssetSource,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
) -> Option<Response> {
    let path = relative_path(uri.path())?;
    if !is_inside_root(&path) {
        return Some(ApiError::new(StatusCode::NOT_FOUND, "Not Found").into_response());
    }
    let (asset, name) = lookup(source, &path)?;
    if !matches!(*method, Method::GET | Method::HEAD) {
        // A frontend path answers every method but GET and HEAD with this 405,
        // and names the two it serves.
        return Some(method_not_allowed_body(Some("GET, HEAD")));
    }
    Some(asset_response(&asset, &name, method, headers))
}

/// The path inside the frontend's root that a request path names.
fn relative_path(path: &str) -> Option<String> {
    percent_decode(path.strip_prefix('/')?)
}

/// Whether a decoded path stays inside the frontend's root.
///
/// A `..` segment, a backslash or a NUL is refused before the source is asked,
/// so no request can name a file the frontend does not contain. The percent
/// escapes are decoded first, so `%2e%2e` is refused exactly like `..`.
fn is_inside_root(path: &str) -> bool {
    !path.contains(['\\', '\0']) && path.split('/').all(|segment| segment != "..")
}

/// The file `path` names, following a directory to its `index.html`.
///
/// The answer is the asset and the name it was found under, which decides the
/// media type. A trailing slash is dropped first, so `/` and a directory path
/// resolve the way `StaticFiles(html=True)` resolved them.
fn lookup(source: &dyn AssetSource, path: &str) -> Option<(Asset, String)> {
    let path = path.trim_end_matches('/');
    if let Some(asset) = source.get(path) {
        return Some((asset, path.to_owned()));
    }
    let index = if path.is_empty() {
        "index.html".to_owned()
    } else {
        format!("{path}/index.html")
    };
    source.get(&index).map(|asset| (asset, index))
}

/// The answer for one asset, honouring `If-None-Match`.
///
/// `content-length` is the file's own length even for `HEAD`: the method asks for
/// the headers a `GET` would carry, not for a zero-length file. Hyper would
/// otherwise derive the length from the empty body and announce `0`.
fn asset_response(asset: &Asset, name: &str, method: &Method, headers: &HeaderMap) -> Response {
    let cache = if name.starts_with("assets/") {
        HASHED_ASSET_CACHE
    } else {
        PAGE_CACHE
    };
    let etag = asset.etag().to_owned();
    if is_not_modified(headers, &etag) {
        return (
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (header::CACHE_CONTROL, cache.to_owned()),
            ],
        )
            .into_response();
    }
    let body = if *method == Method::HEAD {
        Vec::new()
    } else {
        asset.data().to_vec()
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type(name)),
            (header::ETAG, etag),
            (header::CACHE_CONTROL, cache.to_owned()),
            (header::CONTENT_LENGTH, asset.data().len().to_string()),
        ],
        body,
    )
        .into_response()
}

/// Whether the request already holds the bytes `etag` names.
///
/// A 304 is answered for `*` and for any tag in the list that matches with a weak
/// prefix stripped.
fn is_not_modified(headers: &HeaderMap, etag: &str) -> bool {
    let Some(value) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    if value.trim() == "*" {
        return true;
    }
    value.split(',').any(|tag| {
        let tag = tag.trim();
        tag.strip_prefix("W/").unwrap_or(tag) == etag
    })
}

/// The media type of `name`, with a charset on text.
fn content_type(name: &str) -> String {
    let media = mime_guess::from_path(name)
        .first_or_octet_stream()
        .to_string();
    if media.starts_with("text/") {
        format!("{media}; charset=utf-8")
    } else {
        media
    }
}

/// The bytes `%xx` escapes stand for, with anything else kept as written.
///
/// A path is percent-encoded on the wire and the lookup wants the decoded name, so
/// a request for `/assets/index%2Dabc.js` names the same file as the plain form.
/// An escape that is not two hexadecimal digits, and a byte sequence that is not
/// UTF-8, leave the path unreadable, which is the 404 below.
fn percent_decode(raw: &str) -> Option<String> {
    let input = raw.as_bytes();
    let mut decoded = Vec::with_capacity(input.len());
    let mut index = 0;
    while let Some(&byte) = input.get(index) {
        if byte == b'%' {
            let high = input.get(index + 1).copied().and_then(hex_digit);
            let low = input.get(index + 2).copied().and_then(hex_digit);
            if let (Some(high), Some(low)) = (high, low) {
                decoded.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        decoded.push(byte);
        index += 1;
    }
    String::from_utf8(decoded).ok()
}

/// The value of one hexadecimal digit.
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
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
