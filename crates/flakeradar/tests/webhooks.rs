//! The Notification task: one POST per Report's Tier changes, retried per ADR 0009.
//!
//! Ported from `backend/tests/test_webhook_worker.py`. The endpoint is a stub
//! server, so a test asserts what was posted rather than that a function was
//! called, and nothing waits on the real clock.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a stub or a delivery does not behave"
)]

use std::sync::Arc;
use std::time::Duration;

use flakeradar::LeaderTask;
use flakeradar::WebhookWorker;
use flakeradar_core::{Kind, Tier, TierChange, WebhookFormat};
use flakeradar_db::ProcessOutcome;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// One Test's crossing of the Flake threshold.
fn test_change() -> TierChange {
    TierChange {
        kind: Kind::Test,
        id: 1,
        repo: "acme/app".to_owned(),
        project: Some("backend".to_owned()),
        name: "test_total".to_owned(),
        flakiness_score: 0.42,
        tier: Tier::Flaky,
        confirmed_flake_count: 2,
        classname: Some("tests.test_cart".to_owned()),
        pipeline: None,
        failure_category: None,
    }
}

/// One Job's crossing of the Flake threshold.
fn job_change() -> TierChange {
    TierChange {
        kind: Kind::Job,
        id: 2,
        repo: "acme/app".to_owned(),
        project: None,
        name: "unit".to_owned(),
        flakiness_score: 0.5,
        tier: Tier::Flaky,
        confirmed_flake_count: 1,
        classname: None,
        pipeline: Some("ci".to_owned()),
        failure_category: None,
    }
}

/// One processed Report carrying `changes`.
fn outcome(changes: Vec<TierChange>) -> ProcessOutcome {
    ProcessOutcome {
        report_id: 1,
        status: "processed".to_owned(),
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
    WebhookWorker::new(urls, WebhookFormat::Json, "")
}

/// How many requests the stub has recorded.
async fn recorded(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .map(|requests| requests.len())
        .unwrap_or(0)
}

/// Waits until the stub has recorded `expected` requests, without asserting how
/// long that took.
async fn until_count(server: &MockServer, expected: usize) {
    for _ in 0..400 {
        if recorded(server).await == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "the stub recorded {} requests, not {expected}",
        recorded(server).await
    );
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
    until_count(&server, 1).await;
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
    tokio::time::sleep(Duration::from_millis(50)).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");

    assert_eq!(
        worker.queued(),
        0,
        "an outcome with no Tier change is not queued"
    );
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert!(requests.is_empty());
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
    until_count(&server, 2).await;
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
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(400))
        .mount(&server)
        .await;
    let worker = Arc::new(worker(vec![format!("{}/hook", server.uri())]));
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    until_count(&server, 1).await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
    assert_eq!(
        server
            .received_requests()
            .await
            .expect("the stub records requests")
            .len(),
        1
    );
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
    until_count(&server, 2).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
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
    until_count(&server, 3).await;

    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![job_change()])).expect("the queue takes it");
    until_count(&server, 4).await;
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
async fn an_unreachable_endpoint_never_raises_and_never_stops_the_task() {
    // Nothing is listening on this port: every attempt fails to connect.
    let worker = Arc::new(worker(vec!["http://127.0.0.1:1/hook".to_owned()]));
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    // A second batch proves the task is still serving after the failures.
    tokio::time::sleep(Duration::from_millis(20)).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![job_change()])).expect("the queue takes it");
    tokio::time::sleep(Duration::from_millis(20)).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
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
    for _ in 0..400 {
        if recorded(&broken).await == 3 && recorded(&good).await == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
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
    assert_eq!(worker.dropped(), 1, "one batch was dropped");
    assert_eq!(worker.queued(), flakeradar::webhooks::QUEUE_MAX);

    LeaderTask::start(worker.as_ref()).await;
    until_count(&server, flakeradar::webhooks::QUEUE_MAX).await;
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
    let worker = Arc::new(WebhookWorker::new(
        vec![format!("{}/hook", server.uri())],
        WebhookFormat::Slack,
        "",
    ));
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change(), job_change()]))
        .expect("the queue takes it");
    until_count(&server, 1).await;
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
    use std::io::Write as _;
    use std::sync::Mutex;

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

    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || Sink(Arc::clone(&sink)))
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);

    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome(vec![test_change()]))
        .expect("the queue takes it");
    until_count(&server, 1).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
    drop(guard);

    let logged = String::from_utf8_lossy(
        &lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .into_owned();
    assert!(!logged.is_empty(), "the rejected batch logged something");
    assert!(
        !logged.contains(SECRET),
        "no log line carries the URL: {logged}"
    );
    assert!(
        logged.contains("127.0.0.1"),
        "the endpoint's host is enough to find it: {logged}"
    );
    let _ = std::io::stdout().flush();
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
    until_count(&server, 3).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
}
