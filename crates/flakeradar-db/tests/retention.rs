//! Retention: old processed Reports, Executions and Score history go; the rest stays.
//!
//! Every test runs against a database of its own. "Old" is a timestamp the test
//! states, so nothing here waits for a retention window to pass.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a fixture or a prune round does not behave"
)]

use std::sync::{Arc, Mutex};

use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use flakeradar_db::entities::{
    job_executions, job_score_history, reports, test_cases, test_executions, test_runs,
    test_score_history,
};
use flakeradar_db::{DbError, FixedClock, ProcessingSettings, RetentionSettings, prune, run_prune};
use flakeradar_test_support::{
    ScriptedMonotonic, TestDb, make_execution, make_job, make_job_execution, make_job_score,
    make_pipeline, make_project, make_report, make_run, make_test_case, make_test_score,
};
use sea_orm::{ConnectionTrait as _, EntityTrait, QueryOrder as _};

/// The instant every test measures its windows from.
fn now() -> DateTime<FixedOffset> {
    Utc::now().fixed_offset()
}

/// The processor settings a prune rescores with, on a clock fixed at `now`.
fn processing(now: DateTime<FixedOffset>) -> ProcessingSettings {
    ProcessingSettings {
        clock: Arc::new(FixedClock::new(now)),
        ..ProcessingSettings::default()
    }
}

/// The retention settings a test starts from: the shipped defaults.
fn retention() -> RetentionSettings {
    RetentionSettings::default()
}

/// An instant `days` before `now`.
fn days_before(now: DateTime<FixedOffset>, days: i64) -> DateTime<FixedOffset> {
    now - TimeDelta::days(days)
}

/// How many rows the entity's table holds.
async fn count<E>(db: &TestDb) -> u64
where
    E: EntityTrait,
{
    u64::try_from(
        E::find()
            .all(db.connection())
            .await
            .expect("the rows read")
            .len(),
    )
    .unwrap_or(u64::MAX)
}

#[tokio::test]
async fn prune_deletes_only_expired_rows() {
    let now = now();
    let old = days_before(now, 100);
    let recent = days_before(now, 1);
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    let test = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("a Test");

    let old_run = make_run(&project)
        .commit_sha("old")
        .created_at(old)
        .build(db.connection())
        .await
        .expect("an old Run");
    make_execution(&test, &old_run)
        .created_at(old)
        .build(db.connection())
        .await
        .expect("an old Execution");
    let new_run = make_run(&project)
        .commit_sha("new")
        .created_at(recent)
        .build(db.connection())
        .await
        .expect("a recent Run");
    make_execution(&test, &new_run)
        .created_at(recent)
        .build(db.connection())
        .await
        .expect("a recent Execution");

    let stale = make_report(&project, project.repo_id)
        .status("processed")
        .processed_at(days_before(now, 8))
        .build(db.connection())
        .await
        .expect("a stale Report");
    let _ = stale;
    let fresh = make_report(&project, project.repo_id)
        .status("processed")
        .processed_at(days_before(now, 6))
        .build(db.connection())
        .await
        .expect("a fresh Report");
    let failed = make_report(&project, project.repo_id)
        .status("failed")
        .processed_at(days_before(now, 30))
        .build(db.connection())
        .await
        .expect("a failed Report");

    let result = prune(
        db.connection(),
        now,
        &RetentionSettings {
            report_days: 7,
            execution_days: 90,
            ..retention()
        },
        &processing(now),
    )
    .await
    .expect("the prune runs");

    assert_eq!(
        (result.reports, result.executions, result.runs),
        (1, 1, 1),
        "{result:?}"
    );
    let mut remaining: Vec<i32> = reports::Entity::find()
        .order_by_asc(reports::Column::Id)
        .all(db.connection())
        .await
        .expect("the Reports read")
        .into_iter()
        .map(|report| report.id)
        .collect();
    remaining.sort_unstable();
    let mut expected = vec![fresh.id, failed.id];
    expected.sort_unstable();
    assert_eq!(remaining, expected);
    assert_eq!(count::<test_executions::Entity>(&db).await, 1);
    let shas: Vec<String> = test_runs::Entity::find()
        .order_by_asc(test_runs::Column::Id)
        .all(db.connection())
        .await
        .expect("the Runs read")
        .into_iter()
        .map(|run| run.commit_sha)
        .collect();
    assert_eq!(shas, ["new"]);
}

