//! The Report worker: election, the loop, the hooks and the release order.
//!
//! Each test owns a database and states the intervals it runs on, so nothing
//! here waits for a real one. The worker's own seams — the processor, the prune,
//! the wait, the monotonic clock and the lock session — are the test doubles.

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails loudly when the worker does not lead, process or release, \
              and one stages a leader task whose loop panics"
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use flakeradar::webhooks::WebhookWorker;
use flakeradar::worker::{
    AdvisoryLock, LeaderRun, LockFuture, LockSession, Pruner, ReportProcessor, ReportWorker,
    SessionFuture, Waiter, WorkerSettings,
};
use flakeradar_core::model::ReportStatus;
use flakeradar_core::notify::WebhookFormat;
use flakeradar_db::entities::{projects, reports};
use flakeradar_db::{
    DbError, DbSettings, clock::MonotonicClock, processing::ProcessOutcome,
    processing::ProcessingSettings,
};
use flakeradar_test_support::{
    CapturedLog, CountingLock, CountingPruner, FailingLock, FailingOnceProcessor,
    FailingUnlockLock, FixedClock, GateWaiter, ManualMonotonic, RecordingTask, RecordingWaiter,
    ScriptedProcessor, StopProbe, TestDb, database_name, end_session, lock_holder, lock_is_free,
    make_junit, make_project, make_report, set_connections_allowed, terminate_lock_session,
    wait_until,
};
use sea_orm::{DatabaseConnection, EntityTrait, QueryOrder as _};
use serde_json::Value as Json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A fixed instant, so every stored timestamp is deterministic.
fn at() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-10-20T09:00:00+00:00").expect("a fixed instant")
}

/// The processor settings on the shipped defaults, with a stopped clock.
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
async fn an_unreachable_database_drops_leadership_and_re_elects_after_recovery() {
    let db = TestDb::new().await.expect("a test database");
    let counted = Arc::new(CountingLock::new(&db));
    let worker = Arc::new(worker(&db, shared_lock(&counted), fast()));
    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;

    // Kill the session that holds the lock: the server drops the lock with it,
    // so a replacement leader could take over at any moment. New connections are
    // refused as well, which stages a database no session can reach: the lock
    // connection has nowhere left to answer from, which is the failure a server
    // restart or a network partition gives.
    //
    // The holder is read first: the refusal can make the worker's own pool fail,
    // which drops leadership and releases the lock, so a read after it would race
    // the release it is about to stage.
    let database = database_name(&db).await;
    let holder = lock_holder(&db, &database).await;
    set_connections_allowed(&db, &database, false).await;
    end_session(&db, holder).await;

    // The next empty-queue check cannot be answered, so the worker releases and
    // re-elects once connections are allowed again; stopping then still ends with
    // no lock held.
    wait_until("leadership was released after the break", || async {
        counted.unlocks() >= 1
    })
    .await;
    assert!(!worker.is_leader(), "a broken session never keeps leading");
    set_connections_allowed(&db, &database, true).await;
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
async fn a_killed_lock_session_hands_leadership_to_the_standby() {
    // The ordinary failure: the backend holding the lock dies while the server
    // keeps accepting connections, so the pool opens a replacement session that
    // holds no lock and answers a statement just the same.
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db).await;
    let first_lock = Arc::new(CountingLock::new(&db));
    let second_lock = Arc::new(CountingLock::new(&db));
    // The first worker's waits stop as soon as it loses leadership, so it cannot
    // win the lock back while the test asserts the hand-over.
    let gate = Arc::new(GateWaiter::default());
    let first = Arc::new(
        worker(&db, shared_lock(&first_lock), fast())
            .with_waiter(shared_waiter(&gate))
            .with_leader_task(Arc::new(
                RecordingTask::new("GitHub", Arc::new(Mutex::new(Vec::new())))
                    .closing_gate(Arc::clone(&gate)),
            )),
    );
    let second = Arc::new(worker(&db, shared_lock(&second_lock), fast()));

    Arc::clone(&first).start().await;
    wait_until("the first worker leads", || async { first.is_leader() }).await;
    Arc::clone(&second).start().await;
    wait_until("the standby tried the lock", || async {
        second_lock.try_locks() >= 2
    })
    .await;
    assert!(!second.is_leader(), "only one process may lead");

    // The server still accepts connections while the lock session dies.
    terminate_lock_session(&db, &database_name(&db).await).await;

    // The leader that lost its session drops leadership first; only then is the
    // lock free for the standby, so the two never lead at the same time.
    wait_until("the killed leader drops leadership", || async {
        !first.is_leader()
    })
    .await;
    wait_until("the standby takes over", || async { second.is_leader() }).await;
    assert!(
        !first.is_leader(),
        "the worker whose lock session died never keeps leading"
    );

    // The new leader is a working processor, not just a flag.
    queue(&db, &project, "a").await;
    wait_until("the new leader processes the Report", || async {
        report_statuses(&db).await == ["processed"]
    })
    .await;

    // The old leader tries again once its waits return, and finds the lock taken.
    gate.open();
    let attempts = first_lock.try_locks();
    wait_until("the old leader tries the lock again", || async {
        first_lock.try_locks() > attempts
    })
    .await;
    assert!(
        !first.is_leader(),
        "the standby holds the lock, so the old leader stays on standby"
    );

    first.stop().await;
    second.stop().await;
    assert!(!first.is_leader() && !second.is_leader());
    wait_until("the lock is free", || async {
        lock_is_free(db.url()).await
    })
    .await;
}

