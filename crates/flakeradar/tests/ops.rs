//! `GET /api/health/ready` and `GET /metrics`.
//!
//! No request here sends a token: both reads are unauthenticated, like the other
//! read APIs. The app starts no worker, so `processor_leader` is false unless the
//! test holds the processor's advisory lock itself.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the probe or the scrape answers something else"
)]

use std::collections::BTreeMap;

use axum::Router;
use axum::http::StatusCode;
use flakeradar::app::AppState;
use flakeradar::app::build_app;
use flakeradar_core::model::ReportStatus;
use flakeradar_core::types::Score;
use flakeradar_db::WORKER_LOCK_KEY;
use flakeradar_db::entities::projects;
use flakeradar_test_support::{
    CapturedLog, Reply, TestDb, app, get, make_job, make_pipeline, make_project, make_report,
    make_test_case, settings,
};

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait as _};

const OLDEST: &str = "flakeradar_oldest_pending_report_age_seconds";
const LAST_PROCESSED: &str = "flakeradar_last_report_processed_timestamp_seconds";

/// Every sample line of a Prometheus body, keyed by name plus labels.
fn samples(body: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in body.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.rsplit_once(' ') else {
            continue;
        };
        let _ = out.insert(key.to_owned(), value.to_owned());
    }
    out
}

/// Asserts the scrape exposes exactly `expected` series.
///
/// A value of `None` checks presence only, for the float-valued samples.
fn assert_series(body: &str, expected: &[(&str, Option<&str>)]) -> BTreeMap<String, String> {
    let got = samples(body);
    let mut wanted: Vec<&str> = expected.iter().map(|(key, _)| *key).collect();
    wanted.sort_unstable();
    assert_eq!(
        got.keys().map(String::as_str).collect::<Vec<_>>(),
        wanted,
        "{body}"
    );
    for (key, value) in expected {
        if let Some(value) = value {
            assert_eq!(got.get(*key).map(String::as_str), Some(*value), "{key}");
        }
    }
    got
}

/// Seeds `count` pending Reports, the oldest of them 90 seconds old.
async fn seed_pending_reports(db: &DatabaseConnection, count: usize) -> projects::Model {
    let project = make_project()
        .project("backend")
        .build(db)
        .await
        .expect("a Project");
    let mut oldest = None;
    for index in 0..count {
        let report = make_report(&project, project.repo_id)
            .commit_sha(&format!("sha{index}"))
            .build(db)
            .await
            .expect("a Report");
        oldest.get_or_insert(report.id);
    }
    // The server's own clock, so the age the probe reports is exact.
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE reports SET created_at = now() - interval '90 seconds' WHERE id = $1",
        [oldest.expect("at least one Report was seeded").into()],
    );
    db.execute_raw(statement)
        .await
        .expect("the oldest Report is backdated");
    project
}

/// An app whose pool has been closed, so every query fails.
async fn closed_app() -> Router {
    let db = TestDb::new().await.expect("a test database is available");
    let pool = db.connection().clone();
    let app = build_app(
        AppState::new(pool.clone(), std::sync::Arc::new(settings(&[])))
            .expect("the app state builds"),
    );
    pool.close().await.expect("the pool closes");
    app
}

/// The app over `db`.
fn app_for(db: &TestDb) -> Router {
    app(db, &[])
}

#[tokio::test]
async fn ready_reports_the_pending_queue() {
    let db = TestDb::new().await.expect("a test database is available");
    seed_pending_reports(db.connection(), 2).await;
    let reply = get(&app_for(&db), "/api/health/ready").await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["database"], serde_json::json!(true));
    assert_eq!(body["processor_leader"], serde_json::json!(false));
    assert_eq!(body["pending_reports"], serde_json::json!(2));
    let age = body["oldest_pending_age_seconds"]
        .as_f64()
        .expect("the age is a number");
    assert!(age >= 90.0, "{age}");
}

#[tokio::test]
async fn ready_reports_the_processor_leader_lock() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = app_for(&db);
    let lock = db.connection().begin().await.expect("a transaction");
    let taken: bool = lock
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_try_advisory_lock($1) AS locked",
            [WORKER_LOCK_KEY.into()],
        ))
        .await
        .expect("the lock query runs")
        .expect("the lock query returns a row")
        .try_get("", "locked")
        .expect("the answer is a boolean");
    assert!(taken);

    let reply = get(&app, "/api/health/ready").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.json()["processor_leader"],
        serde_json::json!(true),
        "{}",
        reply.text()
    );
    // A leader is reported, never a 503: during a redeploy the lock is briefly
    // held by nobody. The lock is session-scoped, so ending the transaction does
    // not release it.
    lock.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_unlock($1)",
        [WORKER_LOCK_KEY.into()],
    ))
    .await
    .expect("the lock is released");
    lock.rollback().await.expect("the transaction ends");
    let reply = get(&app, "/api/health/ready").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["processor_leader"], serde_json::json!(false));
}