/// A stale processed Report plus failed Reports around a 7-day cutoff.
///
/// `orphaned` never got a `processed_at`, so its age comes from `created_at`.
async fn seed_failed_report_ages(db: &TestDb, now: DateTime<FixedOffset>) -> (i32, i32, i32, i32) {
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    let stale = make_report(&project, project.repo_id)
        .status("processed")
        .processed_at(days_before(now, 8))
        .build(db.connection())
        .await
        .expect("a stale Report");
    let old_failed = make_report(&project, project.repo_id)
        .status("failed")
        .processed_at(days_before(now, 8))
        .build(db.connection())
        .await
        .expect("an old failed Report");
    let recent_failed = make_report(&project, project.repo_id)
        .status("failed")
        .processed_at(days_before(now, 1))
        .build(db.connection())
        .await
        .expect("a recent failed Report");
    let mut orphaned = make_report(&project, project.repo_id)
        .status("failed")
        .build(db.connection())
        .await
        .expect("an orphaned failed Report");
    // The factory writes `created_at` as now; this row's age is its ingest time.
    orphaned.created_at = days_before(now, 8);
    reports::Entity::update(reports::ActiveModel {
        id: sea_orm::Set(orphaned.id),
        created_at: sea_orm::Set(orphaned.created_at),
        ..Default::default()
    })
    .exec(db.connection())
    .await
    .expect("the ingest time is stored");
    (stale.id, old_failed.id, recent_failed.id, orphaned.id)
}

#[tokio::test]
async fn prune_keeps_failed_reports_when_the_setting_is_not_set() {
    let now = now();
    let db = TestDb::new().await.expect("a test database");
    let (_stale, old_failed, recent_failed, orphaned) = seed_failed_report_ages(&db, now).await;

    let result = prune(
        db.connection(),
        now,
        &RetentionSettings {
            report_days: 7,
            execution_days: 90,
            ..retention()
        },
        &processing(now),
    )
    .await
    .expect("the prune runs");

    assert_eq!((result.reports, result.failed_reports), (1, 0));
    let remaining: Vec<i32> = reports::Entity::find()
        .all(db.connection())
        .await
        .expect("the Reports read")
        .into_iter()
        .map(|report| report.id)
        .collect();
    let mut expected = vec![old_failed, recent_failed, orphaned];
    expected.sort_unstable();
    let mut remaining = remaining;
    remaining.sort_unstable();
    assert_eq!(remaining, expected);
}

#[tokio::test]
async fn prune_deletes_expired_failed_reports_when_the_setting_is_set() {
    let now = now();
    let db = TestDb::new().await.expect("a test database");
    let (_stale, _old_failed, recent_failed, _orphaned) = seed_failed_report_ages(&db, now).await;

    let result = prune(
        db.connection(),
        now,
        &RetentionSettings {
            report_days: 7,
            failed_report_days: 7,
            execution_days: 90,
            ..retention()
        },
        &processing(now),
    )
    .await
    .expect("the prune runs");

    // The two failed Reports older than the cutoff go, the recent one stays.
    assert_eq!((result.reports, result.failed_reports), (1, 2));
    let remaining: Vec<i32> = reports::Entity::find()
        .all(db.connection())
        .await
        .expect("the Reports read")
        .into_iter()
        .map(|report| report.id)
        .collect();
    assert_eq!(remaining, [recent_failed]);
}

