//! The Report worker: election, the loop, the hooks and the release order.
//!
//! Each test owns a database and states the intervals it runs on, so nothing
//! here waits for a real one. The worker's own seams — the processor, the prune,
//! the wait, the monotonic clock and the lock session — are the test doubles.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the worker does not lead, process or release"
)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use flakeradar::LeaderTask;
use flakeradar::worker::{
    AdvisoryLock, LockFuture, LockSession, ProcessFuture, PruneFuture, Pruner, ReportProcessor,
    ReportWorker, SessionFuture, StartFuture, StopFuture, WaitFuture, Waiter, WorkerSettings,
};
use flakeradar_core::TierChange;
use flakeradar_db::entities::{projects, reports};
use flakeradar_db::{
    DbError, DbSettings, FixedClock, MonotonicClock, ProcessOutcome, ProcessingSettings,
    PruneResult, WORKER_LOCK_KEY, connect,
};
use flakeradar_test_support::{TestDb, make_junit, make_project, make_report};
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, QueryOrder as _, Statement};

/// A fixed instant, so every stored timestamp is deterministic.
fn at() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-10-20T09:00:00+00:00").expect("a fixed instant")
}

/// The processor settings on the frozen server's defaults, with a stopped clock.
fn processing() -> ProcessingSettings {
    ProcessingSettings {
        clock: Arc::new(FixedClock::new(at())),
        ..ProcessingSettings::default()
    }
}

/// Intervals a test runs on: short enough to observe, never asserted on.
fn fast() -> WorkerSettings {
    WorkerSettings {
        poll: Duration::from_millis(1),
        standby: Duration::from_millis(1),
        prune_interval: Duration::from_secs(3600),
        max_backoff: Duration::from_secs(30),
    }
}

/// A worker over `db` that elects itself through `lock`.
fn worker(db: &TestDb, lock: Arc<dyn LockSession>, settings: WorkerSettings) -> ReportWorker {
    ReportWorker::new(db.connection().clone(), lock, processing(), settings)
}

/// The real advisory lock over `db`'s database.
fn lock(db: &TestDb) -> Arc<AdvisoryLock> {
    Arc::new(AdvisoryLock::new(db.url(), DbSettings::default()))
}

/// A second handle to a lock double as the trait object the worker takes.
fn shared_lock<T: LockSession + 'static>(lock: &Arc<T>) -> Arc<dyn LockSession> {
    let shared: Arc<T> = Arc::clone(lock);
    shared
}

/// A second handle to a prune double as the trait object the worker takes.
fn shared_pruner<T: Pruner + 'static>(pruner: &Arc<T>) -> Arc<dyn Pruner> {
    let shared: Arc<T> = Arc::clone(pruner);
    shared
}

/// A second handle to a processor double as the trait object the worker takes.
fn shared_processor<T: ReportProcessor + 'static>(processor: &Arc<T>) -> Arc<dyn ReportProcessor> {
    let shared: Arc<T> = Arc::clone(processor);
    shared
}

/// A second handle to a monotonic clock double the worker can carry.
fn shared_monotonic<T: MonotonicClock + 'static>(monotonic: &Arc<T>) -> Arc<dyn MonotonicClock> {
    let shared: Arc<T> = Arc::clone(monotonic);
    shared
}

/// A second handle to a waiter double as the trait object the worker takes.
fn shared_waiter<T: Waiter + 'static>(waiter: &Arc<T>) -> Arc<dyn Waiter> {
    let shared: Arc<T> = Arc::clone(waiter);
    shared
}

