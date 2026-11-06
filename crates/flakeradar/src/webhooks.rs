//! The leader-only Notification task: posts a Report's Tier changes to every Webhook URL.
//!
//! Delivery is best effort (ADR 0009): the queue holds a bounded number of
//! batches and drops the oldest when full, the task stores nothing, and a batch
//! queued when the process dies is lost. Three attempts per endpoint with a
//! short wait between them; a client error is permanent and is not retried.
//!
//! This task never logs a Webhook URL: the URL is the credential, so a log line
//! names an endpoint's host only, and a `reqwest` error has its URL removed
//! before it is logged.
//!
//! A delivery and the wait before a retry end the moment leadership does, so a
//! leader that drops never waits out an endpoint's timeout.

// Rust guideline compliant 2026-07-21

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context as _;
use flakeradar_core::{TierChange, WebhookFormat, render};
use flakeradar_db::ProcessOutcome;
use tokio_util::sync::CancellationToken;

use crate::worker::{LeaderRun, LeaderTask, StartFuture, StopFuture, until_stopped};

/// The delays before the second and third Notification attempts, in seconds.
pub const RETRY_DELAYS: [f64; 2] = [0.5, 1.0];

/// How many attempts one endpoint gets for one batch.
pub const ATTEMPTS: usize = RETRY_DELAYS.len() + 1;

/// The ceiling for a rate limit's `retry-after`, so one endpoint cannot park the
/// queue for minutes.
pub const MAX_RETRY_AFTER_SECONDS: f64 = 10.0;

/// How long one Notification attempt may take.
pub const WEBHOOK_TIMEOUT_SECONDS: u64 = 5;

/// How many batches the Notification queue holds before it drops the oldest.
pub const QUEUE_MAX: usize = 1000;

/// The statuses a Notification retries; every other client error is permanent.
pub const RETRYABLE_STATUSES: [u16; 2] = [408, 429];

/// A mutex whose poisoning is recovered from, because a task that panicked
/// elsewhere must not stop the leader from working.
fn guard<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Everything the Notification task shares with its loop.
struct WebhookShared {
    urls: Vec<String>,
    format: WebhookFormat,
    public_url: String,
    retry_delays: [f64; 2],
    client: reqwest::Client,
    queue: Mutex<VecDeque<Vec<TierChange>>>,
    dropped: Mutex<usize>,
    notify: tokio::sync::Notify,
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
        let mut queue = guard(&self.queue);
        if queue.len() >= QUEUE_MAX {
            queue.pop_front();
            let dropped = {
                let mut dropped = guard(&self.dropped);
                *dropped += 1;
                *dropped
            };
            tracing::warn!(dropped, "Webhook queue full; dropped the oldest batch");
        }
        queue.push_back(batch);
        drop(queue);
        self.notify.notify_one();
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
            let notified = self.notify.notified();
            let batch = guard(&self.queue).pop_front();
            let Some(batch) = batch else {
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
                    tracing::error!(events, error = %error, "Webhook delivery failed");
                }
            }
        }
    }

    /// The host an endpoint belongs to. Never log the URL itself: it is a credential.
    fn endpoint(url: &str) -> String {
        reqwest::Url::parse(url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_owned))
            .unwrap_or_else(|| "endpoint".to_owned())
    }

    /// Posts one batch to one endpoint, with the retry policy of ADR 0009.
    ///
    /// The POST and the wait before a retry both end when `stop` fires: an
    /// endpoint that never answers costs the leader no more than the cancellation.
    async fn post(self: &Arc<Self>, url: &str, batch: &[TierChange], stop: &CancellationToken) {
        let body = render(batch, self.format, &self.public_url);
        let label = Self::endpoint(url);
        for attempt in 0..ATTEMPTS {
            let mut delay = self.retry_delays.get(attempt).copied().unwrap_or_default();
            let answer = until_stopped(stop, self.client.post(url).json(&body).send()).await;
            let Some(answer) = answer else {
                tracing::info!(
                    endpoint = %label,
                    "the Notification task stopped with a delivery in flight"
                );
                return;
            };
            let answer = answer.map_err(|error| error.without_url().to_string());
            match answer {
                Err(error) => {
                    tracing::warn!(
                        endpoint = %label,
                        attempt = attempt + 1,
                        attempts = ATTEMPTS,
                        error = %error,
                        "Webhook endpoint unreachable"
                    );
                }
                Ok(response) => {
                    let status = response.status().as_u16();
                    if (200..300).contains(&status) {
                        tracing::info!(
                            notifications = batch.len(),
                            endpoint = %label,
                            "sent a batch of Tier changes"
                        );
                        return;
                    }
                    if !RETRYABLE_STATUSES.contains(&status) && status < 500 {
                        tracing::warn!(
                            endpoint = %label,
                            status,
                            "Webhook endpoint rejected the batch"
                        );
                        return;
                    }
                    tracing::warn!(
                        endpoint = %label,
                        status,
                        attempt = attempt + 1,
                        attempts = ATTEMPTS,
                        "Webhook delivery failed"
                    );
                    if let Some(after) = retry_after_seconds(response.headers()) {
                        delay = after;
                    }
                }
            }
            if attempt + 1 < ATTEMPTS {
                let wait = tokio::time::sleep(
                    Duration::try_from_secs_f64(delay.max(0.0)).unwrap_or_default(),
                );
                if until_stopped(stop, wait).await.is_none() {
                    return;
                }
            }
        }
    }
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

/// The HTTP client every Notification goes through.
///
/// The timeout is what keeps one slow endpoint from parking the queue, and the
/// user agent is what every endpoint expects to be told.
fn webhook_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(WEBHOOK_TIMEOUT_SECONDS))
        .user_agent(format!("flakeradar/{}", env!("CARGO_PKG_VERSION")))
        .build()?)
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
        Self::with_client_build(urls, format, public_url, retry_delays, webhook_client)
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
                client,
                queue: Mutex::new(VecDeque::new()),
                dropped: Mutex::new(0),
                notify: tokio::sync::Notify::new(),
            }),
            run: LeaderRun::new(),
        })
    }

    /// The home of the batches queued and dropped so far, for a test.
    #[must_use]
    pub fn queued(&self) -> usize {
        guard(&self.shared.queue).len()
    }

    /// How many batches the queue has dropped, for a test.
    #[must_use]
    pub fn dropped(&self) -> usize {
        *guard(&self.shared.dropped)
    }
}

impl LeaderTask for WebhookWorker {
    fn name(&self) -> &'static str {
        "Notification"
    }

    fn start(&self) -> StartFuture<'_> {
        Box::pin(async move {
            let shared = Arc::clone(&self.shared);
            self.run
                .start(move |cancel| async move { shared.run(&cancel).await })
                .await;
        })
    }

    fn stop(&self) -> StopFuture<'_> {
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
    use flakeradar_core::{Kind, Tier, TierChange, WebhookFormat};
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
            kind: Kind::Test,
            id,
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
