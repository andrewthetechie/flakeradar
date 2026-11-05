//! The leader-only GitHub task: it drains the queue and syncs on its interval.
//!
//! Ported from `backend/tests/test_github_worker.py`. The GitHub API is a stub
//! server and the sync interval is read from a scripted clock, so nothing waits
//! on a real interval.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a stub or a task does not behave"
)]

use std::sync::Arc;
use std::time::Duration;

use flakeradar::{Api, Settings, settings::Secret};
use flakeradar::{GitHubWorker, LeaderTask};
use flakeradar_test_support::{ScriptedMonotonic, TestDb, make_project, make_test_case};
use sea_orm::ConnectionTrait as _;
use sea_orm::{DbBackend, Statement, Value};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
        .map(|requests| requests.len())
        .unwrap_or(0)
}

/// Waits until the stub has recorded `expected` requests.
async fn until_count(server: &MockServer, expected: usize) {
    for _ in 0..400 {
        if recorded(server).await == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "the stub recorded {} requests, not {expected}",
        recorded(server).await
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
