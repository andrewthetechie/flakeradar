//! The Notification task: one POST per Report's Tier changes, retried per ADR 0009.
//!
//! Ported from `backend/tests/test_webhook_worker.py`. The endpoint is a stub
//! server, so a test asserts what was posted rather than that a function was
//! called, and nothing waits on the real clock.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a stub or a delivery does not behave"
)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use flakeradar::LeaderTask;
use flakeradar::WebhookWorker;
use flakeradar_core::{EntityKind, ReportStatus, Tier, TierChange, WebhookFormat};
use flakeradar_db::ProcessOutcome;
use flakeradar_test_support::{dropped, queued, until_finished, wait_until, within};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The most `stop` may take while a delivery is in flight.
///
/// Far under the five-second delivery timeout and the waits between retries a
/// task that did not end would sit through, and far over the wake a cancellation
/// costs, so the bound separates the two without depending on a machine's speed.
const STOP_BOUND: Duration = Duration::from_secs(2);

/// One Test's crossing of the Flake threshold.
fn test_change() -> TierChange {
    TierChange {
        kind: EntityKind::Test,
        id: 1,
        repo: "acme/app".to_owned(),
        project: Some("backend".to_owned()),
        name: "test_total".to_owned(),
        flakiness_score: 0.42,
        tier: Tier::Flaky,
        proven_flake_count: 2,
        classname: Some("tests.test_cart".to_owned()),
        pipeline: None,
        failure_category: None,
    }
}

/// One Job's crossing of the Flake threshold.
fn job_change() -> TierChange {
    TierChange {
        kind: EntityKind::Job,
        id: 2,
        repo: "acme/app".to_owned(),
        project: None,
        name: "unit".to_owned(),
        flakiness_score: 0.5,
        tier: Tier::Flaky,
        proven_flake_count: 1,
        classname: None,
        pipeline: Some("ci".to_owned()),
        failure_category: None,
    }
}

/// One processed Report carrying `changes`.
fn outcome(changes: Vec<TierChange>) -> ProcessOutcome {
    ProcessOutcome {
        report_id: 1,
        status: ReportStatus::Processed,
        run_id: Some(1),
        counts: None,
        touched_test_ids: Vec::new(),
        error: None,
        touched_job_ids: Vec::new(),
        tier_changes: changes,
        retry_count: 0,
    }
}

/// A worker over `urls` in the JSON format.
fn worker(urls: Vec<String>) -> WebhookWorker {
    WebhookWorker::new(urls, WebhookFormat::Json, "").expect("the HTTP client builds")
}

/// How many requests the stub has recorded.
async fn recorded(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .map_or(0, |requests| requests.len())
}

/// An `io::Write` that appends to the shared buffer.
struct Sink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A subscriber that records every level, and the buffer it writes to.
fn captured() -> (tracing::subscriber::DefaultGuard, Arc<Mutex<Vec<u8>>>) {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || Sink(Arc::clone(&sink)))
        .with_max_level(tracing::Level::TRACE)
        .finish();
    (tracing::subscriber::set_default(subscriber), lines)
}

/// The log text recorded so far.
fn logged(lines: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8_lossy(
        &lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .into_owned()
}

/// How many times `needle` appears in the log so far.
fn log_count(lines: &Arc<Mutex<Vec<u8>>>, needle: &str) -> usize {
    logged(lines).matches(needle).count()
}

#[tokio::test]
async fn one_request_per_report_carries_every_event_of_it() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let worker = Arc::new(worker(vec![format!("{}/hook", server.uri())]));
    LeaderTask::start(worker.as_ref()).await;

    let batch = outcome(vec![test_change(), job_change()]);
    LeaderTask::enqueue(worker.as_ref(), &batch).expect("the queue takes the outcome");
    until_finished(worker.queue(), 1).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");

    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    let events: Vec<&str> = body
        .as_array()
        .expect("the payload is a list")
        .iter()
        .map(|event| event["event"].as_str().expect("an event name"))
        .collect();
    assert_eq!(events, vec!["test.flaky", "job.flaky"]);
}

