//! The leader-only weekly digest task: one weekly message per Webhook URL.
//!
//! A Notification says that one Test crossed a threshold; a digest says what a
//! week held, so it is sent on a schedule rather than on a Report. The task
//! checks the schedule when it starts and then every [`DIGEST_CHECK_SECONDS`],
//! and a check that finds this week's slot due claims it — the single-row ledger
//! in `digest_state` — before it builds and posts the body, so two replicas and a
//! restart through the hour send the week once.
//!
//! Delivery is best effort, like a Notification's (ADR 0009): a failed `POST` is
//! logged and not retried, and a slot claimed by a process that died before its
//! `POST` is a lost digest, which the ledger accepts over a duplicate.
//!
//! This task never logs a Webhook URL: the URL is the credential, so a log line
//! names an endpoint's host only, and a `reqwest` error has its URL removed
//! before it is logged.

// Rust guideline compliant 2026-07-21

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use flakeradar_core::{digest::DigestSchedule, digest::render_digest, notify::WebhookFormat};
use flakeradar_db::{
    digest::{claim_digest_slot, weekly_digest},
    processing::ProcessOutcome,
    reads::ReadSettings,
};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::BoxFuture;
use crate::webhooks::WEBHOOK_TIMEOUT_SECONDS;
use crate::worker::{LeaderRun, LeaderTask, until_stopped};

/// How long the digest task waits between two checks of the schedule, in seconds.
///
/// Five minutes is far finer than the weekly slot it watches, so a leader that
/// took over just after the configured hour still sends within minutes of it,
/// while a check stays two cheap statements — a schedule read and a ledger claim
/// — so waking often costs nothing. Lengthening it delays the digest past the
/// hour an operator asked for; shortening it only adds idle wakeings. A test
/// overrides the interval with [`DigestWorker::with_check_interval`].
pub const DIGEST_CHECK_SECONDS: u64 = 300;

/// The host an endpoint belongs to. Never log the URL itself: it is a credential.
fn endpoint(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
        .unwrap_or_else(|| "endpoint".to_owned())
}

fn log_sent(label: &str) {
    tracing::info!(
        name: "digest.sent",
        endpoint = %label,
        "sent the weekly digest"
    );
}

/// The HTTP client every weekly digest goes through.
///
/// The same client a Notification uses: the same timeout bounds a black-holed
/// endpoint, and the same user agent is what every endpoint expects to be told.
fn digest_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(WEBHOOK_TIMEOUT_SECONDS))
        .user_agent(format!("flakeradar/{}", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// Everything the digest task shares with its loop.
struct DigestShared {
    pool: DatabaseConnection,
    schedule: DigestSchedule,
    format: WebhookFormat,
    urls: Vec<String>,
    client: reqwest::Client,
    reads: ReadSettings,
    check_interval: Duration,
    checks: AtomicUsize,
}

impl std::fmt::Debug for DigestShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DigestShared")
            .field("endpoints", &self.urls.len())
            .field("format", &self.format)
            .field("check_interval", &self.check_interval)
            .finish_non_exhaustive()
    }
}

impl DigestShared {
    /// The task's loop: check the schedule now, and then every interval until
    /// leadership ends. Leadership ends the wait, so a stop does not sit out an
    /// interval it will never use.
    async fn run(&self, cancel: &CancellationToken) {
        loop {
            self.check().await;
            // Every check is counted, sent or not: it is the signal a test waits
            // on before it asserts that nothing was sent.
            self.checks.fetch_add(1, Ordering::SeqCst);
            if until_stopped(cancel, tokio::time::sleep(self.check_interval))
                .await
                .is_none()
            {
                return;
            }
        }
    }

