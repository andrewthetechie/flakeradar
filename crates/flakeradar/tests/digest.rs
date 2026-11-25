//! The weekly digest task: one `POST` per week's slot, claimed before it is sent.
//!
//! The endpoint is a stub server and the clock is a stopped one, so a test
//! asserts what was posted rather than that a function was called, and nothing
//! waits on the real clock.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a stub, a claim or a delivery does not behave"
)]

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Utc};
use flakeradar::digest::DigestWorker;
use flakeradar::worker::LeaderTask;
use flakeradar_core::{
    digest::DigestSchedule,
    model::{EntityKind, ReportStatus, Tier},
    notify::TierChange,
    notify::WebhookFormat,
    types::Score,
};
use flakeradar_db::{processing::ProcessOutcome, reads::ReadSettings};
use flakeradar_test_support::{
    CapturedLog, FixedClock, TestDb, make_project, make_test_case, wait_until,
};
use sea_orm::ConnectionTrait as _;
use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The schedule every case here runs on: Mondays at 09:00 UTC.
const SCHEDULE: &str = "mon@09";

/// An instant from its RFC 3339 text, for a clock a test stops.
fn instant(text: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(text).expect("a fixed instant")
}

/// The slot `digest_state` says has been claimed, or `None` while the ledger is
/// still empty.
async fn stored_slot(db: &TestDb) -> Option<DateTime<Utc>> {
    let row = flakeradar_db::sql::one(
        db.connection(),
        "SELECT last_sent_at FROM digest_state",
        Vec::new(),
    )
    .await
    .expect("the ledger is readable");
    row.map(|row| {
        let stored: DateTime<FixedOffset> =
            flakeradar_db::sql::get(&row, "last_sent_at").expect("a stored instant");
        stored.with_timezone(&Utc)
    })
}

/// The one-row ledger holding this slot, as migration 0007's `id = 1`.
async fn store_slot(db: &TestDb, slot: &str) {
    let sql = format!(
        "INSERT INTO digest_state (id, last_sent_at) VALUES (1, '{slot}') RETURNING last_sent_at"
    );
    let _ = flakeradar_db::sql::one(db.connection(), sql, Vec::new())
        .await
        .expect("the ledger takes a row");
}

/// Drops a table from this test's database, so every later statement against it
/// fails: the seam for a check whose ledger or reads are gone.
async fn drop_table(db: &TestDb, table: &str) {
    db.connection()
        .execute_unprepared(&format!("DROP TABLE {table} CASCADE"))
        .await
        .expect("the test database drops a table");
}

/// A Repo with one Test that became flaky in the week ending 2026-11-08, which is
/// the news the digest reports.
async fn seed_week(db: &TestDb) {
    let project = make_project().repo("acme/app").create(db).await;
    make_test_case(&project)
        .name("test_login")
        .flaky_since(Some(instant("2026-11-08T09:00:00+00:00")))
        .create(db)
        .await;
}

/// A task on a clock stopped at `clock_at`, posting to `urls` in the JSON format.
fn worker(db: &TestDb, clock_at: &str, urls: Vec<String>) -> DigestWorker {
    let reads = ReadSettings {
        clock: Arc::new(FixedClock::new(instant(clock_at))),
        ..ReadSettings::default()
    };
    let schedule: DigestSchedule = SCHEDULE.parse().expect("a documented schedule");
    DigestWorker::new(
        db.connection().clone(),
        schedule,
        WebhookFormat::Json,
        urls,
        reads,
    )
    .expect("the HTTP client builds")
}

/// A stub endpoint that answers every `POST` to `route` with `status`.
async fn stub(route: &str, status: u16) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(status))
        .mount(&server)
        .await;
    server
}

/// How many requests the stub has recorded.
async fn recorded(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .map_or(0, |requests| requests.len())
}

/// One Test's crossing of the Flake threshold, which the digest task ignores.
fn test_change() -> TierChange {
    TierChange {
        kind: EntityKind::Test,
        id: 1,
        repo: "acme/app".to_owned(),
        project: Some("backend".to_owned()),
        name: "test_total".to_owned(),
        flakiness_score: Score::new(0.42).expect("the test states a score"),
        tier: Tier::Flaky,
        proven_flake_count: 2,
        classname: Some("tests.test_cart".to_owned()),
        pipeline: None,
        failure_category: None,
        broken: false,
    }
}

