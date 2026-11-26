//! The leader-only Notification task: posts a Report's Tier changes to every Webhook URL.
//!
//! Delivery is best effort (ADR 0009): the queue holds a bounded number of
//! batches and drops the oldest when full, the task stores nothing, and a batch
//! queued when the process dies is lost. Three attempts per endpoint with a
//! short wait between them; a client error is permanent and is not retried.
//!
//! This task never logs a Webhook URL: the URL is the credential, so a log line
//! names an endpoint's host only, and a `reqwest` error has its URL removed
//! before it is logged. The mechanics that keep that promise — the client, the
//! host-only label, the redaction and the classification of one `POST` — belong
//! to [`crate::webhook_sender`]; what stays here is the Notification's policy.
//!
//! A delivery and the wait before a retry end the moment leadership does, so a
//! leader that drops never waits out an endpoint's timeout.

// Rust guideline compliant 2026-07-21

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use flakeradar_core::{notify::TierChange, notify::WebhookFormat, notify::render};
use flakeradar_db::processing::ProcessOutcome;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::BoxFuture;
use crate::queue::BatchQueue;
use crate::webhook_sender::{self, Delivery, WebhookSender};
use crate::worker::{LeaderRun, LeaderTask, until_stopped};

/// The delays before the second and third Notification attempts, in seconds.
///
/// Half a second and then one second ride out a dropped connection or a Slack
/// hiccup without holding the queue, and the second delay is double the first so
/// two retries span 1.5 seconds rather than one. A longer schedule would delay
/// every Notification queued behind the endpoint; a shorter one retries inside
/// the same outage.
pub const RETRY_DELAYS: [f64; 2] = [0.5, 1.0];

/// How many attempts one endpoint gets for one batch.
pub const ATTEMPTS: usize = RETRY_DELAYS.len() + 1;

/// The ceiling for a rate limit's `retry-after`, so one endpoint cannot park the
/// queue for minutes.
///
/// A 429 names how long to wait, and a webhook endpoint is free to name minutes
/// or hours; the queue is one task, so honouring that would stall every other
/// endpoint's Notifications for the same span. The wait is capped here, and a
/// Notification that still cannot be delivered is lost by design.
///
/// The cap is per retry, not per batch: one endpoint holds its batch for at most
/// [`ATTEMPTS`] attempts of [`crate::webhook_sender::WEBHOOK_TIMEOUT_SECONDS`] each
/// with two capped waits between them.
pub const MAX_RETRY_AFTER_SECONDS: f64 = 10.0;

/// How many batches the Notification queue holds before it drops the oldest.
///
/// The queue is in memory and a Notification is best effort, so a burst of Tier
/// changes must not grow it without bound; past a thousand pending batches the
/// oldest is dropped and logged. A batch holds every Tier change of one Report,
/// so a thousand batches is far past what a working endpoint accumulates.
pub const QUEUE_MAX: usize = 1000;

/// The statuses a Notification retries; every other client error is permanent.
///
/// 408 is a request the endpoint timed out on and 429 is its rate limit, and both
/// describe a moment rather than the message, so a retry can succeed. Any other
/// refusal (404 for a deleted hook, 400 for a body the endpoint dislikes) is the
/// same on the next attempt, so the queue does not spend retries on it.
pub const RETRYABLE_STATUSES: [u16; 2] = [408, 429];

/// Everything the Notification task shares with its loop.
struct WebhookShared {
    urls: Vec<String>,
    format: WebhookFormat,
    public_url: String,
    retry_delays: [f64; 2],
    sender: WebhookSender,
    queue: BatchQueue<Vec<TierChange>>,
}