#[tokio::test]
async fn run_prune_applies_the_failed_report_setting() {
    let now = now();
    let db = TestDb::new().await.expect("a test database");
    let (_stale, _old_failed, recent_failed, _orphaned) = seed_failed_report_ages(&db, now).await;
    let log = CapturedLog::install();

    let result = run_prune(
        db.connection(),
        &RetentionSettings {
            report_days: 7,
            failed_report_days: 7,
            ..retention()
        },
        &processing(now),
        &ScriptedMonotonic::new([0.0]),
    )
    .await
    .expect("the prune runs");

    assert_eq!((result.reports, result.failed_reports), (1, 2));
    assert!(
        log.lines().contains("2 failed reports"),
        "the run reports the failed Reports it deleted: {}",
        log.lines()
    );
    let remaining: Vec<i32> = reports::Entity::find()
        .all(db.connection())
        .await
        .expect("the Reports read")
        .into_iter()
        .map(|report| report.id)
        .collect();
    assert_eq!(remaining, [recent_failed]);
}

#[tokio::test]
async fn run_prune_keeps_failed_reports_at_the_default_setting() {
    let now = now();
    let db = TestDb::new().await.expect("a test database");
    let (_stale, old_failed, recent_failed, orphaned) = seed_failed_report_ages(&db, now).await;
    assert_eq!(
        retention().failed_report_days,
        0,
        "the shipped default keeps them forever"
    );

    let result = run_prune(
        db.connection(),
        &RetentionSettings {
            report_days: 7,
            ..retention()
        },
        &processing(now),
        &ScriptedMonotonic::new([0.0]),
    )
    .await
    .expect("the prune runs");

    assert_eq!((result.reports, result.failed_reports), (1, 0));
    let mut remaining: Vec<i32> = reports::Entity::find()
        .all(db.connection())
        .await
        .expect("the Reports read")
        .into_iter()
        .map(|report| report.id)
        .collect();
    remaining.sort_unstable();
    let mut expected = vec![old_failed, recent_failed, orphaned];
    expected.sort_unstable();
    assert_eq!(remaining, expected);
}

#[tokio::test]
async fn run_prune_batches_failed_report_deletes() {
    let now = now();
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    for _ in 0..5 {
        make_report(&project, project.repo_id)
            .status("failed")
            .processed_at(days_before(now, 8))
            .build(db.connection())
            .await
            .expect("a failed Report");
    }

    let result = run_prune(
        db.connection(),
        &RetentionSettings {
            batch: 2,
            failed_report_days: 7,
            ..retention()
        },
        &processing(now),
        &ScriptedMonotonic::new([0.0]),
    )
    .await
    .expect("the prune runs");

    // A batch of 2 means three rounds: 2 + 2 + 1 rows. One round would leave 3.
    assert_eq!(result.failed_reports, 5, "{result:?}");
    assert_eq!(count::<reports::Entity>(&db).await, 0);
}