/// One processed Report carrying a Tier change.
fn outcome() -> ProcessOutcome {
    ProcessOutcome {
        report_id: 1,
        status: ReportStatus::Processed,
        run_id: Some(1),
        counts: None,
        touched_test_ids: Vec::new(),
        error: None,
        touched_job_ids: Vec::new(),
        tier_changes: vec![test_change()],
        retry_count: 0,
    }
}

#[tokio::test]
async fn the_digest_is_posted_once_for_the_weeks_slot() {
    let db = TestDb::new().await.expect("a test database");
    seed_week(&db).await;
    let server = stub("/hook", 200).await;
    let worker = worker(
        &db,
        "2026-11-09T09:30:00+00:00",
        vec![format!("{}/hook", server.uri())],
    );

    LeaderTask::start(&worker).await;
    wait_until("the first check ran", || async { worker.checks() >= 1 }).await;
    LeaderTask::stop(&worker).await.expect("the task stops");

    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert_eq!(requests.len(), 1, "the week's digest is posted once");
    let body: Value = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    let events = body.as_array().expect("the payload is a list");
    assert_eq!(events.len(), 1, "one event for the one Repo with news");
    assert_eq!(events[0]["event"], "digest.weekly");
    assert_eq!(
        events[0]["week_ending"], "2026-11-09",
        "the week ends the day the check ran on"
    );
    assert_eq!(
        stored_slot(&db).await,
        Some(instant("2026-11-09T09:00:00+00:00").with_timezone(&Utc)),
        "the claim stores the slot, not the instant it ran at"
    );
}

#[tokio::test]
async fn a_second_digest_check_sends_nothing() {
    let db = TestDb::new().await.expect("a test database");
    seed_week(&db).await;
    let server = stub("/hook", 200).await;
    let worker = worker(
        &db,
        "2026-11-09T09:30:00+00:00",
        vec![format!("{}/hook", server.uri())],
    )
    .with_check_interval(Duration::from_millis(10));

    LeaderTask::start(&worker).await;
    wait_until("the second check ran", || async { worker.checks() >= 2 }).await;
    LeaderTask::stop(&worker).await.expect("the task stops");

    assert_eq!(
        recorded(&server).await,
        1,
        "the second check loses the claim, so the week is not sent twice"
    );
}

#[tokio::test]
async fn a_digest_check_interval_applies_only_before_the_task_runs() {
    let db = TestDb::new().await.expect("a test database");
    let server = stub("/hook", 200).await;
    // The shipped 300-second interval: one check, then a wait no test sits through.
    let worker = worker(
        &db,
        "2026-11-09T08:59:00+00:00",
        vec![format!("{}/hook", server.uri())],
    );

    LeaderTask::start(&worker).await;
    wait_until("the first check ran", || async { worker.checks() >= 1 }).await;
    // The loop holds its own share of the state, so the seam cannot reach it
    // once it runs: the builder answers with the same, still-slow task.
    let worker = worker.with_check_interval(Duration::from_millis(10));
    LeaderTask::stop(&worker).await.expect("the task stops");

    assert_eq!(
        worker.checks(),
        1,
        "a running task keeps the interval it started with"
    );
}

#[tokio::test]
async fn a_digest_check_before_the_slot_sends_nothing() {
    let db = TestDb::new().await.expect("a test database");
    seed_week(&db).await;
    let server = stub("/hook", 200).await;
    let worker = worker(
        &db,
        "2026-11-09T08:59:00+00:00",
        vec![format!("{}/hook", server.uri())],
    );

    LeaderTask::start(&worker).await;
    wait_until("the first check ran", || async { worker.checks() >= 1 }).await;
    LeaderTask::stop(&worker).await.expect("the task stops");

    assert_eq!(recorded(&server).await, 0, "the slot has not come yet");
    assert_eq!(
        stored_slot(&db).await,
        None,
        "a check that owes nothing leaves no ledger row"
    );
}

#[tokio::test]
async fn a_digest_slot_already_sent_stays_sent() {
    let db = TestDb::new().await.expect("a test database");
    seed_week(&db).await;
    store_slot(&db, "2026-11-09T09:00:00+00:00").await;
    let server = stub("/hook", 200).await;
    let worker = worker(
        &db,
        "2026-11-09T10:00:00+00:00",
        vec![format!("{}/hook", server.uri())],
    );

    LeaderTask::start(&worker).await;
    wait_until("the first check ran", || async { worker.checks() >= 1 }).await;
    LeaderTask::stop(&worker).await.expect("the task stops");

    assert_eq!(
        recorded(&server).await,
        0,
        "a replica, or a leader that restarted after the hour, sends nothing again"
    );
}