/// Waits for `condition` to hold, failing rather than hanging.
///
/// The deadline is a bound on a failure, not an assertion about time: every
/// condition this suite waits on is a database row or a counter the worker wrote.
async fn wait_until<F, Fut>(what: &str, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !condition().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// The statuses of every Report, ordered by id.
async fn report_statuses(db: &TestDb) -> Vec<String> {
    reports::Entity::find()
        .order_by_asc(reports::Column::Id)
        .all(db.connection())
        .await
        .expect("the Reports read")
        .into_iter()
        .map(|report| report.status)
        .collect()
}

/// Queues one passing JUnit Report for `project` at `sha`.
async fn queue(db: &TestDb, project: &projects::Model, sha: &str) {
    make_report(project, project.repo_id)
        .commit_sha(sha)
        .body(&make_junit(&[("t", "passed")]))
        .build(db.connection())
        .await
        .expect("a queued Report");
}

/// A Project in `acme/app`, created through the worker's own get-or-create.
async fn project(db: &TestDb) -> projects::Model {
    make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project")
}

/// Whether another session can take the processor lock right now.
///
/// The probe takes the lock when it is free, so it releases it again first.
async fn lock_is_free(url: &str) -> bool {
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

/// Ends the session holding the processor lock, as a server restart does.
///
/// The lock is session-scoped, so the server frees it the moment the session
/// dies; a replacement leader could take it at any moment after this.
async fn terminate_lock_session(db: &TestDb) {
    let statement = Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "SELECT pid FROM pg_locks WHERE locktype = 'advisory' AND classid = 0 \
           AND objid::bigint = $1 \
           AND database = (SELECT oid FROM pg_database WHERE datname = current_database())",
        [WORKER_LOCK_KEY.into()],
    );
    let row = db
        .connection()
        .query_one_raw(statement)
        .await
        .expect("the lock query runs")
        .expect("the lock is held");
    let pid: i32 = row.try_get("", "pid").expect("a backend id");
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT pg_terminate_backend($1)",
            [pid.into()],
        ))
        .await
        .expect("the backend is terminated");
}

