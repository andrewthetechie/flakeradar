//! The doubles a worker test avoids the real world with.
//!
//! The real worker takes its processor, its lock session, its waiter, its pruner
//! and its leader tasks through traits, so a test can hand it a double that
//! answers a script, fails on cue or counts what it was asked. These live here
//! rather than in one test file because several need them and none of them
//! belongs in a release binary.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a double fails loudly when it is asked for something it cannot answer"
)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use flakeradar::BoxFuture;
use flakeradar::worker::{AdvisoryLock, LeaderTask, LockSession, Pruner, ReportProcessor, Waiter};
use flakeradar_core::{model::ReportStatus, notify::TierChange};
use flakeradar_db::{
    DbError, DbSettings, WORKER_LOCK_KEY, clock::MonotonicClock, connect,
    processing::ProcessOutcome, processing::ProcessingSettings, retention::PruneResult,
};
use sea_orm::{ConnectionTrait as _, DatabaseConnection, Statement};

use crate::TestDb;

/// Whether another session can take the processor lock right now.
///
/// The probe takes the lock when it is free, so it releases it again first.
pub async fn lock_is_free(url: &str) -> bool {
    let pool = connect(url, DbSettings::default())
        .await
        .expect("the probe's pool");
    let statement = Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "SELECT pg_try_advisory_lock($1) AS locked",
        [WORKER_LOCK_KEY.into()],
    );
    let row = pool
        .query_one_raw(statement)
        .await
        .expect("the probe query runs")
        .expect("the probe query answers");
    let free: bool = row.try_get("", "locked").expect("a flag");
    if free {
        let statement = Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT pg_advisory_unlock($1)",
            [WORKER_LOCK_KEY.into()],
        );
        pool.execute_raw(statement)
            .await
            .expect("the probe releases the lock");
    }
    pool.close().await.expect("the probe's pool closes");
    free
}

/// The backend id that holds the processor lock for `database`, when one does.
///
/// Read while the worker leads and nothing else has been changed, so the answer
/// cannot race the loss the caller stages next.
pub async fn lock_holder_pid(db: &TestDb, database: &str) -> Option<i32> {
    let admin = connect(db.maintenance_url(), DbSettings::default())
        .await
        .expect("the maintenance connection");
    let statement = Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "SELECT pid FROM pg_locks WHERE locktype = 'advisory' AND granted \
           AND classid = 0 AND objid::bigint = $1 \
           AND database = (SELECT oid FROM pg_database WHERE datname = $2)",
        [WORKER_LOCK_KEY.into(), database.into()],
    );
    let row = admin
        .query_one_raw(statement)
        .await
        .expect("the lock query runs");
    let pid: Option<i32> = row.map(|row| row.try_get("", "pid").expect("a backend id"));
    admin
        .close()
        .await
        .expect("the maintenance connection closes");
    pid
}

/// The backend id that holds the processor lock for `database`.
pub async fn lock_holder(db: &TestDb, database: &str) -> i32 {
    lock_holder_pid(db, database)
        .await
        .expect("the worker that leads holds the processor lock")
}

/// Ends the backend `pid`, answering whether it was still there.
pub async fn end_session(db: &TestDb, pid: i32) -> bool {
    let admin = connect(db.maintenance_url(), DbSettings::default())
        .await
        .expect("the maintenance connection");
    let row = admin
        .query_one_raw(Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT pg_terminate_backend($1) AS ended",
            [pid.into()],
        ))
        .await
        .expect("the terminate query runs")
        .expect("the terminate query answers");
    let ended: bool = row.try_get("", "ended").expect("a flag");
    admin
        .close()
        .await
        .expect("the maintenance connection closes");
    ended
}

/// Ends the session holding the processor lock, as a server restart does.
///
/// The lock is session-scoped, so the server frees it the moment the session
/// dies; a replacement leader could take it at any moment after this.
pub async fn terminate_lock_session(db: &TestDb, database: &str) {
    let pid = lock_holder(db, database).await;
    assert!(
        end_session(db, pid).await,
        "the session holding the processor lock was still there to end"
    );
}

