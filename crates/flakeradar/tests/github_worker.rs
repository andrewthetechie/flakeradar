//! The leader-only GitHub task: it drains the queue and syncs on its interval.
//!
//! The GitHub API is a stub server and the sync interval is read from a scripted
//! clock, so nothing waits on a real interval. A call the test needs to hold open is staged by a stub
//! answer that signals when it arrived and waits to be released.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a stub or a task does not behave"
)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chrono::{FixedOffset, TimeZone as _, Utc};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use flakeradar::worker::{
    AdvisoryLock, LockSession, ReportProcessor, ReportWorker, WorkerSettings,
};
use flakeradar::{BoxFuture, github::Api, settings::Secret, settings::Settings};
use flakeradar::{github_worker::GitHubWorker, worker::LeaderTask};
use flakeradar_core::model::ReportStatus;
use flakeradar_db::entities::{projects, reports};
use flakeradar_db::{
    DbError, DbSettings, WORKER_LOCK_KEY, connect, connect_session, processing::ProcessOutcome,
    processing::ProcessingSettings,
};
use flakeradar_test_support::{
    CapturedLog, FixedClock, ScriptedMonotonic, TestDb, Unanswered, app, database_name, dropped,
    end_session, lock_holder, lock_holder_pid, lock_is_free, make_job, make_junit, make_pipeline,
    make_project, make_report, make_test_case, post, queued, until_finished, wait_until, within,
};
use sea_orm::ConnectionTrait as _;
use sea_orm::{
    DatabaseConnection, DbBackend, DbErr, EntityTrait as _, QueryOrder as _, Statement,
    TransactionTrait as _, Value,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// The GitHub token the stub expects.
const TOKEN: &str = "ghs_testtoken";

/// The settings the task runs with.
fn settings() -> Settings {
    let map = [
        ("FLAKERADAR_GITHUB_TOKEN", TOKEN),
        ("FLAKERADAR_GITHUB_ISSUE_MIN_SCORE", "0.3"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect();
    Settings::from_map(&map).expect("the test's settings are valid")
}

/// How many requests the stub has recorded.
async fn recorded(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .map_or(0, |requests| requests.len())
}

/// One flaky Test in `acme/app`.
async fn flaky_test(db: &TestDb) -> i32 {
    flaky_test_in(db, "backend").await
}

/// One flaky Test in `acme/app`, in the Project `project`.
async fn flaky_test_in(db: &TestDb, project: &str) -> i32 {
    let project = make_project()
        .repo("acme/app")
        .project(project)
        .create(db)
        .await;
    let test = make_test_case(&project)
        .name("test_total")
        .flakiness_score(0.5)
        .create(db)
        .await;
    test.id
}

/// One flaky github-provider Job in `acme/app`.
async fn flaky_job(db: &TestDb) -> i32 {
    let pipeline = make_pipeline()
        .repo("acme/app")
        .provider("github")
        .create(db)
        .await;
    make_job(&pipeline)
        .name("unit")
        .flakiness_score(0.5)
        .create(db)
        .await
        .id
}

/// A processed Report outcome for `tests`.
fn processed(tests: &[i32]) -> ProcessOutcome {
    ProcessOutcome {
        report_id: 1,
        status: ReportStatus::Processed,
        run_id: Some(1),
        counts: None,
        touched_test_ids: tests.to_vec(),
        error: None,
        touched_job_ids: Vec::new(),
        tier_changes: Vec::new(),
        retry_count: 0,
    }
}

#[tokio::test]
async fn a_queued_outcome_is_filed_in_the_background() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 7})))
        .mount(&server)
        .await;
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    );
    LeaderTask::start(&worker).await;

    LeaderTask::enqueue(&worker, &processed(&[test])).expect("the queue takes the outcome");
    until_finished(worker.queue(), 1).await;
    let requests = server.received_requests().await.expect("a record");
    assert_eq!(requests[0].method.as_str(), "POST");
    assert_eq!(requests[0].url.path(), "/repos/acme/app/issues");
    LeaderTask::stop(&worker).await.expect("the task stops");

    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT github_issue_number FROM test_cases WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the row reads")
        .expect("the Test exists");
    assert_eq!(
        row.try_get::<Option<i32>>("", "github_issue_number")
            .expect("a number"),
        Some(7)
    );
}

#[tokio::test]
async fn the_first_sync_runs_at_start_and_the_next_one_on_the_interval() {
    let db = TestDb::new().await.expect("a test database");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "open"})))
        .mount(&server)
        .await;
    // One linked Test, so a sync has something to ask about.
    let test = flaky_test(&db).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_number = 12 WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the link writes");

    // The clock is scripted so the interval is crossed without waiting for it: the
    // first three readings say "just synced", and the fourth is past the interval.
    let monotonic = Arc::new(ScriptedMonotonic::new([0.0, 0.0, 0.0, 100.0]));
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_millis(60),
    )
    .with_monotonic(monotonic);
    LeaderTask::start(&worker).await;
    // A sync is not a queue batch, so the stub's own record of the `GET`s is the
    // only thing that says one happened.
    wait_until("the first sync asks about the link", || async {
        recorded(&server).await >= 1
    })
    .await;
    wait_until("the interval's sync asks again", || async {
        recorded(&server).await >= 2
    })
    .await;
    LeaderTask::stop(&worker).await.expect("the task stops");
}

#[tokio::test]
async fn the_sync_stamps_a_checked_link_with_the_tasks_clock() {
    let db = TestDb::new().await.expect("a test database");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "open"})))
        .mount(&server)
        .await;
    let test = flaky_test(&db).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_number = 12 WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the link writes");
    let instant = Utc
        .with_ymd_and_hms(2001, 2, 3, 4, 5, 6)
        .unwrap()
        .fixed_offset();

    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    )
    .with_clock(Arc::new(FixedClock::new(instant)));
    LeaderTask::start(&worker).await;
    wait_until("the sync stamps the link", || async {
        checked_at(&db, test).await.is_some()
    })
    .await;
    LeaderTask::stop(&worker).await.expect("the task stops");

    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT github_issue_checked_at FROM test_cases WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the stamp reads")
        .expect("the Test exists");
    assert_eq!(
        row.try_get::<Option<chrono::DateTime<FixedOffset>>>("", "github_issue_checked_at")
            .expect("a timestamp"),
        Some(instant),
        "the task's own sync stamped the clock it was given"
    );
}

