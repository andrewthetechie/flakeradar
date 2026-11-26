//! The Webhook sender's own suite: what one `POST` answers, and what a caller may
//! log about it.
//!
//! The endpoint is a stub server, so a test asserts the identifiers the sender hands
//! back and the log a caller writes from them, rather than that a function was
//! called. The sender emits no event of its own — a log message is a static literal,
//! so a task's wording cannot travel into it as data — which makes the identifiers
//! the thing to test: no input's URL may reach a line at any level.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails loudly when a stub or a delivery does not behave"
)]

use flakeradar::webhook_sender::{Delivery, WebhookSender, client};
use flakeradar_test_support::CapturedLog;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The body one `POST` carries. The sender never inspects it, so any JSON will do.
fn body() -> Value {
    json!({"text": "one batch of Tier changes"})
}

/// Logs one `Delivery` the way the two leader tasks do — one event per outcome, at
/// the level that outcome is worth — and answers the static message it wrote.
///
/// The sentences stay with the caller, which is the design: every value that reaches
/// the log is one the sender returned, and nothing else identifies the endpoint.
fn log_like_a_caller(delivery: &Delivery) -> &'static str {
    match delivery {
        Delivery::Sent { endpoint } => {
            tracing::info!(
                name: "webhook.delivery.sent",
                endpoint = %endpoint,
                "Webhook endpoint took the batch"
            );
            "Webhook endpoint took the batch"
        }
        Delivery::Refused {
            endpoint, status, ..
        } => {
            tracing::warn!(
                name: "webhook.delivery.rejected",
                endpoint = %endpoint,
                status,
                "Webhook endpoint refused the batch"
            );
            "Webhook endpoint refused the batch"
        }
        Delivery::Unreachable { endpoint, error } => {
            tracing::error!(
                name: "webhook.delivery.failed",
                endpoint = %endpoint,
                error,
                "Webhook endpoint never answered"
            );
            "Webhook endpoint never answered"
        }
    }
}

/// Asserts that `secret` and any URL reach no log line, and that the event the
/// caller wrote — at `level`, the level the outcome deserves — names the endpoint by
/// `label` alone: the host, or the fallback.
#[track_caller]
fn assert_named_by_host_only(
    log: &CapturedLog,
    line: &str,
    level: &str,
    label: &str,
    secret: &str,
    url: &str,
) {
    let logged = log.lines();
    assert!(
        !logged.contains(secret),
        "no log line carries the text that stands for a credential: {logged}"
    );
    assert!(
        !logged.contains(url),
        "no log line carries the URL: {logged}"
    );
    assert!(
        !logged.contains("http://"),
        "no log line carries a URL scheme, only a host: {logged}"
    );
    let event = log.only_line_holding(line);
    assert!(
        event.contains(level),
        "the event is written at the level the outcome is worth: {event}"
    );
    let endpoint_field = format!("endpoint={label}");
    assert!(
        event.contains(&endpoint_field),
        "the {line:?} line must name the endpoint by {endpoint_field:?}: {event}"
    );
}

#[tokio::test]
async fn an_accepted_post_is_answered_sent_and_logged_by_host() {
    /// A credential in the path, which no log line may carry.
    const SECRET: &str = "SECRETTOKEN";

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/{SECRET}/hook")))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let url = format!("{}/{SECRET}/hook", server.uri());
    // The client seam a worker uses: the sender delivers through a client the caller
    // built with this module's builder.
    let sender = WebhookSender::from_client(client().expect("the Webhook client builds"));

    let log = CapturedLog::install();
    let body = body();
    let delivery = sender.post(&url, &body).await;
    let line = log_like_a_caller(&delivery);

    let Delivery::Sent { endpoint } = &delivery else {
        panic!("a 200 answer is classified as sent: {delivery:?}");
    };
    assert_eq!(
        endpoint, "127.0.0.1",
        "the endpoint identifier is the host alone"
    );
    let request = server
        .received_requests()
        .await
        .expect("the stub recorded its requests")
        .into_iter()
        .next()
        .expect("one request was sent");
    assert_eq!(
        request
            .headers
            .get("user-agent")
            .and_then(|value| value.to_str().ok()),
        Some(concat!("flakeradar/", env!("CARGO_PKG_VERSION"))),
        "the builder's user agent reaches the wire"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&request.body).expect("the body is JSON"),
        body,
        "the body is posted as JSON"
    );
    assert_named_by_host_only(&log, line, "INFO", "127.0.0.1", SECRET, &url);
}