impl std::fmt::Debug for WebhookShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookShared")
            .field("endpoints", &self.urls.len())
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl WebhookShared {
    /// Queues one batch, dropping the oldest when the queue is full.
    fn push(&self, batch: Vec<TierChange>) {
        if self.queue.push(batch).is_some() {
            tracing::warn!(
                name: "webhook.queue.dropped",
                dropped = self.queue.dropped(),
                "Webhook queue full; dropped the oldest batch"
            );
        }
    }

    /// The task's loop: take a batch, deliver it to every endpoint.
    async fn run(self: &Arc<Self>, cancel: &CancellationToken) {
        let stop = cancel.clone();
        self.run_with(cancel, move |shared, url, batch| {
            let stop = stop.clone();
            async move {
                shared.post(&url, &batch, &stop).await;
            }
        })
        .await;
    }

    /// `run` with one endpoint's delivery injected, so a test can prove that a
    /// delivery which fails unexpectedly is logged and the loop keeps working.
    async fn run_with<F, Fut>(self: &Arc<Self>, cancel: &CancellationToken, deliver: F)
    where
        F: Fn(Arc<Self>, String, Vec<TierChange>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        while !cancel.is_cancelled() {
            let notified = self.queue.notified();
            let Some(batch) = self.queue.pop() else {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = notified => continue,
                }
            };
            let events = batch.len();
            let mut set = tokio::task::JoinSet::new();
            for url in &self.urls {
                set.spawn(deliver(Arc::clone(self), url.clone(), batch.clone()));
            }
            while let Some(result) = set.join_next().await {
                if let Err(error) = result {
                    tracing::error!(
                        name: "webhook.delivery.failed",
                        events,
                        error = %error,
                        "Webhook delivery failed",
                    );
                }
            }
            self.queue.finish();
        }
    }

    /// Posts one batch to one endpoint, with the retry policy of ADR 0009.
    ///
    /// The POST and the wait before a retry both end when `stop` fires: an
    /// endpoint that never answers costs the leader no more than the cancellation.
    async fn post(self: &Arc<Self>, url: &str, batch: &[TierChange], stop: &CancellationToken) {
        let body = render(batch, self.format, &self.public_url);
        let label = webhook_sender::endpoint(url);
        for attempt in 0..ATTEMPTS {
            let delay = self.retry_delays.get(attempt).copied().unwrap_or_default();
            let after = match self
                .attempt(url, &body, &label, batch.len(), attempt, stop)
                .await
            {
                Attempt::Stopped | Attempt::Finished => return,
                Attempt::Retry { after } => after.unwrap_or(delay),
            };
            // The last attempt has nothing left to wait for.
            if attempt + 1 == ATTEMPTS {
                return;
            }
            let wait =
                tokio::time::sleep(Duration::try_from_secs_f64(after.max(0.0)).unwrap_or_default());
            if until_stopped(stop, wait).await.is_none() {
                return;
            }
        }
    }

    /// One POST of `body` to `url` through the sender, and what its answer means.
    ///
    /// The sender makes the request and classifies it; this decides what the
    /// classification costs the retry policy. `events` is how many Tier changes
    /// the batch holds, and `attempt` is which try this is, both of which the
    /// endpoint's log line carries. `label` is the host the delivery went to,
    /// which the stop arm needs because a stopped delivery answers with nothing.
    async fn attempt(
        &self,
        url: &str,
        body: &Value,
        label: &str,
        events: usize,
        attempt: usize,
        stop: &CancellationToken,
    ) -> Attempt {
        let answer = until_stopped(stop, self.sender.post(url, body)).await;
        let Some(answer) = answer else {
            tracing::info!(
                name: "webhook.task.stopped",
                endpoint = %label,
                "the Notification task stopped with a delivery in flight"
            );
            return Attempt::Stopped;
        };
        match answer {
            Delivery::Sent { endpoint } => {
                tracing::info!(
                    name: "webhook.delivery.sent",
                    notifications = events,
                    endpoint = %endpoint,
                    "sent a batch of Tier changes"
                );
                Attempt::Finished
            }
            Delivery::Unreachable { endpoint, error } => {
                tracing::warn!(
                    name: "webhook.delivery.unreachable",
                    endpoint = %endpoint,
                    attempt = attempt + 1,
                    attempts = ATTEMPTS,
                    error = %error,
                    "Webhook endpoint unreachable"
                );
                Attempt::Retry { after: None }
            }
            Delivery::Refused {
                endpoint,
                status,
                headers,
            } => {
                if is_permanent(status) {
                    tracing::warn!(
                        name: "webhook.delivery.rejected",
                        endpoint = %endpoint,
                        status,
                        "Webhook endpoint rejected the batch"
                    );
                    return Attempt::Finished;
                }
                tracing::warn!(
                    name: "webhook.delivery.failed",
                    endpoint = %endpoint,
                    status,
                    attempt = attempt + 1,
                    attempts = ATTEMPTS,
                    "Webhook delivery failed"
                );
                Attempt::Retry {
                    after: retry_after_seconds(&headers),
                }
            }
        }
    }
}

