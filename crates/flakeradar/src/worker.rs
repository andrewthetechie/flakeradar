//! The Report processor: one elected process drains the queue, prunes and stops cleanly.
//!
//! Every server process runs one [`ReportWorker`], and only the holder of the
//! Report processor advisory lock leads (ADR 0002: scoring depends on Execution
//! order, so exactly one processor runs). The others stay on standby and take
//! over when the leader stops or its connection dies.
//!
//! The loop claims one Report at a time, prunes at most once per interval, and
//! hands each outcome to the [`LeaderTask`]s that need it — the GitHub and
//! Webhook work of task 10 — without waiting for their I/O. Hands over are
//! non-blocking, so a slow or hanging external call cannot stop processing. When
//! leadership ends, the leader tasks stop *before* the lock is released, so a
//! replacement leader never sees the lock free while this process still has an
//! issue POST in flight.
//!
//! Everything the loop reads from the outside — the processor, the prune, the
//! waits, the monotonic clock and the lock session — is a trait this module
//! takes, so a test drives the loop without a clock or a real interval.

// Rust guideline compliant 2026-07-21

use std::fmt;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use flakeradar_db::{
    DbError, DbSettings, ProcessOutcome, ProcessingSettings, PruneResult, REPORT_PENDING,
    RetentionSettings, SharedMonotonic, SystemMonotonic, WORKER_LOCK_KEY, connect_session,
    process_next, run_prune,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::settings::Settings;

/// The advisory lock every process tries to take to become the processor.
///
/// It answers which backend took the lock in the same statement, because a
/// session lock belongs to that backend and to nothing else.
const TRY_LOCK_SQL: &str =
    "SELECT pg_try_advisory_lock($1) AS locked, pg_backend_pid() AS backend_pid";

/// Releases the session lock the leader holds.
const UNLOCK_SQL: &str = "SELECT pg_advisory_unlock($1) AS released";

/// Asks whether the lock connection still answers as the backend that took the lock.
///
/// A replacement session answers a statement just as happily as the one the
/// lock was taken on, so the answer is the backend id rather than `SELECT 1`.
const ALIVE_SQL: &str = "SELECT pg_backend_pid() = $1 AS same_session";

/// What a [`LeaderTask`] does once it is running.
pub type StartFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// What a [`LeaderTask`] does to stop; the error says why it could not.
pub type StopFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

/// Work only the leader runs: it starts with leadership and stops before the lock is released.
///
/// Task 10 implements this for the GitHub issue filer and the Notification
/// sender. [`LeaderTask::enqueue`] never waits for the task's network I/O, so a
/// slow endpoint cannot delay Report processing, and its error is logged rather
/// than allowed to stop the loop.
pub trait LeaderTask: fmt::Debug + Send + Sync {
    /// The name a log line uses for this task, such as `GitHub`.
    fn name(&self) -> &'static str;

    /// Starts the task's own background work. Calling it twice is the task's problem.
    fn start(&self) -> StartFuture<'_>;

    /// Stops the task, finishing or cancelling whatever it has in flight.
    ///
    /// # Errors
    ///
    /// Returns why the task could not stop. The lock is released anyway.
    fn stop(&self) -> StopFuture<'_>;

    /// Hands a processed Report over without waiting for the task's I/O.
    ///
    /// # Errors
    ///
    /// Returns why the hand-over failed; the worker logs it and carries on.
    fn enqueue(&self, outcome: &ProcessOutcome) -> anyhow::Result<()>;
}

/// What the loop claims a Report with.
pub type ProcessFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<ProcessOutcome>, DbError>> + Send + 'a>>;

/// Claims and processes the next pending Report.
///
/// The loop holds [`DatabaseProcessor`]; a test substitutes its own to reach the
/// loop's error and backoff paths without a database that fails on cue.
pub trait ReportProcessor: fmt::Debug + Send + Sync {
    /// Processes the oldest pending Report, or returns `None` when the queue is empty.
    fn process_next(
        &self,
        pool: DatabaseConnection,
        settings: ProcessingSettings,
    ) -> ProcessFuture<'static>;
}

/// The real processor: [`flakeradar_db::process_next`].
#[derive(Debug, Clone, Copy, Default)]
pub struct DatabaseProcessor;

impl ReportProcessor for DatabaseProcessor {
    fn process_next(
        &self,
        pool: DatabaseConnection,
        settings: ProcessingSettings,
    ) -> ProcessFuture<'static> {
        Box::pin(async move { process_next(&pool, &settings).await })
    }
}

