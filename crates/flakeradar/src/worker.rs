//! The Report processor: one elected process drains the queue, prunes and stops cleanly.
//!
//! Every server process runs one [`ReportWorker`], and only the holder of the
//! Report processor advisory lock leads (ADR 0002: scoring depends on Execution
//! order, so exactly one processor runs). The others stay on standby and take
//! over when the leader stops or its connection dies.
//!
//! The loop claims one Report at a time, prunes at most once per interval, and
//! hands each outcome to the [`LeaderTask`]s that need it — the GitHub and
//! Webhook work — without waiting for their I/O. Hands over are
//! non-blocking, so a slow or hanging external call cannot stop processing. When
//! leadership ends, the leader tasks stop *before* the lock is released, so a
//! replacement leader never sees the lock free while this process still has an
//! issue POST in flight.
//!
//! Everything the loop reads from the outside — the processor, the prune, the
//! waits, the monotonic clock and the lock session — is a trait this module
//! takes, so a test drives the loop without a clock or a real interval.

use std::fmt;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use flakeradar_core::model::ReportStatus;
use flakeradar_db::{
    DbError, DbSettings, WORKER_LOCK_KEY, clock::SharedMonotonic, clock::SystemMonotonic,
    connect_session, processing::ProcessOutcome, processing::ProcessingSettings,
    processing::process_next, retention::PruneResult, retention::RetentionSettings,
    retention::run_prune,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::settings::{Secret, Settings};

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

/// What a [`LeaderTask`] does to stop; the error says why it could not.
pub type StopFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

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

/// The real processor: [`flakeradar_db::processing::process_next`].
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

/// The real retention run: [`flakeradar_db::retention::run_prune`] with these settings.
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
    url: Secret,
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
    ///
    /// The URL is kept as a [`Secret`], so a `Debug` line on this lock — and on
    /// the worker that holds it — prints no database password.
    #[must_use]
    pub fn new(url: impl Into<String>, settings: DbSettings) -> Self {
        Self {
            url: Secret::new(url),
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
                connection: connect_session(self.url.expose(), self.settings).await?,
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

/// How long a standby replica waits before it asks for the lock again.
///
/// A standby's whole loop is one `pg_try_advisory_lock`, run on every replica
/// that is not the leader, so five seconds keeps that query off the database's
/// hot path while still handing a lost leadership over within one poll. Raising
/// it delays every takeover by that much, and a Report waits in the queue for
/// the whole delay; lowering it puts an idle write to `pg_locks` on every
/// standby several times a second.
const STANDBY_WAIT: Duration = Duration::from_secs(5);

/// The longest wait between the retries of one Report.
///
/// A Report requeued for a retry waits `poll * 2^retry_count`, so the wait grows
/// with each failure; this ceiling stops that growth at 30 seconds, which keeps
/// the last of `max_report_retries` retries inside a few minutes. Removing it
/// would let a long database outage space one Report's retries hours apart;
/// lowering it makes a still-failing Report spend its retries before the
/// database is back.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

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
    /// The configured poll and prune intervals with the two fixed waits.
    #[must_use]
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            poll: settings.worker_poll,
            standby: STANDBY_WAIT,
            prune_interval: settings.prune_interval,
            max_backoff: MAX_BACKOFF,
        }
    }
}