#[tokio::test]
async fn a_killed_lock_session_is_never_alive_again() {
    // The server keeps accepting connections here, so after the first statement
    // finds the session dead the pool opens a replacement for it. That
    // replacement answers a statement, but it holds no lock, so the check must
    // never report this process alive.
    let db = TestDb::new().await.expect("a test database");
    let session = AdvisoryLock::new(db.url(), DbSettings::default());
    assert!(
        session.try_lock().await.expect("the first election"),
        "the lock is free at the start"
    );
    terminate_lock_session(&db, &database_name(&db).await).await;

    session
        .check()
        .await
        .expect_err("the killed session cannot be alive");
    // This answer comes from the replacement, which is not the backend that took
    // the lock: the comparison with the recorded backend is what catches it.
    session
        .check()
        .await
        .expect_err("a replacement session does not hold the lock");
    session
        .unlock()
        .await
        .expect("a dropped session is not unlocked again");
    wait_until("the lock is free", || async {
        lock_is_free(db.url()).await
    })
    .await;
}

#[tokio::test]
async fn a_lock_session_that_never_won_an_election_never_checks_alive() {
    let db = TestDb::new().await.expect("a test database");
    // A session that never even tried the lock has nothing to prove.
    let untried = AdvisoryLock::new(db.url(), DbSettings::default());
    untried
        .check()
        .await
        .expect_err("a session that never tried the lock cannot be alive");
    // A session that tried and lost is a standby, and a standby does not lead.
    let holder = AdvisoryLock::new(db.url(), DbSettings::default());
    assert!(holder.try_lock().await.expect("the holder's election"));
    let standby = AdvisoryLock::new(db.url(), DbSettings::default());
    assert!(
        !standby.try_lock().await.expect("the standby's election"),
        "only one session can take the lock"
    );
    standby
        .check()
        .await
        .expect_err("a standby session does not lead");
    standby
        .unlock()
        .await
        .expect("a standby session holds no lock to release");
    holder.unlock().await.expect("the holder releases the lock");
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
    let database = database_name(&db).await;
    let session = AdvisoryLock::new(db.url(), DbSettings::default());
    assert!(
        session.try_lock().await.expect("the first election"),
        "the lock is free at the start"
    );
    // Refuse new connections first, so the pool cannot replace the session this
    // statement is about to run on, and end that session from the maintenance
    // database, which the refusal does not touch. The holder is read before the
    // refusal, because only the refusal comes between the read and the end.
    let holder = lock_holder(&db, &database).await;
    set_connections_allowed(&db, &database, false).await;
    end_session(&db, holder).await;

    let failed = session.try_lock().await;
    // The failed statement dropped the session, so ending it needs no statement
    // of its own: a session that was kept would try to unlock over a connection
    // the pool can no longer open, and report that instead of nothing.
    let ended = session.unlock().await;
    // Connections are allowed again before either answer is asserted, so a
    // failure here cannot leave a database that refuses them.
    set_connections_allowed(&db, &database, true).await;
    failed.expect_err("a statement on the dead session fails");
    ended.expect("a dropped session is not unlocked again");
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
    let line = log.only_line_holding("a leader task failed to stop");
    assert!(
        line.contains("task=\"GitHub\""),
        "the failed stop names its task: {line}"
    );
    assert!(
        line.contains("error=GitHub could not stop"),
        "the failed stop carries the reason it gave: {line}"
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
    let line = log.only_line_holding("a leader task failed to stop");
    assert!(
        line.contains("task=\"Notification\""),
        "the second task's failure is the logged one: {line}"
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
    let failures = log.lines_holding("a leader task failed to take a processed Report");
    assert_eq!(
        failures.len(),
        2,
        "one line for each Report the hand-over lost: {failures:?}"
    );
    let lost: Vec<i32> = reports::Entity::find()
        .all(db.connection())
        .await
        .expect("the Reports read")
        .into_iter()
        .map(|report| report.id)
        .collect();
    for line in &failures {
        assert!(
            line.contains("task=\"hook\""),
            "the line names its task: {line}"
        );
        assert!(
            line.contains("error=hook could not take the outcome"),
            "the line carries the reason it gave: {line}"
        );
        assert!(
            lost.iter()
                .any(|id| line.contains(&format!("report_id={id}"))),
            "the line names the Report it lost: {line}"
        );
    }
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
async fn the_leader_checks_its_lock_connection() {
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

#[tokio::test]
async fn a_tier_change_reaches_the_webhook_task() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db).await;
    for (status, sha) in [("failed", "a"), ("passed", "b")] {
        make_report(&project, project.repo_id)
            .commit_sha(sha)
            .body(&make_junit(&[("t", status)]))
            .build(db.connection())
            .await
            .expect("a queued Report");
    }
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let webhook = Arc::new(
        WebhookWorker::new(
            vec![format!("{}/hook", server.uri())],
            WebhookFormat::Json,
            "",
        )
        .expect("the HTTP client builds"),
    );
    let processing = ProcessingSettings {
        flake_threshold: 0.05,
        score_decay: 0.0,
        clock: Arc::new(FixedClock::new(at())),
        ..ProcessingSettings::default()
    };
    let worker = Arc::new(
        ReportWorker::new(db.connection().clone(), lock(&db), processing, fast())
            .with_leader_task(webhook),
    );
    Arc::clone(&worker).start().await;

    wait_until("the tier change is delivered", || async {
        server
            .received_requests()
            .await
            .is_some_and(|requests| requests.len() == 1)
    })
    .await;
    worker.stop().await;

    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert_eq!(requests.len(), 1);
    let body: Json = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    let events: Vec<&str> = body
        .as_array()
        .expect("the payload is a list")
        .iter()
        .map(|event| event["event"].as_str().expect("an event name"))
        .collect();
    assert_eq!(events, vec!["test.flaky"]);
    assert!(
        lock_is_free(db.url()).await,
        "the Notification task stopped before the lock was released"
    );
}

/// A processor that answers one claimed Report after another, an endless backlog.
#[derive(Debug)]
struct AlwaysSomeProcessor {
    claims: Arc<AtomicUsize>,
}

impl AlwaysSomeProcessor {
    /// A processor with a backlog that never drains.
    #[must_use]
    fn new() -> Self {
        Self {
            claims: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// How many claims the worker made.
    #[must_use]
    fn claims(&self) -> usize {
        self.claims.load(Ordering::SeqCst)
    }
}

impl ReportProcessor for AlwaysSomeProcessor {
    fn process_next(
        &self,
        _pool: DatabaseConnection,
        _settings: ProcessingSettings,
    ) -> flakeradar::worker::ProcessFuture<'static> {
        let claims = Arc::clone(&self.claims);
        Box::pin(async move {
            claims.fetch_add(1, Ordering::SeqCst);
            Ok(Some(ProcessOutcome {
                report_id: 1,
                status: ReportStatus::Pending,
                run_id: None,
                counts: None,
                touched_test_ids: Vec::new(),
                error: None,
                touched_job_ids: Vec::new(),
                tier_changes: Vec::new(),
                retry_count: 0,
            }))
        })
    }
}

/// A processor whose every claim fails at the database, a persistent outage.
#[derive(Debug, Default)]
struct AlwaysErrProcessor;

impl ReportProcessor for AlwaysErrProcessor {
    fn process_next(
        &self,
        _pool: DatabaseConnection,
        _settings: ProcessingSettings,
    ) -> flakeradar::worker::ProcessFuture<'static> {
        Box::pin(async {
            Err(DbError::Database(sea_orm::DbErr::Custom(
                "a persistent processing failure".to_owned(),
            )))
        })
    }
}

/// A lock session that elects at once and fails every liveness check.
#[derive(Debug, Default)]
struct DeadLock {
    try_locks: AtomicUsize,
    checks: AtomicUsize,
}

impl DeadLock {
    /// A session whose check always fails, like a lock connection that died.
    #[must_use]
    fn new() -> Self {
        Self::default()
    }

    /// How many liveness checks were asked for.
    #[must_use]
    fn checks(&self) -> usize {
        self.checks.load(Ordering::SeqCst)
    }
}

impl LockSession for DeadLock {
    fn try_lock(&self) -> LockFuture<'_> {
        self.try_locks.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(true) })
    }

    fn check(&self) -> SessionFuture<'_> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(DbError::Database(sea_orm::DbErr::Custom(
                "the lock connection died".to_owned(),
            )))
        })
    }

    fn unlock(&self) -> SessionFuture<'_> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn a_leader_with_a_backlog_checks_its_lock_between_reports() {
    let db = TestDb::new().await.expect("a test database");
    let processor = Arc::new(AlwaysSomeProcessor::new());
    let lock = Arc::new(DeadLock::new());
    let worker = Arc::new(
        worker(&db, shared_lock(&lock), fast()).with_processor(shared_processor(&processor)),
    );
    Arc::clone(&worker).start().await;

    // The lock's check runs on a backlog as well as on an empty queue, so a
    // leader whose lock connection died stops claiming at once instead of
    // draining Reports alongside the standby that took the lock.
    wait_until("the failing lock check stopped the claims", || async {
        lock.checks() >= 1 && processor.claims() == 0
    })
    .await;
    worker.stop().await;
}