/// Refuses, or again allows, new connections to the test's own database.
///
/// Ending the session that holds the lock is not enough on its own: the pool
/// opens a replacement for a connection the server closed, so the next lock
/// statement can still be answered by a fresh session and the test would race
/// the pool. With new connections refused, the replacement cannot be opened and
/// the failure is certain.
async fn refuse_new_connections(db: &TestDb, refuse: bool) {
    let row = db
        .connection()
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT current_database() AS name".to_owned(),
        ))
        .await
        .expect("the database name reads")
        .expect("a database name");
    let name: String = row.try_get("", "name").expect("a database name");
    // The server refuses to disallow connections for the database the statement
    // runs in, so the policy changes from the maintenance database.
    let admin = connect(db.maintenance_url(), DbSettings::default())
        .await
        .expect("the maintenance connection");
    let allowed = if refuse { "false" } else { "true" };
    admin
        .execute_unprepared(&format!(
            "ALTER DATABASE \"{name}\" ALLOW_CONNECTIONS {allowed}"
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
struct RecordingTask {
    name: &'static str,
    events: Arc<Mutex<Vec<String>>>,
    fails_to_stop: bool,
    fails_to_enqueue: bool,
    probe: Option<Arc<StopProbe>>,
}

impl RecordingTask {
    /// A task that succeeds at everything, writing into `events`.
    fn new(name: &'static str, events: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            name,
            events,
            fails_to_stop: false,
            fails_to_enqueue: false,
            probe: None,
        }
    }

    /// A task whose stop fails.
    fn failing_to_stop(mut self) -> Self {
        self.fails_to_stop = true;
        self
    }

    /// A task whose enqueue fails.
    fn failing_to_enqueue(mut self) -> Self {
        self.fails_to_enqueue = true;
        self
    }

    /// A task that checks whether the lock is still held when it stops.
    fn probing(mut self, probe: Arc<StopProbe>) -> Self {
        self.probe = Some(probe);
        self
    }
}

impl LeaderTask for RecordingTask {
    fn name(&self) -> &'static str {
        self.name
    }

    fn start(&self) -> StartFuture<'_> {
        Box::pin(async move {
            self.events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("{}:start", self.name));
        })
    }

    fn stop(&self) -> StopFuture<'_> {
        Box::pin(async move {
            self.events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("{}:stop", self.name));
            if let Some(probe) = &self.probe {
                probe.record().await;
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
struct StopProbe {
    url: String,
    free: Mutex<Option<bool>>,
}

impl StopProbe {
    /// A probe over `db`'s database.
    fn new(db: &TestDb) -> Self {
        Self {
            url: db.url().to_owned(),
            free: Mutex::new(None),
        }
    }

    /// Proves the lock is still held, from a session that is not the worker's.
    async fn record(&self) {
        let free = lock_is_free(&self.url).await;
        *self.free.lock().unwrap_or_else(PoisonError::into_inner) = Some(free);
    }

    /// What the probe saw, once the task has stopped.
    fn saw_free(&self) -> Option<bool> {
        *self.free.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The real lock, with a count of the elections and releases it was asked for.
#[derive(Debug)]
struct CountingLock {
    inner: AdvisoryLock,
    try_locks: AtomicUsize,
    unlocks: AtomicUsize,
    checks: AtomicUsize,
}

impl CountingLock {
    fn new(db: &TestDb) -> Self {
        Self {
            inner: AdvisoryLock::new(db.url(), DbSettings::default()),
            try_locks: AtomicUsize::new(0),
            unlocks: AtomicUsize::new(0),
            checks: AtomicUsize::new(0),
        }
    }

    fn try_locks(&self) -> usize {
        self.try_locks.load(Ordering::SeqCst)
    }

    fn unlocks(&self) -> usize {
        self.unlocks.load(Ordering::SeqCst)
    }

    fn checks(&self) -> usize {
        self.checks.load(Ordering::SeqCst)
    }
}

impl LockSession for CountingLock {
    fn try_lock(&self) -> LockFuture<'_> {
        self.try_locks.fetch_add(1, Ordering::SeqCst);
        self.inner.try_lock()
    }

    fn check(&self) -> SessionFuture<'_> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        self.inner.check()
    }

    fn unlock(&self) -> SessionFuture<'_> {
        self.unlocks.fetch_add(1, Ordering::SeqCst);
        self.inner.unlock()
    }
}

/// A lock session whose election always fails, like a lock query the server refuses.
#[derive(Debug)]
struct FailingLock {
    try_locks: AtomicUsize,
}

impl FailingLock {
    fn new() -> Self {
        Self {
            try_locks: AtomicUsize::new(0),
        }
    }

    fn try_locks(&self) -> usize {
        self.try_locks.load(Ordering::SeqCst)
    }
}

impl LockSession for FailingLock {
    fn try_lock(&self) -> LockFuture<'_> {
        self.try_locks.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Err(DbError::Database(sea_orm::DbErr::Custom(
                "the lock query is not a number".to_owned(),
            )))
        })
    }

    fn check(&self) -> SessionFuture<'_> {
        Box::pin(async { Ok(()) })
    }

    fn unlock(&self) -> SessionFuture<'_> {
        Box::pin(async { Ok(()) })
    }
}

/// A lock session that leads normally and whose unlock fails, like a shutdown
/// where the server killed the session first.
#[derive(Debug)]
struct FailingUnlockLock {
    inner: AdvisoryLock,
}

impl FailingUnlockLock {
    fn new(db: &TestDb) -> Self {
        Self {
            inner: AdvisoryLock::new(db.url(), DbSettings::default()),
        }
    }
}

impl LockSession for FailingUnlockLock {
    fn try_lock(&self) -> LockFuture<'_> {
        self.inner.try_lock()
    }

    fn check(&self) -> SessionFuture<'_> {
        self.inner.check()
    }

    fn unlock(&self) -> SessionFuture<'_> {
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
struct ScriptedProcessor {
    outcomes: Arc<Mutex<VecDeque<ProcessOutcome>>>,
    claims: Arc<AtomicUsize>,
}

