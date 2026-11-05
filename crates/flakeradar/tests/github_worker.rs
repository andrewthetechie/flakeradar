//! The leader-only GitHub task: it drains the queue and syncs on its interval.
//!
//! Ported from `backend/tests/test_github_worker.py`. The GitHub API is a stub
//! server and the sync interval is read from a scripted clock, so nothing waits
//! on a real interval. A call the test needs to hold open is staged by a stub
//! answer that signals when it arrived and waits to be released.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a stub or a task does not behave"
)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use flakeradar::worker::{AdvisoryLock, LockSession, ReportWorker, WorkerSettings};
use flakeradar::{Api, Settings, settings::Secret};
use flakeradar::{GitHubWorker, LeaderTask};
use flakeradar_db::entities::{projects, reports};
use flakeradar_db::{DbSettings, ProcessingSettings, WORKER_LOCK_KEY, connect};
use flakeradar_test_support::{
    ScriptedMonotonic, TestDb, make_job, make_junit, make_pipeline, make_project, make_report,
    make_test_case,
};
use sea_orm::ConnectionTrait as _;
use sea_orm::{DbBackend, EntityTrait as _, QueryOrder as _, Statement, Value};
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

/// Waits until the stub has recorded `expected` requests.
async fn until_count(server: &MockServer, expected: usize) {
    for _ in 0..400 {
        if recorded(server).await == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        recorded(server).await,
        expected,
        "the stub never recorded the expected requests: {:?}",
        requested_calls(server).await
    );
}

/// One flaky Test in `acme/app`.
async fn flaky_test(db: &TestDb) -> i32 {
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let test = make_test_case(&project)
        .name("test_total")
        .flakiness_score(0.5)
        .build(db.connection())
        .await
        .expect("the Test inserts");
    test.id
}

/// One flaky github-provider Job in `acme/app`.
async fn flaky_job(db: &TestDb) -> i32 {
    let pipeline = make_pipeline()
        .repo("acme/app")
        .provider("github")
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    make_job(&pipeline)
        .name("unit")
        .flakiness_score(0.5)
        .build(db.connection())
        .await
        .expect("the Job inserts")
        .id
}

/// A processed Report outcome for `tests`.
fn processed(tests: &[i32]) -> flakeradar_db::ProcessOutcome {
    flakeradar_db::ProcessOutcome {
        report_id: 1,
        status: "processed".to_owned(),
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
        Api::new(Secret::new(TOKEN), &server.uri()),
        Duration::from_secs(3600),
    );
    LeaderTask::start(&worker).await;

    LeaderTask::enqueue(&worker, &processed(&[test])).expect("the queue takes the outcome");
    until_count(&server, 1).await;
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
        Api::new(Secret::new(TOKEN), &server.uri()),
        Duration::from_millis(60),
    )
    .with_monotonic(monotonic);
    LeaderTask::start(&worker).await;
    until_count(&server, 1).await;
    until_count(&server, 2).await;
    LeaderTask::stop(&worker).await.expect("the task stops");
}

#[tokio::test]
async fn an_outcome_with_no_touched_ids_files_nothing() {
    let db = TestDb::new().await.expect("a test database");
    let server = MockServer::start().await;
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &server.uri()),
        // A long interval keeps the start-up sync out of the way, and the sync
        // finds no linked row to check anyway.
        Duration::from_secs(3600),
    );
    LeaderTask::start(&worker).await;
    LeaderTask::enqueue(&worker, &processed(&[])).expect("the queue takes the outcome");
    tokio::time::sleep(Duration::from_millis(50)).await;
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
async fn stopping_a_task_that_never_started_is_fine() {
    let db = TestDb::new().await.expect("a test database");
    let worker = GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), "http://127.0.0.1:1"),
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
        Api::new(Secret::new(TOKEN), &server.uri()),
        Duration::from_secs(3600),
    );
    let test = flaky_test(&db).await;
    LeaderTask::start(&worker).await;
    LeaderTask::enqueue(&worker, &processed(&[test])).expect("the queue takes the outcome");
    until_count(&server, 1).await;
    // A second outcome still goes out, so the task survived the failed one.
    LeaderTask::enqueue(&worker, &processed(&[test])).expect("the queue takes the outcome");
    until_count(&server, 2).await;
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
    let lock: Arc<dyn LockSession> = Arc::new(AdvisoryLock::new(db.url(), DbSettings::default()));
    Arc::new(
        ReportWorker::new(
            db.connection().clone(),
            lock,
            ProcessingSettings::default(),
            worker_settings(),
        )
        .with_leader_task(github),
    )
}

