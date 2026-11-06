//! The leader-only GitHub task: files issues and re-arms closed links.
//!
//! Every GitHub call runs here, never in the processor loop, so a slow or hanging
//! GitHub API cannot delay Report processing. The processor hands outcomes over
//! with a non-blocking [`GitHubWorker::enqueue`]; the task drains the queue and
//! runs the issue-state sync once per interval, the first time at start-up.
//!
//! Queue contents live in this process only: if it dies, outcomes that were
//! queued but not filed are lost, which is accepted because the next Report that
//! touches the Test or Job files it. The queue is bounded, so a GitHub outage
//! cannot grow it without limit: past the bound the oldest outcome is dropped,
//! logged, and the stalest news loses, as the Notification queue does.
//!
//! The task stops when leadership does. A sync's `GET` and the fixed note's
//! comment end at once, because neither can be interrupted without costing more
//! than a retry; an issue-creating `POST` runs on through storing its link and
//! the batch stops after it, because cancelling it mid-flight would strand an
//! issue and file a duplicate on the next Report.

// Rust guideline compliant 2026-07-21

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use flakeradar_db::{ProcessOutcome, SharedClock, SharedMonotonic, SystemClock, SystemMonotonic};
use sea_orm::DatabaseConnection;
use tokio_util::sync::CancellationToken;

use crate::github::Api;
use crate::queue::BatchQueue;
use crate::settings::Settings;
use crate::worker::{LeaderRun, LeaderTask, StartFuture, StopFuture};

/// How many outcomes the queue holds before it drops the oldest.
///
/// The same bound as the Notification queue: a GitHub outage is measured in
/// Reports, and a queue this deep already outlasts the outage that filled it.
pub const QUEUE_MAX: usize = 1000;

/// A mutex whose poisoning is recovered from, because a task that panicked
/// elsewhere must not stop the leader from working.
fn guard<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Everything the GitHub task shares with its loop.
struct GitHubShared {
    pool: DatabaseConnection,
    settings: Arc<Settings>,
    api: Api,
    sync_interval: Duration,
    queue: BatchQueue<ProcessOutcome>,
    last_sync: Mutex<Option<f64>>,
    monotonic: SharedMonotonic,
    clock: SharedClock,
}

impl std::fmt::Debug for GitHubShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubShared")
            .field("sync_interval", &self.sync_interval)
            .field("api", &self.api)
            .finish_non_exhaustive()
    }
}

impl GitHubShared {
    /// Seconds until the next sync, or `None` when one is due right now.
    fn seconds_until_sync(&self) -> Option<f64> {
        let last = *guard(&self.last_sync);
        let elapsed = self.monotonic.now() - last?;
        let interval = self.sync_interval.as_secs_f64();
        (elapsed < interval).then_some(interval - elapsed)
    }

    /// Queues one outcome, dropping the oldest when the queue is full.
    fn push(&self, outcome: ProcessOutcome) {
        if self.queue.push(outcome).is_some() {
            tracing::warn!(
                dropped = self.queue.dropped(),
                "GitHub queue full; dropped the oldest outcome"
            );
        }
    }

    /// Files issues for one outcome's touched Tests and Jobs.
    async fn file(&self, outcome: ProcessOutcome, stop: &CancellationToken) {
        crate::github::on_report_processed(
            &self.pool,
            &outcome,
            &self.settings,
            &self.api,
            &self.clock,
            stop,
        )
        .await;
    }

    /// Re-arms the links whose issue GitHub reports closed.
    async fn sync(&self, stop: &CancellationToken) {
        *guard(&self.last_sync) = Some(self.monotonic.now());
        crate::github::sync_closed_issues(&self.pool, &self.settings, &self.api, &self.clock, stop)
            .await;
    }

    /// Takes the next queued outcome, waiting at most `seconds`.
    async fn next_outcome(
        &self,
        seconds: f64,
        cancel: &CancellationToken,
    ) -> Option<ProcessOutcome> {
        let delay = Duration::try_from_secs_f64(seconds.max(0.0)).unwrap_or_default();
        loop {
            // The notification future is created before the queue is read, so an
            // enqueue between the two cannot be missed.
            let notified = self.queue.notified();
            if let Some(outcome) = self.queue.pop() {
                return Some(outcome);
            }
            tokio::select! {
                () = cancel.cancelled() => return None,
                () = tokio::time::sleep(delay) => return None,
                () = notified => {}
            }
        }
    }

    /// The task's loop: sync when due, otherwise file one queued outcome.
    async fn run(&self, cancel: &CancellationToken) {
        while !cancel.is_cancelled() {
            match self.seconds_until_sync() {
                None => self.sync(cancel).await,
                Some(seconds) => {
                    if let Some(outcome) = self.next_outcome(seconds, cancel).await {
                        self.file(outcome, cancel).await;
                        self.queue.finish();
                    }
                }
            }
        }
    }
}

/// The leader-only GitHub task: files issues and re-arms closed links.
///
/// Every GitHub call runs here. [`GitHubWorker::enqueue`] never waits, and the
/// task loops on the queue, running the issue-state sync once per interval, the
/// first time at start-up.
pub struct GitHubWorker {
    shared: Arc<GitHubShared>,
    run: LeaderRun,
}

impl std::fmt::Debug for GitHubWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubWorker")
            .field("shared", &self.shared)
            .field("running", &self.run.is_running())
            .finish_non_exhaustive()
    }
}

impl GitHubWorker {
    /// A task over `pool`, filing into the Repo `api` reaches.
    #[must_use]
    pub fn new(
        pool: DatabaseConnection,
        settings: Arc<Settings>,
        api: Api,
        sync_interval: Duration,
    ) -> Self {
        Self {
            shared: Arc::new(GitHubShared {
                pool,
                settings,
                api,
                sync_interval,
                queue: BatchQueue::new(QUEUE_MAX),
                last_sync: Mutex::new(None),
                monotonic: Arc::new(SystemMonotonic::new()),
                clock: Arc::new(SystemClock),
            }),
            run: LeaderRun::new(),
        }
    }

    /// Reads the sync interval from `monotonic` instead of the real clock.
    #[must_use]
    pub fn with_monotonic(mut self, monotonic: SharedMonotonic) -> Self {
        if let Some(shared) = Arc::get_mut(&mut self.shared) {
            shared.monotonic = monotonic;
        }
        self
    }

    /// Reads the instant it stamps a checked link with from `clock`.
    #[must_use]
    pub fn with_clock(mut self, clock: SharedClock) -> Self {
        if let Some(shared) = Arc::get_mut(&mut self.shared) {
            shared.clock = clock;
        }
        self
    }

    /// The queue the task drains, and its own record of what it did with it.
    #[must_use]
    pub fn queue(&self) -> &BatchQueue<ProcessOutcome> {
        &self.shared.queue
    }
}

impl LeaderTask for GitHubWorker {
    fn name(&self) -> &'static str {
        "GitHub"
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
        self.shared.push(outcome.clone());
        Ok(())
    }
}