#[tokio::test]
async fn an_outcome_with_no_touched_ids_files_nothing() {
    let db = TestDb::new().await.expect("a test database");
    let server = MockServer::start().await;
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        // A long interval keeps the start-up sync out of the way, and the sync
        // finds no linked row to check anyway.
        Duration::from_secs(3600),
    );
    LeaderTask::start(&worker).await;
    LeaderTask::enqueue(&worker, &processed(&[])).expect("the queue takes the outcome");
    until_finished(worker.queue(), 1).await;
    LeaderTask::stop(&worker).await.expect("the task stops");
    assert!(
        server
            .received_requests()
            .await
            .expect("a record")
            .is_empty()
    );
}

#[tokio::test]
async fn the_task_names_itself_and_its_debug_line_carries_no_token() {
    let db = TestDb::new().await.expect("a test database");
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), "http://127.0.0.1:1").expect("the HTTP client builds"),
        Duration::from_secs(3600),
    );
    assert_eq!(LeaderTask::name(&worker), "GitHub");
    let debug = format!("{worker:?}");
    assert!(debug.contains("GitHubWorker"), "{debug}");
    assert!(
        debug.contains("sync_interval"),
        "the shared state's interval is formatted: {debug}"
    );
    assert!(
        !debug.contains(TOKEN),
        "the Debug line names no token: {debug}"
    );
}

#[tokio::test]
async fn stopping_a_task_that_never_started_is_fine() {
    let db = TestDb::new().await.expect("a test database");
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), "http://127.0.0.1:1").expect("the HTTP client builds"),
        Duration::from_secs(3600),
    );
    LeaderTask::stop(&worker).await.expect("nothing to stop");

    let worker = Arc::new(worker);
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
}

#[tokio::test]
async fn a_failing_github_call_never_stops_the_task() {
    let db = TestDb::new().await.expect("a test database");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    );
    let test = flaky_test(&db).await;
    LeaderTask::start(&worker).await;
    LeaderTask::enqueue(&worker, &processed(&[test])).expect("the queue takes the outcome");
    until_finished(worker.queue(), 1).await;
    // A second outcome still goes out, so the task survived the failed one.
    LeaderTask::enqueue(&worker, &processed(&[test])).expect("the queue takes the outcome");
    until_finished(worker.queue(), 2).await;
    LeaderTask::stop(&worker).await.expect("the task stops");
}

// --- leadership, the release order and a call held in flight --------------

/// The worker intervals a leadership test runs on: short enough to observe,
/// never asserted on.
fn worker_settings() -> WorkerSettings {
    WorkerSettings {
        poll: Duration::from_millis(1),
        standby: Duration::from_millis(1),
        prune_interval: Duration::from_secs(3600),
        max_backoff: Duration::from_secs(30),
    }
}

/// A `ReportWorker` over `db` that leads through the real advisory lock and
/// carries `github` as its leader task.
fn leader_worker(db: &TestDb, github: Arc<GitHubWorker>) -> Arc<ReportWorker> {
    leader_worker_over(db, github as Arc<dyn LeaderTask>)
}

/// A `ReportWorker` over `db` that leads through the real advisory lock and
/// carries `task` as its leader task.
fn leader_worker_over(db: &TestDb, task: Arc<dyn LeaderTask>) -> Arc<ReportWorker> {
    let lock: Arc<dyn LockSession> = Arc::new(AdvisoryLock::new(db.url(), DbSettings::default()));
    Arc::new(
        ReportWorker::new(
            db.connection().clone(),
            lock,
            ProcessingSettings::default(),
            worker_settings(),
        )
        .with_leader_task(task),
    )
}

/// One Test's stored issue-check stamp.
async fn checked_at(db: &TestDb, id: i32) -> Option<chrono::DateTime<FixedOffset>> {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT github_issue_checked_at FROM test_cases WHERE id = $1",
            [Value::from(id)],
        ))
        .await
        .expect("the stamp reads")
        .expect("the Test exists");
    row.try_get("", "github_issue_checked_at")
        .expect("the stamp is a timestamp")
}

/// A lock session that counts the attempts to take the processor lock.
///
/// A stand-by worker never asks for a Report, so its lock attempts are the only
/// thing its loop does that a test can see; counting them is how a test knows the
/// worker ran rather than assuming it did.
#[derive(Debug)]
struct CountingLock {
    inner: Arc<dyn LockSession>,
    attempts: AtomicUsize,
}

impl CountingLock {
    fn new(db: &TestDb) -> Self {
        Self {
            inner: Arc::new(AdvisoryLock::new(db.url(), DbSettings::default())),
            attempts: AtomicUsize::new(0),
        }
    }

    /// How many times the worker has tried the lock.
    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
}

impl LockSession for CountingLock {
    fn try_lock(&self) -> BoxFuture<'_, Result<bool, DbError>> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        self.inner.try_lock()
    }

    fn check(&self) -> BoxFuture<'_, Result<(), DbError>> {
        self.inner.check()
    }

    fn unlock(&self) -> BoxFuture<'_, Result<(), DbError>> {
        self.inner.unlock()
    }
}

/// A GitHub task that says when a stop has reached it.
///
/// `LeaderTask::stop` is where the release order is decided: the worker that
/// stops its tasks before it unlocks calls it while the task's own call is still
/// in flight. A test that has to look at that window waits on this signal rather
/// than on a sleep, so the window is a fact rather than a hope.
#[derive(Debug)]
struct Announced {
    inner: Arc<GitHubWorker>,
    stopping: tokio::sync::Notify,
    seen: AtomicBool,
}

impl Announced {
    fn new(inner: Arc<GitHubWorker>) -> Self {
        Self {
            inner,
            stopping: tokio::sync::Notify::new(),
            seen: AtomicBool::new(false),
        }
    }

    /// Whether the release has asked the task to stop.
    fn seen(&self) -> bool {
        self.seen.load(Ordering::SeqCst)
    }

    /// Waits until the release has asked the task to stop.
    async fn announced(&self) {
        let waiting = self.stopping.notified();
        tokio::pin!(waiting);
        waiting.as_mut().enable();
        if self.seen() {
            return;
        }
        waiting.await;
    }
}

impl LeaderTask for Announced {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn start(&self) -> BoxFuture<'_, ()> {
        self.inner.start()
    }

    fn stop(&self) -> BoxFuture<'_, anyhow::Result<()>> {
        self.seen.store(true, Ordering::SeqCst);
        self.stopping.notify_waiters();
        self.inner.stop()
    }

    fn enqueue(&self, outcome: &ProcessOutcome) -> anyhow::Result<()> {
        self.inner.enqueue(outcome)
    }
}