#[tokio::test]
async fn a_persistent_process_error_backs_off_up_to_the_cap() {
    let db = TestDb::new().await.expect("a test database");
    let processor = Arc::new(AlwaysErrProcessor);
    let waiter = Arc::new(RecordingWaiter::default());
    let lock = Arc::new(CountingLock::new(&db));
    let settings = WorkerSettings {
        poll: Duration::from_millis(1),
        standby: Duration::from_millis(1),
        prune_interval: Duration::from_secs(3600),
        max_backoff: Duration::from_millis(8),
    };
    let worker = Arc::new(
        worker(&db, lock, settings)
            .with_processor(shared_processor(&processor))
            .with_waiter(shared_waiter(&waiter)),
    );
    Arc::clone(&worker).start().await;

    wait_until("the backoff reached its cap", || async {
        waiter
            .waits()
            .iter()
            .any(|wait| *wait >= settings.max_backoff)
    })
    .await;
    worker.stop().await;

    // The consecutive failures wait `poll * 2^n`, so the waits grow up to the
    // cap instead of staying at one poll.
    let waits: Vec<Duration> = waiter.waits();
    assert!(
        waits.windows(2).all(|pair| pair[1] >= pair[0]),
        "the waits grow, not stay flat: {waits:?}"
    );
    assert!(
        waits.contains(&settings.max_backoff),
        "a wait reaches the cap: {waits:?}"
    );
}

#[tokio::test]
async fn a_run_whose_loop_panicked_is_reported_when_it_is_stopped() {
    let run = LeaderRun::new();
    run.start(|stop| async move {
        stop.cancelled().await;
        panic!("the test's loop panicked on purpose");
    })
    .await;
    assert!(run.is_running(), "the run is going until it is stopped");

    let answer = run.stop().await;
    assert!(
        answer.is_err(),
        "the panic is reported rather than hidden: {answer:?}"
    );
    assert!(!run.is_running(), "a stopped run is not going");
}