#[tokio::test]
async fn a_failed_digest_post_is_logged_once_by_host_and_not_retried() {
    /// A credential in the path, which no log line may carry.
    const SECRET: &str = "SECRETTOKEN";

    let db = TestDb::new().await.expect("a test database");
    seed_week(&db).await;
    let route = format!("/{SECRET}/hook");
    let server = stub(&route, 500).await;
    let url = format!("{}{route}", server.uri());
    let worker = worker(&db, "2026-11-09T09:30:00+00:00", vec![url])
        .with_check_interval(Duration::from_millis(10));

    let log = CapturedLog::install();
    LeaderTask::start(&worker).await;
    wait_until("the second check ran", || async { worker.checks() >= 2 }).await;
    LeaderTask::stop(&worker).await.expect("the task stops");

    assert_eq!(
        recorded(&server).await,
        1,
        "a refused digest is not retried, and the next check loses the claim"
    );
    let failure = log.only_line_holding("rejected the weekly digest");
    assert!(
        failure.contains("127.0.0.1"),
        "the endpoint's host is enough to find it: {failure}"
    );
    let logged = log.lines();
    assert!(
        !logged.contains(SECRET),
        "no log line carries the URL: {logged}"
    );
}

/// A week whose ledger is gone: every claim fails, so every check logs the
/// refusal and sends nothing.
#[tokio::test]
async fn a_digest_whose_claim_fails_is_logged_and_sends_nothing() {
    let db = TestDb::new().await.expect("a test database");
    seed_week(&db).await;
    drop_table(&db, "digest_state").await;
    let server = stub("/hook", 200).await;
    let worker = worker(
        &db,
        "2026-11-09T09:30:00+00:00",
        vec![format!("{}/hook", server.uri())],
    );

    let log = CapturedLog::install();
    LeaderTask::start(&worker).await;
    wait_until("the first check ran", || async { worker.checks() >= 1 }).await;
    LeaderTask::stop(&worker).await.expect("the task stops");

    assert_eq!(
        recorded(&server).await,
        0,
        "a slot that cannot be claimed sends nothing"
    );
    let failure = log.only_line_holding("could not claim the weekly digest slot");
    assert!(
        failure.contains("2026-11-09T09:00:00+00:00"),
        "the refusal names the slot it could not claim: {failure}"
    );
}

/// A week whose ledger is claimed but whose digest cannot be built: the claim
/// stays written, so the lost digest is never sent twice.
#[tokio::test]
async fn a_digest_that_cannot_be_built_keeps_its_claimed_slot() {
    let db = TestDb::new().await.expect("a test database");
    seed_week(&db).await;
    let server = stub("/hook", 200).await;
    let worker = worker(
        &db,
        "2026-11-09T09:30:00+00:00",
        vec![format!("{}/hook", server.uri())],
    );
    // The ledger survives, the Repo list does not: the claim is reached and the
    // first digest statement fails.
    drop_table(&db, "repos").await;

    let log = CapturedLog::install();
    LeaderTask::start(&worker).await;
    wait_until("the first check ran", || async { worker.checks() >= 1 }).await;
    LeaderTask::stop(&worker).await.expect("the task stops");

    assert_eq!(
        recorded(&server).await,
        0,
        "a digest that could not be built is not posted"
    );
    let failure = log.only_line_holding("could not build the weekly digest");
    assert!(
        failure.contains("2026-11-09T09:00:00+00:00"),
        "the refusal names the slot it could not fill: {failure}"
    );
    assert_eq!(
        stored_slot(&db).await,
        Some(instant("2026-11-09T09:00:00+00:00").with_timezone(&Utc)),
        "the claimed slot stays claimed: the week is lost, not duplicated"
    );
}