#[tokio::test]
async fn an_outcome_without_a_tier_change_posts_nothing() {
    let server = MockServer::start().await;
    let worker = Arc::new(worker(vec![format!("{}/hook", server.uri())]));
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(Vec::new())).expect("the queue takes it");
    assert_eq!(
        queued(worker.queue()),
        0,
        "an outcome with no Tier change is not queued"
    );
    // A batch with a Tier change is the proof the task was serving: it is
    // delivered, and the batch that carried nothing was never posted.
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    until_finished(worker.queue(), 1).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");

    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert_eq!(requests.len(), 1, "only the batch with events was posted");
    let body: Value = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    assert_eq!(
        body.as_array().expect("the payload is a list").len(),
        1,
        "the one event is the one the second batch carried"
    );
}

#[tokio::test]
async fn a_transient_failure_is_retried_until_it_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let worker = Arc::new(worker(vec![format!("{}/hook", server.uri())]));
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    until_finished(worker.queue(), 1).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
    assert_eq!(
        server
            .received_requests()
            .await
            .expect("the stub records requests")
            .len(),
        2,
        "the retry stopped as soon as the endpoint accepted the batch"
    );
}

#[tokio::test]
async fn a_client_error_is_permanent_and_is_not_retried() {
    // The retry delays are zero, so an attempt this policy retried would show up
    // as a second request rather than as a wait.
    for status in [400, 401, 403, 404] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        let worker = Arc::new(
            WebhookWorker::with_retry_delays(
                vec![format!("{}/hook", server.uri())],
                WebhookFormat::Json,
                "",
                [0.0, 0.0],
            )
            .expect("the HTTP client builds"),
        );
        LeaderTask::start(worker.as_ref()).await;
        LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
            .expect("the queue takes it");
        // The batch is finished, so a retry this policy allowed would already be
        // in the stub's record below.
        until_finished(worker.queue(), 1).await;
        LeaderTask::stop(worker.as_ref())
            .await
            .expect("the task stops");
        assert_eq!(
            server
                .received_requests()
                .await
                .expect("the stub records requests")
                .len(),
            1,
            "{status} is a permanent client error and was not retried"
        );
    }
}

#[tokio::test]
async fn a_rate_limited_attempt_is_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let worker = Arc::new(worker(vec![format!("{}/hook", server.uri())]));
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    until_finished(worker.queue(), 1).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
    assert_eq!(
        recorded(&server).await,
        2,
        "the rate limit was retried and accepted"
    );
}

#[tokio::test]
async fn three_attempts_then_the_task_keeps_serving() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(3)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let worker = Arc::new(worker(vec![format!("{}/hook", server.uri())]));
    LeaderTask::start(worker.as_ref()).await;

    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    until_finished(worker.queue(), 1).await;

    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![job_change()])).expect("the queue takes it");
    until_finished(worker.queue(), 2).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
    assert_eq!(
        server
            .received_requests()
            .await
            .expect("the stub records requests")
            .len(),
        4,
        "three attempts for the first batch, then the second batch goes out"
    );
}

#[tokio::test]
async fn an_unreachable_endpoint_is_tried_three_times_and_the_task_keeps_serving() {
    // Nothing is listening on this port: every attempt fails to connect.
    let (guard, lines) = captured();
    let worker = Arc::new(
        WebhookWorker::with_retry_delays(
            vec!["http://127.0.0.1:1/hook".to_owned()],
            WebhookFormat::Json,
            "",
            [0.0, 0.0],
        )
        .expect("the HTTP client builds"),
    );
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    until_finished(worker.queue(), 1).await;
    // A second batch proves the task is still serving after the failures.
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![job_change()])).expect("the queue takes it");
    until_finished(worker.queue(), 2).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
    drop(guard);
    assert_eq!(
        log_count(&lines, "Webhook endpoint unreachable"),
        6,
        "three attempts per batch, and the task delivered the second one: {}",
        logged(&lines)
    );
}