/// The name of the test's own database, read while its pool still answers.
///
pub async fn database_name(db: &TestDb) -> String {
    let row = db
        .connection()
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT current_database() AS name".to_owned(),
        ))
        .await
        .expect("the database name reads")
        .expect("a database name");
    row.try_get("", "name").expect("a database name")
}

/// Refuses, or again allows, new connections to `database`.
///
/// Ending the session that holds the lock is not enough on its own: the pool
/// opens a replacement for a connection the server closed, so the next lock
/// statement can still be answered by a fresh session and the test would race
/// the pool. With new connections refused, the replacement cannot be opened and
/// the failure is certain. The policy changes from the maintenance database,
/// because the server refuses to disallow connections for the database the
/// statement runs in, and an existing session is unaffected either way.
pub async fn set_connections_allowed(db: &TestDb, database: &str, allowed: bool) {
    let admin = connect(db.maintenance_url(), DbSettings::default())
        .await
        .expect("the maintenance connection");
    let allowed = if allowed { "true" } else { "false" };
    admin
        .execute_unprepared(&format!(
            "ALTER DATABASE \"{database}\" ALLOW_CONNECTIONS {allowed}"
        ))
        .await
        .expect("the connection policy changes");
    admin
        .close()
        .await
        .expect("the maintenance connection closes");
}

/// A leader task that records every hand-over and can fail on cue.
#[derive(Debug)]
pub struct RecordingTask {
    name: &'static str,
    events: Arc<Mutex<Vec<String>>>,
    fails_to_stop: bool,
    fails_to_enqueue: bool,
    probe: Option<Arc<StopProbe>>,
    gate: Option<Arc<GateWaiter>>,
}

impl RecordingTask {
    /// A task that succeeds at everything, writing into `events`.
    #[must_use]
    pub fn new(name: &'static str, events: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            name,
            events,
            fails_to_stop: false,
            fails_to_enqueue: false,
            probe: None,
            gate: None,
        }
    }

    /// A task that closes `gate` when it stops.
    #[must_use]
    pub fn closing_gate(mut self, gate: Arc<GateWaiter>) -> Self {
        self.gate = Some(gate);
        self
    }

    /// A task whose stop fails.
    #[must_use]
    pub fn failing_to_stop(mut self) -> Self {
        self.fails_to_stop = true;
        self
    }

    /// A task whose enqueue fails.
    #[must_use]
    pub fn failing_to_enqueue(mut self) -> Self {
        self.fails_to_enqueue = true;
        self
    }

    /// A task that checks whether the lock is still held when it stops.
    #[must_use]
    pub fn probing(mut self, probe: Arc<StopProbe>) -> Self {
        self.probe = Some(probe);
        self
    }
}

impl LeaderTask for RecordingTask {
    fn name(&self) -> &'static str {
        self.name
    }

    fn start(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("{}:start", self.name));
        })
    }

    fn stop(&self) -> BoxFuture<'_, anyhow::Result<()>> {
        Box::pin(async move {
            self.events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("{}:stop", self.name));
            if let Some(probe) = &self.probe {
                probe.record().await;
            }
            if let Some(gate) = &self.gate {
                gate.close();
            }
            if self.fails_to_stop {
                return Err(anyhow::anyhow!("{} could not stop", self.name));
            }
            Ok(())
        })
    }

    fn enqueue(&self, outcome: &ProcessOutcome) -> anyhow::Result<()> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{}:enqueue:{}", self.name, outcome.report_id));
        if self.fails_to_enqueue {
            return Err(anyhow::anyhow!("{} could not take the outcome", self.name));
        }
        Ok(())
    }
}

/// Whether the lock was still held at the moment a leader task stopped.
#[derive(Debug)]
pub struct StopProbe {
    url: String,
    free: Mutex<Option<bool>>,
}

impl StopProbe {
    /// A probe over `db`'s database.
    #[must_use]
    pub fn new(db: &TestDb) -> Self {
        Self {
            url: db.url().to_owned(),
            free: Mutex::new(None),
        }
    }

    /// Proves the lock is still held, from a session that is not the worker's.
    pub async fn record(&self) {
        let free = lock_is_free(&self.url).await;
        *self.free.lock().unwrap_or_else(PoisonError::into_inner) = Some(free);
    }