#[tokio::test]
async fn a_refused_post_carries_the_status_and_headers_and_is_logged_by_host() {
    /// A credential in the path, which no log line may carry.
    const SECRET: &str = "SECRETTOKEN";

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/{SECRET}/hook")))
        .respond_with(ResponseTemplate::new(503).append_header("retry-after", "7"))
        .mount(&server)
        .await;
    let url = format!("{}/{SECRET}/hook", server.uri());
    let sender = WebhookSender::new().expect("the HTTP client builds");

    let log = CapturedLog::install();
    let delivery = sender.post(&url, &body()).await;
    let line = log_like_a_caller(&delivery);

    let Delivery::Refused {
        endpoint,
        status,
        headers,
    } = &delivery
    else {
        panic!("a refusal is classified as a refusal: {delivery:?}");
    };
    assert_eq!(
        endpoint, "127.0.0.1",
        "the endpoint identifier is the host alone"
    );
    assert_eq!(*status, 503, "the refusal carries the status");
    assert_eq!(
        headers
            .get("retry-after")
            .and_then(|value| value.to_str().ok()),
        Some("7"),
        "the refusal carries its headers, so a caller parses retry-after itself"
    );
    assert_eq!(
        server
            .received_requests()
            .await
            .expect("the stub recorded its requests")
            .len(),
        1,
        "the sender posts once and leaves the retries to its caller"
    );
    assert_named_by_host_only(&log, line, "WARN", "127.0.0.1", SECRET, &url);
}

#[tokio::test]
async fn an_unreachable_endpoint_is_logged_by_host_without_the_url_it_was_given() {
    /// A credential in the path, which no log line may carry.
    const SECRET: &str = "SECRETTOKEN";

    // Nothing listens on port 1: the connect fails before the request is sent.
    let url = format!("http://127.0.0.1:1/{SECRET}/hook");
    let sender = WebhookSender::new().expect("the HTTP client builds");

    let log = CapturedLog::install();
    let delivery = sender.post(&url, &body()).await;
    let line = log_like_a_caller(&delivery);

    let Delivery::Unreachable { endpoint, error } = &delivery else {
        panic!("a connect failure is classified as unreachable: {delivery:?}");
    };
    assert_eq!(
        endpoint, "127.0.0.1",
        "the endpoint identifier is the host alone"
    );
    assert!(
        !error.contains(SECRET) && !error.contains("http"),
        "the returned error is the stripped one: {error}"
    );
    assert!(!error.is_empty(), "the failure still says what went wrong");
    assert_named_by_host_only(&log, line, "ERROR", "127.0.0.1", SECRET, &url);
}

#[tokio::test]
async fn an_endpoint_that_is_not_a_url_is_logged_as_endpoint() {
    /// A credential in the text that stands in for a URL, which no log line may
    /// carry.
    const SECRET: &str = "SECRETTOKEN";

    let url = format!("not a url/{SECRET}");
    let sender = WebhookSender::new().expect("the HTTP client builds");

    let log = CapturedLog::install();
    let delivery = sender.post(&url, &body()).await;
    let line = log_like_a_caller(&delivery);

    let Delivery::Unreachable { endpoint, error } = &delivery else {
        panic!("an unsendable URL is classified as unreachable: {delivery:?}");
    };
    assert_eq!(
        endpoint, "endpoint",
        "an unparseable URL leaves only the generic label"
    );
    assert!(
        !error.contains(SECRET),
        "the returned error names no URL: {error}"
    );
    assert_named_by_host_only(&log, line, "ERROR", "endpoint", SECRET, &url);
}