/// Restarts the server under `db`, as far as a client can tell: every backend on
/// the database dies at once, which is what a restarted PostgreSQL hands its
/// clients. The lock session goes with them, so the worker has to earn leadership
/// again from a new session, and every pooled connection has to be replaced.
///
/// Answers how many sessions the restart ended. This is the reachable form of a
/// `docker restart`: a CI job's `postgres` service container cannot be restarted
/// from inside the test process without stopping the tests running beside it.
async fn restart(db: &TestDb) -> i64 {
    let database = database_name(db).await;
    let admin = connect(db.maintenance_url(), DbSettings::default())
        .await
        .expect("the maintenance connection");
    let row = admin
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(pg_terminate_backend(pid)) AS ended FROM pg_stat_activity \
             WHERE datname = $1 AND pid <> pg_backend_pid()",
            [database.into()],
        ))
        .await
        .expect("the terminate query runs")
        .expect("the terminate query answers");
    let ended: i64 = row.try_get("", "ended").expect("a count");
    admin
        .close()
        .await
        .expect("the maintenance connection closes");
    ended
}

/// How many issue reads the stub has answered, which is what a sync does.
async fn issue_reads(server: &MockServer) -> usize {
    requested_calls(server)
        .await
        .iter()
        .filter(|(method, path)| method == "GET" && path.starts_with("/repos/acme/app/issues/"))
        .count()
}

/// A claim on the processor lock, queued on a session of the test's own.
///
/// `pg_advisory_lock` blocks on the session until the server grants it, so a claim
/// queued before the holder's session dies is granted the instant the lock is free.
/// That puts it ahead of a worker which must first find the loss, stop its task,
/// release and wait a poll before trying again, so the server's queue — not elapsed
/// time — decides who holds the lock next.
#[derive(Debug)]
struct LockClaim {
    waiting: tokio::task::JoinHandle<(DatabaseConnection, Result<(), DbErr>)>,
}

impl LockClaim {
    /// Queues for the processor lock, on a session of its own.
    async fn queue(db: &TestDb) -> Self {
        let session = connect_session(db.url(), DbSettings::default())
            .await
            .expect("the claim's own session");
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_lock($1)",
            [WORKER_LOCK_KEY.into()],
        );
        let waiting = tokio::spawn(async move {
            let answer = session.query_one_raw(statement).await.map(|_row| ());
            (session, answer)
        });
        Self { waiting }
    }

    /// Waits until the server grants the claim, then answers its session.
    async fn granted(self) -> DatabaseConnection {
        let (session, answer) = self.waiting.await.expect("the claim's task finishes");
        answer.expect("the claim's lock statement answers");
        session
    }
}

/// Releases the claim's lock and closes its session.
async fn release_lock(session: DatabaseConnection) {
    session
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_unlock($1)",
            [WORKER_LOCK_KEY.into()],
        ))
        .await
        .expect("the claim releases the lock");
    session.close().await.expect("the claim's session closes");
}

/// A linked Test in `acme/app` at issue `number`, so a sync has work to do.
async fn linked_test(db: &TestDb, number: i32) -> i32 {
    linked_test_in(db, "acme/app", number).await
}

/// A linked Test in the Repo `repo` at issue `number`, so a sync has work to do.
async fn linked_test_in(db: &TestDb, repo: &str, number: i32) -> i32 {
    let project = make_project().repo(repo).project("linked").create(db).await;
    make_test_case(&project)
        .name("test_linked")
        .flakiness_score(0.5)
        .github_issue_number(number)
        .create(db)
        .await
        .id
}

/// A linked Test at the fixed-note threshold, so the note step has work to do.
async fn noteable_test(db: &TestDb, number: i32, streak: i32) -> i32 {
    noteable_test_in(db, "acme/app", number, streak).await
}

/// The same, in the Repo `repo`.
async fn noteable_test_in(db: &TestDb, repo: &str, number: i32, streak: i32) -> i32 {
    let project = make_project()
        .repo(repo)
        .project("noteable")
        .create(db)
        .await;
    make_test_case(&project)
        .name("test_noteable")
        .flakiness_score(0.5)
        .clean_streak(streak)
        .github_issue_number(number)
        .create(db)
        .await
        .id
}

/// The settings a fixed-note run uses, with the streak the tests stage.
fn note_settings() -> Settings {
    let mut map = std::collections::BTreeMap::new();
    map.insert("FLAKERADAR_GITHUB_TOKEN".to_owned(), TOKEN.to_owned());
    map.insert("FLAKERADAR_GITHUB_FIXED_STREAK".to_owned(), "3".to_owned());
    Settings::from_map(&map).expect("the test's settings are valid")
}

/// The stored issue number of one Test.
async fn stored_number(db: &TestDb, id: i32) -> Option<i32> {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT github_issue_number FROM test_cases WHERE id = $1",
            [Value::from(id)],
        ))
        .await
        .expect("the row reads")
        .expect("the Test exists");
    row.try_get("", "github_issue_number").expect("a number")
}

/// Whether one Test carries the fixed-note stamp.
async fn noted(db: &TestDb, id: i32) -> bool {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT github_issue_fixed_noted_at IS NOT NULL AS noted FROM test_cases WHERE id = $1",
            [Value::from(id)],
        ))
        .await
        .expect("the row reads")
        .expect("the Test exists");
    row.try_get("", "noted").expect("noted")
}

/// The method and path of every request a stub server was asked for.
async fn requested_calls(server: &MockServer) -> Vec<(String, String)> {
    server
        .received_requests()
        .await
        .expect("the stub records requests")
        .iter()
        .map(|request| {
            (
                request.method.as_str().to_owned(),
                request.url.path().to_owned(),
            )
        })
        .collect()
}

/// The comment requests a stub server answered, whatever the issue number.
async fn comment_requests(server: &MockServer) -> usize {
    requested_calls(server)
        .await
        .into_iter()
        .filter(|(method, path)| method == "POST" && path.ends_with("/comments"))
        .count()
}