    /// One look at the schedule: sends this week's digest when its slot has come
    /// and this caller won the claim on it.
    async fn check(&self) {
        // The same clock the digest reads are cut on, so one injected instant
        // decides both when the week is owed and what is in it.
        let now: DateTime<Utc> = self.reads.clock.now().with_timezone(&Utc);
        let slot = self.schedule.slot(now);
        if now < slot {
            return;
        }
        // The claim comes first, before anything is built or sent, so a second
        // replica or a restart through the hour settles against the ledger rather
        // than against the endpoint.
        match claim_digest_slot(&self.pool, slot).await {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                tracing::warn!(
                    name: "digest.claim.failed",
                    slot = %slot.to_rfc3339(),
                    error = %error,
                    "could not claim the weekly digest slot"
                );
                return;
            }
        }
        let digests = match weekly_digest(&self.pool, now, &self.reads).await {
            Ok(digests) => digests,
            Err(error) => {
                tracing::warn!(
                    name: "digest.build.failed",
                    slot = %slot.to_rfc3339(),
                    error = %error,
                    "could not build the weekly digest"
                );
                return;
            }
        };
        if digests.is_empty() {
            return;
        }
        let body = render_digest(&digests, self.format, now.date_naive());
        for url in &self.urls {
            self.post(url, &body).await;
        }
    }

    /// Posts one digest to one endpoint. A refusal is logged and not retried: the
    /// slot is already claimed, so a retry would be a second `POST` for the week.
    async fn post(&self, url: &str, body: &Value) {
        let label = endpoint(url);
        match self.client.post(url).json(body).send().await {
            Ok(response) if response.status().is_success() => {
                log_sent(&label);
            }
            Ok(response) => {
                tracing::warn!(
                    name: "digest.delivery.failed",
                    endpoint = %label,
                    status = response.status().as_u16(),
                    "Webhook endpoint rejected the weekly digest"
                );
            }
            Err(error) => {
                tracing::warn!(
                    name: "digest.delivery.failed",
                    endpoint = %label,
                    error = %error.without_url().to_string(),
                    "Webhook endpoint unreachable for the weekly digest"
                );
            }
        }
    }
}

/// The leader-only weekly digest task: posts the week's digest to every endpoint.
///
/// [`DigestWorker`] answers at the weekday and hour its [`DigestSchedule`] names,
/// once per week across every replica, and its [`LeaderTask::enqueue`] does
/// nothing because a digest is never triggered by a Report.
pub struct DigestWorker {
    shared: Arc<DigestShared>,
    run: LeaderRun,
}

impl std::fmt::Debug for DigestWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DigestWorker")
            .field("shared", &self.shared)
            .field("running", &self.run.is_running())
            .finish_non_exhaustive()
    }
}

impl DigestWorker {
    /// A task that sends the digest `schedule` names, in `format`, to `urls`.
    ///
    /// `reads` is the read layer's settings, whose clock the task also asks what
    /// time it is: one injected instant drives both the schedule and the history
    /// cut-offs the digest lists use.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built, which is the TLS
    /// backend failing to initialize; a client without the timeout would let one
    /// black-holed endpoint hold the task, so none is used.
    pub fn new(
        pool: DatabaseConnection,
        schedule: DigestSchedule,
        format: WebhookFormat,
        urls: Vec<String>,
        reads: ReadSettings,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            shared: Arc::new(DigestShared {
                pool,
                schedule,
                format,
                urls,
                client: digest_client().context("building the digest HTTP client")?,
                reads,
                check_interval: Duration::from_secs(DIGEST_CHECK_SECONDS),
                checks: AtomicUsize::new(0),
            }),
            run: LeaderRun::new(),
        })
    }

    /// The same task, checking the schedule every `interval` instead of every
    /// [`DIGEST_CHECK_SECONDS`].
    ///
    /// The seam is what lets a test watch more than one check without waiting
    /// minutes for the shipped interval.
    #[must_use]
    pub fn with_check_interval(mut self, interval: Duration) -> Self {
        if let Some(shared) = Arc::get_mut(&mut self.shared) {
            shared.check_interval = interval;
        }
        self
    }

    /// How many checks the task has finished, sent or not.
    #[must_use]
    pub fn checks(&self) -> usize {
        self.shared.checks.load(Ordering::SeqCst)
    }
}

impl LeaderTask for DigestWorker {
    fn name(&self) -> &'static str {
        "Digest"
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

    /// Does nothing: a weekly digest is on a schedule, never on a Report.
    fn enqueue(&self, _outcome: &ProcessOutcome) -> anyhow::Result<()> {
        Ok(())
    }
}