#[tokio::test]
async fn an_endpoint_that_is_not_a_url_is_retried_and_never_logged() {
    /// A credential no log line may carry.
    const SECRET: &str = "SECRETTOKEN";

    let (guard, lines) = captured();
    let worker = Arc::new(
        WebhookWorker::with_retry_delays(
            vec![format!("not a url/{SECRET}")],
            WebhookFormat::Json,
            "",
            [0.0, 0.0],
        )
        .expect("the HTTP client builds"),
    );
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    until_finished(worker.queue(), 1).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
    drop(guard);

    let logged = logged(&lines);
    assert!(
        log_count(&lines, "endpoint=endpoint") >= 3,
        "a URL that cannot be parsed is named by its fallback label: {logged}"
    );
    assert!(
        !logged.contains(SECRET),
        "no log line carries the URL: {logged}"
    );
}

#[test]
fn a_failed_client_build_is_an_error_at_construction() {
    let error = WebhookWorker::with_client_build(
        vec!["https://hooks.example.com/a".to_owned()],
        WebhookFormat::Json,
        "",
        [0.0, 0.0],
        || Err(anyhow::anyhow!("the TLS backend did not initialize")),
    )
    .expect_err("a failed build is an error, not a task without its timeout");
    assert_eq!(
        format!("{error:#}"),
        "building the Webhook HTTP client: the TLS backend did not initialize",
        "the error carries the builder's own"
    );
}

#[tokio::test]
async fn the_task_names_itself_and_its_debug_line_carries_no_endpoint() {
    /// A credential in the path, which no `Debug` line may carry.
    const SECRET: &str = "SECRETTOKEN";

    let worker = worker(vec![format!("https://hooks.example.com/{SECRET}")]);
    assert_eq!(LeaderTask::name(&worker), "Notification");
    let debug = format!("{worker:?}");
    assert!(debug.contains("WebhookWorker"), "{debug}");
    assert!(
        debug.contains("endpoints: 1"),
        "the shared state's endpoint count is formatted: {debug}"
    );
    assert!(
        !debug.contains(SECRET),
        "the Debug line names no Webhook URL: {debug}"
    );
}

#[tokio::test]
async fn every_endpoint_gets_the_batch_on_its_own_terms() {
    let good = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&good)
        .await;
    let broken = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&broken)
        .await;
    let worker = Arc::new(worker(vec![
        format!("{}/hook", good.uri()),
        format!("{}/hook", broken.uri()),
    ]));
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    until_finished(worker.queue(), 1).await;
    assert_eq!(
        recorded(&broken).await,
        3,
        "the broken endpoint used every attempt"
    );
    assert_eq!(
        recorded(&good).await,
        1,
        "the healthy endpoint got one request"
    );
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
}

#[tokio::test]
async fn a_full_queue_drops_the_oldest_batch_and_delivers_the_newest() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let worker = Arc::new(worker(vec![format!("{}/hook", server.uri())]));

    // Fill the queue without a task draining it: the oldest batch is the stalest news.
    for _ in 0..flakeradar::webhooks::QUEUE_MAX {
        LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
            .expect("the queue takes it");
    }
    let mut recovered = test_change();
    recovered.tier = Tier::Stable;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![recovered])).expect("the queue takes it");
    assert_eq!(dropped(worker.queue()), 1, "one batch was dropped");
    assert_eq!(queued(worker.queue()), flakeradar::webhooks::QUEUE_MAX);

    LeaderTask::start(worker.as_ref()).await;
    until_finished(worker.queue(), flakeradar::webhooks::QUEUE_MAX).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");

    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    let last: Value =
        serde_json::from_slice(&requests.last().expect("a request was recorded").body)
            .expect("the body is JSON");
    assert_eq!(
        last[0]["event"],
        json!("test.recovered"),
        "the newest batch survived the drop"
    );
}