impl ScriptedProcessor {
    /// A processor that answers `retry_counts` as requeued Reports, then `None`.
    fn requeued(retry_counts: impl IntoIterator<Item = i32>) -> Self {
        let outcomes = retry_counts
            .into_iter()
            .map(|retry_count| ProcessOutcome {
                report_id: 1,
                status: flakeradar_db::REPORT_PENDING.to_owned(),
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
    fn empty() -> Self {
        Self {
            outcomes: Arc::new(Mutex::new(VecDeque::new())),
            claims: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn claims(&self) -> usize {
        self.claims.load(Ordering::SeqCst)
    }
}

impl ReportProcessor for ScriptedProcessor {
    fn process_next(
        &self,
        _pool: DatabaseConnection,
        _settings: ProcessingSettings,
    ) -> ProcessFuture<'static> {
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
struct FailingOnceProcessor {
    calls: AtomicUsize,
}

impl FailingOnceProcessor {
    fn new() -> Self {
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
    ) -> ProcessFuture<'static> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Box::pin(async {
                Err(DbError::Database(sea_orm::DbErr::Custom(
                    "unexpected loop error".to_owned(),
                )))
            })
        } else {
            Box::pin(async move { flakeradar_db::process_next(&pool, &settings).await })
        }
    }
}

/// A waiter that records each delay and returns at once.
#[derive(Debug, Default)]
struct RecordingWaiter {
    waits: Mutex<Vec<Duration>>,
}

impl RecordingWaiter {
    fn waits(&self) -> Vec<Duration> {
        self.waits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Waiter for RecordingWaiter {
    fn wait(&self, delay: Duration) -> WaitFuture<'_> {
        self.waits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(delay);
        Box::pin(async {})
    }
}

/// A prune that counts its runs and answers what the test scripted.
#[derive(Debug)]
struct CountingPruner {
    runs: Arc<AtomicUsize>,
    fails: bool,
}

impl CountingPruner {
    fn new(fails: bool) -> Self {
        Self {
            runs: Arc::new(AtomicUsize::new(0)),
            fails,
        }
    }

    fn runs(&self) -> usize {
        self.runs.load(Ordering::SeqCst)
    }
}

impl Pruner for CountingPruner {
    fn prune(&self, _pool: DatabaseConnection) -> PruneFuture<'static> {
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
struct ManualMonotonic {
    now: Mutex<f64>,
}

impl ManualMonotonic {
    fn set(&self, seconds: f64) {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner) = seconds;
    }
}

impl MonotonicClock for ManualMonotonic {
    fn now(&self) -> f64 {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A log sink a test can read back, installed for the current thread only.
struct CapturedLog {
    lines: Arc<Mutex<Vec<u8>>>,
    _guard: tracing::subscriber::DefaultGuard,
}

impl CapturedLog {
    fn install() -> Self {
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

    fn lines(&self) -> String {
        let guard = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&guard).into_owned()
    }
}

/// An `io::Write` that appends to the shared buffer.
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

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

#[tokio::test]
async fn the_leader_processes_queued_reports_and_hands_them_over() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db).await;
    for sha in ["a", "b"] {
        queue(&db, &project, sha).await;
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    let task = Arc::new(RecordingTask::new("hook", Arc::clone(&events)));
    let worker = Arc::new(worker(&db, lock(&db), fast()).with_leader_task(task));
    Arc::clone(&worker).start().await;

    wait_until("both Reports are processed", || async {
        report_statuses(&db).await == ["processed", "processed"]
    })
    .await;
    worker.stop().await;

    let events = events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(events.first().map(String::as_str), Some("hook:start"));
    assert_eq!(events.last().map(String::as_str), Some("hook:stop"));
    // Both outcomes reached the hook, in the order the queue held them.
    let handed = events
        .iter()
        .filter(|event| event.contains("enqueue"))
        .count();
    assert_eq!(handed, 2, "{events:?}");
    assert!(events.contains(&"hook:stop".to_owned()), "{events:?}");
    assert!(!worker.is_leader());
}

#[tokio::test]
async fn a_single_leader_processes_and_a_standby_takes_over() {
    let db = TestDb::new().await.expect("a test database");
    let first_lock = Arc::new(CountingLock::new(&db));
    let second_lock = Arc::new(CountingLock::new(&db));
    let first = Arc::new(worker(&db, shared_lock(&first_lock), fast()));
    let second = Arc::new(worker(&db, shared_lock(&second_lock), fast()));

    Arc::clone(&first).start().await;
    wait_until("the first worker leads", || async { first.is_leader() }).await;
    Arc::clone(&second).start().await;
    // A standby that has tried at least twice is really on standby, and it lost
    // both elections: the first worker still leads.
    wait_until("the standby has tried the lock", || async {
        second_lock.try_locks() >= 2
    })
    .await;
    assert!(first.is_leader());
    assert!(!second.is_leader(), "only one process may lead");

    first.stop().await;
    wait_until("the standby takes over", || async { second.is_leader() }).await;
    second.stop().await;
    wait_until("the lock is free", || async {
        lock_is_free(db.url()).await
    })
    .await;
}

#[tokio::test]
async fn stopping_releases_the_lock() {
    let db = TestDb::new().await.expect("a test database");
    let worker = Arc::new(worker(&db, lock(&db), fast()));
    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;
    assert!(!lock_is_free(db.url()).await, "the leader holds the lock");

    worker.stop().await;

    assert!(!worker.is_leader());
    wait_until("the lock is free after the stop", || async {
        lock_is_free(db.url()).await
    })
    .await;
}

#[tokio::test]
async fn a_failing_unlock_is_logged_and_the_lock_is_let_go() {
    let db = TestDb::new().await.expect("a test database");
    let lock = Arc::new(FailingUnlockLock::new(&db));
    let worker = Arc::new(worker(&db, shared_lock(&lock), fast()));
    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;

    let log = CapturedLog::install();
    worker.stop().await;

    assert!(!worker.is_leader());
    assert!(
        log.lines()
            .contains("the lock connection is broken; it has been dropped"),
        "the failed unlock is logged: {}",
        log.lines()
    );
    wait_until("the lock is free", || async {
        lock_is_free(db.url()).await
    })
    .await;
}

#[tokio::test]
async fn a_broken_lock_connection_drops_leadership_and_re_elects() {
    let db = TestDb::new().await.expect("a test database");
    let counted = Arc::new(CountingLock::new(&db));
    let worker = Arc::new(worker(&db, shared_lock(&counted), fast()));
    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;

    // Kill the session that holds the lock: the server drops the lock with it,
    // so a replacement leader could take over at any moment.
    // Refuse new connections before ending the session: the pool replaces a
    // connection the server closed, and a replacement opened before the refusal
    // would answer the check and keep the worker leading.
    refuse_new_connections(&db, true).await;
    terminate_lock_session(&db).await;

    // The next empty-queue check cannot be answered, so the worker releases and
    // re-elects once connections are allowed again; stopping then still ends with
    // no lock held.
    wait_until("leadership was released after the break", || async {
        counted.unlocks() >= 1
    })
    .await;
    assert!(!worker.is_leader(), "a broken session never keeps leading");
    refuse_new_connections(&db, false).await;
    wait_until("the worker re-elects", || async {
        worker.is_leader() && counted.try_locks() >= 2
    })
    .await;
    worker.stop().await;
    assert!(!worker.is_leader());
    wait_until("the lock is free", || async {
        lock_is_free(db.url()).await
    })
    .await;
}

#[tokio::test]
async fn a_failed_lock_statement_drops_the_session() {
    // A session the server closed has already lost the lock, so a failed lock
    // statement must end that session: a worker that kept using it would go on
    // believing it leads on a connection that no longer holds the lock.
    let db = TestDb::new().await.expect("a test database");
    let session = AdvisoryLock::new(db.url(), DbSettings::default());
    assert!(
        session.try_lock().await.expect("the first election"),
        "the lock is free at the start"
    );
    // Refuse new connections first, so the pool cannot replace the session this
    // statement is about to run on.
    refuse_new_connections(&db, true).await;
    terminate_lock_session(&db).await;

    session
        .try_lock()
        .await
        .expect_err("a statement on the dead session fails");
    refuse_new_connections(&db, false).await;
    // The next election opens a fresh session, which wins the lock the server
    // freed with the old one; a reused broken session would fail again.
    assert!(
        session.try_lock().await.expect("the re-election"),
        "the session was replaced"
    );
    session.unlock().await.expect("the lock is released");
    wait_until("the lock is free", || async {
        lock_is_free(db.url()).await
    })
    .await;
}

#[tokio::test]
async fn stopping_a_worker_that_never_started_is_a_no_op() {
    let db = TestDb::new().await.expect("a test database");
    let worker = Arc::new(worker(&db, lock(&db), fast()));
    worker.stop().await;
    assert!(!worker.is_leader());
}

#[tokio::test]
async fn a_failed_election_is_retried() {
    let db = TestDb::new().await.expect("a test database");
    let failing = Arc::new(FailingLock::new());
    let worker = Arc::new(worker(&db, shared_lock(&failing), fast()));
    let log = CapturedLog::install();
    Arc::clone(&worker).start().await;
    wait_until("the election was retried", || async {
        failing.try_locks() >= 2
    })
    .await;
    assert!(!worker.is_leader());
    worker.stop().await;
    assert!(
        log.lines()
            .contains("Report worker error; dropping leadership and retrying"),
        "a failed election is logged: {}",
        log.lines()
    );
    assert!(
        !worker.is_leader(),
        "a failed election never makes a leader"
    );
}

#[tokio::test]
async fn a_failed_prune_is_logged_and_processing_continues() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db).await;
    queue(&db, &project, "a").await;
    let pruner = Arc::new(CountingPruner::new(true));
    let worker = Arc::new(worker(&db, lock(&db), fast()).with_pruner(shared_pruner(&pruner)));
    let log = CapturedLog::install();
    Arc::clone(&worker).start().await;

    // The prune runs before the claim, so a processed Report proves the loop
    // carried on after the prune failed.
    wait_until("the Report is processed", || async {
        report_statuses(&db).await == ["processed"]
    })
    .await;
    worker.stop().await;
    assert_eq!(pruner.runs(), 1, "the failed prune is not retried per poll");
    assert!(
        log.lines()
            .contains("Retention prune failed; will retry next interval"),
        "the failed prune is logged: {}",
        log.lines()
    );
}

#[tokio::test]
async fn processing_works_without_any_leader_task() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db).await;
    queue(&db, &project, "a").await;
    let worker = Arc::new(worker(&db, lock(&db), fast()));
    Arc::clone(&worker).start().await;
    wait_until("the Report is processed", || async {
        report_statuses(&db).await == ["processed"]
    })
    .await;
    worker.stop().await;
}