#[tokio::test]
async fn prune_clears_expired_quarantines_only() {
    let now = now();
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    let expired = make_test_case(&project)
        .name("expired")
        .quarantined_at(days_before(now, 20))
        .quarantine_reason("flaky on arm")
        .quarantine_expires_at(days_before(now, 1))
        .build(db.connection())
        .await
        .expect("an expired quarantine");
    let future = make_test_case(&project)
        .name("future")
        .quarantined_at(days_before(now, 2))
        .quarantine_reason("waiting on upstream")
        .quarantine_expires_at(now + TimeDelta::days(5))
        .build(db.connection())
        .await
        .expect("a live quarantine");
    let future_expiry = future.quarantine_expires_at;
    let never = make_test_case(&project)
        .name("never")
        .quarantined_at(now)
        .quarantine_reason("owner decided")
        .build(db.connection())
        .await
        .expect("a quarantine with no expiry");
    let log = CapturedLog::install();

    let result = prune(db.connection(), now, &retention(), &processing(now))
        .await
        .expect("the prune runs");

    assert_eq!(result.expired_quarantines, 1, "{result:?}");
    let cleared = test_cases::Entity::find_by_id(expired.id)
        .one(db.connection())
        .await
        .expect("the Test reads")
        .expect("the Test exists");
    assert!(!cleared.quarantined);
    assert_eq!(cleared.quarantined_at, None);
    assert_eq!(cleared.quarantine_reason, None);
    assert_eq!(cleared.quarantine_expires_at, None);
    let kept_future = test_cases::Entity::find_by_id(future.id)
        .one(db.connection())
        .await
        .expect("the Test reads")
        .expect("the Test exists");
    assert!(kept_future.quarantined);
    assert_eq!(
        kept_future.quarantine_reason.as_deref(),
        Some("waiting on upstream")
    );
    assert_eq!(kept_future.quarantine_expires_at, future_expiry);
    let kept_never = test_cases::Entity::find_by_id(never.id)
        .one(db.connection())
        .await
        .expect("the Test reads")
        .expect("the Test exists");
    assert!(kept_never.quarantined);
    assert_eq!(
        kept_never.quarantine_reason.as_deref(),
        Some("owner decided")
    );
    assert_eq!(kept_never.quarantine_expires_at, None);
    assert!(
        log.lines().contains(&format!("test_id={}", expired.id)),
        "the clear names the Test: {}",
        log.lines()
    );
}

#[tokio::test]
async fn an_expiry_round_clears_every_due_quarantine_in_one_statement() {
    let now = now();
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    for name in ["one", "two", "three"] {
        make_test_case(&project)
            .name(name)
            .quarantined_at(days_before(now, 30))
            .quarantine_reason("flaky on arm")
            .quarantine_expires_at(days_before(now, 1))
            .build(db.connection())
            .await
            .expect("an expired quarantine");
    }
    audit_test_case_updates(&db).await;

    let result = prune(db.connection(), now, &retention(), &processing(now))
        .await
        .expect("the prune runs");

    assert_eq!(result.expired_quarantines, 3, "{result:?}");
    // The audit counts UPDATE statements, not rows: one statement clears every
    // due quarantine, so the number of statements stays 1 whatever the backlog.
    let statements = test_case_updates(&db).await;
    assert_eq!(
        statements, 1,
        "one statement clears every due quarantine, not one per Test"
    );
}

/// Counts the `UPDATE` statements a prune round sends at `test_cases`.
///
/// A statement-level trigger fires once per `UPDATE` whatever the row count, so
/// a step that issues one statement per row is visible as a count rather than as
/// a duration. The audit table and the trigger are created after the fixtures, so
/// nothing the test itself writes is counted.
async fn audit_test_case_updates(db: &TestDb) {
    db.connection()
        .execute_unprepared(
            "CREATE TABLE test_case_update_audit (id SERIAL);
             CREATE FUNCTION audit_test_case_update() RETURNS TRIGGER
                LANGUAGE plpgsql AS $$
                BEGIN
                    INSERT INTO test_case_update_audit DEFAULT VALUES;
                    RETURN NULL;
                END $$;
             CREATE TRIGGER audit_test_case_update
                AFTER UPDATE ON test_cases
                FOR EACH STATEMENT EXECUTE FUNCTION audit_test_case_update();",
        )
        .await
        .expect("the update audit is installed");
}

/// How many `UPDATE` statements the audit saw.
async fn test_case_updates(db: &TestDb) -> i64 {
    let row = db
        .connection()
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT count(*) AS statements FROM test_case_update_audit",
        ))
        .await
        .expect("the audit reads")
        .expect("a count row");
    row.try_get::<i64>("", "statements").expect("a count")
}

