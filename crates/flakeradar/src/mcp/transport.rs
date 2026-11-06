//! The MCP transport: the router, the bearer check and the JSON-body guard.
//!
//! `POST /mcp/` is rmcp's Streamable HTTP transport with no sessions, so a
//! client may connect, call and disconnect without the server remembering it.
//! Only a POST has to be JSON: the frozen server's transport parsed the body
//! itself and answered a body it could not read with JSON-RPC `-32700`, which
//! rmcp answers differently, so [`require_json_body`] reads it first.

// Rust guideline compliant 2026-07-21

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde_json::{Value, json};

use crate::app::{AppState, JSON_BODY_LIMIT};
use crate::mcp::McpServer;
use crate::middleware::constant_time_eq;

/// The path the Streamable HTTP transport is mounted at.
pub const MCP_PATH: &str = "/mcp/";

/// The path clients that drop the trailing slash ask for.
const MCP_BARE_PATH: &str = "/mcp";

/// The MCP router: the transport at [`MCP_PATH`], and the 307 a client that
/// drops the trailing slash is sent.
///
/// The token check wraps the transport alone, so the redirect answers without
/// one: the frozen server redirected before it checked the token, and a client
/// that has not sent its credentials yet must still find the right path.
pub fn router(state: AppState) -> Router<AppState> {
    let pool = state.pool().clone();
    let settings = Arc::new(state.settings().clone());
    let service = StreamableHttpService::new(
        move || Ok(McpServer::new(pool.clone(), Arc::clone(&settings))),
        Arc::new(NeverSessionManager::default()),
        server_config(),
    );
    let guarded = Router::new()
        .route_service(MCP_PATH, service)
        // Inner to outer: the body has to be JSON before the transport parses
        // it, and the token is checked before either, as the frozen server's
        // auth middleware ran first.
        .route_layer(axum::middleware::from_fn(require_json_body))
        .route_layer(axum::middleware::from_fn_with_state(
            state,
            require_bearer_token,
        ));
    Router::new()
        .route(MCP_BARE_PATH, any(redirect_to_mcp_path))
        .merge(guarded)
}

/// The transport's configuration: stateless, JSON responses, no host check.
///
/// `json_response` answers a single request with `application/json` instead of
/// an event stream, which every MCP client accepts. The Host allow-list is
/// disabled because FlakeRadar is self-hosted under arbitrary names, and the
/// frozen server never checked it.
fn server_config() -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .disable_allowed_hosts()
}

/// Sends `POST /mcp` to `POST /mcp/` with a 307, as Starlette's
/// `redirect_slashes` did.
async fn redirect_to_mcp_path() -> Response {
    axum::response::Redirect::temporary(MCP_PATH).into_response()
}

/// Reads a POST's body, and answers a body that is not JSON the way the frozen
/// server did: HTTP 400 with JSON-RPC `-32700`.
///
/// rmcp answers 415 with a plain-text body when it cannot deserialize one, while
/// the frozen server's transport (fastmcp 4.1.6, measured over a loopback port)
/// answered 400 with `{"jsonrpc":"2.0","id":null,"error":{"code":-32700,
/// "message":"Parse error: <message>"}}`. Both sides parse with serde's JSON
/// decoder, so the message after the prefix is the same text for the same body.
/// The body is put back for the transport to read; a request that is not a POST
/// passes through untouched, and so does one whose body reads to a JSON value.
async fn require_json_body(request: Request, next: Next) -> Response {
    if request.method() != Method::POST {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, JSON_BODY_LIMIT).await {
        Ok(bytes) => bytes,
        Err(error) => return parse_error(&error.to_string()),
    };
    if let Err(error) = serde_json::from_slice::<Value>(&bytes) {
        return parse_error(&error.to_string());
    }
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

/// The protocol's parse-error answer, `-32700`, as the frozen server sent it.
fn parse_error(detail: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": {"code": -32700, "message": format!("Parse error: {detail}")},
        })),
    )
        .into_response()
}

/// Refuses a request whose `Authorization` is not `Bearer <token>`.
///
/// The comparison does not stop at the first differing byte, so the time a
/// rejection takes says nothing about the token. A missing header is refused
/// even when the configured token is empty, which is what the frozen server's
/// `RequireAuthMiddleware` did.
async fn require_bearer_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(bearer_token);
    let allowed = presented.is_some_and(|token| {
        constant_time_eq(
            token.as_bytes(),
            state.settings().api_token.expose().as_bytes(),
        )
    });
    if !allowed {
        return unauthorized();
    }
    next.run(request).await
}

/// The credentials of an `Authorization: Bearer …` header.
fn bearer_token(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then_some(token)
}

/// The 401 a missing or wrong token is answered with.
fn unauthorized() -> Response {
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    let _ = response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}