/// The single elected Report processor of one server process.
///
/// [`ReportWorker::start`] spawns the loop, [`ReportWorker::stop`] cancels it,
/// waits for it and then ends leadership. A worker that never started is
/// stopped without any task to cancel.
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
    /// Signals when the loop `start` spawned has ended, by stop or by panic.
    ended: tokio::sync::Mutex<Option<Arc<tokio::sync::Notify>>>,
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
            ended: tokio::sync::Mutex::new(None),
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
        // A fresh end signal per run. The loop is awaited on a second task, so a
        // panic inside it is captured by the JoinHandle and still signals an end;
        // that is what lets `serve` turn a dead leader into a process exit.
        let ended = Arc::new(tokio::sync::Notify::new());
        *self.ended.lock().await = Some(Arc::clone(&ended));
        let run = tokio::spawn(async move { worker.run().await });
        let ended_in_loop = Arc::clone(&ended);
        let supervisor = tokio::spawn(async move {
            match run.await {
                Ok(()) => {}
                Err(error) => {
                    // A panic inside `run` is held by the JoinHandle and only
                    // surfaces here, so this line is what an operator sees when
                    // a leader stopped processing without a stop.
                    tracing::error!(
                        name: "worker.processor.panicked",
                        error = %error,
                        "the Report worker loop ended with a panic",
                    );
                }
            }
            ended_in_loop.notify_one();
        });
        *task = Some(supervisor);
    }

    /// Resolves once the loop `start` spawned has ended.
    ///
    /// `serve` selects on this as well as on the shutdown signal, so a loop that
    /// ended before a stop — the panic case — becomes an error and the process
    /// exits. A worker that never started, or whose loop is still going,
    /// resolves nothing.
    pub async fn ended(&self) {
        let notify = {
            let guard = self.ended.lock().await;
            guard.clone()
        };
        if let Some(notify) = notify {
            notify.notified().await;
        } else {
            std::future::pending::<()>().await;
        }
    }

    /// Cancels the loop, waits for it and then ends leadership.
    ///
    /// The wait is the graceful part: the loop checks the token between Reports,
    /// not inside one, so a Report whose processing has started runs on to its
    /// commit and the stop costs at most that one Report. Leadership ends in the
    /// worker's own release step, which stops the leader tasks before it unlocks.
    pub async fn stop(&self) {
        self.cancel.cancel();
        let handle = self.task.lock().await.take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
        self.release().await;
    }

    /// The loop: elect, prove the lock, prune, claim, hand over, wait.
    async fn run(&self) {
        // Consecutive `process_next` failures, so a persistent one backs off up
        // to `max_backoff` instead of claiming the same Report every poll. The
        // count resets as soon as a claim or an empty queue answers.
        let mut consecutive_errors = 0i32;
        while !self.cancel.is_cancelled() {
            if !self.is_leader() {
                match self.lock.try_lock().await {
                    Ok(true) => self.take_leadership().await,
                    Ok(false) => {
                        self.sleep(self.settings.standby).await;
                        continue;
                    }
                    Err(error) => {
                        self.drop_leadership(&error, self.settings.poll).await;
                        continue;
                    }
                }
            }
            // The leader proves its lock is still the session that took it on
            // every loop, on a backlog as well as on an empty queue: a lock
            // connection that died frees the lock for a standby even while
            // Reports keep arriving. Checking only an empty queue would let the
            // two claim the same Report; see `differences.md`.
            if let Err(error) = self.lock.check().await {
                self.drop_leadership(&error, self.settings.poll).await;
                continue;
            }
            self.maybe_prune().await;
            match self
                .processor
                .process_next(self.pool.clone(), self.processing.clone())
                .await
            {
                Ok(Some(outcome)) => {
                    consecutive_errors = 0;
                    self.deliver(&outcome).await;
                }
                Ok(None) => {
                    consecutive_errors = 0;
                    self.sleep(self.settings.poll).await;
                }
                Err(error) => {
                    consecutive_errors = consecutive_errors.saturating_add(1);
                    self.drop_leadership(&error, self.backoff(consecutive_errors))
                        .await;
                }
            }
        }
    }

    /// Marks this process as the leader and starts every leader task.
    async fn take_leadership(&self) {
        self.leader.store(true, Ordering::SeqCst);
        for task in &self.leader_tasks {
            task.start().await;
        }
        tracing::info!(
            name: "worker.processor.elected",
            "This process is now the Report processor",
        );
    }

    /// Logs the error, ends leadership and waits `wait` before trying again.
    async fn drop_leadership(&self, error: &DbError, wait: Duration) {
        tracing::error!(
            name: "worker.processor.failed",
            error = %error,
            "Report worker error; dropping leadership and retrying",
        );
        self.release().await;
        self.sleep(wait).await;
    }

    /// Hands one outcome to every leader task, then waits out a retry's backoff.
    async fn deliver(&self, outcome: &ProcessOutcome) {
        for task in &self.leader_tasks {
            if let Err(error) = task.enqueue(outcome) {
                let task = task.name();
                let report_id = outcome.report_id;
                tracing::error!(
                    name: "worker.leader_task.enqueue_failed",
                    task,
                    report_id,
                    error = %error,
                    "a leader task failed to take a processed Report",
                );
            }
        }
        if outcome.status == ReportStatus::Pending {
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
            tracing::error!(
                name: "worker.prune.failed",
                error = %error,
                "Retention prune failed; will retry next interval",
            );
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
                let task = task.name();
                tracing::error!(
                    name: "worker.leader_task.stop_failed",
                    task,
                    error = %error,
                    "a leader task failed to stop; the lock is released anyway",
                );
            }
        }
        self.leader.store(false, Ordering::SeqCst);
        if let Err(error) = self.lock.unlock().await {
            // The server already dropped the session lock with the connection.
            tracing::warn!(
                name: "worker.leadership.lock_lost",
                error = %error,
                "the lock connection is broken; it has been dropped",
            );
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