#[tokio::test]
async fn prune_deletes_old_job_executions() {
    let now = now();
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("a Job");
    make_job_execution(&job)
        .ci_job_id("old")
        .created_at(days_before(now, 100))
        .build(db.connection())
        .await
        .expect("an old Job execution");
    make_job_execution(&job)
        .ci_job_id("new")
        .created_at(days_before(now, 1))
        .build(db.connection())
        .await
        .expect("a recent Job execution");

    let result = prune(db.connection(), now, &retention(), &processing(now))
        .await
        .expect("the prune runs");

    assert_eq!(result.job_executions, 1);
    let remaining: Vec<String> = job_executions::Entity::find()
        .order_by_asc(job_executions::Column::Id)
        .all(db.connection())
        .await
        .expect("the Job executions read")
        .into_iter()
        .map(|execution| execution.ci_job_id)
        .collect();
    assert_eq!(remaining, ["new"]);
}

#[tokio::test]
async fn prune_deletes_old_score_history_rows() {
    let now = now();
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    let test = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("a Test");
    make_test_score(&test, days_before(now, 400).date_naive(), 0.5)
        .build(db.connection())
        .await
        .expect("an old Score history row");
    make_test_score(&test, days_before(now, 10).date_naive(), 0.5)
        .build(db.connection())
        .await
        .expect("a recent Score history row");

    let result = prune(
        db.connection(),
        now,
        &RetentionSettings {
            history_days: 365,
            ..retention()
        },
        &processing(now),
    )
    .await
    .expect("the prune runs");

    assert_eq!(result.test_score_history, 1);
    assert_eq!(result.score_history(), 1);
    assert_eq!(count::<test_score_history::Entity>(&db).await, 1);
}

#[tokio::test]
async fn prune_deletes_old_job_score_history_rows() {
    let now = now();
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("a Job");
    make_job_score(&job, days_before(now, 400).date_naive(), 0.5)
        .build(db.connection())
        .await
        .expect("an old Score history row");
    make_job_score(&job, days_before(now, 10).date_naive(), 0.5)
        .build(db.connection())
        .await
        .expect("a recent Score history row");

    let result = prune(
        db.connection(),
        now,
        &RetentionSettings {
            history_days: 365,
            ..retention()
        },
        &processing(now),
    )
    .await
    .expect("the prune runs");

    assert_eq!(result.job_score_history, 1);
    assert_eq!(count::<job_score_history::Entity>(&db).await, 1);
}

#[tokio::test]
async fn prune_rescores_test_with_no_executions_left() {
    let now = now();
    let old = days_before(now, 100);
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    let test = make_test_case(&project)
        .flakiness_score(0.5)
        .clean_streak(3)
        .build(db.connection())
        .await
        .expect("a Test");
    let run = make_run(&project)
        .commit_sha("old")
        .created_at(old)
        .build(db.connection())
        .await
        .expect("an old Run");
    make_execution(&test, &run)
        .created_at(old)
        .build(db.connection())
        .await
        .expect("an old Execution");

    let result = prune(db.connection(), now, &retention(), &processing(now))
        .await
        .expect("the prune runs");

    assert_eq!(result.executions, 1);
    let row = test_cases::Entity::find_by_id(test.id)
        .one(db.connection())
        .await
        .expect("the Test reads")
        .expect("the Test exists");
    assert!(row.flakiness_score.abs() < f64::EPSILON, "{row:?}");
    assert_eq!(row.clean_streak, 0);
}