/// What one Notification attempt did, and whether another is worth making.
#[derive(Debug, Clone, Copy)]
enum Attempt {
    /// The leader stopped while the request was in flight, so it ends here.
    Stopped,
    /// The endpoint took the batch, or refused it for good.
    Finished,
    /// Another attempt may succeed, after the `retry-after` the endpoint named
    /// or the policy's own delay when it named none.
    Retry { after: Option<f64> },
}

/// Whether `status` is the same on the next attempt, so a retry cannot help.
///
/// 408 is a request the endpoint timed out on and 429 is its rate limit, and both
/// describe a moment rather than the message; every server error may be a moment
/// too. Any other refusal (404 for a deleted hook, 400 for a body the endpoint
/// dislikes) is the same on the next attempt, so the queue does not spend
/// retries on it.
fn is_permanent(status: u16) -> bool {
    !RETRYABLE_STATUSES.contains(&status) && status < 500
}

/// The `retry-after` a rate limit asked for, capped; `None` when absent or unusable.
///
/// A numeric value replaces the wait; an HTTP-date is rare enough to ignore.
fn retry_after_seconds(headers: &reqwest::header::HeaderMap) -> Option<f64> {
    let raw = headers.get("retry-after")?.to_str().ok()?;
    let seconds: f64 = raw.trim().parse().ok()?;
    Some(seconds.clamp(0.0, MAX_RETRY_AFTER_SECONDS))
}

/// The leader-only Notification task: posts a Report's Tier changes to every endpoint.
pub struct WebhookWorker {
    shared: Arc<WebhookShared>,
    run: LeaderRun,
}

impl std::fmt::Debug for WebhookWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookWorker")
            .field("shared", &self.shared)
            .field("running", &self.run.is_running())
            .finish_non_exhaustive()
    }
}

impl WebhookWorker {
    /// A task that posts to `urls` in `format`.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built, which is the TLS
    /// backend failing to initialize; a client without the timeout would break
    /// this module's error policy, so none is used.
    pub fn new(urls: Vec<String>, format: WebhookFormat, public_url: &str) -> anyhow::Result<Self> {
        Self::with_retry_delays(urls, format, public_url, RETRY_DELAYS)
    }

    /// The same task, waiting `retry_delays` before its second and third attempt.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built; see
    /// [`WebhookWorker::new`].
    pub fn with_retry_delays(
        urls: Vec<String>,
        format: WebhookFormat,
        public_url: &str,
        retry_delays: [f64; 2],
    ) -> anyhow::Result<Self> {
        Self::with_client_build(
            urls,
            format,
            public_url,
            retry_delays,
            webhook_sender::client,
        )
    }

    /// The same task, with `build` making the HTTP client instead.
    ///
    /// The seam is for the one failure a test cannot stage: only a TLS backend
    /// that cannot initialize fails a build of a healthy client, so a test asks
    /// `build` for the error and asserts the constructor carries it rather than
    /// answering a task without the timeout.
    ///
    /// # Errors
    ///
    /// Returns `build`'s error, contexted with what could not be built.
    pub fn with_client_build(
        urls: Vec<String>,
        format: WebhookFormat,
        public_url: &str,
        retry_delays: [f64; 2],
        build: impl FnOnce() -> anyhow::Result<reqwest::Client>,
    ) -> anyhow::Result<Self> {
        let client = build().context("building the Webhook HTTP client")?;
        Ok(Self {
            shared: Arc::new(WebhookShared {
                urls,
                format,
                public_url: public_url.to_owned(),
                retry_delays,
                sender: WebhookSender::from_client(client),
                queue: BatchQueue::new(QUEUE_MAX),
            }),
            run: LeaderRun::new(),
        })
    }

    /// The queue the task drains, and its own record of what it did with it.
    #[must_use]
    pub fn queue(&self) -> &BatchQueue<Vec<TierChange>> {
        &self.shared.queue
    }
}

