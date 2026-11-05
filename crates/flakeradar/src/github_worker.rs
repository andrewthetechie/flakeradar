//! The leader-only GitHub task: files issues and re-arms closed links.
//!
//! Every GitHub call runs here, never in the processor loop, so a slow or hanging
//! GitHub API cannot delay Report processing. The processor hands outcomes over
//! with a non-blocking [`GitHubWorker::enqueue`]; the task drains the queue and
//! runs the issue-state sync once per interval, the first time at start-up.
//!
//! Queue contents live in this process only: if it dies, outcomes that were
//! queued but not filed are lost, which is accepted because the next Report that
//! touches the Test or Job files it.

// Rust guideline compliant 2026-07-21

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use flakeradar_db::{ProcessOutcome, SharedMonotonic, SystemMonotonic};
use sea_orm::DatabaseConnection;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::github::Api;
use crate::settings::Settings;
use crate::worker::{LeaderTask, StartFuture, StopFuture};

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
    queue: Mutex<VecDeque<ProcessOutcome>>,
    notify: tokio::sync::Notify,
    last_sync: Mutex<Option<f64>>,
    monotonic: SharedMonotonic,
}

impl std::fmt::Debug for GitHubShared {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GitHubShared")
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

    /// Files issues for one outcome's touched Tests and Jobs.
    async fn file(&self, outcome: ProcessOutcome) {
        crate::github::on_report_processed(&self.pool, &outcome, &self.settings, &self.api).await;
    }

    /// Re-arms the links whose issue GitHub reports closed.
    async fn sync(&self) {
        *guard(&self.last_sync) = Some(self.monotonic.now());
        crate::github::sync_closed_issues(&self.pool, &self.settings, &self.api).await;
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
            let notified = self.notify.notified();
            if let Some(outcome) = guard(&self.queue).pop_front() {
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
                None => self.sync().await,
                Some(seconds) => {
                    if let Some(outcome) = self.next_outcome(seconds, cancel).await {
                        self.file(outcome).await;
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
    cancel: CancellationToken,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for GitHubWorker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GitHubWorker")
            .field("shared", &self.shared)
            .field(
                "running",
                &self.task.try_lock().is_ok_and(|task| task.is_some()),
            )
            .finish()
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
                queue: Mutex::new(VecDeque::new()),
                notify: tokio::sync::Notify::new(),
                last_sync: Mutex::new(None),
                monotonic: Arc::new(SystemMonotonic::new()),
            }),
            cancel: CancellationToken::new(),
            task: tokio::sync::Mutex::new(None),
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
}

impl LeaderTask for GitHubWorker {
    fn name(&self) -> &'static str {
        "GitHub"
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
        guard(&self.shared.queue).push_back(outcome.clone());
        self.shared.notify.notify_one();
        Ok(())
    }
}