#[tokio::test]
async fn a_failing_leader_task_stop_is_logged_and_the_lock_released_anyway() {
    let db = TestDb::new().await.expect("a test database");
    let events = Arc::new(Mutex::new(Vec::new()));
    let github = Arc::new(RecordingTask::new("GitHub", Arc::clone(&events)).failing_to_stop());
    let webhook = Arc::new(RecordingTask::new("Notification", Arc::clone(&events)));
    let worker = Arc::new(
        worker(&db, lock(&db), fast())
            .with_leader_task(github)
            .with_leader_task(webhook),
    );
    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;

    let log = CapturedLog::install();
    worker.stop().await;
    assert!(
        log.lines().contains(
            "stopping the GitHub leader task failed; releasing the lock anyway: GitHub could not \
             stop"
        ),
        "the failed stop is logged: {}",
        log.lines()
    );

    assert!(!worker.is_leader());
    // The second task still stopped, in order, after the first one failed.
    let recorded = events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(recorded.contains(&"GitHub:stop".to_owned()), "{recorded:?}");
    assert!(
        recorded.contains(&"Notification:stop".to_owned()),
        "{recorded:?}"
    );
    wait_until("the lock is free despite the failed stop", || async {
        lock_is_free(db.url()).await
    })
    .await;
}