/// Mounts `GET /repos/acme/app/issues/{number}` answering that the issue is open.
async fn mount_open_issue(server: &MockServer, number: i32) {
    Mock::given(method("GET"))
        .and(path(format!("/repos/acme/app/issues/{number}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "open"})))
        .mount(server)
        .await;
}

/// A stub answer that signals its request as in flight and waits to be released.
///
/// Holding a GitHub call open this way keeps "while a call is in flight"
/// deterministic: no part of the test waits on elapsed time.
struct GatedAnswer {
    arrived: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    released: AtomicBool,
    status: u16,
    body: serde_json::Value,
}

impl GatedAnswer {
    /// A gate answering with an issue number, as a create or a comment does.
    fn new(
        arrived: tokio::sync::oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        status: u16,
        number: i32,
    ) -> Self {
        Self::answering(arrived, release, status, json!({ "number": number }))
    }

    /// A gate answering with an issue state, as a sync's `GET` does.
    fn state(
        arrived: tokio::sync::oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        state: &str,
    ) -> Self {
        Self::answering(arrived, release, 200, json!({ "state": state }))
    }

    /// A gate answering `body` with `status`.
    fn answering(
        arrived: tokio::sync::oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        status: u16,
        body: serde_json::Value,
    ) -> Self {
        Self {
            arrived: Mutex::new(Some(arrived)),
            release: Mutex::new(release),
            released: AtomicBool::new(false),
            status,
            body,
        }
    }
}

impl wiremock::Respond for GatedAnswer {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let arrived = self
            .arrived
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(arrived) = arrived {
            let _ = arrived.send(());
        }
        if !self.released.load(Ordering::SeqCst) {
            let _ = self
                .release
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .recv_timeout(Duration::from_secs(30));

            self.released.store(true, Ordering::SeqCst);
        }
        ResponseTemplate::new(self.status).set_body_json(self.body.clone())
    }
}

/// Queues one flaky JUnit Report for `project` at `sha`.
async fn queue_flaky(db: &TestDb, project: &projects::Model, sha: &str) {
    make_report(project, project.repo_id)
        .commit_sha(sha)
        .body(&make_junit(&[("t", "failed"), ("t", "passed")]))
        .create(db)
        .await;
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

#[tokio::test]
async fn a_failing_filing_is_logged_and_the_next_outcome_still_files() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&attempts);
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(move |_request: &Request| {
            // Only the first attempt fails; the second files.
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(500)
            } else {
                ResponseTemplate::new(201).set_body_json(json!({"number": 7}))
            }
        })
        .mount(&server)
        .await;
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    );

    let logged = CapturedLog::during(async {
        LeaderTask::start(&worker).await;
        LeaderTask::enqueue(&worker, &processed(&[test]))
            .expect("the queue takes the first outcome");
        until_finished(worker.queue(), 1).await;
        assert_eq!(
            stored_number(&db, test).await,
            None,
            "the first attempt stored nothing"
        );
        LeaderTask::enqueue(&worker, &processed(&[test]))
            .expect("the queue takes the second outcome");
        until_finished(worker.queue(), 2).await;
        LeaderTask::stop(&worker).await.expect("the task stops");
    })
    .await;
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "both outcomes were tried"
    );
    assert!(
        logged.contains("GitHub issue creation failed"),
        "the failure is logged: {logged}"
    );
}

#[tokio::test]
async fn the_fixed_note_runs_in_the_background_for_touched_ids() {
    let db = TestDb::new().await.expect("a test database");
    let test = noteable_test(&db, 77, 3).await;
    let server = MockServer::start().await;
    mount_open_issue(&server, 77).await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/77/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 99})))
        .mount(&server)
        .await;
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(note_settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    );

    LeaderTask::start(&worker).await;
    LeaderTask::enqueue(&worker, &processed(&[test])).expect("the queue takes the outcome");
    until_finished(worker.queue(), 1).await;
    assert_eq!(
        requested_calls(&server).await,
        vec![
            ("GET".to_owned(), "/repos/acme/app/issues/77".to_owned()),
            (
                "POST".to_owned(),
                "/repos/acme/app/issues/77/comments".to_owned()
            )
        ],
        "filing is silent for a linked row, so the POST can only be the note"
    );
    assert!(
        noted(&db, test).await,
        "the batch is finished, so the note has been stamped"
    );
    LeaderTask::stop(&worker).await.expect("the task stops");
}

#[tokio::test]
async fn the_github_task_starts_and_stops_with_leadership() {
    let db = TestDb::new().await.expect("a test database");
    let linked = linked_test(&db, 12).await;
    let server = MockServer::start().await;
    mount_open_issue(&server, 12).await;
    let github = Arc::new(GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    ));
    let worker = leader_worker(&db, Arc::clone(&github));

    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;
    // The task's first sync proves it started; a sync is not a queue batch.
    wait_until("the task's first sync asks", || async {
        recorded(&server).await >= 1
    })
    .await;
    let test = flaky_test(&db).await;
    worker.stop().await;
    assert!(!worker.is_leader());

    LeaderTask::enqueue(github.as_ref(), &processed(&[test])).expect("the queue takes the outcome");
    // Nothing is left running to take it, which the queue itself says.
    assert_eq!(
        queued(github.queue()),
        1,
        "a stopped task never takes the outcome"
    );
    assert_eq!(
        requested_calls(&server).await.len(),
        1,
        "the task stopped with the leadership, so a queued outcome is never filed"
    );
    assert_eq!(stored_number(&db, test).await, None);
    assert_eq!(stored_number(&db, linked).await, Some(12));
}

#[tokio::test]
async fn a_lost_lock_connection_stops_the_github_task() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let linked = linked_test(&db, 12).await;
    let server = MockServer::start().await;
    mount_open_issue(&server, 12).await;
    let github = Arc::new(GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    ));
    let worker = leader_worker(&db, Arc::clone(&github));

    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;
    wait_until("the task's first sync asks", || async {
        recorded(&server).await >= 1
    })
    .await;

    // Queue for the lock before the session that holds it dies: the server grants
    // the claim the moment the lock is free, ahead of a worker that must first find
    // the loss, stop its task, release and wait a poll. So the worker cannot
    // re-elect while the test proves its GitHub task stopped.
    let database = database_name(&db).await;
    let claim = LockClaim::queue(&db).await;

    // The worker leads, so it holds the lock: read the session that holds it and end
    // it while nothing has been disturbed, so neither read races the loss that
    // follows the way it would if the read came after the database was touched.
    let holder = lock_holder(&db, &database).await;
    assert!(
        end_session(&db, holder).await,
        "the session that held the processor lock was still there to end"
    );
    let claim = claim.granted().await;
    wait_until("leadership is released", || async { !worker.is_leader() }).await;

    // The GitHub task stopped with the leadership: an outcome queued now has no
    // running task to drain it, however often the worker asks for the lock.
    LeaderTask::enqueue(github.as_ref(), &processed(&[test])).expect("the queue takes it");
    assert_eq!(
        queued(github.queue()),
        1,
        "no task is left running to take the outcome"
    );
    assert_eq!(
        requested_calls(&server).await.len(),
        1,
        "the GitHub task stopped on the error path, so nothing is filed"
    );
    assert_eq!(stored_number(&db, test).await, None);
    assert_eq!(stored_number(&db, linked).await, Some(12));
    release_lock(claim).await;
    worker.stop().await;
}