#[tokio::test]
async fn a_slack_payload_is_one_text_with_a_line_per_event() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let worker = Arc::new(
        WebhookWorker::new(
            vec![format!("{}/hook", server.uri())],
            WebhookFormat::Slack,
            "",
        )
        .expect("the HTTP client builds"),
    );
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change(), job_change()]))
        .expect("the queue takes it");
    until_finished(worker.queue(), 1).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");

    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    let body: Value = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    let text = body["text"].as_str().expect("one text field");
    assert_eq!(text.lines().count(), 2);
    assert!(text.contains("became flaky"), "{text}");
}

#[tokio::test]
async fn a_webhook_url_is_never_logged_at_any_level() {
    /// A credential in the path, which no log line may carry.
    const SECRET: &str = "SECRETTOKEN";

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/{SECRET}/hook")))
        .respond_with(ResponseTemplate::new(400))
        .mount(&server)
        .await;
    let url = format!("{}/{SECRET}/hook", server.uri());
    let worker = Arc::new(worker(vec![url]));

    let (guard, lines) = captured();

    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    until_finished(worker.queue(), 1).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
    drop(guard);

    let logged = logged(&lines);
    assert!(!logged.is_empty(), "the rejected batch logged something");
    assert!(
        !logged.contains(SECRET),
        "no log line carries the URL: {logged}"
    );
    assert!(
        logged.contains("127.0.0.1"),
        "the endpoint's host is enough to find it: {logged}"
    );
}

#[tokio::test]
async fn stop_ends_the_task_without_starting_one_twice() {
    let worker = worker(vec!["http://127.0.0.1:1/hook".to_owned()]);
    LeaderTask::stop(&worker)
        .await
        .expect("stopping a task that never ran is fine");

    let worker = Arc::new(worker);
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
}

#[tokio::test]
async fn an_http_date_retry_after_uses_the_default_wait_and_still_retries() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "Wed, 21 Oct 2015 07:28:00 GMT"),
        )
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let worker = Arc::new(worker(vec![format!("{}/hook", server.uri())]));
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    until_finished(worker.queue(), 1).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
}

#[tokio::test]
async fn a_stopped_webhook_task_delivers_again_after_a_restart() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let worker = Arc::new(worker(vec![format!("{}/hook", server.uri())]));
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    until_finished(worker.queue(), 1).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");

    // The same worker, re-elected: a fresh term starts a fresh task.
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![job_change()])).expect("the queue takes it");
    until_finished(worker.queue(), 2).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
}

#[tokio::test]
async fn stopping_ends_the_wait_before_a_retry_at_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    // Waits a task that did not stop would sit through, so the bound below
    // separates a stop from a wait rather than measuring a machine's speed.
    let worker = Arc::new(
        WebhookWorker::with_retry_delays(
            vec![format!("{}/hook", server.uri())],
            WebhookFormat::Json,
            "",
            [60.0, 60.0],
        )
        .expect("the HTTP client builds"),
    );
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    // The batch cannot finish here: the retry waits 60 seconds. The stub's first
    // request is what says the wait has begun.
    wait_until("the first attempt is answered", || async {
        recorded(&server).await == 1
    })
    .await;
    within(
        "the token ends the wait between retries instead of the wait ending it",
        STOP_BOUND,
        LeaderTask::stop(worker.as_ref()),
    )
    .await
    .expect("the task stops");
    assert_eq!(
        recorded(&server).await,
        1,
        "a stopped task makes no second attempt"
    );
}

#[tokio::test]
async fn stopping_ends_an_unanswered_delivery_at_once() {
    let endpoint = common::Unanswered::start().await;
    let worker = Arc::new(worker(vec![format!("{}/hook", endpoint.base)]));
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    endpoint.arrived().await; // the delivery is in flight
    within(
        "the token ends the POST instead of the timeout",
        STOP_BOUND,
        LeaderTask::stop(worker.as_ref()),
    )
    .await
    .expect("the task stops");
    assert_eq!(
        endpoint.requests(),
        1,
        "a stopped delivery is neither retried nor waited out"
    );
}