#[tokio::test]
async fn prune_logs_a_count_of_the_notifications_it_drops() {
    // Pruning can move a Test or Job out of the flaky tier, silently except for
    // this count: losing old Executions is not news.
    let now = now();
    let old = days_before(now, 100);
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    let test = make_test_case(&project)
        .flakiness_score(0.5)
        .build(db.connection())
        .await
        .expect("a Test");
    let run = make_run(&project)
        .commit_sha("old")
        .created_at(old)
        .build(db.connection())
        .await
        .expect("an old Run");
    make_execution(&test, &run)
        .created_at(old)
        .build(db.connection())
        .await
        .expect("an old Execution");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let job = make_job(&pipeline)
        .flakiness_score(0.5)
        .build(db.connection())
        .await
        .expect("a Job");
    make_job_execution(&job)
        .ci_job_id("old")
        .created_at(old)
        .build(db.connection())
        .await
        .expect("an old Job execution");
    let log = CapturedLog::install();

    prune(db.connection(), now, &retention(), &processing(now))
        .await
        .expect("the prune runs");

    assert!(
        log.lines().contains("dropped=2"),
        "the prune counts both dropped Notifications: {}",
        log.lines()
    );
}

#[tokio::test]
async fn prune_rescores_test_from_remaining_executions() {
    let now = now();
    let old = days_before(now, 100);
    let recent = days_before(now, 1);
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    let test = make_test_case(&project)
        .flakiness_score(0.9)
        .clean_streak(3)
        .build(db.connection())
        .await
        .expect("a Test");
    for (sha, status) in [("old1", "failed"), ("old2", "passed")] {
        let run = make_run(&project)
            .commit_sha(sha)
            .created_at(old)
            .build(db.connection())
            .await
            .expect("an old Run");
        make_execution(&test, &run)
            .status(status)
            .created_at(old)
            .build(db.connection())
            .await
            .expect("an old Execution");
    }
    for sha in ["new1", "new2"] {
        let run = make_run(&project)
            .commit_sha(sha)
            .created_at(recent)
            .build(db.connection())
            .await
            .expect("a recent Run");
        make_execution(&test, &run)
            .created_at(recent)
            .build(db.connection())
            .await
            .expect("a recent Execution");
    }

    let result = prune(db.connection(), now, &retention(), &processing(now))
        .await
        .expect("the prune runs");

    assert_eq!(result.executions, 2);
    let row = test_cases::Entity::find_by_id(test.id)
        .one(db.connection())
        .await
        .expect("the Test reads")
        .expect("the Test exists");
    // Not the stale 0.9: the two remaining passes score 0.
    assert!(row.flakiness_score.abs() < f64::EPSILON, "{row:?}");
    assert_eq!(row.clean_streak, 2);
}

#[tokio::test]
async fn prune_rescores_job_with_no_executions_left() {
    let now = now();
    let old = days_before(now, 100);
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let job = make_job(&pipeline)
        .flakiness_score(0.5)
        .clean_streak(3)
        .build(db.connection())
        .await
        .expect("a Job");
    make_job_execution(&job)
        .ci_job_id("old")
        .created_at(old)
        .build(db.connection())
        .await
        .expect("an old Job execution");

    let result = prune(db.connection(), now, &retention(), &processing(now))
        .await
        .expect("the prune runs");

    assert_eq!(result.job_executions, 1);
    let row = flakeradar_db::entities::jobs::Entity::find_by_id(job.id)
        .one(db.connection())
        .await
        .expect("the Job reads")
        .expect("the Job exists");
    assert!(row.flakiness_score.abs() < f64::EPSILON, "{row:?}");
    assert_eq!(row.clean_streak, 0);
}

#[tokio::test]
async fn prune_rescores_job_from_remaining_executions() {
    let now = now();
    let old = days_before(now, 100);
    let recent = days_before(now, 1);
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let job = make_job(&pipeline)
        .flakiness_score(0.9)
        .clean_streak(3)
        .build(db.connection())
        .await
        .expect("a Job");
    make_job_execution(&job)
        .status("failed")
        .ci_job_id("old")
        .created_at(old)
        .build(db.connection())
        .await
        .expect("an old Job execution");
    for ci_job_id in ["new1", "new2"] {
        make_job_execution(&job)
            .ci_job_id(ci_job_id)
            .created_at(recent)
            .build(db.connection())
            .await
            .expect("a recent Job execution");
    }

    let result = prune(db.connection(), now, &retention(), &processing(now))
        .await
        .expect("the prune runs");

    assert_eq!(result.job_executions, 1);
    let row = flakeradar_db::entities::jobs::Entity::find_by_id(job.id)
        .one(db.connection())
        .await
        .expect("the Job reads")
        .expect("the Job exists");
    // Not the stale 0.9: the two remaining passes score 0.
    assert!(row.flakiness_score.abs() < f64::EPSILON, "{row:?}");
    assert_eq!(row.clean_streak, 2);
}