impl LeaderTask for WebhookWorker {
    fn name(&self) -> &'static str {
        "Notification"
    }

    fn start(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let shared = Arc::clone(&self.shared);
            self.run
                .start(move |cancel| async move { shared.run(&cancel).await })
                .await;
        })
    }

    fn stop(&self) -> BoxFuture<'_, anyhow::Result<()>> {
        Box::pin(async move { self.run.stop().await })
    }

    fn enqueue(&self, outcome: &ProcessOutcome) -> anyhow::Result<()> {
        if outcome.tier_changes.is_empty() {
            return Ok(());
        }
        self.shared.push(outcome.tier_changes.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{WebhookShared, WebhookWorker, retry_after_seconds};
    use flakeradar_core::{
        model::EntityKind, model::Tier, notify::TierChange, notify::WebhookFormat, types::Score,
    };
    use reqwest::header::{HeaderMap, HeaderValue};
    use tokio_util::sync::CancellationToken;

    /// A header map carrying one `retry-after` spelling.
    fn retry_after(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "retry-after",
            HeaderValue::from_str(value).expect("the test value is a header"),
        );
        headers
    }

    /// One Test crossing the Flake threshold.
    fn change(id: i64) -> TierChange {
        TierChange {
            kind: EntityKind::Test,
            id,
            repo: "acme/app".to_owned(),
            project: Some("backend".to_owned()),
            name: "test_total".to_owned(),
            flakiness_score: Score::new(0.42).expect("a score"),
            tier: Tier::Flaky,
            proven_flake_count: 2,
            classname: Some("tests.test_cart".to_owned()),
            pipeline: None,
            failure_category: None,
            broken: false,
        }
    }

    #[test]
    fn a_rate_limit_waits_as_long_as_the_retry_after_says() {
        assert_eq!(retry_after_seconds(&retry_after("1")), Some(1.0));
        assert_eq!(retry_after_seconds(&retry_after("2.5")), Some(2.5));
        assert_eq!(
            retry_after_seconds(&retry_after("15")),
            Some(10.0),
            "the wait is capped at ten seconds"
        );
        assert_eq!(
            retry_after_seconds(&retry_after("-1")),
            Some(0.0),
            "a negative wait is no wait"
        );
        assert_eq!(
            retry_after_seconds(&retry_after("Wed, 21 Oct 2015 07:28:00 GMT")),
            None,
            "an HTTP date is ignored and the default wait used"
        );
        assert_eq!(retry_after_seconds(&retry_after("soon")), None);
        assert_eq!(retry_after_seconds(&HeaderMap::new()), None);
    }

    #[tokio::test]
    async fn a_delivery_that_raises_is_logged_and_the_task_keeps_serving() {
        use std::io::Write as _;

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

        let worker = Arc::new(
            WebhookWorker::new(
                vec!["http://127.0.0.1:1/hook".to_owned()],
                WebhookFormat::Json,
                "",
            )
            .expect("the HTTP client builds"),
        );
        let shared = Arc::clone(&worker.shared);
        let cancel = CancellationToken::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        let delivered = Arc::new(Mutex::new(Vec::<i64>::new()));

        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || Sink(Arc::clone(&sink)))
            .with_max_level(tracing::Level::ERROR)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);

        let handle = {
            let shared = Arc::clone(&shared);
            let cancel = cancel.clone();
            let attempts = Arc::clone(&attempts);
            let delivered = Arc::clone(&delivered);
            tokio::spawn(async move {
                WebhookShared::run_with(&shared, &cancel, move |_shared, _url, batch| {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    let delivered = Arc::clone(&delivered);
                    async move {
                        assert!(attempt > 0, "the delivery raised");
                        delivered
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(batch[0].id);
                    }
                })
                .await;
            })
        };

        shared.push(vec![change(1)]);
        for _ in 0..1000 {
            if attempts.load(Ordering::SeqCst) >= 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        shared.push(vec![change(2)]);
        for _ in 0..1000 {
            if !delivered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        let delivered = delivered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        cancel.cancel();
        handle.await.expect("the loop ends when it is cancelled");
        drop(guard);

        let logged = String::from_utf8_lossy(
            &lines
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_owned();
        assert!(
            logged.contains("Webhook delivery failed"),
            "a delivery that raised is logged: {logged}"
        );
        assert!(
            !logged.contains("127.0.0.1"),
            "the log names no endpoint: {logged}"
        );
        assert_eq!(
            delivered,
            vec![2],
            "the task delivered the next batch after the failure"
        );
        let _ = std::io::stdout().flush();
    }
}