#[tokio::test]
async fn stopping_the_task_ends_its_work() {
    let db = TestDb::new().await.expect("a test database");
    let linked = linked_test(&db, 12).await;
    let server = MockServer::start().await;
    mount_open_issue(&server, 12).await;
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    );

    LeaderTask::start(&worker).await;
    wait_until("the task's first sync asks", || async {
        recorded(&server).await >= 1
    })
    .await;
    LeaderTask::stop(&worker).await.expect("the task stops");
    // The queue is dead after a stop: nothing in it is ever filed, which the
    // queue itself proves by still holding the outcome.
    let test = flaky_test(&db).await;
    LeaderTask::enqueue(&worker, &processed(&[test])).expect("the queue takes the outcome");
    assert_eq!(queued(worker.queue()), 1, "nothing took the outcome");
    assert_eq!(requested_calls(&server).await.len(), 1);
    assert_eq!(stored_number(&db, test).await, None);
    assert_eq!(stored_number(&db, linked).await, Some(12));
}

#[tokio::test]
async fn a_queued_outcome_files_through_the_workers_api_for_tests_and_jobs() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let job = flaky_job(&db).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 7})))
        .mount(&server)
        .await;
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    );
    let mut outcome = processed(&[test]);
    outcome.touched_job_ids = vec![job];

    LeaderTask::start(&worker).await;
    LeaderTask::enqueue(&worker, &outcome).expect("the queue takes the outcome");
    until_finished(worker.queue(), 1).await;
    LeaderTask::stop(&worker).await.expect("the task stops");

    assert_eq!(
        requested_calls(&server).await,
        vec![
            ("POST".to_owned(), "/repos/acme/app/issues".to_owned()),
            ("POST".to_owned(), "/repos/acme/app/issues".to_owned())
        ],
        "the worker's own API reaches both the Test and the Job filer"
    );
}

#[tokio::test]
async fn a_slow_github_call_does_not_block_processing() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .create(&db)
        .await;
    queue_flaky(&db, &project, "a").await;

    let (arrived, arrived_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(GatedAnswer::new(arrived, release_rx, 201, 77))
        .mount(&server)
        .await;
    let github = Arc::new(GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    ));
    let worker = leader_worker(&db, Arc::clone(&github));

    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;
    arrived_rx.await.expect("the first issue POST is in flight");

    queue_flaky(&db, &project, "b").await;
    wait_until("the second Report is processed", || async {
        report_statuses(&db).await == ["processed", "processed"]
    })
    .await;
    // The gate is still held, so the second Report got there before the answer.
    release_tx.send(()).expect("the stub is released");
    worker.stop().await;
}

#[tokio::test]
async fn a_sync_that_cannot_answer_is_retried_on_the_next_interval() {
    let db = TestDb::new().await.expect("a test database");
    let linked = linked_test(&db, 12).await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    // The first readings are below the interval, the fourth crosses it: the sync
    // that got a 500 is retried without waiting for a real interval.
    let monotonic = Arc::new(ScriptedMonotonic::new([0.0, 0.0, 0.0, 100.0]));
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_millis(60),
    )
    .with_monotonic(monotonic);

    LeaderTask::start(&worker).await;
    wait_until("the first sync is answered", || async {
        recorded(&server).await >= 1
    })
    .await;
    wait_until("the interval's sync is answered", || async {
        recorded(&server).await >= 2
    })
    .await;
    LeaderTask::stop(&worker).await.expect("the task stops");

    assert_eq!(
        requested_calls(&server).await,
        vec![
            ("GET".to_owned(), "/repos/acme/app/issues/12".to_owned()),
            ("GET".to_owned(), "/repos/acme/app/issues/12".to_owned())
        ],
        "the interval re-fires after a sync GitHub could not answer"
    );
    assert_eq!(
        stored_number(&db, linked).await,
        Some(12),
        "the link stands"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn the_github_task_ends_before_the_lock_is_released() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let (arrived, arrived_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(GatedAnswer::new(arrived, release_rx, 201, 77))
        .mount(&server)
        .await;
    let github = Arc::new(GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    ));
    let announced = Arc::new(Announced::new(Arc::clone(&github)));
    let worker = leader_worker_over(&db, Arc::clone(&announced) as Arc<dyn LeaderTask>);

    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;
    LeaderTask::enqueue(github.as_ref(), &processed(&[test])).expect("the queue takes the outcome");
    arrived_rx.await.expect("the issue POST is in flight");

    let stopping = {
        let worker = Arc::clone(&worker);
        tokio::spawn(async move { worker.stop().await })
    };
    // The loop has finished and release is now waiting for the GitHub task, whose
    // call is still in flight: the lock must still be held.
    announced.announced().await;
    assert!(
        !lock_is_free(db.url()).await,
        "unlocking now would let a replacement leader file the same issue"
    );

    release_tx.send(()).expect("the stub is released");
    stopping.await.expect("the stop finishes");
    assert!(!worker.is_leader());
    wait_until("the lock is free", || async {
        lock_is_free(db.url()).await
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_standby_leads_only_after_the_former_github_task_stops() {
    let db = TestDb::new().await.expect("a test database");
    let linked = linked_test(&db, 12).await;
    let (arrived, arrived_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let former_server = MockServer::start().await;
    mount_open_issue(&former_server, 12).await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(GatedAnswer::new(arrived, release_rx, 201, 77))
        .mount(&former_server)
        .await;
    let successor_server = MockServer::start().await;
    mount_open_issue(&successor_server, 12).await;

    let former_github = Arc::new(GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &former_server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    ));
    let former_end = Arc::new(Announced::new(Arc::clone(&former_github)));
    let former = leader_worker_over(&db, Arc::clone(&former_end) as Arc<dyn LeaderTask>);
    let successor_github = Arc::new(GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &successor_server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    ));
    let standby = leader_worker(&db, successor_github);

    Arc::clone(&former).start().await;
    wait_until("the former leads", || async { former.is_leader() }).await;
    wait_until("the former's own first sync asks", || async {
        recorded(&former_server).await >= 1
    })
    .await;
    Arc::clone(&standby).start().await;
    assert!(!standby.is_leader(), "only one process may lead");
    assert_eq!(
        requested_calls(&successor_server).await.len(),
        0,
        "a standby has no GitHub task running"
    );

    let test = flaky_test(&db).await;
    LeaderTask::enqueue(former_github.as_ref(), &processed(&[test]))
        .expect("the queue takes the outcome");
    arrived_rx.await.expect("the issue POST is in flight");
    let stopping = {
        let former = Arc::clone(&former);
        tokio::spawn(async move { former.stop().await })
    };
    // The former's release has reached its GitHub task, whose POST is still in
    // flight: the lock is therefore still held, and the standby cannot lead.
    former_end.announced().await;
    assert!(!standby.is_leader(), "the standby cannot lead yet");
    assert_eq!(
        requested_calls(&successor_server).await.len(),
        0,
        "and its GitHub task cannot start while the former one is in flight"
    );

    release_tx.send(()).expect("the stub is released");
    stopping.await.expect("the former stop finishes");
    wait_until("the standby leads", || async { standby.is_leader() }).await;
    // Its first sync proves its GitHub task started; by then the former's filing
    // has stored one more link, so the sync checks both rows.
    wait_until("the successor's GitHub task starts", || async {
        !requested_calls(&successor_server).await.is_empty()
    })
    .await;
    standby.stop().await;
    assert_eq!(stored_number(&db, linked).await, Some(12));
    wait_until("the lock is free", || async {
        lock_is_free(db.url()).await
    })
    .await;
}

// --- the lifecycle a leadership change puts a task through ------------------

/// The most `stop` may take while a call is in flight.
///
/// Far under the fifteen-second request timeout a call it cannot end would wait
/// out, and far over the wake a cancellation costs, so the bound separates the
/// two without depending on a machine's speed.
const STOP_BOUND: Duration = Duration::from_secs(2);

/// A processor that fails once, then hands over `outcome` and idles.
///
/// The failure is what the loop treats as a lost leader: it drops leadership,
/// stops its leader tasks and takes the lock again on its next attempt.
#[derive(Debug)]
struct FailsOnce {
    calls: AtomicUsize,
    outcome: ProcessOutcome,
}

impl FailsOnce {
    /// A processor that drops leadership before its first outcome.
    fn new(outcome: ProcessOutcome) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            outcome,
        }
    }
}