/// Waits for `condition`, failing rather than hanging.
///
/// The deadline bounds a failure; it is never the thing asserted.
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

/// Whether another session can take the processor lock right now.
async fn lock_is_free(url: &str) -> bool {
    let pool = connect(url, DbSettings::default())
        .await
        .expect("the probe's pool");
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
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
            DbBackend::Postgres,
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

/// The name of `db`'s database, for ending sessions on it.
async fn database_name(db: &TestDb) -> String {
    let row = db
        .connection()
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT current_database() AS name".to_owned(),
        ))
        .await
        .expect("the database name reads")
        .expect("a database name");
    row.try_get("", "name").expect("a database name")
}

/// Ends the backend holding the processor lock.
async fn terminate_lock_session(db: &TestDb, database: &str) {
    // Both statements run from the maintenance database, so this works while the
    // test's own database already refuses new connections.
    let admin = connect(db.maintenance_url(), DbSettings::default())
        .await
        .expect("the maintenance connection");
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pid FROM pg_locks WHERE locktype = 'advisory' AND classid = 0 \
           AND objid::bigint = $1 \
           AND database = (SELECT oid FROM pg_database WHERE datname = $2)",
        [WORKER_LOCK_KEY.into(), database.into()],
    );
    let row = admin
        .query_one_raw(statement)
        .await
        .expect("the lock query runs")
        .expect("the lock is held");
    let pid: i32 = row.try_get("", "pid").expect("a backend id");
    admin
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_terminate_backend($1)",
            [pid.into()],
        ))
        .await
        .expect("the backend ends");
    admin
        .close()
        .await
        .expect("the maintenance connection closes");
}