#[tokio::test]
async fn a_failing_notification_stop_is_logged_and_the_lock_released_anyway() {
    // The Notification task is the second one to stop, so a failure there must
    // not keep the lock either: a process that no longer processes must not hold it.
    let db = TestDb::new().await.expect("a test database");
    let events = Arc::new(Mutex::new(Vec::new()));
    let github = Arc::new(RecordingTask::new("GitHub", Arc::clone(&events)));
    let notification =
        Arc::new(RecordingTask::new("Notification", Arc::clone(&events)).failing_to_stop());
    let worker = Arc::new(
        worker(&db, lock(&db), fast())
            .with_leader_task(github)
            .with_leader_task(notification),
    );
    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;

    let log = CapturedLog::install();
    worker.stop().await;
    assert!(
        log.lines()
            .contains("stopping the Notification leader task failed; releasing the lock anyway"),
        "the failed stop is logged: {}",
        log.lines()
    );

    assert!(!worker.is_leader());
    wait_until("the lock is free despite the failed stop", || async {
        lock_is_free(db.url()).await
    })
    .await;
}

#[tokio::test]
async fn leader_tasks_stop_before_the_lock_is_released() {
    let db = TestDb::new().await.expect("a test database");
    let probe = Arc::new(StopProbe::new(&db));
    let task = Arc::new(
        RecordingTask::new("GitHub", Arc::new(Mutex::new(Vec::new()))).probing(Arc::clone(&probe)),
    );
    let worker = Arc::new(worker(&db, lock(&db), fast()).with_leader_task(task));
    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;

    worker.stop().await;

    // A replacement leader must never see the lock free while this process still
    // has an issue POST in flight, so the probe saw it held when the task stopped.
    assert_eq!(
        probe.saw_free(),
        Some(false),
        "the lock was released too early"
    );
    wait_until("the lock is free after the release", || async {
        lock_is_free(db.url()).await
    })
    .await;
}