impl ReportProcessor for FailsOnce {
    fn process_next(
        &self,
        _pool: DatabaseConnection,
        _settings: ProcessingSettings,
    ) -> BoxFuture<'static, Result<Option<ProcessOutcome>, DbError>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let outcome = self.outcome.clone();
        Box::pin(async move {
            match call {
                0 => Err(DbError::LockSessionLost {
                    detail: "the test staged a database error".to_owned(),
                }),
                1 => Ok(Some(outcome)),
                _ => Ok(None),
            }
        })
    }
}

#[tokio::test]
async fn a_stopped_github_task_files_again_after_a_restart() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test(&db).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 7})))
        .mount(&server)
        .await;
    mount_open_issue(&server, 7).await;
    let worker = Arc::new(GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    ));

    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &processed(&[first]))
        .expect("the queue takes the outcome");
    wait_until("the first term files", || async {
        stored_number(&db, first).await == Some(7)
    })
    .await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");

    // The same worker, re-elected: a fresh term starts a fresh task.
    let second = flaky_test_in(&db, "second").await;
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &processed(&[second]))
        .expect("the queue takes the outcome");
    wait_until("the second term files", || async {
        stored_number(&db, second).await == Some(7)
    })
    .await;
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
}

#[tokio::test]
async fn leadership_a_failing_processor_drops_files_after_it_returns() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 7})))
        .mount(&server)
        .await;
    let github = Arc::new(GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    ));
    let leader: Arc<dyn LeaderTask> = Arc::clone(&github) as Arc<dyn LeaderTask>;
    let worker = Arc::new(
        ReportWorker::new(
            db.connection().clone(),
            Arc::new(AdvisoryLock::new(db.url(), DbSettings::default())),
            ProcessingSettings::default(),
            worker_settings(),
        )
        .with_leader_task(leader)
        .with_processor(Arc::new(FailsOnce::new(processed(&[test])))),
    );

    Arc::clone(&worker).start().await;
    // The first claim drops leadership; the report the second claim hands over is
    // the new Proven flake, and it is filed only if the task started again.
    wait_until("the issue is filed after leadership returns", || async {
        stored_number(&db, test).await == Some(7)
    })
    .await;
    assert!(
        worker.is_leader(),
        "the process leads again by the time its task files"
    );
    worker.stop().await;
    assert_eq!(stored_number(&db, test).await, Some(7));
}

/// A processor that hands over what the test queued, and idles otherwise.
#[derive(Debug, Default)]
struct HandsOver {
    next: Mutex<Option<ProcessOutcome>>,
}

impl HandsOver {
    /// Hands `outcome` over on the next claim the leader makes.
    fn hand_over(&self, outcome: ProcessOutcome) {
        *self.next.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
    }
}

impl ReportProcessor for HandsOver {
    fn process_next(
        &self,
        _pool: DatabaseConnection,
        _settings: ProcessingSettings,
    ) -> BoxFuture<'static, Result<Option<ProcessOutcome>, DbError>> {
        let handed = self
            .next
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        Box::pin(async move { Ok(handed) })
    }
}