/// Refuses, or again allows, new connections to `database`.
///
/// The pool opens a replacement for a connection the server closed, so a
/// terminated lock backend is not enough on its own: with new connections
/// refused, the replacement cannot be opened and the failure is certain. The
/// policy changes from the maintenance database because the server refuses to
/// disallow connections for the database the statement runs in.
async fn set_connections_allowed(db: &TestDb, database: &str, allowed: bool) {
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

/// A linked Test in `acme/app` at issue `number`, so a sync has work to do.
async fn linked_test(db: &TestDb, number: i32) -> i32 {
    let project = make_project()
        .repo("acme/app")
        .project("linked")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    make_test_case(&project)
        .name("test_linked")
        .flakiness_score(0.5)
        .github_issue_number(number)
        .build(db.connection())
        .await
        .expect("the Test inserts")
        .id
}

/// A linked Test at the fixed-note threshold, so the note step has work to do.
async fn noteable_test(db: &TestDb, number: i32, streak: i32) -> i32 {
    let project = make_project()
        .repo("acme/app")
        .project("noteable")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    make_test_case(&project)
        .name("test_noteable")
        .flakiness_score(0.5)
        .clean_streak(streak)
        .github_issue_number(number)
        .build(db.connection())
        .await
        .expect("the Test inserts")
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
    number: i32,
}

impl GatedAnswer {
    /// A gate whose first request is reported on `arrived` and held until released.
    fn new(
        arrived: tokio::sync::oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        status: u16,
        number: i32,
    ) -> Self {
        Self {
            arrived: Mutex::new(Some(arrived)),
            release: Mutex::new(release),
            released: AtomicBool::new(false),
            status,
            number,
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
        ResponseTemplate::new(self.status).set_body_json(json!({ "number": self.number }))
    }
}

/// Queues one flaky JUnit Report for `project` at `sha`.
async fn queue_flaky(db: &TestDb, project: &projects::Model, sha: &str) {
    make_report(project, project.repo_id)
        .commit_sha(sha)
        .body(&make_junit(&[("t", "failed"), ("t", "passed")]))
        .build(db.connection())
        .await
        .expect("a queued Report");
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

/// The log lines `future` writes, with a subscriber installed for this thread.
async fn logged_while(future: impl Future) -> String {
    use std::io::Write as _;

    /// An `io::Write` that appends to the shared buffer.
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || Sink(Arc::clone(&sink)))
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    future.await;
    drop(guard);
    let text =
        String::from_utf8_lossy(&lines.lock().unwrap_or_else(PoisonError::into_inner)).into_owned();
    let _ = std::io::stdout().flush();
    text
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
        Api::new(Secret::new(TOKEN), &server.uri()),
        Duration::from_secs(3600),
    );

    let logged = logged_while(async {
        LeaderTask::start(&worker).await;
        LeaderTask::enqueue(&worker, &processed(&[test]))
            .expect("the queue takes the first outcome");
        until_count(&server, 1).await;
        assert_eq!(
            stored_number(&db, test).await,
            None,
            "the first attempt stored nothing"
        );
        LeaderTask::enqueue(&worker, &processed(&[test]))
            .expect("the queue takes the second outcome");
        until_count(&server, 2).await;
        wait_until("the second outcome is filed", || async {
            stored_number(&db, test).await == Some(7)
        })
        .await;
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
        Api::new(Secret::new(TOKEN), &server.uri()),
        Duration::from_secs(3600),
    );

    LeaderTask::start(&worker).await;
    LeaderTask::enqueue(&worker, &processed(&[test])).expect("the queue takes the outcome");
    until_count(&server, 2).await;
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
    wait_until("the note is stamped", || async { noted(&db, test).await }).await;
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
        Api::new(Secret::new(TOKEN), &server.uri()),
        Duration::from_secs(3600),
    ));
    let worker = leader_worker(&db, Arc::clone(&github));

    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;
    until_count(&server, 1).await; // the task's first sync proves it started
    let test = flaky_test(&db).await;
    worker.stop().await;
    assert!(!worker.is_leader());

    LeaderTask::enqueue(github.as_ref(), &processed(&[test])).expect("the queue takes the outcome");
    tokio::time::sleep(Duration::from_millis(50)).await;
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
        Api::new(Secret::new(TOKEN), &server.uri()),
        Duration::from_secs(3600),
    ));
    let worker = leader_worker(&db, Arc::clone(&github));

    Arc::clone(&worker).start().await;
    wait_until("the worker leads", || async { worker.is_leader() }).await;
    until_count(&server, 1).await;

    // The error path releases through the same door as stop: break the session
    // that holds the lock and refuse a replacement, so the worker must release.
    let database = database_name(&db).await;
    set_connections_allowed(&db, &database, false).await;
    terminate_lock_session(&db, &database).await;
    wait_until("leadership is released", || async { !worker.is_leader() }).await;

    // While the database refuses new connections the worker cannot re-elect, so
    // an outcome queued now has no running task to drain it.
    LeaderTask::enqueue(github.as_ref(), &processed(&[test])).expect("the queue takes it");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        requested_calls(&server).await.len(),
        1,
        "the GitHub task stopped on the error path, so nothing is filed"
    );
    assert_eq!(stored_number(&db, test).await, None);
    assert_eq!(stored_number(&db, linked).await, Some(12));
    set_connections_allowed(&db, &database, true).await;
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
        Api::new(Secret::new(TOKEN), &server.uri()),
        Duration::from_secs(3600),
    );

    LeaderTask::start(&worker).await;
    until_count(&server, 1).await;
    LeaderTask::stop(&worker).await.expect("the task stops");
    // The queue is dead after a stop: nothing in it is ever filed.
    let test = flaky_test(&db).await;
    LeaderTask::enqueue(&worker, &processed(&[test])).expect("the queue takes the outcome");
    tokio::time::sleep(Duration::from_millis(50)).await;
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
        Api::new(Secret::new(TOKEN), &server.uri()),
        Duration::from_secs(3600),
    );
    let mut outcome = processed(&[test]);
    outcome.touched_job_ids = vec![job];

    LeaderTask::start(&worker).await;
    LeaderTask::enqueue(&worker, &outcome).expect("the queue takes the outcome");
    until_count(&server, 2).await;
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
        .build(db.connection())
        .await
        .expect("the Project inserts");
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
        Api::new(Secret::new(TOKEN), &server.uri()),
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
        Api::new(Secret::new(TOKEN), &server.uri()),
        Duration::from_millis(60),
    )
    .with_monotonic(monotonic);

    LeaderTask::start(&worker).await;
    until_count(&server, 1).await;
    until_count(&server, 2).await;
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
        Api::new(Secret::new(TOKEN), &server.uri()),
        Duration::from_secs(3600),
    ));
    let worker = leader_worker(&db, Arc::clone(&github));

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
    tokio::time::sleep(Duration::from_millis(50)).await;
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
        Api::new(Secret::new(TOKEN), &former_server.uri()),
        Duration::from_secs(3600),
    ));
    let former = leader_worker(&db, Arc::clone(&former_github));
    let successor_github = Arc::new(GitHubWorker::new(
        db.connection().clone(),
        Arc::new(settings()),
        Api::new(Secret::new(TOKEN), &successor_server.uri()),
        Duration::from_secs(3600),
    ));
    let standby = leader_worker(&db, successor_github);

    Arc::clone(&former).start().await;
    wait_until("the former leads", || async { former.is_leader() }).await;
    until_count(&former_server, 1).await; // its own first sync
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
    tokio::time::sleep(Duration::from_millis(50)).await;
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
