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

// Rust guideline compliant 2026-07-21

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use flakeradar_core::{TierChange, WebhookFormat, render};
use flakeradar_db::ProcessOutcome;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::worker::{LeaderTask, StartFuture, StopFuture};

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
        while !cancel.is_cancelled() {
            let notified = self.notify.notified();
            let batch = guard(&self.queue).pop_front();
            let Some(batch) = batch else {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = notified => continue,
                }
            };
            let mut set = tokio::task::JoinSet::new();
            for url in &self.urls {
                let shared = Arc::clone(self);
                let url = url.clone();
                let batch = batch.clone();
                set.spawn(async move { shared.post(&url, &batch).await });
            }
            while set.join_next().await.is_some() {}
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
    async fn post(self: &Arc<Self>, url: &str, batch: &[TierChange]) {
        let body = render(batch, self.format, &self.public_url);
        let label = Self::endpoint(url);
        for attempt in 0..ATTEMPTS {
            let mut delay = RETRY_DELAYS.get(attempt).copied().unwrap_or_default();
            let answer = self
                .client
                .post(url)
                .json(&body)
                .send()
                .await
                .map_err(|error| error.without_url().to_string());
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
                tokio::time::sleep(Duration::try_from_secs_f64(delay.max(0.0)).unwrap_or_default())
                    .await;
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
    cancel: CancellationToken,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for WebhookWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookWorker")
            .field("shared", &self.shared)
            .finish_non_exhaustive()
    }
}

impl WebhookWorker {
    /// A task that posts to `urls` in `format`.
    #[must_use]
    pub fn new(urls: Vec<String>, format: WebhookFormat, public_url: &str) -> Self {
        Self {
            shared: Arc::new(WebhookShared {
                urls,
                format,
                public_url: public_url.to_owned(),
                client: reqwest::Client::builder()
                    .timeout(Duration::from_secs(WEBHOOK_TIMEOUT_SECONDS))
                    .user_agent(format!("flakeradar/{}", env!("CARGO_PKG_VERSION")))
                    .build()
                    .unwrap_or_default(),
                queue: Mutex::new(VecDeque::new()),
                dropped: Mutex::new(0),
                notify: tokio::sync::Notify::new(),
            }),
            cancel: CancellationToken::new(),
            task: tokio::sync::Mutex::new(None),
        }
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
            let mut task = self.task.lock().await;
            if task.as_ref().is_some_and(|handle| !handle.is_finished()) {
                return;
            }
            let shared = Arc::clone(&self.shared);
            let cancel = self.cancel.clone();
            *task = Some(tokio::spawn(async move { shared.run(&cancel).await }));
        })
    }

    fn stop(&self) -> StopFuture<'_> {
        Box::pin(async move {
            self.cancel.cancel();
            let handle = self.task.lock().await.take();
            if let Some(handle) = handle {
                handle.await?;
            }
            Ok(())
        })
    }

    fn enqueue(&self, outcome: &ProcessOutcome) -> anyhow::Result<()> {
        if outcome.tier_changes.is_empty() {
            return Ok(());
        }
        self.shared.push(outcome.tier_changes.clone());
        Ok(())
    }
}