#[tokio::test]
async fn run_prune_deletes_in_batches() {
    let now = now();
    let old = days_before(now, 100);
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    let test = make_test_case(&project)
        .flakiness_score(0.5)
        .clean_streak(3)
        .build(db.connection())
        .await
        .expect("a Test");
    let run = make_run(&project)
        .commit_sha("old")
        .created_at(old)
        .build(db.connection())
        .await
        .expect("an old Run");
    for _ in 0..5 {
        make_execution(&test, &run)
            .created_at(old)
            .build(db.connection())
            .await
            .expect("an old Execution");
    }

    let result = run_prune(
        db.connection(),
        &RetentionSettings {
            batch: 2,
            ..retention()
        },
        &processing(now),
        &ScriptedMonotonic::new([0.0]),
    )
    .await
    .expect("the prune runs");

    // 2 + 2 + 1 rows: the loop continues while a round deletes a full batch.
    assert_eq!(result.executions, 5, "{result:?}");
    assert_eq!(count::<test_executions::Entity>(&db).await, 0);
    let row = test_cases::Entity::find_by_id(test.id)
        .one(db.connection())
        .await
        .expect("the Test reads")
        .expect("the Test exists");
    // Every round rescored the Test whose Executions it deleted.
    assert!(row.flakiness_score.abs() < f64::EPSILON, "{row:?}");
    assert_eq!(row.clean_streak, 0);
}

#[tokio::test]
async fn run_prune_drains_both_score_history_tables() {
    let now = now();
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    let test = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("a Test");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("a Job");
    // Composite primary keys require distinct days.
    for day in 400..405 {
        make_test_score(&test, days_before(now, day).date_naive(), 0.5)
            .build(db.connection())
            .await
            .expect("an old Score history row");
        make_job_score(&job, days_before(now, day).date_naive(), 0.5)
            .build(db.connection())
            .await
            .expect("an old Score history row");
    }

    let result = run_prune(
        db.connection(),
        &RetentionSettings {
            batch: 2,
            ..retention()
        },
        &processing(now),
        &ScriptedMonotonic::new([0.0]),
    )
    .await
    .expect("the prune runs");

    // Each round deletes a full batch from BOTH history tables, so a check on
    // the sum would never equal the batch size and the loop would stop early.
    assert_eq!(result.test_score_history, 5, "{result:?}");
    assert_eq!(result.job_score_history, 5, "{result:?}");
    assert_eq!(result.score_history(), 10, "{result:?}");
    assert_eq!(count::<test_score_history::Entity>(&db).await, 0);
    assert_eq!(count::<job_score_history::Entity>(&db).await, 0);
}