/// What one retention run does.
pub type PruneFuture<'a> = Pin<Box<dyn Future<Output = Result<PruneResult, DbError>> + Send + 'a>>;

/// The retention run the leader repeats once per interval.
pub trait Pruner: fmt::Debug + Send + Sync {
    /// Prunes one time-budgeted run of the expired backlog.
    fn prune(&self, pool: DatabaseConnection) -> PruneFuture<'static>;
}

/// The real retention run: [`flakeradar_db::run_prune`] with these settings.
#[derive(Debug)]
pub struct RetentionPrune {
    retention: RetentionSettings,
    processing: ProcessingSettings,
    monotonic: SharedMonotonic,
}

impl RetentionPrune {
    /// A prune run with the settings the server was configured with.
    #[must_use]
    pub fn new(retention: RetentionSettings, processing: ProcessingSettings) -> Self {
        Self {
            retention,
            processing,
            monotonic: Arc::new(SystemMonotonic::new()),
        }
    }
}

impl Pruner for RetentionPrune {
    fn prune(&self, pool: DatabaseConnection) -> PruneFuture<'static> {
        let retention = self.retention;
        let processing = self.processing.clone();
        let monotonic = Arc::clone(&self.monotonic);
        Box::pin(async move { run_prune(&pool, &retention, &processing, monotonic.as_ref()).await })
    }
}

/// What the loop waits on between claims, leadership attempts and prunes.
pub type WaitFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// How the loop waits, so a test never spends real time on an interval.
pub trait Waiter: fmt::Debug + Send + Sync {
    /// Resolves after `delay`, unless the worker is cancelled first.
    fn wait(&self, delay: Duration) -> WaitFuture<'_>;
}

/// The real wait: [`tokio::time::sleep`], which `tokio::time::pause` controls.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioWaiter;

impl Waiter for TokioWaiter {
    fn wait(&self, delay: Duration) -> WaitFuture<'_> {
        Box::pin(tokio::time::sleep(delay))
    }
}

/// What a lock query answers.
pub type LockFuture<'a> = Pin<Box<dyn Future<Output = Result<bool, DbError>> + Send + 'a>>;

/// What a lock statement answers when it reports no value.
pub type SessionFuture<'a> = Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>>;

/// The dedicated connection that carries the processor's advisory lock.
///
/// The real one is [`AdvisoryLock`]; a test substitutes its own to make an
/// election fail or to count attempts without a database on cue.
pub trait LockSession: fmt::Debug + Send + Sync {
    /// Tries to take the lock, answering whether this session now holds it.
    fn try_lock(&self) -> LockFuture<'_>;

    /// Proves the session that took the lock is still the one answering.
    ///
    /// A statement that fails, and one answered by any other backend, both mean
    /// the lock is gone: the server releases a session lock with the backend
    /// that held it, and the pool replaces that backend with one that holds
    /// nothing.
    fn check(&self) -> SessionFuture<'_>;

    /// Releases the lock and closes the session. A broken session is reported.
    fn unlock(&self) -> SessionFuture<'_>;
}

/// The session lock, held on a pool of exactly one connection.
///
/// The lock is session-scoped and survives a connection being returned to a
/// pool, so the leader keeps one dedicated connection for as long as it leads
/// and closes it after unlocking. The connection is opened lazily, so a standby
/// replica that never wins the election only ever holds the standby connection
/// it needs to try.
///
/// The pool still replaces a connection the server closed when it next hands it
/// out, and the replacement holds no lock, so the session also remembers which
/// backend took the lock and checks every liveness answer against it.
#[derive(Debug)]
pub struct AdvisoryLock {
    url: String,
    settings: DbSettings,
    session: tokio::sync::Mutex<Option<LockedSession>>,
}

/// One connection of the lock pool, and the backend that took the lock on it.
#[derive(Debug)]
struct LockedSession {
    connection: DatabaseConnection,
    /// The `pg_backend_pid()` that took the lock; `None` until an election is won.
    backend_pid: Option<i32>,
}

impl AdvisoryLock {
    /// A lock session over the pool `url` names.
    #[must_use]
    pub fn new(url: impl Into<String>, settings: DbSettings) -> Self {
        Self {
            url: url.into(),
            settings,
            session: tokio::sync::Mutex::new(None),
        }
    }

