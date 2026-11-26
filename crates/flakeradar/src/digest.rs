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
//! before it is logged. The mechanics that keep that promise — the client, the
//! host-only label, the redaction and the classification of one `POST` — belong
//! to [`crate::webhook_sender`]; what stays here is the digest's policy: one
//! `POST` per endpoint, with no retry.

// Rust guideline compliant 2026-07-21

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use flakeradar_core::{digest::DigestSchedule, digest::render_digest, notify::WebhookFormat};
use flakeradar_db::{
    DbError,
    digest::{claim_digest_slot, weekly_digest},
    processing::ProcessOutcome,
    reads::ReadSettings,
};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::BoxFuture;
use crate::webhook_sender::{Delivery, WebhookSender};
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

fn log_sent(label: &str) {
    tracing::info!(name: "digest.sent", endpoint = %label, "sent the weekly digest");
}

/// A refused claim: the slot and the error, and never a URL.
fn log_claim_failed(slot: &DateTime<Utc>, error: &DbError) {
    tracing::warn!(
        name: "digest.claim.failed",
        slot = %slot.to_rfc3339(), error = %error, "could not claim the weekly digest slot"
    );
}

/// A slot that was claimed but whose digest could not be built: the ledger keeps
/// the loss, so the week is never sent twice.
fn log_build_failed(slot: &DateTime<Utc>, error: &DbError) {
    tracing::warn!(
        name: "digest.build.failed",
        slot = %slot.to_rfc3339(), error = %error, "could not build the weekly digest"
    );
}

/// A `POST` the endpoint refused: host and status, no retry (the claim stands).
fn log_rejected(label: &str, status: u16) {
    tracing::warn!(
        name: "digest.delivery.failed",
        endpoint = %label, status, "Webhook endpoint rejected the weekly digest"
    );
}

/// A `POST` that never got an answer, by host and a URL-stripped error.
fn log_unreachable(label: &str, error: &str) {
    tracing::warn!(
        name: "digest.delivery.failed",
        endpoint = %label, error, "Webhook endpoint unreachable for the weekly digest"
    );
}

/// Everything the digest task shares with its loop.
struct DigestShared {
    pool: DatabaseConnection,
    schedule: DigestSchedule,
    format: WebhookFormat,
    urls: Vec<String>,
    sender: WebhookSender,
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
                log_claim_failed(&slot, &error);
                return;
            }
        }
        let digests = match weekly_digest(&self.pool, now, &self.reads).await {
            Ok(digests) => digests,
            Err(error) => {
                log_build_failed(&slot, &error);
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
        match self.sender.post(url, body).await {
            Delivery::Sent { endpoint } => log_sent(&endpoint),
            // No retry: the slot is claimed, so a second `POST` would be a second
            // digest. The refusal's headers are the Notification's business, not
            // the digest's.
            Delivery::Refused {
                endpoint, status, ..
            } => log_rejected(&endpoint, status),
            Delivery::Unreachable { endpoint, error } => log_unreachable(&endpoint, &error),
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
                sender: WebhookSender::new().context("building the digest HTTP client")?,
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