    /// What the probe saw, once the task has stopped.
    #[must_use]
    pub fn saw_free(&self) -> Option<bool> {
        *self.free.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The real lock, with a count of the elections and releases it was asked for.
#[derive(Debug)]
pub struct CountingLock {
    inner: AdvisoryLock,
    try_locks: AtomicUsize,
    unlocks: AtomicUsize,
    checks: AtomicUsize,
}

impl CountingLock {
    /// The real lock over `db`'s database, counting what it is asked.
    #[must_use]
    pub fn new(db: &TestDb) -> Self {
        Self {
            inner: AdvisoryLock::new(db.url(), DbSettings::default()),
            try_locks: AtomicUsize::new(0),
            unlocks: AtomicUsize::new(0),
            checks: AtomicUsize::new(0),
        }
    }

    /// How many elections the worker asked for.
    #[must_use]
    pub fn try_locks(&self) -> usize {
        self.try_locks.load(Ordering::SeqCst)
    }

    /// How many releases the worker asked for.
    #[must_use]
    pub fn unlocks(&self) -> usize {
        self.unlocks.load(Ordering::SeqCst)
    }

    /// How many liveness checks the worker asked for.
    #[must_use]
    pub fn checks(&self) -> usize {
        self.checks.load(Ordering::SeqCst)
    }
}

impl LockSession for CountingLock {
    fn try_lock(&self) -> BoxFuture<'_, Result<bool, DbError>> {
        self.try_locks.fetch_add(1, Ordering::SeqCst);
        self.inner.try_lock()
    }

    fn check(&self) -> BoxFuture<'_, Result<(), DbError>> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        self.inner.check()
    }

    fn unlock(&self) -> BoxFuture<'_, Result<(), DbError>> {
        self.unlocks.fetch_add(1, Ordering::SeqCst);
        self.inner.unlock()
    }
}

/// A lock session whose election always fails, like a lock query the server refuses.
#[derive(Debug)]
pub struct FailingLock {
    try_locks: AtomicUsize,
}

impl Default for FailingLock {
    fn default() -> Self {
        Self::new()
    }
}

impl FailingLock {
    /// A session whose election always fails.
    #[must_use]
    pub fn new() -> Self {
        Self {
            try_locks: AtomicUsize::new(0),
        }
    }

    /// How many elections it refused.
    #[must_use]
    pub fn try_locks(&self) -> usize {
        self.try_locks.load(Ordering::SeqCst)
    }
}

impl LockSession for FailingLock {
    fn try_lock(&self) -> BoxFuture<'_, Result<bool, DbError>> {
        self.try_locks.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Err(DbError::Database(sea_orm::DbErr::Custom(
                "the lock query is not a number".to_owned(),
            )))
        })
    }

    fn check(&self) -> BoxFuture<'_, Result<(), DbError>> {
        Box::pin(async { Ok(()) })
    }

    fn unlock(&self) -> BoxFuture<'_, Result<(), DbError>> {
        Box::pin(async { Ok(()) })
    }
}

/// A lock session that leads normally and whose unlock fails, like a shutdown
/// where the server killed the session first.
#[derive(Debug)]
pub struct FailingUnlockLock {
    inner: AdvisoryLock,
}

impl FailingUnlockLock {
    /// A lock that leads normally and cannot unlock.
    #[must_use]
    pub fn new(db: &TestDb) -> Self {
        Self {
            inner: AdvisoryLock::new(db.url(), DbSettings::default()),
        }
    }
}

impl LockSession for FailingUnlockLock {
    fn try_lock(&self) -> BoxFuture<'_, Result<bool, DbError>> {
        self.inner.try_lock()
    }

    fn check(&self) -> BoxFuture<'_, Result<(), DbError>> {
        self.inner.check()
    }

    fn unlock(&self) -> BoxFuture<'_, Result<(), DbError>> {
        Box::pin(async move {
            // Release the lock for real first, so the test can prove the worker
            // logged the failure and let go anyway.
            self.inner.unlock().await?;
            Err(DbError::Database(sea_orm::DbErr::Custom(
                "unlock failed".to_owned(),
            )))
        })
    }
}