    /// The dedicated session, opened on first use.
    ///
    /// A standby replica that never wins holds the one connection it tried
    /// with, so the next attempt is a query rather than a new session. The
    /// session is taken out of the guard: the caller puts it back only when it
    /// is still usable, and dropping it is what ends a broken one for good.
    async fn open(
        &self,
        guard: &mut tokio::sync::MutexGuard<'_, Option<LockedSession>>,
    ) -> Result<LockedSession, DbError> {
        match guard.take() {
            Some(session) => Ok(session),
            None => Ok(LockedSession {
                connection: connect_session(&self.url, self.settings).await?,
                backend_pid: None,
            }),
        }
    }
}

impl LockSession for AdvisoryLock {
    fn try_lock(&self) -> LockFuture<'_> {
        Box::pin(async move {
            let mut guard = self.session.lock().await;
            let mut session = self.open(&mut guard).await?;
            let statement = Statement::from_sql_and_values(
                DbBackend::Postgres,
                TRY_LOCK_SQL,
                [WORKER_LOCK_KEY.into()],
            );
            let row = match session.connection.query_one_raw(statement).await {
                Ok(row) => row,
                // A session the server closed has already lost the lock, so it
                // must not be reused: dropping the pool ends it for good.
                Err(error) => return Err(DbError::Database(error)),
            };
            // A query with no FROM clause always answers one row; the arm for a
            // missing one is logged in coverage-gaps.md and means "not locked".
            let Some(row) = row else {
                *guard = Some(session);
                return Ok(false);
            };
            let locked: bool = row.try_get("", "locked")?;
            if locked {
                // The lock belongs to the backend that answered this statement,
                // and every later check is compared against it.
                session.backend_pid = Some(row.try_get("", "backend_pid")?);
            }
            *guard = Some(session);
            Ok(locked)
        })
    }

    fn check(&self) -> SessionFuture<'_> {
        Box::pin(async move {
            let mut guard = self.session.lock().await;
            let Some(session) = guard.take() else {
                return Err(DbError::LockSessionLost {
                    detail: "this process holds no lock session".to_owned(),
                });
            };
            let Some(locked) = session.backend_pid else {
                // The session is still usable, so it stays; it just never won an
                // election, and no session that did not take the lock leads.
                *guard = Some(session);
                return Err(DbError::LockSessionLost {
                    detail: "no lock was taken on this session".to_owned(),
                });
            };
            let statement =
                Statement::from_sql_and_values(DbBackend::Postgres, ALIVE_SQL, [locked.into()]);
            let answer = session.connection.query_one_raw(statement).await;
            let same_session = match &answer {
                Ok(Some(row)) => row
                    .try_get::<bool>("", "same_session")
                    .is_ok_and(|same| same),
                // No answer, or a column that is not there, cannot prove the lock.
                Ok(None) | Err(_) => false,
            };
            if same_session {
                *guard = Some(session);
                return Ok(());
            }
            match answer {
                // The statement failed, so the lock is lost either way. The pool
                // is not what broke: sqlx replaces a connection the server
                // closed, and it is that replacement's answer to the next check
                // which names the backend that no longer holds the lock.
                Err(error) => {
                    *guard = Some(session);
                    Err(DbError::Database(error))
                }
                // A statement that answered as another backend is a replacement
                // the pool opened over a session the server had already closed.
                // It holds nothing, so the session is dropped rather than kept.
                Ok(_) => Err(DbError::LockSessionLost {
                    detail: format!(
                        "the connection no longer answers as backend {locked}, which took the lock"
                    ),
                }),
            }
        })
    }

    fn unlock(&self) -> SessionFuture<'_> {
        Box::pin(async move {
            let Some(session) = self.session.lock().await.take() else {
                return Ok(());
            };
            let statement = Statement::from_sql_and_values(
                DbBackend::Postgres,
                UNLOCK_SQL,
                [WORKER_LOCK_KEY.into()],
            );
            // Whether the unlock worked or the session was broken, this session
            // is over; dropping it is what closes it.
            session
                .connection
                .query_one_raw(statement)
                .await
                .map(|_row| ())
                .map_err(DbError::Database)
        })
    }
}

/// The loop's intervals, so no test depends on a real one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerSettings {
    /// How long the leader waits before asking for another Report.
    pub poll: Duration,
    /// How long a standby replica waits before trying the lock again.
    pub standby: Duration,
    /// How long the leader waits between retention runs.
    pub prune_interval: Duration,
    /// The ceiling for the wait after a Report was requeued for a retry.
    pub max_backoff: Duration,
}

impl WorkerSettings {
    /// The frozen server's `worker_poll_seconds` and `prune_interval_seconds`,
    /// with its fixed 5-second standby and 30-second retry ceiling.
    #[must_use]
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            poll: seconds(settings.worker_poll_seconds),
            standby: Duration::from_secs(5),
            prune_interval: seconds(settings.prune_interval_seconds),
            max_backoff: Duration::from_secs(30),
        }
    }
}