#[tokio::test]
async fn ready_ignores_a_leader_lock_held_in_another_database() {
    // pg_locks is cluster-wide, so an unscoped probe would report a leader this
    // environment does not have.
    let db = TestDb::new().await.expect("a test database is available");
    let other = flakeradar_db::connect(db.maintenance_url(), flakeradar_db::DbSettings::default())
        .await
        .expect("the maintenance database is reachable");
    assert!(
        !db.url().ends_with("/postgres"),
        "the test database must not be the maintenance database"
    );
    let lock = other.begin().await.expect("a transaction");
    let taken: bool = lock
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_try_advisory_lock($1) AS locked",
            [WORKER_LOCK_KEY.into()],
        ))
        .await
        .expect("the lock query runs")
        .expect("the lock query returns a row")
        .try_get("", "locked")
        .expect("the answer is a boolean");
    assert!(taken);

    let app = app_for(&db);
    let reply = get(&app, "/api/health/ready").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["processor_leader"], serde_json::json!(false));
    let reply = get(&app, "/metrics").await;
    assert_eq!(
        samples(&reply.text())
            .get("flakeradar_processor_leader")
            .map(String::as_str),
        Some("0")
    );
    lock.rollback().await.expect("the transaction ends");
}

#[tokio::test]
async fn ready_reports_a_missing_database_rather_than_failing() {
    let reply = get(&closed_app().await, "/api/health/ready").await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.json(), serde_json::json!({"database": false}));
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("application/json")
    );
}

#[tokio::test]
async fn ready_answers_a_plain_text_500_when_the_queue_query_fails() {
    // `SELECT 1` answers, so the probe goes on to the queue query, which reads
    // `reports`; a database without that table fails exactly there, which is
    // the plain-text 500 a query outside the route's error handling becomes.
    let db = TestDb::new().await.expect("a test database is available");
    db.connection()
        .execute_raw(Statement::from_string(
            DbBackend::Postgres,
            "DROP TABLE reports",
        ))
        .await
        .expect("the reports table is dropped");

    let reply = get(&app_for(&db), "/api/health/ready").await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(reply.text(), "Internal Server Error");
}

#[tokio::test]
async fn metrics_names_every_series_with_its_labels() {
    let db = TestDb::new().await.expect("a test database is available");
    let project = seed_pending_reports(db.connection(), 1).await;
    // One Report per status; the processed one carries processed_at.
    let failed = make_report(&project, project.repo_id)
        .commit_sha("failed")
        .status("failed")
        .build(db.connection())
        .await
        .expect("a Report");
    let processed = make_report(&project, project.repo_id)
        .commit_sha("done")
        .status("processed")
        .build(db.connection())
        .await
        .expect("a Report");
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE reports SET processed_at = now() WHERE id = $1",
            [processed.id.into()],
        ))
        .await
        .expect("the processed Report is stamped");
    assert_eq!(failed.status, ReportStatus::Failed.as_str());

    // Cached tiers: at or above the 0.30 threshold is flaky, above 0 suspect, 0 stable.
    for (name, score) in [("t_flaky", 0.5), ("t_suspect", 0.1), ("t_stable", 0.0)] {
        make_test_case(&project)
            .name(name)
            .flakiness_score(score)
            .build(db.connection())
            .await
            .expect("a Test");
    }
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    for (name, score) in [("j_flaky", 0.4), ("j_stable", 0.0)] {
        make_job(&pipeline)
            .name(name)
            .flakiness_score(score)
            .build(db.connection())
            .await
            .expect("a Job");
    }

    let reply = get(&app_for(&db), "/metrics").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("text/plain; version=0.0.4; charset=utf-8")
    );
    let got = assert_series(
        &reply.text(),
        &[
            ("flakeradar_reports{status=\"pending\"}", Some("1")),
            ("flakeradar_reports{status=\"processed\"}", Some("1")),
            ("flakeradar_reports{status=\"failed\"}", Some("1")),
            (OLDEST, None),
            ("flakeradar_processor_leader", Some("0")),
            ("flakeradar_tests{tier=\"flaky\"}", Some("1")),
            ("flakeradar_tests{tier=\"suspect\"}", Some("1")),
            ("flakeradar_tests{tier=\"stable\"}", Some("1")),
            ("flakeradar_jobs{tier=\"flaky\"}", Some("1")),
            ("flakeradar_jobs{tier=\"suspect\"}", Some("0")),
            ("flakeradar_jobs{tier=\"stable\"}", Some("1")),
            (LAST_PROCESSED, None),
        ],
    );
    let oldest: f64 = got[OLDEST].parse().expect("the age is a number");
    assert!(oldest >= 90.0, "{oldest}");
    let last_processed: f64 = got[LAST_PROCESSED].parse().expect("the time is a number");
    assert!(last_processed > 0.0, "{last_processed}");
    // Every family declares its type exactly once.
    let text = reply.text();
    for metric in [
        "flakeradar_reports",
        OLDEST,
        "flakeradar_tests",
        "flakeradar_jobs",
        LAST_PROCESSED,
    ] {
        assert_eq!(
            text.matches(&format!("# TYPE {metric} gauge")).count(),
            1,
            "{metric}"
        );
    }
}