#[tokio::test]
async fn run_prune_stops_at_time_budget() {
    let now = now();
    let old = days_before(now, 100);
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    let test = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("a Test");
    let run = make_run(&project)
        .commit_sha("old")
        .created_at(old)
        .build(db.connection())
        .await
        .expect("an old Run");
    for _ in 0..6 {
        make_execution(&test, &run)
            .created_at(old)
            .build(db.connection())
            .await
            .expect("an old Execution");
    }

    let settings = RetentionSettings {
        batch: 2,
        ..retention()
    };
    // The first reading sets the deadline; the later ones are past it.
    let past_deadline =
        std::iter::once(0.0).chain(std::iter::repeat_n(settings.time_budget_seconds + 1.0, 8));
    let log = CapturedLog::install();
    let result = run_prune(
        db.connection(),
        &settings,
        &processing(now),
        &ScriptedMonotonic::new(past_deadline),
    )
    .await
    .expect("running out of budget is a normal return");

    // Only the first committed batch; the backlog waits for the next interval.
    assert_eq!(result.executions, 2, "{result:?}");
    assert_eq!(count::<test_executions::Entity>(&db).await, 4);
    // The frozen server's line, budget included: "Retention prune hit its 60s
    // budget; the rest waits for the next interval". Python prints the budget
    // as a float ("60.0s") and Rust's Display drops the zero ("60s"), so only
    // the number's spelling differs, which log format may.
    let budget_stop = format!(
        "Retention prune hit its {}s budget; the rest waits for the next interval",
        settings.time_budget_seconds
    );
    assert!(
        log.lines().contains(&budget_stop),
        "the budget stop is logged with the reference's wording: {}",
        log.lines()
    );
}

#[tokio::test]
async fn a_failed_round_rolls_back_its_deletes() {
    // A round is one transaction: a statement that fails late must not leave the
    // earlier deletes committed.
    let now = now();
    let old = days_before(now, 100);
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    let test = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("a Test");
    let run = make_run(&project)
        .commit_sha("old")
        .created_at(old)
        .build(db.connection())
        .await
        .expect("an old Run");
    make_execution(&test, &run)
        .created_at(old)
        .build(db.connection())
        .await
        .expect("an old Execution");
    make_report(&project, project.repo_id)
        .status("processed")
        .processed_at(days_before(now, 8))
        .build(db.connection())
        .await
        .expect("a stale Report");
    db.connection()
        .execute_unprepared("DROP TABLE job_score_history")
        .await
        .expect("the last table the round deletes from is gone");

    let error = run_prune(
        db.connection(),
        &retention(),
        &processing(now),
        &ScriptedMonotonic::new([0.0]),
    )
    .await
    .expect_err("the missing table fails the round");
    assert!(matches!(error, DbError::Database(_)), "{error}");

    // The Report and Execution the same round had already deleted are back.
    assert_eq!(count::<reports::Entity>(&db).await, 1);
    assert_eq!(count::<test_executions::Entity>(&db).await, 1);
}

#[tokio::test]
async fn a_retention_window_past_the_calendar_is_clamped() {
    // A window wider than `chrono` can subtract is clamped rather than panicking:
    // a huge positive window expires only what predates the clamp, and a huge
    // negative one expires every processed Report.
    let now = now();
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    let ancient = make_report(&project, project.repo_id)
        .status("processed")
        .processed_at(
            DateTime::from_timestamp(-7_500_000_000, 0)
                .expect("an instant in 1732")
                .fixed_offset(),
        )
        .build(db.connection())
        .await
        .expect("an ancient Report");
    let fresh = make_report(&project, project.repo_id)
        .status("processed")
        .processed_at(now)
        .build(db.connection())
        .await
        .expect("a fresh Report");

    let clamped = prune(
        db.connection(),
        now,
        &RetentionSettings {
            report_days: i64::MAX,
            ..retention()
        },
        &processing(now),
    )
    .await
    .expect("the prune runs");
    assert_eq!(clamped.reports, 1);
    let remaining: Vec<i32> = reports::Entity::find()
        .all(db.connection())
        .await
        .expect("the Reports read")
        .into_iter()
        .map(|report| report.id)
        .collect();
    assert_eq!(
        remaining,
        [fresh.id],
        "the 1732 Report went, the fresh one stayed"
    );

    let all = prune(
        db.connection(),
        now,
        &RetentionSettings {
            report_days: i64::MIN,
            ..retention()
        },
        &processing(now),
    )
    .await
    .expect("the prune runs");
    assert_eq!(all.reports, 1);
    assert_eq!(count::<reports::Entity>(&db).await, 0);
    let _ = ancient;
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
        let guard = self
            .lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