/// A configured number of seconds as a duration, never a panic.
fn seconds(value: f64) -> Duration {
    Duration::try_from_secs_f64(value).unwrap_or_default()
}

/// The single elected Report processor of one server process.
///
/// [`ReportWorker::start`] spawns the loop, [`ReportWorker::stop`] cancels it,
/// waits for it and then ends leadership. A worker that never started is
/// stopped without any task to cancel, exactly as the frozen server's was.
pub struct ReportWorker {
    pool: DatabaseConnection,
    lock: Arc<dyn LockSession>,
    processing: ProcessingSettings,
    settings: WorkerSettings,
    processor: Arc<dyn ReportProcessor>,
    pruner: Option<Arc<dyn Pruner>>,
    waiter: Arc<dyn Waiter>,
    monotonic: SharedMonotonic,
    leader_tasks: Vec<Arc<dyn LeaderTask>>,
    leader: AtomicBool,
    last_prune: Mutex<Option<f64>>,
    cancel: CancellationToken,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl fmt::Debug for ReportWorker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReportWorker")
            .field("settings", &self.settings)
            .field("leader", &self.is_leader())
            .field("leader_tasks", &self.leader_tasks)
            .finish_non_exhaustive()
    }
}

impl ReportWorker {
    /// A worker over `pool` that elects itself through `lock`.
    #[must_use]
    pub fn new(
        pool: DatabaseConnection,
        lock: Arc<dyn LockSession>,
        processing: ProcessingSettings,
        settings: WorkerSettings,
    ) -> Self {
        Self {
            pool,
            lock,
            processing,
            settings,
            processor: Arc::new(DatabaseProcessor),
            pruner: None,
            waiter: Arc::new(TokioWaiter),
            monotonic: Arc::new(SystemMonotonic::new()),
            leader_tasks: Vec::new(),
            leader: AtomicBool::new(false),
            last_prune: Mutex::new(None),
            cancel: CancellationToken::new(),
            task: tokio::sync::Mutex::new(None),
        }
    }

    /// Replaces the claim of the next Report.
    #[must_use]
    pub fn with_processor(mut self, processor: Arc<dyn ReportProcessor>) -> Self {
        self.processor = processor;
        self
    }

    /// Adds the retention run the leader repeats.
    #[must_use]
    pub fn with_pruner(mut self, pruner: Arc<dyn Pruner>) -> Self {
        self.pruner = Some(pruner);
        self
    }

    /// Adds a task that starts and stops with leadership.
    #[must_use]
    pub fn with_leader_task(mut self, task: Arc<dyn LeaderTask>) -> Self {
        self.leader_tasks.push(task);
        self
    }

    /// Replaces the wait, so a test never spends a configured interval.
    #[must_use]
    pub fn with_waiter(mut self, waiter: Arc<dyn Waiter>) -> Self {
        self.waiter = waiter;
        self
    }

    /// Reads the prune interval from `monotonic` instead of the real clock.
    #[must_use]
    pub fn with_monotonic(mut self, monotonic: SharedMonotonic) -> Self {
        self.monotonic = monotonic;
        self
    }

    /// Whether this process currently holds the processor lock.
    #[must_use]
    pub fn is_leader(&self) -> bool {
        self.leader.load(Ordering::SeqCst)
    }

    /// Starts the loop. Starting a running worker is a no-op.
    ///
    /// The worker is handed over rather than borrowed because the loop outlives
    /// the call; keep the other handle to [`ReportWorker::stop`] it.
    pub async fn start(self: Arc<Self>) {
        let worker = Arc::clone(&self);
        let mut task = self.task.lock().await;
        if task.as_ref().is_some_and(|handle| !handle.is_finished()) {
            return;
        }
        *task = Some(tokio::spawn(async move { worker.run().await }));
    }

    /// Cancels the loop, waits for it and then ends leadership.
    ///
    /// The wait is the graceful part: a Report transaction in flight is rolled
    /// back by the processor's own transaction, so the Report simply stays
    /// pending. Leadership actually ends in [`ReportWorker::release`], which
    /// stops the leader tasks before it unlocks.
    pub async fn stop(&self) {
        self.cancel.cancel();
        let handle = self.task.lock().await.take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
        self.release().await;
    }