#[tokio::test]
async fn a_database_that_drops_the_leader_files_the_next_proven_flake() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test(&db).await;
    let second = flaky_test_in(&db, "second").await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 7})))
        .mount(&server)
        .await;
    mount_open_issue(&server, 7).await;
    // The sync interval is short because a restart leaves dead connections in the
    // pool: the run that begins with the new leadership may lose its first read,
    // and the next attempt is one interval away.
    let github = Arc::new(GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_millis(20),
    ));
    let processor = Arc::new(HandsOver::default());
    let leader: Arc<dyn LeaderTask> = Arc::clone(&github) as Arc<dyn LeaderTask>;
    let report_processor: Arc<dyn ReportProcessor> =
        Arc::clone(&processor) as Arc<dyn ReportProcessor>;
    let lock = Arc::new(CountingLock::new(&db));
    let lock_session: Arc<dyn LockSession> = Arc::clone(&lock) as Arc<dyn LockSession>;
    let worker = Arc::new(
        ReportWorker::new(
            db.connection().clone(),
            lock_session,
            ProcessingSettings::default(),
            worker_settings(),
        )
        .with_leader_task(leader)
        .with_processor(report_processor),
    );

    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;
    processor.hand_over(processed(&[first]));
    wait_until("the first Proven flake is filed", || async {
        stored_number(&db, first).await == Some(7)
    })
    .await;

    // A Postgres restart kills every session the leader held, the one carrying the
    // processor lock above all. That loss is what a stopped server delivers, and the
    // claim queued below is granted the instant the holder's session dies, so the
    // lock is out of the worker's hands while it notices — which is the state a
    // restart leaves it in until it asks the new server for the lock again.
    let database = database_name(&db).await;
    let claim = LockClaim::queue(&db).await;
    let holder = lock_holder(&db, &database).await;
    assert!(
        end_session(&db, holder).await,
        "the session that held the processor lock was still there to end"
    );
    let claim = claim.granted().await;
    wait_until("leadership is released", || async { !worker.is_leader() }).await;

    // The queue outlives the leadership: the outcome the next leader claims is
    // filed even though it was handed over by nobody.
    let attempts = lock.attempts();
    processor.hand_over(processed(&[second]));
    // A stand-by worker does nothing but ask for the lock, so that it asked again
    // is what proves it ran without claiming, and the empty link below is about a
    // worker that looked rather than about a test that slept.
    wait_until("the standing-by worker tries the lock again", || async {
        lock.attempts() > attempts
    })
    .await;
    assert_eq!(
        stored_number(&db, second).await,
        None,
        "no task is running while the worker stands by, so nothing is filed"
    );

    release_lock(claim).await;
    wait_until("the new Proven flake is filed", || async {
        stored_number(&db, second).await == Some(7)
    })
    .await;
    worker.stop().await;
    assert_eq!(stored_number(&db, second).await, Some(7));
}

#[tokio::test]
async fn a_database_restart_under_the_leader_files_a_real_issue_and_syncs_again() {
    // A restart ends every session the server had, so the worker loses the
    // processor lock it held, has to take it again from a new session, and files
    // the next Proven flake through GitHub with a link it really stores. The
    // linked Test beside it proves the sync runs again too.
    let db = TestDb::new().await.expect("a test database");
    let before = flaky_test(&db).await;
    let after = flaky_test_in(&db, "second").await;
    let linked = linked_test(&db, 12).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 7})))
        .mount(&server)
        .await;
    mount_open_issue(&server, 7).await;
    mount_open_issue(&server, 12).await;
    // The sync interval is short because a restart leaves dead connections in the
    // pool: the run that begins with the new leadership may lose its first read,
    // and the next attempt is one interval away.
    let github = Arc::new(GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_millis(20),
    ));
    let processor = Arc::new(HandsOver::default());
    let leader: Arc<dyn LeaderTask> = Arc::clone(&github) as Arc<dyn LeaderTask>;
    let report_processor: Arc<dyn ReportProcessor> =
        Arc::clone(&processor) as Arc<dyn ReportProcessor>;
    let worker = Arc::new(
        ReportWorker::new(
            db.connection().clone(),
            Arc::new(AdvisoryLock::new(db.url(), DbSettings::default())),
            ProcessingSettings::default(),
            worker_settings(),
        )
        .with_leader_task(leader)
        .with_processor(report_processor),
    );

    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;
    processor.hand_over(processed(&[before]));
    wait_until("the first Proven flake is filed", || async {
        stored_number(&db, before).await == Some(7)
    })
    .await;

    let database = database_name(&db).await;
    let holder = lock_holder(&db, &database).await;
    let reads = issue_reads(&server).await;
    assert!(
        restart(&db).await > 0,
        "the restart ended the sessions that were there"
    );

    // The lock the old session held is gone, so the worker can only lead again
    // from a backend the restart did not end.
    wait_until("the worker leads from a new session", || async {
        worker.is_leader()
            && lock_holder_pid(&db, &database)
                .await
                .is_some_and(|pid| pid != holder)
    })
    .await;

    processor.hand_over(processed(&[after]));
    wait_until("the new leader files the next Proven flake", || async {
        stored_number(&db, after).await == Some(7)
    })
    .await;
    assert_eq!(
        stored_number(&db, linked).await,
        Some(12),
        "the sync leaves an open link alone"
    );
    wait_until(
        "the sync after the restart reads a linked issue",
        || async { issue_reads(&server).await > reads },
    )
    .await;
    worker.stop().await;
}

#[tokio::test]
async fn a_full_queue_drops_every_outcome_past_its_bound() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let worker = Arc::new(GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), "http://127.0.0.1:1").expect("the HTTP client builds"),
        Duration::from_secs(3600),
    ));

    // No task drains the queue, so every outcome past the bound is one too many.
    let over = 3;
    for _ in 0..flakeradar::github_worker::QUEUE_MAX + over {
        LeaderTask::enqueue(worker.as_ref(), &processed(&[test]))
            .expect("the queue takes the outcome");
    }

    assert_eq!(
        queued(worker.queue()),
        flakeradar::github_worker::QUEUE_MAX,
        "the queue holds no more than its bound"
    );
    assert_eq!(
        dropped(worker.queue()),
        over,
        "every outcome past the bound is dropped"
    );
}