#[tokio::test]
async fn a_failing_hook_does_not_stop_processing() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db).await;
    for sha in ["a", "b"] {
        queue(&db, &project, sha).await;
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    let task = Arc::new(RecordingTask::new("hook", Arc::clone(&events)).failing_to_enqueue());
    let worker = Arc::new(worker(&db, lock(&db), fast()).with_leader_task(task));
    let log = CapturedLog::install();
    Arc::clone(&worker).start().await;

    wait_until("both Reports are processed", || async {
        report_statuses(&db).await == ["processed", "processed"]
    })
    .await;
    worker.stop().await;
    assert!(
        log.lines().contains(
            "the hook leader task failed to take processed Report 2: hook could not take the \
             outcome"
        ),
        "a failed hand-over is logged with its Report: {}",
        log.lines()
    );
    let handed = events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|event| event.contains("enqueue"))
        .count();
    assert_eq!(handed, 2, "a failed hand-over is logged, not fatal");
}

#[tokio::test]
async fn an_unexpected_loop_error_drops_leadership_and_processing_goes_on() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db).await;
    queue(&db, &project, "a").await;
    let counted = Arc::new(CountingLock::new(&db));
    let worker = Arc::new(
        worker(&db, shared_lock(&counted), fast())
            .with_processor(Arc::new(FailingOnceProcessor::new())),
    );
    Arc::clone(&worker).start().await;

    wait_until("the Report is processed after the error", || async {
        report_statuses(&db).await == ["processed"]
    })
    .await;
    assert!(worker.is_leader(), "leadership was dropped and taken again");
    assert!(counted.unlocks() >= 1, "the error dropped leadership");
    assert!(counted.try_locks() >= 2, "the worker re-elected itself");
    worker.stop().await;
}

