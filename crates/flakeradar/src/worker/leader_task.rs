//! The leader's own work: the run that starts with leadership and stops before
//! the lock is released.
//!
//! [`LeaderTask`] is the seam the GitHub filer and the Notification sender
//! implement, and [`LeaderRun`] is the one run of such a task, with the token
//! that stops it. [`until_stopped`] is what lets a call end the moment
//! leadership does.

// Rust guideline compliant 2026-07-21

use std::fmt;

use flakeradar_db::processing::ProcessOutcome;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::BoxFuture;

/// One run of a [`LeaderTask`]: the loop's handle and the token that stops it.
///
/// A [`CancellationToken`] cannot be reset, so a task that outlived one `stop`
/// would exit the moment it started again. Every run therefore makes a token of
/// its own and keeps it beside the handle, which is what lets a re-elected
/// leader file, notify and sync again. [`LeaderRun::stop`] cancels that token and
/// waits for the loop, so a task that ends promptly makes release prompt too.
#[derive(Debug, Default)]
pub struct LeaderRun {
    /// The current run, or the last one while its loop is still finishing.
    running: tokio::sync::Mutex<Option<RunningTask>>,
}

/// The handle of one run, and the token that only it listens to.
#[derive(Debug)]
struct RunningTask {
    /// The stop signal this run's loop watches.
    cancel: CancellationToken,
    /// The spawned loop.
    handle: JoinHandle<()>,
}

impl LeaderRun {
    /// A run that is not going yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts `run` on a token of its own, unless the last run is still going.
    ///
    /// Starting a task that is already running is a no-op, and so is starting one
    /// whose loop is still finishing its last await.
    pub async fn start<F, Fut>(&self, run: F)
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut running = self.running.lock().await;
        if running
            .as_ref()
            .is_some_and(|task| !task.handle.is_finished())
        {
            return;
        }
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(run(cancel.clone()));
        *running = Some(RunningTask { cancel, handle });
    }

    /// Cancels the run and waits for it; a task that is not running has nothing to do.
    ///
    /// # Errors
    ///
    /// Returns why the run's loop ended badly, such as a panic inside it.
    pub async fn stop(&self) -> anyhow::Result<()> {
        let running = self.running.lock().await.take();
        if let Some(task) = running {
            task.cancel.cancel();
            task.handle.await?;
        }
        Ok(())
    }

    /// Whether a run is going, for a `Debug` line.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running.try_lock().is_ok_and(|running| {
            running
                .as_ref()
                .is_some_and(|task| !task.handle.is_finished())
        })
    }
}

/// Runs `call`, answering `None` when `stop` cancels it first.
///
/// A sync's `GET`, a Notification and a wait between retries end the moment
/// leadership does, because each costs at most a retry. A call that could have
/// landed at the other end — the issue `POST`, the fixed note's comment and the
/// close that follows it — is not wrapped in this, because dropping it would
/// repeat a comment or leave a fixed issue open; those run to their own timeout
/// instead, which is the longest a stop waits.
pub async fn until_stopped<F>(stop: &CancellationToken, call: F) -> Option<F::Output>
where
    F: Future,
{
    tokio::select! {
        () = stop.cancelled() => None,
        output = call => Some(output),
    }
}

/// Work only the leader runs: it starts with leadership and stops before the lock is released.
///
/// The GitHub issue filer and the Notification sender are the two implementations.
/// [`LeaderTask::enqueue`] never waits for the task's network I/O, so a slow
/// endpoint cannot delay Report processing, and its error is logged rather than
/// allowed to stop the loop.
pub trait LeaderTask: fmt::Debug + Send + Sync {
    /// The name a log line uses for this task, such as `GitHub`.
    fn name(&self) -> &'static str;

    /// Starts the task's own background work. Calling it twice is the task's problem.
    fn start(&self) -> BoxFuture<'_, ()>;

    /// Stops the task, finishing or cancelling whatever it has in flight.
    ///
    /// # Errors
    ///
    /// Returns why the task could not stop. The lock is released anyway.
    fn stop(&self) -> BoxFuture<'_, anyhow::Result<()>>;

    /// Hands a processed Report over without waiting for the task's I/O.
    ///
    /// # Errors
    ///
    /// Returns why the hand-over failed; the worker logs it and carries on.
    fn enqueue(&self, outcome: &ProcessOutcome) -> anyhow::Result<()>;
}