    /// The loop: elect, prune, claim, hand over, wait.
    async fn run(&self) {
        while !self.cancel.is_cancelled() {
            if !self.is_leader() {
                match self.lock.try_lock().await {
                    Ok(true) => self.take_leadership().await,
                    Ok(false) => {
                        self.sleep(self.settings.standby).await;
                        continue;
                    }
                    Err(error) => {
                        self.drop_leadership(&error).await;
                        continue;
                    }
                }
            }
            self.maybe_prune().await;
            match self
                .processor
                .process_next(self.pool.clone(), self.processing.clone())
                .await
            {
                Ok(Some(outcome)) => self.deliver(&outcome).await,
                Ok(None) => {
                    // The queue is empty, so prove the session that took the lock
                    // is still the one answering: if it died, the lock is already
                    // free for another process -- and the pool answers a
                    // statement from its replacement just the same.
                    if let Err(error) = self.lock.check().await {
                        self.drop_leadership(&error).await;
                        continue;
                    }
                    self.sleep(self.settings.poll).await;
                }
                Err(error) => self.drop_leadership(&error).await,
            }
        }
    }

    /// Marks this process as the leader and starts every leader task.
    async fn take_leadership(&self) {
        self.leader.store(true, Ordering::SeqCst);
        for task in &self.leader_tasks {
            task.start().await;
        }
        tracing::info!("This process is now the Report processor");
    }

    /// Logs the error, ends leadership and waits one poll before trying again.
    async fn drop_leadership(&self, error: &DbError) {
        tracing::error!(error = %error, "Report worker error; dropping leadership and retrying");
        self.release().await;
        self.sleep(self.settings.poll).await;
    }

    /// Hands one outcome to every leader task, then waits out a retry's backoff.
    async fn deliver(&self, outcome: &ProcessOutcome) {
        for task in &self.leader_tasks {
            if let Err(error) = task.enqueue(outcome) {
                let name = task.name();
                let report_id = outcome.report_id;
                let message = format!(
                    "the {name} leader task failed to take processed Report {report_id}: {error}"
                );
                tracing::error!("{message}");
            }
        }
        if outcome.status == REPORT_PENDING {
            self.sleep(self.backoff(outcome.retry_count)).await;
        }
    }

    /// The wait after a Report was requeued for a retry: `poll * 2^retry_count`, capped.
    ///
    /// Without the wait, a transiently failed Report is still the oldest pending
    /// one, so claiming again right away would spend the whole retry budget in
    /// milliseconds.
    fn backoff(&self, retry_count: i32) -> Duration {
        let seconds = (self.settings.poll.as_secs_f64() * 2f64.powi(retry_count))
            .min(self.settings.max_backoff.as_secs_f64());
        Duration::from_secs_f64(seconds.max(0.0))
    }

    /// Runs the retention prune, at most once per `prune_interval`.
    ///
    /// The interval is claimed before the run, so a failed prune waits a whole
    /// interval instead of retrying on every poll; it is logged and the loop
    /// carries on.
    async fn maybe_prune(&self) {
        let Some(pruner) = &self.pruner else {
            return;
        };
        let now = self.monotonic.now();
        let last = *self.last_prune();
        if last.is_some_and(|last| now - last < self.settings.prune_interval.as_secs_f64()) {
            return;
        }
        *self.last_prune() = Some(now);
        if let Err(error) = pruner.prune(self.pool.clone()).await {
            tracing::error!(error = %error, "Retention prune failed; will retry next interval");
        }
    }

    /// Ends leadership: the leader tasks stop first, then the lock is given up.
    ///
    /// The order matters. Unlocking while a leader task still has a call in
    /// flight would let a replacement leader take the lock and file the same
    /// issue twice. A task that cannot stop is logged, and the lock is released
    /// anyway rather than held by a process that no longer processes.
    async fn release(&self) {
        for task in &self.leader_tasks {
            if let Err(error) = task.stop().await {
                let name = task.name();
                let message = format!(
                    "stopping the {name} leader task failed; releasing the lock anyway: {error}"
                );
                tracing::error!("{message}");
            }
        }
        self.leader.store(false, Ordering::SeqCst);
        if let Err(error) = self.lock.unlock().await {
            // The server already dropped the session lock with the connection.
            tracing::warn!(error = %error, "the lock connection is broken; it has been dropped");
        }
    }

    /// Waits `delay`, or returns early when the worker is cancelled.
    async fn sleep(&self, delay: Duration) {
        tokio::select! {
            () = self.waiter.wait(delay) => {}
            () = self.cancel.cancelled() => {}
        }
    }

    /// The last prune time, tolerating a panic in another task that held the lock.
    fn last_prune(&self) -> MutexGuard<'_, Option<f64>> {
        self.last_prune
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}