/// A processor that answers a script of outcomes, then an empty queue.
#[derive(Debug)]
pub struct ScriptedProcessor {
    outcomes: Arc<Mutex<VecDeque<ProcessOutcome>>>,
    claims: Arc<AtomicUsize>,
}

impl ScriptedProcessor {
    /// A processor that answers `retry_counts` as requeued Reports, then `None`.
    #[must_use]
    pub fn requeued(retry_counts: impl IntoIterator<Item = i32>) -> Self {
        let outcomes = retry_counts
            .into_iter()
            .map(|retry_count| ProcessOutcome {
                report_id: 1,
                status: ReportStatus::Pending,
                run_id: None,
                counts: None,
                touched_test_ids: Vec::new(),
                error: Some("OperationalError: db blip".to_owned()),
                touched_job_ids: Vec::new(),
                tier_changes: Vec::<TierChange>::new(),
                retry_count,
            })
            .collect();
        Self {
            outcomes: Arc::new(Mutex::new(outcomes)),
            claims: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// A processor that always answers an empty queue.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            outcomes: Arc::new(Mutex::new(VecDeque::new())),
            claims: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// How many claims the worker has made.
    #[must_use]
    pub fn claims(&self) -> usize {
        self.claims.load(Ordering::SeqCst)
    }
}

impl ReportProcessor for ScriptedProcessor {
    fn process_next(
        &self,
        _pool: DatabaseConnection,
        _settings: ProcessingSettings,
    ) -> BoxFuture<'static, Result<Option<ProcessOutcome>, DbError>> {
        let claims = Arc::clone(&self.claims);
        let outcomes = Arc::clone(&self.outcomes);
        Box::pin(async move {
            claims.fetch_add(1, Ordering::SeqCst);
            let next = outcomes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front();
            Ok(next)
        })
    }
}

/// A processor that fails once, then processes for real.
#[derive(Debug)]
pub struct FailingOnceProcessor {
    calls: AtomicUsize,
}

impl Default for FailingOnceProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl FailingOnceProcessor {
    /// A processor that fails its first claim, then processes for real.
    #[must_use]
    pub fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

impl ReportProcessor for FailingOnceProcessor {
    fn process_next(
        &self,
        pool: DatabaseConnection,
        settings: ProcessingSettings,
    ) -> BoxFuture<'static, Result<Option<ProcessOutcome>, DbError>> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Box::pin(async {
                Err(DbError::Database(sea_orm::DbErr::Custom(
                    "unexpected loop error".to_owned(),
                )))
            })
        } else {
            Box::pin(async move { flakeradar_db::processing::process_next(&pool, &settings).await })
        }
    }
}

/// A waiter that records each delay and returns at once.
#[derive(Debug, Default)]
pub struct RecordingWaiter {
    waits: Mutex<Vec<Duration>>,
}

impl RecordingWaiter {
    /// How many waits the worker asked for, in order.
    #[must_use]
    pub fn waits(&self) -> Vec<Duration> {
        self.waits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Waiter for RecordingWaiter {
    fn wait(&self, delay: Duration) -> BoxFuture<'_, ()> {
        self.waits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(delay);
        Box::pin(async {})
    }
}

/// A wait a test closes, so a worker that has lost leadership stays put until
/// the test has looked and the standby has taken over.
///
/// While it is open it waits like [`TokioWaiter`](flakeradar::worker::TokioWaiter);
/// once closed, a wait returns only when the gate is opened again.
#[derive(Debug)]
pub struct GateWaiter {
    open: AtomicBool,
    signal: tokio::sync::Notify,
}

impl Default for GateWaiter {
    fn default() -> Self {
        Self {
            open: AtomicBool::new(true),
            signal: tokio::sync::Notify::new(),
        }
    }
}

impl GateWaiter {
    /// Stops every wait from returning until [`GateWaiter::open`].
    pub fn close(&self) {
        self.open.store(false, Ordering::SeqCst);
    }

    /// Lets waits return again, including the ones already waiting.
    pub fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
        self.signal.notify_waiters();
    }
}

impl Waiter for GateWaiter {
    fn wait(&self, delay: Duration) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            while !self.open.load(Ordering::SeqCst) {
                // Registered before the second look, so an open between the load
                // and the wait cannot be missed.
                let notified = self.signal.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.open.load(Ordering::SeqCst) {
                    return;
                }
                notified.await;
            }
            tokio::time::sleep(delay).await;
        })
    }
}