/// One `POST` to a port nothing listens on: the failure is logged once, by host,
/// and never retried.
#[tokio::test]
async fn an_unreachable_digest_endpoint_is_logged_once_by_host() {
    let db = TestDb::new().await.expect("a test database");
    seed_week(&db).await;
    // Nothing listens on port 1: every attempt fails to connect.
    let worker = worker(
        &db,
        "2026-11-09T09:30:00+00:00",
        vec!["http://127.0.0.1:1/hook".to_owned()],
    )
    .with_check_interval(Duration::from_millis(10));

    let log = CapturedLog::install();
    LeaderTask::start(&worker).await;
    wait_until("the second check ran", || async { worker.checks() >= 2 }).await;
    LeaderTask::stop(&worker).await.expect("the task stops");

    let failures = log.lines_holding("unreachable for the weekly digest");
    assert_eq!(
        failures.len(),
        1,
        "the refused `POST` is not retried, and the next check loses the claim: {}",
        log.lines()
    );
    assert!(
        failures[0].contains("127.0.0.1"),
        "the endpoint's host is enough to find it: {}",
        failures[0]
    );
}

/// A digest URL that is not a URL: the failure names the endpoint only as
/// `endpoint`, and never carries the text that stood in for one.
#[tokio::test]
async fn a_digest_endpoint_that_is_not_a_url_is_logged_as_endpoint() {
    /// A credential in the text, which no log line may carry.
    const SECRET: &str = "SECRETTOKEN";

    let db = TestDb::new().await.expect("a test database");
    seed_week(&db).await;
    let worker = worker(
        &db,
        "2026-11-09T09:30:00+00:00",
        vec![format!("not a url/{SECRET}")],
    );

    let log = CapturedLog::install();
    LeaderTask::start(&worker).await;
    wait_until("the first check ran", || async { worker.checks() >= 1 }).await;
    LeaderTask::stop(&worker).await.expect("the task stops");

    let failure = log.only_line_holding("unreachable for the weekly digest");
    assert!(
        failure.contains("endpoint"),
        "an unparseable URL leaves only the generic label: {failure}"
    );
    let logged = log.lines();
    assert!(
        !logged.contains(SECRET),
        "no log line carries the URL text: {logged}"
    );
}

#[tokio::test]
async fn the_digest_task_enqueue_is_a_no_op() {
    let db = TestDb::new().await.expect("a test database");
    // Nothing is seeded, so a check has no news to send either: the only thing
    // that could make this endpoint ring is a hand-over the task was not
    // supposed to take.
    let server = stub("/hook", 200).await;
    let worker = worker(
        &db,
        "2026-11-09T09:30:00+00:00",
        vec![format!("{}/hook", server.uri())],
    )
    .with_check_interval(Duration::from_millis(10));

    LeaderTask::start(&worker).await;
    wait_until("the first check ran", || async { worker.checks() >= 1 }).await;
    // The hand-over a Notification task would have queued and sent.
    LeaderTask::enqueue(&worker, &outcome()).expect("the digest task takes any outcome");
    wait_until("a check after the hand-over", || async {
        worker.checks() >= 2
    })
    .await;
    LeaderTask::stop(&worker).await.expect("the task stops");

    assert_eq!(
        recorded(&server).await,
        0,
        "a Report cannot queue a weekly digest, and a week with no news is not sent"
    );
}

#[tokio::test]
async fn the_digest_debug_line_names_no_endpoint() {
    let db = TestDb::new().await.expect("a test database");
    let server = stub("/hook", 200).await;
    let url = format!("{}/hook", server.uri());
    let worker = worker(&db, "2026-11-09T09:30:00+00:00", vec![url.clone()]);

    let line = format!("{worker:?}");
    assert!(
        line.contains("DigestWorker"),
        "the Debug line names the task"
    );
    assert!(!line.contains(&url), "the URL is a credential: {line}");
    assert_eq!(
        LeaderTask::name(&worker),
        "Digest",
        "the leader registry names the task"
    );
}

#[tokio::test]
async fn tmp_probe_sent_log_lines() {
    let db = TestDb::new().await.expect("a test database");
    seed_week(&db).await;
    let server = stub("/hook", 200).await;
    let worker = worker(
        &db,
        "2026-11-09T09:30:00+00:00",
        vec![format!("{}/hook", server.uri())],
    );
    let log = CapturedLog::install();
    LeaderTask::start(&worker).await;
    wait_until("the first check ran", || async { worker.checks() >= 1 }).await;
    LeaderTask::stop(&worker).await.expect("the task stops");
    let line = log.only_line_holding("sent the weekly digest");
    assert!(
        line.contains("127.0.0.1"),
        "the host names the endpoint: {line}"
    );
    drop(log);
}
