//! The REST client's own tests: what an answer classifies as, and what a token does.

// Rust guideline compliant 2026-07-21

use flakeradar::github::{self, Api};
use flakeradar::settings::Secret;
use serde_json::json;
use wiremock::MockServer;

use crate::support::{CapturedLog, TOKEN, mount_close, requested_paths};

#[test]
fn an_issue_state_reason_that_is_not_text_reads_as_none() {
    let answer = github::Answer::new(
        200,
        wiremock::http::HeaderMap::new(),
        json!({"state": "closed", "state_reason": 42}).to_string(),
    );
    assert_eq!(answer.state_reason(), None);
    assert_eq!(answer.issue_state().as_deref(), Some("closed"));
}

#[test]
fn an_api_never_prints_its_token() {
    let api =
        Api::new(Secret::new(TOKEN), "https://api.github.com").expect("the HTTP client builds");
    let debug = format!("{api:?}");
    assert!(debug.contains("https://api.github.com"), "{debug}");
    assert!(
        !debug.contains(TOKEN),
        "the Debug line redacts the token: {debug}"
    );
}

#[test]
fn a_failed_client_build_is_an_error_at_construction() {
    let error = Api::with_client_build(Secret::new(TOKEN), "https://api.github.com", || {
        Err(anyhow::anyhow!("the TLS backend did not initialize"))
    })
    .expect_err("a failed build is an error, not a client without its timeout");
    assert_eq!(
        format!("{error:#}"),
        "building the GitHub HTTP client: the TLS backend did not initialize",
        "the error carries the builder's own"
    );
}

#[tokio::test]
async fn a_token_the_header_cannot_carry_goes_out_with_no_credentials() {
    let server = MockServer::start().await;
    mount_close(&server, 123, 201, false).await;
    mount_close(&server, 123, 200, true).await;
    // A newline cannot be part of a header value, so `Bearer <token>` is no
    // header at all: the request still goes out, carrying no credentials.
    let api = Api::new(Secret::new("tok\nen"), &server.uri()).expect("the HTTP client builds");

    let logged =
        CapturedLog::during(github::close_issues_after_removal("acme/app", &[123], &api)).await;

    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues/123/comments".to_owned(),
            "/repos/acme/app/issues/123".to_owned(),
        ],
        "an unrepresentable token does not stop the request"
    );
    assert!(
        requests
            .first()
            .is_some_and(|request| request.headers.get("authorization").is_none()),
        "the request carries no authorization header"
    );
    // Not a log scrape: that a credential never reaches a log line is the rule this
    // test exists for, so the line is the outcome and not a proxy for one.
    assert!(
        !logged.contains("tok"),
        "a token never reaches a log line: {logged}"
    );
}

#[tokio::test]
async fn a_rate_limit_classifies_only_throttling() {
    use reqwest::header::HeaderMap;

    let answer = |status: u16, headers: &[(&str, &str)], text: &str| {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
                reqwest::header::HeaderValue::from_str(value).expect("a header value"),
            );
        }
        github::Answer::new(status, map, text.to_owned())
    };

    assert!(answer(429, &[], "").rate_limited());
    assert!(answer(403, &[("x-ratelimit-remaining", "0")], "{}").rate_limited());
    assert!(answer(403, &[("retry-after", "60")], "{}").rate_limited());
    assert!(
        answer(
            403,
            &[],
            r#"{"message": "You have exceeded a secondary rate limit"}"#
        )
        .rate_limited()
    );
    assert!(
        answer(
            403,
            &[],
            r#"{"message": "You have exceeded a Secondary Rate Limit"}"#
        )
        .rate_limited()
    );
    assert!(
        !answer(
            403,
            &[],
            r#"{"message": "Resource not accessible by personal access token"}"#
        )
        .rate_limited(),
        "a permission 403 is not throttling"
    );
    assert!(!answer(403, &[], "{}").rate_limited());
    assert!(
        !answer(403, &[], "<html>Gateway</html>").rate_limited(),
        "a non-JSON body must not raise"
    );
    assert!(!answer(403, &[], "[1, 2]").rate_limited());
    assert!(!answer(403, &[], "null").rate_limited());
    assert!(!answer(404, &[], "{}").rate_limited());
    assert!(!answer(410, &[], "{}").rate_limited());
    assert!(!answer(201, &[], r#"{"number": 5}"#).rate_limited());
}
