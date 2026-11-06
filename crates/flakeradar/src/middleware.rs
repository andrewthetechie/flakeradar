//! Middleware: the shared-token check, the CORS policy and the request log line.

// Rust guideline compliant 2026-07-21

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::trace::{
    DefaultOnBodyChunk, DefaultOnEos, DefaultOnFailure, DefaultOnRequest, DefaultOnResponse,
    MakeSpan, TraceLayer,
};
use tracing::{Level, Span};

use crate::app::AppState;
use crate::error::ApiError;
use crate::settings::Settings;

/// The request header a CI system presents the shared token in.
///
/// MCP clients present the same token as an `Authorization: Bearer`, which the
/// MCP transport checks; this header is what `actions/report/report.sh` sends.
pub const API_KEY_HEADER: &str = "x-api-key";

/// What a request with a missing or wrong token is told.
pub const INVALID_TOKEN_DETAIL: &str = "Invalid or missing X-API-Key";

/// Refuses a request whose `X-API-Key` is not the configured token.
///
/// The comparison does not stop at the first differing byte and does not depend
/// on where the difference is, so the time a rejection takes says nothing about
/// the token. A route group attaches this with
/// `.route_layer(axum::middleware::from_fn_with_state(state, require_token))`.
pub async fn require_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(API_KEY_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !constant_time_eq(
        presented.as_bytes(),
        state.settings().api_token.expose().as_bytes(),
    ) {
        return ApiError::new(StatusCode::UNAUTHORIZED, INVALID_TOKEN_DETAIL).into_response();
    }
    next.run(request).await
}

/// Whether two byte strings are equal, in time that does not depend on where.
///
/// The length is compared first, which is all this check gives away either: a
/// token of the wrong length is rejected, and the bytes of a token of the right
/// length are compared without an early exit.
pub(crate) fn constant_time_eq(presented: &[u8], expected: &[u8]) -> bool {
    if presented.len() != expected.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in presented.iter().zip(expected) {
        difference |= left ^ right;
    }
    difference == 0
}

/// The CORS policy every route answers with.
///
/// The configured origins are echoed back, any method and any header are allowed,
/// and credentials are off. An origin that is not configured is answered without
/// an `Access-Control-Allow-Origin`, so the browser blocks it.
pub(crate) fn cors_layer(settings: &Settings) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(cors_origins(settings)))
        .allow_methods(Any)
        .allow_headers(Any)
}

/// The configured origins that can be echoed as a header value.
///
/// An entry that cannot be one, such as a value with a newline in it, is dropped
/// rather than failing every request: a policy with one unusable origin still
/// serves the usable ones.
fn cors_origins(settings: &Settings) -> Vec<HeaderValue> {
    settings
        .cors_origin_list()
        .iter()
        .filter_map(|origin| HeaderValue::try_from(origin).ok())
        .collect()
}

/// The span one request is logged under.
///
/// It records the method and the path, never the URI: a query string can carry a
/// token, and a logged outbound URL can carry a Webhook URL, which is a
/// credential (ADR 0009). The response line adds the status and the latency.
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestSpan;

impl<B> MakeSpan<B> for RequestSpan {
    fn make_span(&mut self, request: &axum::http::Request<B>) -> Span {
        tracing::info_span!(
            "request",
            method = %request.method(),
            path = %request.uri().path(),
        )
    }
}

/// The request log's layer, named so its constructor can say what it returns.
pub(crate) type RequestLog = TraceLayer<
    SharedClassifier<ServerErrorsAsFailures>,
    RequestSpan,
    DefaultOnRequest,
    DefaultOnResponse,
    DefaultOnBodyChunk,
    DefaultOnEos,
    DefaultOnFailure,
>;

/// The request log: one span per request, one line per response.
pub(crate) fn request_log_layer() -> RequestLog {
    TraceLayer::new_for_http()
        .make_span_with(RequestSpan)
        .on_response(DefaultOnResponse::new().level(Level::INFO))
        .on_failure(DefaultOnFailure::new().level(Level::ERROR))
}

#[cfg(test)]
mod tests {
    //! The comparison and the CORS policy need no server, so they are checked here.

    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderValue, Method, Request, StatusCode, header};
    use axum::routing::get;
    use tower::ServiceExt as _;

    use super::{constant_time_eq, cors_layer, cors_origins};
    use crate::settings::Settings;

    #[test]
    fn the_token_comparison_is_an_exact_match() {
        assert!(constant_time_eq(b"parity-token", b"parity-token"));
        assert!(!constant_time_eq(b"parity-token", b"parity-tokeN"));
        assert!(!constant_time_eq(b"parity-token ", b"parity-token"));
        assert!(!constant_time_eq(b"", b"parity-token"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn an_unusable_cors_origin_is_dropped_and_the_rest_kept() {
        let settings = Settings {
            cors_origins: "http://localhost:5173,\n,https://a.example.com".to_owned(),
            ..Settings::default()
        };
        let origins = cors_origins(&settings);
        assert_eq!(
            origins,
            [
                HeaderValue::from_static("http://localhost:5173"),
                HeaderValue::from_static("https://a.example.com")
            ]
        );
    }

    #[test]
    fn a_preflight_from_a_configured_origin_is_allowed() {
        let settings = Settings {
            cors_origins: "http://localhost:5173".to_owned(),
            ..Settings::default()
        };
        let allowed = preflight(&settings, "http://localhost:5173");
        assert_eq!(allowed.status(), StatusCode::OK);
        assert_eq!(
            allowed.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static("http://localhost:5173"))
        );
        assert_eq!(
            allowed.headers().get(header::ACCESS_CONTROL_ALLOW_METHODS),
            Some(&HeaderValue::from_static("*"))
        );

        let refused = preflight(&settings, "https://elsewhere.example.com");
        assert_eq!(
            refused.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            None
        );
    }

    /// The answer a browser's preflight from `origin` gets.
    fn preflight(settings: &Settings, origin: &str) -> axum::response::Response {
        let app = Router::new()
            .route("/", get(|| async {}))
            .layer(cors_layer(settings));
        let request = Request::builder()
            .method(Method::OPTIONS)
            .uri("/")
            .header(header::ORIGIN, origin)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
            .body(Body::empty())
            .expect("the fixture request builds");
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current-thread runtime builds")
            .block_on(app.oneshot(request))
            .expect("the app answers")
    }
}