#[tokio::test]
async fn a_pending_outcome_backs_off_before_the_next_claim() {
    // Each outcome is a transient failure that requeued the Report; 99 is far
    // past the retry cap, to show the wait is bounded.
    let db = TestDb::new().await.expect("a test database");
    let processor = Arc::new(ScriptedProcessor::requeued([1, 2, 3, 99]));
    let waiter = Arc::new(RecordingWaiter::default());
    let lock = Arc::new(CountingLock::new(&db));
    let settings = WorkerSettings {
        poll: Duration::from_millis(50),
        ..fast()
    };
    let worker = Arc::new(
        worker(&db, lock, settings)
            .with_processor(shared_processor(&processor))
            .with_waiter(shared_waiter(&waiter)),
    );
    Arc::clone(&worker).start().await;

    wait_until("the scripted outcomes were claimed", || async {
        processor.claims() >= 5
    })
    .await;
    worker.stop().await;

    // Poll waits are exactly the poll interval; the backoff waits are the
    // doubling ones, capped at 30 seconds.
    let poll = Duration::from_millis(50).as_secs_f64();
    let backoffs: Vec<f64> = waiter
        .waits()
        .into_iter()
        .map(|wait| wait.as_secs_f64())
        .filter(|wait| *wait > poll && *wait <= 30.0)
        .collect();
    assert_eq!(backoffs, [0.1, 0.2, 0.4, 30.0], "{:?}", waiter.waits());
}

#[tokio::test]
async fn the_leader_prunes_once_per_interval() {
    let db = TestDb::new().await.expect("a test database");
    let pruner = Arc::new(CountingPruner::new(false));
    let processor = Arc::new(ScriptedProcessor::empty());
    let monotonic = Arc::new(ManualMonotonic::default());
    let worker = Arc::new(
        worker(&db, lock(&db), fast())
            .with_pruner(shared_pruner(&pruner))
            .with_processor(shared_processor(&processor))
            .with_monotonic(shared_monotonic(&monotonic)),
    );
    Arc::clone(&worker).start().await;

    // The first leading loop prunes, and the polls that follow do not.
    wait_until("the first prune ran", || async { pruner.runs() == 1 }).await;
    wait_until("the loop polled again", || async {
        processor.claims() >= 3
    })
    .await;
    assert_eq!(pruner.runs(), 1, "a prune ran inside the interval");

    // A clock past the interval lets the next loop prune again.
    monotonic.set(3601.0);
    wait_until("the second prune ran", || async { pruner.runs() == 2 }).await;
    worker.stop().await;
}

#[tokio::test]
async fn starting_a_running_worker_is_a_no_op() {
    let db = TestDb::new().await.expect("a test database");
    let processor = Arc::new(ScriptedProcessor::empty());
    let counted = Arc::new(CountingLock::new(&db));
    let worker = Arc::new(
        worker(&db, shared_lock(&counted), fast()).with_processor(shared_processor(&processor)),
    );
    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;
    // A second start leaves the running loop alone rather than spawning another.
    Arc::clone(&worker).start().await;
    let elections = counted.try_locks();
    wait_until("the loop polled again", || async {
        processor.claims() >= 3
    })
    .await;
    assert!(
        counted.try_locks() <= elections + 1,
        "a second start did not spawn a second leader loop"
    );
    assert!(
        format!("{worker:?}").contains("leader: true"),
        "the worker describes its leadership: {worker:?}"
    );
    worker.stop().await;
}

#[tokio::test]
async fn the_leader_checks_its_lock_connection_when_the_queue_is_empty() {
    let db = TestDb::new().await.expect("a test database");
    let counted = Arc::new(CountingLock::new(&db));
    let processor = Arc::new(ScriptedProcessor::empty());
    let worker = Arc::new(
        worker(&db, shared_lock(&counted), fast()).with_processor(shared_processor(&processor)),
    );
    Arc::clone(&worker).start().await;

    wait_until("the lock connection was checked", || async {
        processor.claims() >= 2 && counted.checks() >= 1
    })
    .await;
    worker.stop().await;
}