#[tokio::test]
async fn metrics_tiers_classify_like_tier_for() {
    // The scrape's CASE must classify exactly like the Flake threshold rule every
    // API tier uses, so a future edit to either boundary fails here.
    let db = TestDb::new().await.expect("a test database is available");
    let threshold: f64 = 0.30;
    let scores = [0.0, f64::from_bits(threshold.to_bits() - 1), threshold, 1.0];
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    for (index, score) in scores.iter().enumerate() {
        make_test_case(&project)
            .name(&format!("t{index}"))
            .flakiness_score(*score)
            .build(db.connection())
            .await
            .expect("a Test");
        make_job(&pipeline)
            .name(&format!("j{index}"))
            .flakiness_score(*score)
            .build(db.connection())
            .await
            .expect("a Job");
    }
    let expected = tier_counts(&scores, threshold);

    let reply = get(
        &app(&db, &[("FLAKERADAR_FLAKE_THRESHOLD", "0.3")]),
        "/metrics",
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    let got = samples(&reply.text());
    for tier in ["flaky", "suspect", "stable"] {
        let count = expected.get(tier).copied().unwrap_or_default().to_string();
        assert_eq!(
            got.get(&format!("flakeradar_tests{{tier=\"{tier}\"}}"))
                .map(String::as_str),
            Some(count.as_str()),
            "{tier}"
        );
        assert_eq!(
            got.get(&format!("flakeradar_jobs{{tier=\"{tier}\"}}"))
                .map(String::as_str),
            Some(count.as_str()),
            "{tier}"
        );
    }
}

/// The tier each score falls in, counted, by the rule the API uses.
fn tier_counts(scores: &[f64], threshold: f64) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for score in scores {
        let score = Score::new(*score).expect("the test states a score");
        let tier = flakeradar_core::helpers::tier_for(score, threshold)
            .as_str()
            .to_owned();
        *counts.entry(tier).or_insert(0) += 1;
    }
    counts
}

#[tokio::test]
async fn metrics_reports_zeroes_and_no_processed_timestamp_on_an_empty_instance() {
    let db = TestDb::new().await.expect("a test database is available");
    let reply = get(&app_for(&db), "/metrics").await;
    assert_eq!(reply.status, StatusCode::OK);
    let got = assert_series(
        &reply.text(),
        &[
            ("flakeradar_reports{status=\"pending\"}", Some("0")),
            ("flakeradar_reports{status=\"processed\"}", Some("0")),
            ("flakeradar_reports{status=\"failed\"}", Some("0")),
            (OLDEST, Some("0.0")),
            ("flakeradar_processor_leader", Some("0")),
            ("flakeradar_tests{tier=\"flaky\"}", Some("0")),
            ("flakeradar_tests{tier=\"suspect\"}", Some("0")),
            ("flakeradar_tests{tier=\"stable\"}", Some("0")),
            ("flakeradar_jobs{tier=\"flaky\"}", Some("0")),
            ("flakeradar_jobs{tier=\"suspect\"}", Some("0")),
            ("flakeradar_jobs{tier=\"stable\"}", Some("0")),
        ],
    );
    // A 0 would falsely mean the epoch, so the sample is absent until the first
    // Report is processed; its HELP/TYPE header stays, keeping the scrape stable.
    assert!(!got.contains_key(LAST_PROCESSED));
    assert!(
        reply
            .text()
            .contains(&format!("# TYPE {LAST_PROCESSED} gauge"))
    );
}

#[tokio::test]
async fn metrics_reports_a_missing_database_with_an_empty_body() {
    let reply: Reply = get(&closed_app().await, "/metrics").await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.text(), "");
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("text/plain; version=0.0.4; charset=utf-8")
    );
}

#[tokio::test]
/// The metrics scrape's unreadable-table failure, which only the log can carry.
async fn metrics_logs_the_error_it_answers_503_for() {
    // This one is a log requirement, not a log scrape: the 503 body is empty by
    // contract, so the reason the scrape failed has no carrier a test could read.
    let db = TestDb::new().await.expect("a test database is available");
    db.connection()
        .execute_raw(Statement::from_string(
            DbBackend::Postgres,
            "DROP TABLE reports".to_owned(),
        ))
        .await
        .expect("the reports table is dropped");
    let app = app_for(&db);

    let logged = CapturedLog::during(async {
        let reply: Reply = get(&app, "/metrics").await;
        assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(reply.text(), "");
    })
    .await;
    let warnings: Vec<&str> = logged
        .lines()
        .filter(|line| line.contains("metrics scrape failed"))
        .collect();
    assert_eq!(warnings.len(), 1, "{logged}");
    assert!(warnings[0].contains("reports"), "{logged}");
}