#[tokio::test]
async fn stopping_ends_an_unanswered_sync_get_at_once() {
    let db = TestDb::new().await.expect("a test database");
    let linked = linked_test(&db, 12).await;
    // A second linked row in another Repo: a batch that was not stopped moves on
    // to it after the first GET gave up, so the request count is a second fact
    // about the stop, independent of how long anything took.
    let other = linked_test_in(&db, "other/app", 13).await;
    let endpoint = Unanswered::start().await;
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &endpoint.base).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    );

    LeaderTask::start(&worker).await;
    endpoint.arrived().await; // the start-up sync's first GET is in flight
    within(
        "the token ends the GET instead of the request timeout",
        STOP_BOUND,
        LeaderTask::stop(&worker),
    )
    .await
    .expect("the task stops");
    assert_eq!(
        endpoint.requests(),
        1,
        "the sync ended at the stop signal instead of timing out the GET and moving on"
    );
    assert_eq!(
        stored_number(&db, linked).await,
        Some(12),
        "the link stands"
    );
    assert_eq!(stored_number(&db, other).await, Some(13), "the link stands");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn stopping_waits_for_a_comment_post_and_keeps_its_stamp() {
    let db = TestDb::new().await.expect("a test database");
    let test = noteable_test(&db, 77, 3).await;
    let (arrived, arrived_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let server = MockServer::start().await;
    mount_open_issue(&server, 77).await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/77/comments"))
        .respond_with(GatedAnswer::new(arrived, release_rx, 201, 99))
        .mount(&server)
        .await;
    let worker = Arc::new(GitHubWorker::new(
        db.connection().clone(),
        Arc::new(note_settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    ));

    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &processed(&[test])).expect("the queue takes the outcome");
    arrived_rx.await.expect("the comment POST is in flight");

    let stopping = {
        let worker = Arc::clone(&worker);
        tokio::spawn(async move { LeaderTask::stop(worker.as_ref()).await })
    };
    release_tx.send(()).expect("the stub is released");
    stopping
        .await
        .expect("the stop finishes")
        .expect("the task stops");

    // Dropping a comment POST does not un-send it: GitHub would have the comment
    // and no stamp would record it, so the same comment lands again on the next
    // Report. The POST runs through its stamp, and only then does the batch stop.
    assert!(
        noted(&db, test).await,
        "the comment GitHub received was stamped before the task stopped"
    );

    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &processed(&[test])).expect("the queue takes the outcome");
    until_finished(worker.queue(), 2).await;
    assert_eq!(
        comment_requests(&server).await,
        1,
        "the next Report posts no second comment: the first one is recorded"
    );
    LeaderTask::stop(worker.as_ref())
        .await
        .expect("the task stops");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn stopping_waits_for_a_filing_post_and_keeps_its_link() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    // A second candidate the batch must not reach: the stop lands between the two.
    let next = flaky_test_in(&db, "next").await;
    let (arrived, arrived_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(GatedAnswer::new(arrived, release_rx, 201, 77))
        .mount(&server)
        .await;
    let worker = Arc::new(GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    ));

    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &processed(&[test, next]))
        .expect("the queue takes the outcome");
    arrived_rx.await.expect("the issue POST is in flight");

    let stopping = {
        let worker = Arc::clone(&worker);
        tokio::spawn(async move { LeaderTask::stop(worker.as_ref()).await })
    };
    release_tx.send(()).expect("the stub is released");
    stopping
        .await
        .expect("the stop finishes")
        .expect("the task stops");

    // Cancelling the POST would strand an issue no row ever points at, so the POST
    // runs through storing its link, and only then does the batch stop.
    assert_eq!(
        stored_number(&db, test).await,
        Some(77),
        "the link of the in-flight POST was stored before the task stopped"
    );
    assert_eq!(
        stored_number(&db, next).await,
        None,
        "the batch stopped after the POST it could not interrupt"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sync_holds_no_row_lock_across_its_get() {
    let db = TestDb::new().await.expect("a test database");
    // The first row in the rotation is answered and written; the second row's `GET`
    // is then held open, so the first row's write is the one a second connection
    // races against. Only a sync that keeps its statements open across the call
    // can still be holding that row.
    let checked = linked_test(&db, 12).await;
    let held = linked_test_in(&db, "other/app", 13).await;
    let (arrived, arrived_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let server = MockServer::start().await;
    mount_open_issue(&server, 12).await;
    Mock::given(method("GET"))
        .and(path("/repos/other/app/issues/13"))
        .respond_with(GatedAnswer::state(arrived, release_rx, "open"))
        .mount(&server)
        .await;
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds"),
        Duration::from_secs(3600),
    );

    LeaderTask::start(&worker).await;
    arrived_rx.await.expect("the second row's GET is in flight");

    // A second connection writes the row the sync already checked, under a
    // lock_timeout far shorter than any GitHub call: a held row lock fails this
    // with 55P03 instead of waiting for it.
    let neighbour = db.connection().begin().await.expect("a transaction");
    neighbour
        .execute_unprepared("SET LOCAL lock_timeout = '250ms'")
        .await
        .expect("the short timeout is set");
    neighbour
        .execute_unprepared(&format!(
            "UPDATE test_cases SET quarantine_reason = 'seen' WHERE id = {checked}"
        ))
        .await
        .expect("the row the sync already wrote is not locked");
    neighbour.commit().await.expect("the write commits");

    // The quarantine route wants that same row, and answers 200 rather than the
    // 500 a statement timeout would give it.
    let app = app(&db, &[]);
    let reply = post(
        &app,
        &format!("/api/tests/{checked}/quarantine"),
        &[("content-type", "application/json")],
        json!({"quarantined": true}).to_string(),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", reply.text());

    // A Report touching that Test has to write it too, and reaches `processed` on
    // its first try instead of spending its retries on a lock.
    let project_id: i32 = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT project_id FROM test_cases WHERE id = $1",
            [Value::from(checked)],
        ))
        .await
        .expect("the Test reads")
        .expect("the Test exists")
        .try_get("", "project_id")
        .expect("a project id");
    let project = projects::Entity::find_by_id(project_id)
        .one(db.connection())
        .await
        .expect("the Project reads")
        .expect("the Project exists");
    make_report(&project, project.repo_id)
        .commit_sha("s1")
        .body(&make_junit(&[("test_linked", "failed")]))
        .create(&db)
        .await;
    let outcome =
        flakeradar_db::processing::process_next(db.connection(), &ProcessingSettings::default())
            .await
            .expect("the processor runs")
            .expect("the Report is queued");
    assert_eq!(
        outcome.status,
        ReportStatus::Processed,
        "{}",
        outcome.error.clone().unwrap_or_default()
    );
    assert_eq!(outcome.retry_count, 0, "no retry was spent on a lock");
    assert_eq!(
        outcome.touched_test_ids,
        vec![checked],
        "the Report touched the row the sync wrote"
    );
    assert_eq!(
        stored_number(&db, held).await,
        Some(13),
        "the held row is untouched"
    );

    release_tx.send(()).expect("the stub is released");
    LeaderTask::stop(&worker).await.expect("the task stops");
}