/// A prune that counts its runs and answers what the test scripted.
#[derive(Debug)]
pub struct CountingPruner {
    runs: Arc<AtomicUsize>,
    fails: bool,
}

impl CountingPruner {
    /// A prune that counts its runs and fails when `fails`.
    #[must_use]
    pub fn new(fails: bool) -> Self {
        Self {
            runs: Arc::new(AtomicUsize::new(0)),
            fails,
        }
    }

    /// How many times the worker pruned.
    #[must_use]
    pub fn runs(&self) -> usize {
        self.runs.load(Ordering::SeqCst)
    }
}

impl Pruner for CountingPruner {
    fn prune(&self, _pool: DatabaseConnection) -> BoxFuture<'static, Result<PruneResult, DbError>> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let fails = self.fails;
        Box::pin(async move {
            if fails {
                return Err(DbError::Database(sea_orm::DbErr::Custom(
                    "prune failed".to_owned(),
                )));
            }
            Ok(PruneResult::default())
        })
    }
}

/// A monotonic clock the test moves by hand.
#[derive(Debug, Default)]
pub struct ManualMonotonic {
    now: Mutex<f64>,
}

impl ManualMonotonic {
    /// Moves the clock to `seconds` from its origin.
    pub fn set(&self, seconds: f64) {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner) = seconds;
    }
}

impl MonotonicClock for ManualMonotonic {
    fn now(&self) -> f64 {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A log sink a test can read back, installed for the current thread only.
#[derive(Debug)]
pub struct CapturedLog {
    lines: Arc<Mutex<Vec<u8>>>,
    _guard: tracing::subscriber::DefaultGuard,
}

impl CapturedLog {
    /// Captures this thread's log lines until the value is dropped.
    #[must_use]
    pub fn install() -> Self {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || SharedWriter(Arc::clone(&sink)))
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        Self {
            lines,
            _guard: guard,
        }
    }

    /// The log text recorded so far.
    #[must_use]
    pub fn lines(&self) -> String {
        let guard = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&guard).into_owned()
    }

    /// The captured lines whose text holds `needle`, in log order.
    #[must_use]
    pub fn lines_holding(&self, needle: &str) -> Vec<String> {
        self.lines()
            .lines()
            .filter(|line| line.contains(needle))
            .map(str::to_owned)
            .collect()
    }

    /// The one captured line whose text holds `needle`.
    ///
    /// A test names the event by the static message only that event carries, then
    /// asserts the values beside it, so the fields are what the test checks rather
    /// than the sentence. The `tracing` fmt layer prints an event's `name:` as
    /// metadata, not as text, so the message is the handle a text capture has.
    ///
    /// # Panics
    ///
    /// Panics when no line or more than one line holds `needle`.
    #[must_use]
    #[track_caller]
    pub fn only_line_holding(&self, needle: &str) -> String {
        let found = self.lines_holding(needle);
        assert!(
            found.len() == 1,
            "expected exactly one line holding {needle:?}, got {}",
            found.len()
        );
        found.into_iter().next().expect("one line was found")
    }

    /// Runs `future` with this capture installed and answers what it logged.
    ///
    /// The subscriber is installed for the current thread and removed again before
    /// the answer is read, so a test asserts on the work of one call rather than on
    /// everything the process has logged. Where the call has an answer as well, a
    /// test asserts both: the answer is the outcome, and the lines are checked only
    /// where the event itself is the requirement — a cap on an outgoing body, or a
    /// refusal no returned value carries.
    pub async fn during(future: impl Future) -> String {
        Self::during_call(async {
            future.await;
        })
        .await
        .1
    }

    /// [`CapturedLog::during`] for a call whose answer the test asserts as well.
    pub async fn during_call<T>(future: impl Future<Output = T>) -> (T, String) {
        let log = Self::install();
        let answer = future.await;
        let lines = log.lines();
        drop(log);
        (answer, lines)
    }
}

/// An `io::Write` that appends to the shared buffer.
#[derive(Debug)]
pub struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_error| std::io::Error::other("the log sink is poisoned"))?;
        guard.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
