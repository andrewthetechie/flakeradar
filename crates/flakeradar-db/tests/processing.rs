//! The Report processor: Runs, Executions, Tests, Locations, scoring and Jobs.
//!
//! Both Report kinds, the retry budget and the answers each one leaves behind:
//! every case below is an end-state rule, and `tests/parity` replays the rest.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the processor misbehaves"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset};
use flakeradar_core::{
    dto::JobResultIn, dto::PipelineReportIn, dto::pipeline_body, helpers::is_broken,
    helpers::tier_for, model::EntityKind, model::ExecutionStatus, model::JobStatus,
    model::ReportStatus, scoring::ScoredExecution, types::CiRunId, types::Score,
};
use flakeradar_db::entities::{
    job_executions, job_score_history, jobs, pipelines, projects, reports, repos, test_cases,
    test_executions, test_runs, test_score_history,
};
use flakeradar_db::{
    get_or_create_project, processing::ProcessingSettings, processing::claim_next_report,
    processing::process_next, processing::rescore,
};
use flakeradar_test_support::{
    FixedClock, TestDb, make_job, make_job_execution, make_junit, make_pipeline, make_report,
};
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait as _, QueryFilter, QueryOrder,
    TransactionTrait as _,
};

/// A fixed instant, so every stored timestamp is deterministic.
fn at() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-10-20T09:00:00+00:00").expect("a fixed instant")
}

/// Settings on the shipped defaults, with a stopped clock.
fn settings() -> ProcessingSettings {
    ProcessingSettings {
        clock: Arc::new(FixedClock::new(at())),
        ..ProcessingSettings::default()
    }
}

/// A settings value with one field changed.
fn settings_with(change: impl FnOnce(&mut ProcessingSettings)) -> ProcessingSettings {
    let mut settings = settings();
    change(&mut settings);
    settings
}

/// Decay 0 makes the score depend
/// on one transition and a 0.05 threshold makes a crossing exact.
fn notify_settings() -> ProcessingSettings {
    settings_with(|settings| {
        settings.flake_threshold = 0.05;
        settings.score_decay = 0.0;
        settings.score_window = 50;
        settings.broken_streak = 5;
    })
}

/// [`notify_settings`] with one field changed, like `_notify_settings(**kw)`.
fn notify_settings_with(change: impl FnOnce(&mut ProcessingSettings)) -> ProcessingSettings {
    let mut settings = notify_settings();
    change(&mut settings);
    settings
}

/// Gets or creates the Project `project` in `repo`, at the fixed instant.
async fn project(db: &TestDb, repo: &str, project: &str) -> projects::Model {
    get_or_create_project(db.connection(), repo, project, at().fixed_offset())
        .await
        .expect("a Project")
}

/// Queues a JUnit Report of `body` for `project`.
async fn queue(db: &TestDb, project: &projects::Model, body: &[u8]) -> reports::Model {
    make_report(project, project.repo_id)
        .body(body)
        .create(db)
        .await
}

/// Queues a JUnit Report with extra attribution.
async fn queue_with(
    db: &TestDb,
    project: &projects::Model,
    body: &[u8],
    change: impl FnOnce(
        flakeradar_test_support::ReportBuilder,
    ) -> flakeradar_test_support::ReportBuilder,
) -> reports::Model {
    change(make_report(project, project.repo_id).body(body))
        .create(db)
        .await
}

/// Queues a Pipeline Report.
async fn queue_pipeline(
    db: &TestDb,
    project: &projects::Model,
    payload: &PipelineReportIn,
) -> reports::Model {
    make_report(project, project.repo_id)
        .kind("pipeline")
        .body(&pipeline_body(payload))
        .commit_sha(&payload.commit_sha)
        .branch(&payload.branch)
        .ci_run_id(payload.ci_run_id.as_str())
        .create(db)
        .await
}

/// Processes until the queue is empty, returning the outcomes.
async fn drain(
    db: &TestDb,
    settings: &ProcessingSettings,
) -> Vec<flakeradar_db::processing::ProcessOutcome> {
    let mut outcomes = Vec::new();
    while let Some(outcome) = process_next(db.connection(), settings)
        .await
        .expect("the processor runs")
    {
        outcomes.push(outcome);
    }
    outcomes
}

/// Makes every new Run fail with SQLSTATE 57014 (`statement_timeout`), which is
/// transient, so the processor's retry path runs.
async fn fail_test_run_inserts(db: &TestDb) {
    db.connection()
        .execute_unprepared("CREATE FUNCTION boom() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'timeout' USING ERRCODE = '57014'; END $$")
        .await
        .expect("the trigger function");
    db.connection()
        .execute_unprepared("CREATE TRIGGER boom_insert BEFORE INSERT ON test_runs FOR EACH ROW EXECUTE FUNCTION boom()")
        .await
        .expect("the trigger");
}

/// Applies the manual-retry transition to the stored Report, as the route does.
async fn apply_manual_retry(db: &TestDb, id: i32) -> reports::Model {
    let mut stored = reports::Entity::find_by_id(id)
        .one(db.connection())
        .await
        .expect("the Report")
        .expect("the Report exists");
    flakeradar_db::reset_for_retry(&mut stored);
    reports::Entity::update(reports::ActiveModel {
        id: sea_orm::Set(stored.id),
        retry_count: sea_orm::Set(stored.retry_count),
        status: sea_orm::Set(stored.status.clone()),
        error: sea_orm::Set(stored.error.clone()),
        processed_at: sea_orm::Set(stored.processed_at),
        ..Default::default()
    })
    .exec(db.connection())
    .await
    .expect("the retried Report");
    reports::Entity::find_by_id(id)
        .one(db.connection())
        .await
        .expect("the Report")
        .expect("the Report exists")
}

/// Every Test, ordered by name.
async fn tests(db: &TestDb) -> Vec<test_cases::Model> {
    test_cases::Entity::find()
        .order_by_asc(test_cases::Column::Name)
        .all(db.connection())
        .await
        .expect("the Tests")
}

/// Every Job, ordered by name.
async fn jobs(db: &TestDb) -> Vec<jobs::Model> {
    jobs::Entity::find()
        .order_by_asc(jobs::Column::Name)
        .all(db.connection())
        .await
        .expect("the Jobs")
}

/// The one Test row.
async fn test(db: &TestDb) -> test_cases::Model {
    tests(db).await.pop().expect("one Test")
}

/// The one Job row.
async fn job(db: &TestDb) -> jobs::Model {
    jobs(db).await.pop().expect("one Job")
}

/// The one Repo row.
async fn repo(db: &TestDb) -> repos::Model {
    repos::Entity::find()
        .one(db.connection())
        .await
        .expect("the Repo")
        .expect("one Repo")
}

/// One Pipeline Report Job.
fn job_in(ci_job_id: &str, name: &str, status: JobStatus) -> JobResultIn {
    JobResultIn {
        ci_job_id: ci_job_id.to_owned(),
        name: name.to_owned(),
        status,
        url: String::new(),
        runner_name: String::new(),
        runner_labels: Vec::new(),
        started_at: None,
        completed_at: None,
        summary: false,
    }
}

/// One Pipeline payload with the documented defaults.
fn payload(jobs: Vec<JobResultIn>) -> PipelineReportIn {
    PipelineReportIn {
        repo: "acme/app".to_owned(),
        provider: "github".to_owned(),
        pipeline: ".github/workflows/ci.yml".to_owned(),
        commit_sha: "sha1".to_owned(),
        branch: "main".to_owned(),
        default_branch: Some("main".to_owned()),
        ci_run_id: CiRunId::new("1").expect("an id"),
        ci_run_attempt: 1,
        jobs,
    }
}

/// The stored score, as the read layer would hand it out.
fn stored_score(value: f64) -> Score {
    Score::new(value).expect("a stored score is a score")
}

/// A JUnit Report from owned `(name, status)` pairs.
fn junit_with(cases: &[(String, &'static str)]) -> Vec<u8> {
    let borrowed: Vec<(&str, &str)> = cases
        .iter()
        .map(|(name, status)| (name.as_str(), *status))
        .collect();
    make_junit(&borrowed)
}

/// The Report the infrastructure rule reads: Tests `t0..t{total - 1}`, the first
/// `failing` of them with `status` and the rest `passed`.
fn cases(total: usize, failing: usize, status: &'static str) -> Vec<(String, &'static str)> {
    (0..total)
        .map(|index| {
            let status = if index < failing { status } else { "passed" };
            (format!("t{index}"), status)
        })
        .collect()
}

/// The Run a processed Report became.
async fn run(db: &TestDb, id: i32) -> test_runs::Model {
    test_runs::Entity::find_by_id(id)
        .one(db.connection())
        .await
        .expect("the Run")
        .expect("the Run exists")
}

#[tokio::test]
async fn an_empty_queue_returns_none() {
    let db = TestDb::new().await.expect("a test database");
    assert!(
        process_next(db.connection(), &settings())
            .await
            .expect("the processor runs")
            .is_none()
    );
}

#[tokio::test]
async fn a_report_becomes_a_run_with_its_counts() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let report = queue_with(
        &db,
        &project,
        &make_junit(&[("t1", "passed"), ("t2", "failed")]),
        |builder| builder.commit_sha("abc").branch("feat").ci_run_id("9-1"),
    )
    .await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(outcome.status, ReportStatus::Processed);
    assert_eq!(outcome.report_id, report.id);
    assert_eq!(
        outcome.counts,
        Some(serde_json::json!({
            "passed": 1, "failed": 1, "error": 0, "skipped": 0, "retried": 0
        }))
    );

    let stored = reports::Entity::find_by_id(report.id)
        .one(db.connection())
        .await
        .expect("the Report")
        .expect("the Report exists");
    assert_eq!(stored.status, "processed");
    assert_eq!(stored.run_id, outcome.run_id);
    assert_eq!(stored.counts, outcome.counts);
    assert!(stored.processed_at.is_some());
    let run = test_runs::Entity::find()
        .one(db.connection())
        .await
        .expect("the Run")
        .expect("one Run");
    assert_eq!(
        (
            run.commit_sha.as_str(),
            run.branch.as_str(),
            run.ci_run_id.as_str()
        ),
        ("abc", "feat", "9-1")
    );
    let rows = tests(&db).await;
    assert_eq!(
        rows.iter()
            .map(|row| (row.name.as_str(), row.last_status.as_str()))
            .collect::<Vec<_>>(),
        [("t1", "passed"), ("t2", "failed")]
    );
    let mut ids: Vec<i32> = rows.iter().map(|row| row.id).collect();
    ids.sort_unstable();
    assert_eq!(outcome.touched_test_ids, ids);
    let message: String = test_executions::Entity::find()
        .filter(test_executions::Column::Status.eq("failed"))
        .one(db.connection())
        .await
        .expect("the Execution")
        .expect("a failed Execution")
        .message;
    assert_eq!(message, "assert 1 == 2");
}

#[tokio::test]
async fn a_same_sha_retry_is_a_proven_flake() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_with(&db, &project, &make_junit(&[("t_flaky", "failed")]), |b| {
        b.commit_sha("dead")
    })
    .await;
    queue_with(&db, &project, &make_junit(&[("t_flaky", "passed")]), |b| {
        b.commit_sha("dead")
    })
    .await;
    drain(&db, &settings()).await;
    let row = test(&db).await;
    assert_eq!(row.confirmed_flake_count, 1);
    assert!(row.flakiness_score >= 0.6);
}

#[tokio::test]
async fn the_last_occurrence_of_a_test_wins() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let body = b"<testsuite name=\"unit\">\
        <testcase classname=\"tests.test_mod\" name=\"t\"><failure message=\"x\"/></testcase>\
        <testcase classname=\"tests.test_mod\" name=\"t\"/></testsuite>";
    queue(&db, &project, body).await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    let row = test(&db).await;
    assert_eq!(row.last_status, "passed");
    assert_eq!(row.confirmed_flake_count, 1);
    let counts = outcome.counts.expect("counts");
    assert_eq!(counts["failed"], 1);
    assert_eq!(counts["passed"], 1);
    assert_eq!(
        test_executions::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        2
    );
    let attempts: Vec<i32> = test_executions::Entity::find()
        .order_by_asc(test_executions::Column::Id)
        .all(db.connection())
        .await
        .expect("the Executions")
        .into_iter()
        .map(|row| row.attempt)
        .collect();
    assert_eq!(attempts, [0, 1]);
}

#[tokio::test]
async fn a_flaky_retry_is_a_proven_flake_from_one_report() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let body = b"<testsuite name=\"unit\"><testcase classname=\"tests.test_mod\" name=\"t\">\
        <flakyFailure message=\"Timeout 30000ms exceeded\" type=\"FAILURE\" time=\"30.1\">\
        <stackTrace>Error: boom</stackTrace></flakyFailure></testcase>\
        <testcase classname=\"c\" name=\"plain\"/></testsuite>";
    queue(&db, &project, body).await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(
        outcome.counts,
        Some(serde_json::json!({
            "passed": 2, "failed": 1, "error": 0, "skipped": 0, "retried": 1
        }))
    );
    let flaky = tests(&db)
        .await
        .into_iter()
        .find(|row| row.name == "t")
        .expect("the flaky Test");
    assert_eq!(flaky.confirmed_flake_count, 1);
    assert!(flaky.flakiness_score >= 0.6);
    let rows: Vec<(String, i32)> = test_executions::Entity::find()
        .filter(test_executions::Column::TestCaseId.eq(flaky.id))
        .order_by_asc(test_executions::Column::Id)
        .all(db.connection())
        .await
        .expect("the Executions")
        .into_iter()
        .map(|row| (row.status, row.attempt))
        .collect();
    assert_eq!(rows, [("failed".to_owned(), 0), ("passed".to_owned(), 1)]);
    assert!(
        test_executions::Entity::find()
            .filter(test_executions::Column::TestCaseId.eq(flaky.id))
            .order_by_asc(test_executions::Column::Id)
            .all(db.connection())
            .await
            .expect("the Executions")[0]
            .failure_category
            .is_some()
    );
}

#[tokio::test]
async fn failures_are_classified_and_the_test_gets_its_dominant_category() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let body = b"<testsuite name=\"s\"><testcase classname=\"c\" name=\"t\">\
        <failure message=\"Test timeout of 30000ms exceeded.\">trace</failure></testcase>\
        <testcase classname=\"c\" name=\"ok\"/></testsuite>";
    queue(&db, &project, body).await;
    process_next(db.connection(), &settings())
        .await
        .expect("the processor runs");
    let rows: BTreeMap<String, (String, Option<String>)> = test_cases::Entity::find()
        .all(db.connection())
        .await
        .expect("the Tests")
        .into_iter()
        .map(|row| (row.name, (row.last_status, row.failure_category)))
        .collect();
    assert_eq!(rows["t"], ("failed".to_owned(), Some("timing".to_owned())));
    assert_eq!(rows["ok"], ("passed".to_owned(), None));
}

#[tokio::test]
async fn the_same_name_in_two_projects_is_two_tests() {
    let db = TestDb::new().await.expect("a test database");
    let backend = project(&db, "acme/app", "backend").await;
    let frontend = project(&db, "acme/app", "frontend").await;
    queue(&db, &backend, &make_junit(&[("test_login", "passed")])).await;
    queue(&db, &frontend, &make_junit(&[("test_login", "passed")])).await;
    drain(&db, &settings()).await;
    let mut project_ids: Vec<i32> = tests(&db).await.iter().map(|row| row.project_id).collect();
    project_ids.sort_unstable();
    assert_eq!(project_ids, [backend.id, frontend.id]);
}

#[tokio::test]
async fn the_location_is_the_latest_but_is_never_erased() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let with_location = b"<testsuite name=\"s\"><testcase classname=\"c\" name=\"t\" file=\"tests/a.py\" line=\"3\"/></testsuite>";
    let moved = b"<testsuite name=\"s\"><testcase classname=\"c\" name=\"t\" file=\"tests/b.py\" line=\"9\"/></testsuite>";
    let no_location = b"<testsuite name=\"s\"><testcase classname=\"c\" name=\"t\"/></testsuite>";
    queue(&db, &project, with_location).await;
    drain(&db, &settings()).await;
    queue(&db, &project, no_location).await;
    drain(&db, &settings()).await;
    let row = test(&db).await;
    assert_eq!(
        (row.file.as_deref(), row.line),
        (Some("tests/a.py"), Some(3))
    );
    queue(&db, &project, moved).await;
    drain(&db, &settings()).await;
    let row = test(&db).await;
    assert_eq!(
        (row.file.as_deref(), row.line),
        (Some("tests/b.py"), Some(9))
    );
}

#[tokio::test]
async fn a_line_past_the_column_without_a_file_does_not_fail_the_report() {
    // A Location is written only when the report gave a `file`, so a line on
    // its own is never stored and never has to fit an `int4` column (S13b).
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let body = b"<testsuite name=\"s\"><testcase classname=\"c\" name=\"t\" line=\"3000000000\"/></testsuite>";
    queue(&db, &project, body).await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report was processed");
    assert_eq!(outcome.status, ReportStatus::Processed);
    let row = test(&db).await;
    assert_eq!((row.file.as_deref(), row.line), (None, None));
}

#[tokio::test]
async fn line_base_zero_stores_a_one_based_location() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let body = b"<testsuite name=\"s\"><testcase classname=\"c\" name=\"t\" file=\"tests/a.py\" line=\"11\"/></testsuite>";
    queue_with(&db, &project, body, |b| b.line_base(0)).await;
    drain(&db, &settings()).await;
    let row = test(&db).await;
    assert_eq!(
        (row.file.as_deref(), row.line),
        (Some("tests/a.py"), Some(12))
    );
}

#[tokio::test]
async fn the_root_is_set_only_when_sent() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.root("server")
    })
    .await;
    drain(&db, &settings()).await;
    queue(&db, &project, &make_junit(&[("t", "passed")])).await;
    drain(&db, &settings()).await;
    let stored = projects::Entity::find_by_id(project.id)
        .one(db.connection())
        .await
        .expect("the Project")
        .expect("the Project exists");
    assert_eq!(stored.root, "server");
}

#[tokio::test]
async fn a_bad_report_fails_and_the_queue_moves_on() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let bad = queue(&db, &project, b"not xml").await;
    let good = queue(&db, &project, &make_junit(&[("t", "passed")])).await;
    let first = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(first.status, ReportStatus::Failed);
    assert_eq!(first.report_id, bad.id);
    assert!(
        first
            .error
            .expect("an error")
            .starts_with("ParseError: Not a valid JUnit XML report")
    );
    let second = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("the next Report");
    assert_eq!(
        (second.status.as_str(), second.report_id),
        ("processed", good.id)
    );
    let stored = reports::Entity::find_by_id(bad.id)
        .one(db.connection())
        .await
        .expect("the Report")
        .expect("the Report exists");
    assert_eq!(stored.status, "failed");
    assert_eq!(stored.retry_count, 0);
    assert_eq!(
        test_runs::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        1
    );
}

#[tokio::test]
async fn a_transient_failure_is_retried_then_capped() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    fail_test_run_inserts(&db).await;
    let report = queue(&db, &project, &make_junit(&[("t", "passed")])).await;
    let settings = settings_with(|settings| settings.max_report_retries = 3);
    for expected in [1, 2, 3] {
        let outcome = process_next(db.connection(), &settings)
            .await
            .expect("the processor runs")
            .expect("a Report");
        assert_eq!(outcome.status, ReportStatus::Pending);
        assert_eq!(outcome.retry_count, expected);
        let stored = reports::Entity::find_by_id(report.id)
            .one(db.connection())
            .await
            .expect("the Report")
            .expect("the Report exists");
        assert_eq!(
            (stored.status.as_str(), stored.retry_count),
            ("pending", expected)
        );
        assert!(
            stored
                .error
                .expect("an error")
                .starts_with("OperationalError:")
        );
    }
    let outcome = process_next(db.connection(), &settings)
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(outcome.status, ReportStatus::Failed);
    let stored = reports::Entity::find_by_id(report.id)
        .one(db.connection())
        .await
        .expect("the Report")
        .expect("the Report exists");
    assert_eq!((stored.status.as_str(), stored.retry_count), ("failed", 3));
}

#[tokio::test]
async fn a_permanent_error_fails_without_spending_the_retry_budget() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    // A broken CHECK constraint is a data error (SQLSTATE 23514), which is permanent.
    db.connection()
        .execute_unprepared(
            "ALTER TABLE test_runs ADD CONSTRAINT reject_all CHECK (false) NOT VALID",
        )
        .await
        .expect("the constraint is added");
    let report = queue(&db, &project, &make_junit(&[("t", "passed")])).await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(outcome.status, ReportStatus::Failed);
    assert!(outcome.error.expect("an error").starts_with("DBAPIError:"));
    let stored = reports::Entity::find_by_id(report.id)
        .one(db.connection())
        .await
        .expect("the Report")
        .expect("the Report exists");
    assert_eq!((stored.status.as_str(), stored.retry_count), ("failed", 0));
}

#[tokio::test]
async fn both_retry_kinds_reset_the_queue_fields() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let body = make_junit(&[("t", "failed")]);
    let manual = make_report(&project, project.repo_id)
        .body(&body)
        .commit_sha("manual")
        .status("failed")
        .error("ParseError: boom")
        .processed_at(at())
        .retry_count(5)
        .create(&db)
        .await;
    let transient = queue_with(&db, &project, &body, |builder| {
        builder.commit_sha("transient")
    })
    .await;
    fail_test_run_inserts(&db).await;
    let settings = settings_with(|settings| settings.max_report_retries = 3);

    // The transient kind first: the manual retry would leave its row pending, and
    // the processor claims the oldest pending Report.
    let outcome = process_next(db.connection(), &settings)
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(
        (outcome.report_id, outcome.status.as_str()),
        (transient.id, "pending")
    );
    assert_eq!(outcome.retry_count, 1);
    let manual = apply_manual_retry(&db, manual.id).await;
    let transient = reports::Entity::find_by_id(transient.id)
        .one(db.connection())
        .await
        .expect("the Report")
        .expect("the Report exists");

    // Back in the queue, both rows look the same.
    assert_eq!(
        (manual.status.as_str(), transient.status.as_str()),
        ("pending", "pending")
    );
    assert_eq!((manual.processed_at, transient.processed_at), (None, None));
    // `error` and `retry_count` are the fields the two kinds differ on: a transient
    // retry spends one automatic retry, a manual one resets the budget to 0.
    assert_eq!(manual.error, None);
    assert!(transient.error.is_some());
    assert_eq!(transient.error, outcome.error);
    assert_eq!((manual.retry_count, transient.retry_count), (0, 1));
}

#[tokio::test]
async fn a_manual_retry_gives_a_fresh_budget() {
    // After the reset the budget is really usable: the next transient failure retries.
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let manual = make_report(&project, project.repo_id)
        .body(&make_junit(&[("t", "passed")]))
        .commit_sha("refill")
        .status("failed")
        .error("OperationalError: connection reset")
        .processed_at(at())
        .retry_count(5)
        .create(&db)
        .await;
    fail_test_run_inserts(&db).await;
    let settings = settings_with(|settings| settings.max_report_retries = 3);

    let retried = apply_manual_retry(&db, manual.id).await;
    assert_eq!(
        (retried.status.as_str(), retried.retry_count, retried.error),
        ("pending", 0, None)
    );
    let outcome = process_next(db.connection(), &settings)
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(outcome.status, ReportStatus::Pending);
    let stored = reports::Entity::find_by_id(manual.id)
        .one(db.connection())
        .await
        .expect("the Report")
        .expect("the Report exists");
    assert_eq!((stored.status.as_str(), stored.retry_count), ("pending", 1));
}

#[tokio::test]
async fn the_claim_skips_a_locked_row() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let first = queue(&db, &project, &make_junit(&[("t", "passed")])).await;
    let second = queue(&db, &project, &make_junit(&[("t", "passed")])).await;
    let transaction_a = db.connection().begin().await.expect("a transaction");
    let transaction_b = db.connection().begin().await.expect("a transaction");
    let claimed_a = claim_next_report(&transaction_a)
        .await
        .expect("the claim")
        .expect("a Report");
    let claimed_b = claim_next_report(&transaction_b)
        .await
        .expect("the claim")
        .expect("the next Report");
    assert_eq!((claimed_a.id, claimed_b.id), (first.id, second.id));
    transaction_a
        .rollback()
        .await
        .expect("the transaction ends");
    transaction_b
        .rollback()
        .await
        .expect("the transaction ends");
}

#[tokio::test]
async fn a_large_report_crosses_chunk_boundaries() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let cases: Vec<(String, &str)> = (0..2500)
        .map(|index| {
            (
                format!("t{index}"),
                if index % 50 == 0 { "failed" } else { "passed" },
            )
        })
        .collect();
    let case_refs: Vec<(&str, &str)> = cases
        .iter()
        .map(|(name, status)| (name.as_str(), *status))
        .collect();
    queue(&db, &project, &make_junit(&case_refs)).await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(outcome.status, ReportStatus::Processed);
    assert_eq!(outcome.touched_test_ids.len(), 2500);
    assert_eq!(
        test_executions::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        2500
    );
}

#[tokio::test]
async fn a_default_branch_ignores_feature_flips() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_with(&db, &project, &make_junit(&[("t", "failed")]), |b| {
        b.branch("feat").commit_sha("a").default_branch("main")
    })
    .await;
    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.branch("feat").commit_sha("b")
    })
    .await;
    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.branch("main").commit_sha("c")
    })
    .await;
    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.branch("main").commit_sha("d")
    })
    .await;
    drain(&db, &settings()).await;
    assert_eq!(repo(&db).await.default_branch.as_deref(), Some("main"));
    let row = test(&db).await;
    assert_eq!(row.flakiness_score, 0.0);
    assert_eq!(row.confirmed_flake_count, 0);
}

#[tokio::test]
async fn a_same_sha_flip_on_a_feature_branch_is_still_proven() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_with(&db, &project, &make_junit(&[("t", "failed")]), |b| {
        b.branch("feat").commit_sha("x").default_branch("main")
    })
    .await;
    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.branch("feat").commit_sha("x")
    })
    .await;
    drain(&db, &settings()).await;
    let row = test(&db).await;
    assert_eq!(row.confirmed_flake_count, 1);
    assert!(row.flakiness_score >= 0.6);
}

#[tokio::test]
async fn a_branch_change_rescores_tests_outside_the_report() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (name, status, sha) in [
        ("t1", "failed", "a"),
        ("t1", "passed", "b"),
        ("t2", "failed", "c"),
        ("t2", "passed", "d"),
    ] {
        queue_with(&db, &project, &make_junit(&[(name, status)]), |b| {
            b.branch("feat").commit_sha(sha)
        })
        .await;
    }
    drain(&db, &settings()).await;
    let before = tests(&db).await;
    assert!(before.iter().all(|row| row.flakiness_score > 0.0));
    queue_with(&db, &project, &make_junit(&[("t1", "passed")]), |b| {
        b.branch("main").commit_sha("e").default_branch("main")
    })
    .await;
    drain(&db, &settings()).await;
    let after = tests(&db).await;
    assert!(after.iter().all(|row| row.flakiness_score == 0.0));
}

#[tokio::test]
async fn the_default_branch_is_applied_only_at_processing() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.default_branch("main")
    })
    .await;
    assert_eq!(repo(&db).await.default_branch, None);
    drain(&db, &settings()).await;
    assert_eq!(repo(&db).await.default_branch.as_deref(), Some("main"));
}

#[tokio::test]
async fn a_report_without_a_default_branch_never_clears_one() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.default_branch("main")
    })
    .await;
    drain(&db, &settings()).await;
    queue(&db, &project, &make_junit(&[("t", "passed")])).await;
    drain(&db, &settings()).await;
    assert_eq!(repo(&db).await.default_branch.as_deref(), Some("main"));
}

#[tokio::test]
async fn the_default_branch_window_keeps_older_default_flips() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let settings = settings_with(|settings| settings.score_window = 3);
    for (index, status) in ["failed", "passed", "failed", "passed", "failed"]
        .iter()
        .enumerate()
    {
        queue_with(&db, &project, &make_junit(&[("t", status)]), |b| {
            let builder = b.branch("main").commit_sha(&format!("m{index}"));
            if index == 0 {
                builder.default_branch("main")
            } else {
                builder
            }
        })
        .await;
    }
    for index in 0..10 {
        queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
            b.branch("feat").commit_sha(&format!("f{index}"))
        })
        .await;
    }
    drain(&db, &settings).await;
    assert!(test(&db).await.flakiness_score > 0.0);
}

#[tokio::test]
async fn a_rescore_matches_the_branch_scoped_score_over_all_rows() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let settings = settings_with(|settings| settings.score_window = 3);
    db.connection()
        .execute_unprepared("UPDATE repos SET default_branch = 'main'")
        .await
        .expect("the Default branch");
    let row = flakeradar_test_support::make_test_case(&project)
        .create(&db)
        .await;
    // Oldest first, so reversing the list gives the newest-first history the
    // route serves.
    let mut history: Vec<ScoredExecution> = Vec::new();
    for (index, status) in ["failed", "passed", "failed", "passed", "failed"]
        .iter()
        .enumerate()
    {
        let run = flakeradar_test_support::make_run(&project)
            .commit_sha(&format!("m{index}"))
            .branch("main")
            .create(&db)
            .await;
        flakeradar_test_support::make_execution(&row, &run)
            .status(status)
            .create(&db)
            .await;
        history.push(ScoredExecution {
            commit_sha: format!("m{index}"),
            branch: "main".to_owned(),
            status: ExecutionStatus::from_wire(status).expect("a status"),
        });
    }
    for index in 0..10 {
        let run = flakeradar_test_support::make_run(&project)
            .commit_sha(&format!("f{index}"))
            .branch("feat")
            .create(&db)
            .await;
        flakeradar_test_support::make_execution(&row, &run)
            .create(&db)
            .await;
        history.push(ScoredExecution {
            commit_sha: format!("f{index}"),
            branch: "feat".to_owned(),
            status: ExecutionStatus::Passed,
        });
    }
    rescore(
        db.connection(),
        EntityKind::Test,
        &[row.id],
        None,
        &settings,
    )
    .await
    .expect("the rescore");
    let stored = test(&db).await;
    history.reverse();
    let history = history;
    let (expected, confirmed) = flakeradar_core::scoring::branch_scoped_score(
        &history,
        Some("main"),
        settings.score_decay,
        settings.score_window,
    );
    // The newest Default-branch flips still count despite ten newer feature rows.
    assert!(expected.as_f64() > 0.0);
    assert_eq!(stored.flakiness_score, expected.as_f64());
    assert_eq!(
        stored.confirmed_flake_count,
        i32::try_from(confirmed).expect("a count")
    );
    assert_eq!(
        stored.clean_streak,
        i32::try_from(flakeradar_core::scoring::branch_scoped_streak(
            &history,
            Some("main")
        ))
        .expect("a streak")
    );
    assert_eq!(
        stored.fail_streak,
        i32::try_from(flakeradar_core::scoring::branch_scoped_fail_streak(
            &history,
            Some("main")
        ))
        .expect("a streak")
    );
}

#[tokio::test]
async fn same_day_reports_share_one_history_row() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_with(&db, &project, &make_junit(&[("t", "failed")]), |b| {
        b.commit_sha("s1")
    })
    .await;
    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.commit_sha("s1")
    })
    .await;
    drain(&db, &settings()).await;
    let row = test(&db).await;
    let history = test_score_history::Entity::find()
        .all(db.connection())
        .await
        .expect("the history");
    assert_eq!(history.len(), 1);
    assert_eq!((history[0].executions, history[0].failures), (2, 1));
    assert_eq!(history[0].day, at().date_naive());
    assert_eq!(history[0].flakiness_score, row.flakiness_score);
}

#[tokio::test]
async fn the_clean_streak_counts_passes_since_the_last_failure() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for status in ["failed", "passed", "passed"] {
        queue_with(&db, &project, &make_junit(&[("t", status)]), |b| {
            b.commit_sha("s1")
        })
        .await;
    }
    drain(&db, &settings()).await;
    assert_eq!(test(&db).await.clean_streak, 2);
}

#[tokio::test]
async fn a_zero_clean_streak_clears_the_fixed_note_and_a_positive_one_keeps_it() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let held = flakeradar_test_support::make_test_case(&project)
        .name("t_keep")
        .clean_streak(3)
        .github_issue_fixed_noted_at(at().fixed_offset())
        .create(&db)
        .await;
    let broke = flakeradar_test_support::make_test_case(&project)
        .name("t_fail")
        .clean_streak(3)
        .github_issue_fixed_noted_at(at().fixed_offset())
        .create(&db)
        .await;
    queue_with(
        &db,
        &project,
        &make_junit(&[("t_fail", "failed"), ("t_keep", "passed")]),
        |b| b.branch("main").commit_sha("f9").default_branch("main"),
    )
    .await;
    drain(&db, &settings()).await;
    let broke = test_cases::Entity::find_by_id(broke.id)
        .one(db.connection())
        .await
        .expect("the Test")
        .expect("the Test exists");
    let held = test_cases::Entity::find_by_id(held.id)
        .one(db.connection())
        .await
        .expect("the Test")
        .expect("the Test exists");
    assert_eq!(broke.clean_streak, 0);
    assert_eq!(broke.github_issue_fixed_noted_at, None);
    assert!(held.clean_streak > 0);
    assert!(held.github_issue_fixed_noted_at.is_some());
}

#[tokio::test]
async fn a_positive_clean_streak_keeps_the_fixed_note() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    flakeradar_test_support::make_test_case(&project)
        .name("t")
        .clean_streak(3)
        .github_issue_fixed_noted_at(at().fixed_offset())
        .create(&db)
        .await;
    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.branch("main").commit_sha("f9").default_branch("main")
    })
    .await;
    drain(&db, &settings()).await;
    let row = test(&db).await;
    assert!(row.clean_streak > 0);
    assert!(row.github_issue_fixed_noted_at.is_some());
}

#[tokio::test]
async fn five_default_branch_failures_mark_a_test_broken() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for index in 0..5 {
        queue_with(&db, &project, &make_junit(&[("t", "failed")]), |b| {
            let builder = b.branch("main").commit_sha(&format!("m{index}"));
            if index == 0 {
                builder.default_branch("main")
            } else {
                builder
            }
        })
        .await;
    }
    drain(&db, &settings()).await;
    let row = test(&db).await;
    assert_eq!(
        row.flakiness_score, 0.0,
        "a Test that only fails is not flaky"
    );
    assert_eq!(row.fail_streak, 5);
    let broken_streak = settings().broken_streak;
    assert!(
        is_broken(i64::from(row.fail_streak), broken_streak),
        "{} Default-branch failures reach the broken threshold of {broken_streak}",
        row.fail_streak
    );

    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.branch("main").commit_sha("m5")
    })
    .await;
    drain(&db, &settings()).await;
    let after = test(&db).await;
    assert_eq!(after.fail_streak, 0, "a pass ends the streak");
    assert!(
        !is_broken(i64::from(after.fail_streak), broken_streak),
        "and with it the broken flag"
    );
}

#[tokio::test]
async fn an_upward_tier_crossing_is_reported() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_with(&db, &project, &make_junit(&[("t", "failed")]), |b| {
        b.commit_sha("a")
    })
    .await;
    let first = process_next(db.connection(), &notify_settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(first.tier_changes, Vec::new());
    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.commit_sha("b")
    })
    .await;
    let second = process_next(db.connection(), &notify_settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    let row = test(&db).await;
    let change = second.tier_changes.first().expect("a tier change");
    assert_eq!(change.kind.as_str(), "test");
    assert_eq!(change.event(), "test.flaky");
    assert_eq!(change.id, i64::from(row.id));
    assert_eq!(
        (change.repo.as_str(), change.project.as_deref()),
        ("acme/app", Some("backend"))
    );
    assert_eq!(
        (change.classname.as_deref(), change.name.as_str()),
        (Some("tests.test_mod"), "t")
    );
    assert_eq!(change.flakiness_score, stored_score(row.flakiness_score));
    assert_eq!(
        change.tier,
        tier_for(stored_score(row.flakiness_score), 0.05)
    );
    assert_eq!(
        change.proven_flake_count,
        usize::try_from(row.confirmed_flake_count).expect("a count")
    );
    assert_eq!(change.pipeline, None);
}

#[tokio::test]
async fn a_downward_tier_crossing_is_reported() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (status, sha) in [("failed", "a"), ("passed", "b")] {
        queue_with(&db, &project, &make_junit(&[("t", status)]), |b| {
            b.commit_sha(sha)
        })
        .await;
        process_next(db.connection(), &notify_settings())
            .await
            .expect("the processor runs");
    }
    queue_with(&db, &project, &make_junit(&[("t", "failed")]), |b| {
        b.commit_sha("c")
    })
    .await;
    let middle = process_next(db.connection(), &notify_settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    // The score is still flaky, so this Report crosses nothing: a `test.recovered`
    // here would be a spurious event, not a tier this build may report.
    assert_eq!(middle.tier_changes, Vec::new());
    queue_with(&db, &project, &make_junit(&[("t", "failed")]), |b| {
        b.commit_sha("d")
    })
    .await;
    let outcome = process_next(db.connection(), &notify_settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    let change = outcome.tier_changes.first().expect("a tier change");
    assert_eq!(change.event(), "test.recovered");
    assert_eq!(change.flakiness_score, Score::ZERO);
}

#[tokio::test]
async fn staying_flaky_reports_no_second_event() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (status, sha) in [("failed", "a"), ("passed", "b")] {
        queue_with(&db, &project, &make_junit(&[("t", status)]), |b| {
            b.commit_sha(sha)
        })
        .await;
        process_next(db.connection(), &notify_settings())
            .await
            .expect("the processor runs");
    }
    queue_with(&db, &project, &make_junit(&[("t", "failed")]), |b| {
        b.commit_sha("a")
    })
    .await;
    let outcome = process_next(db.connection(), &notify_settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(outcome.tier_changes, Vec::new());
}

#[tokio::test]
async fn touched_tests_are_rescored_in_id_order_not_report_order() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    // The first Report gives the three Tests their ids in this order.
    queue(
        &db,
        &project,
        &make_junit(&[("t1", "passed"), ("t2", "passed"), ("t3", "passed")]),
    )
    .await;
    drain(&db, &settings()).await;
    let rows = tests(&db).await;
    let by_name: BTreeMap<&str, i32> = rows.iter().map(|row| (row.name.as_str(), row.id)).collect();
    let report_order = vec![by_name["t3"], by_name["t1"], by_name["t2"]];
    let mut by_id = report_order.clone();
    by_id.sort_unstable();
    assert_ne!(
        report_order, by_id,
        "the ids must not follow this report's order"
    );

    // The touched Tests are rescored by id, not
    // in the order this report first named them.
    queue(
        &db,
        &project,
        &make_junit(&[("t3", "passed"), ("t1", "passed"), ("t2", "passed")]),
    )
    .await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(outcome.touched_test_ids, by_id);
}

#[tokio::test]
async fn tier_changes_are_emitted_in_id_order() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    db.connection()
        .execute_unprepared("UPDATE repos SET default_branch = 'main'")
        .await
        .expect("the Default branch");
    let first = flakeradar_test_support::make_test_case(&project)
        .name("a")
        .create(&db)
        .await;
    let second = flakeradar_test_support::make_test_case(&project)
        .name("b")
        .create(&db)
        .await;
    // A fail then a pass on different commits is a flake, and neither Test's score
    // has been written yet, so both cross the Flake threshold.
    for (sha, status) in [("f", "failed"), ("g", "passed")] {
        let run = flakeradar_test_support::make_run(&project)
            .commit_sha(sha)
            .branch("main")
            .create(&db)
            .await;
        for row in [&first, &second] {
            flakeradar_test_support::make_execution(row, &run)
                .status(status)
                .create(&db)
                .await;
        }
    }
    // The Tests are read back into a map keyed by id, so the
    // events go out in id order whatever order the caller lists the ids in.
    let changes = rescore(
        db.connection(),
        EntityKind::Test,
        &[second.id, first.id],
        None,
        &notify_settings(),
    )
    .await
    .expect("the rescore");
    assert_eq!(
        changes.iter().map(|change| change.id).collect::<Vec<_>>(),
        [i64::from(first.id), i64::from(second.id)]
    );
}

#[tokio::test]
async fn job_tier_changes_are_emitted_in_id_order() {
    // The webhook task posts Tier changes in the order this rescore emits them,
    // so a Job rescore on ids listed out of order must still emit by id.
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline().repo("acme/app").create(&db).await;
    db.connection()
        .execute_unprepared("UPDATE repos SET default_branch = 'main'")
        .await
        .expect("the Default branch");
    let first = make_job(&pipeline).name("first").create(&db).await;
    let second = make_job(&pipeline).name("second").create(&db).await;
    // An unexplained failure then a pass on different commits of the Default
    // branch is a flake, and neither Job's score has been written yet, so both
    // cross the Flake threshold.
    for (ci_job_id, status) in [("f", "failed"), ("g", "passed")] {
        for job in [&first, &second] {
            make_job_execution(job)
                .ci_job_id(ci_job_id)
                .status(status)
                .create(&db)
                .await;
        }
    }
    let changes = rescore(
        db.connection(),
        EntityKind::Job,
        &[second.id, first.id],
        None,
        &notify_settings(),
    )
    .await
    .expect("the rescore");
    assert_eq!(
        changes.iter().map(|change| change.id).collect::<Vec<_>>(),
        [i64::from(first.id), i64::from(second.id)]
    );
}

#[tokio::test]
async fn a_broken_test_reports_no_recovery() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let settings = notify_settings_with(|settings| settings.broken_streak = 2);
    for (status, sha) in [("failed", "a"), ("passed", "b")] {
        queue_with(&db, &project, &make_junit(&[("t", status)]), |b| {
            b.commit_sha(sha)
        })
        .await;
        process_next(db.connection(), &settings)
            .await
            .expect("the processor runs");
    }
    for (status, sha) in [("failed", "c"), ("failed", "d")] {
        queue_with(&db, &project, &make_junit(&[("t", status)]), |b| {
            b.commit_sha(sha)
        })
        .await;
        let outcome = process_next(db.connection(), &settings)
            .await
            .expect("the processor runs")
            .expect("a Report");
        if sha == "d" {
            let row = test(&db).await;
            assert_eq!((row.flakiness_score, row.fail_streak), (0.0, 2));
            assert_eq!(outcome.tier_changes, Vec::new());
        }
    }
}

#[tokio::test]
async fn fixing_a_broken_test_reports_flaky_again() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let settings = notify_settings_with(|settings| settings.broken_streak = 2);
    for (status, sha) in [
        ("failed", "a"),
        ("passed", "b"),
        ("failed", "c"),
        ("failed", "d"),
    ] {
        queue_with(&db, &project, &make_junit(&[("t", status)]), |b| {
            b.commit_sha(sha)
        })
        .await;
        process_next(db.connection(), &settings)
            .await
            .expect("the processor runs");
    }
    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.commit_sha("e")
    })
    .await;
    let outcome = process_next(db.connection(), &settings)
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(outcome.tier_changes[0].event(), "test.flaky");
    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.commit_sha("f")
    })
    .await;
    let outcome = process_next(db.connection(), &settings)
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(outcome.tier_changes[0].event(), "test.recovered");
}

#[tokio::test]
async fn a_new_test_above_the_threshold_is_flaky() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_with(
        &db,
        &project,
        &make_junit(&[("t", "failed"), ("t", "passed")]),
        |b| b.commit_sha("a"),
    )
    .await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    let change = outcome.tier_changes.first().expect("a tier change");
    assert_eq!(change.event(), "test.flaky");
    assert_eq!(change.proven_flake_count, 1);
}

#[tokio::test]
async fn a_move_into_suspect_reports_nothing() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let settings = notify_settings_with(|settings| settings.flake_threshold = 0.5);
    for (status, sha) in [("failed", "a"), ("passed", "b")] {
        queue_with(&db, &project, &make_junit(&[("t", status)]), |b| {
            b.commit_sha(sha)
        })
        .await;
        let outcome = process_next(db.connection(), &settings)
            .await
            .expect("the processor runs")
            .expect("a Report");
        if sha == "b" {
            let row = test(&db).await;
            assert!(row.flakiness_score > 0.0 && row.flakiness_score < 0.5);
            assert_eq!(outcome.tier_changes, Vec::new());
        }
    }
}

#[tokio::test]
async fn rescore_repo_drops_crossings() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (status, sha) in [("failed", "a"), ("passed", "b")] {
        queue_with(&db, &project, &make_junit(&[("t2", status)]), |b| {
            b.branch("feat").commit_sha(sha)
        })
        .await;
        process_next(db.connection(), &notify_settings())
            .await
            .expect("the processor runs");
    }
    queue_with(&db, &project, &make_junit(&[("t1", "passed")]), |b| {
        b.branch("main").commit_sha("c").default_branch("main")
    })
    .await;
    let outcome = process_next(db.connection(), &notify_settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    let rows = tests(&db).await;
    assert!(rows.iter().all(|row| row.flakiness_score == 0.0));
    assert_eq!(outcome.tier_changes, Vec::new());
}

// --- Pipeline Reports ---------------------------------------------------

#[tokio::test]
async fn one_pipeline_creates_its_pipeline_jobs_and_executions() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let report = queue_pipeline(
        &db,
        &project,
        &payload(vec![
            job_in("1", "lint", JobStatus::Passed),
            job_in("2", "test (ubuntu, 3.12)", JobStatus::Failed),
        ]),
    )
    .await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(outcome.status, ReportStatus::Processed);
    assert_eq!(
        outcome.counts,
        Some(serde_json::json!({"passed": 1, "failed": 1, "skipped": 0, "duplicate": 0}))
    );
    assert_eq!(
        pipelines::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        1
    );
    assert_eq!(
        jobs::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        2
    );
    assert_eq!(
        job_executions::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        2
    );
    let stored = reports::Entity::find_by_id(report.id)
        .one(db.connection())
        .await
        .expect("the Report")
        .expect("the Report exists");
    assert_eq!((stored.status.as_str(), stored.run_id), ("processed", None));
    assert_eq!(outcome.touched_test_ids, Vec::<i32>::new());
    let mut ids: Vec<i32> = jobs(&db).await.iter().map(|row| row.id).collect();
    ids.sort_unstable();
    assert_eq!(outcome.touched_job_ids, ids);
}

#[tokio::test]
async fn a_retried_job_in_one_report_keeps_the_newest_try() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let mut report = payload(vec![
        job_in("102", "test", JobStatus::Failed),
        job_in("110", "test", JobStatus::Passed),
    ]);
    report.provider = "gitlab".to_owned();
    report.pipeline = ".gitlab-ci.yml".to_owned();
    queue_pipeline(&db, &project, &report).await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(
        outcome.counts,
        Some(serde_json::json!({"passed": 1, "failed": 1, "skipped": 0, "duplicate": 0}))
    );
    assert_eq!(job(&db).await.last_status, "passed");
    let mut executions: Vec<String> = job_executions::Entity::find()
        .order_by_desc(job_executions::Column::Id)
        .all(db.connection())
        .await
        .expect("the executions")
        .into_iter()
        .map(|row| row.ci_job_id)
        .collect();
    executions.sort();
    assert_eq!(executions, ["102", "110"]);
}

#[tokio::test]
async fn a_rerun_on_the_same_sha_is_a_proven_flake() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (ci_job_id, status) in [("100", JobStatus::Failed), ("101", JobStatus::Passed)] {
        let mut report = payload(vec![job_in(ci_job_id, "test", status)]);
        report.commit_sha = "dead".to_owned();
        queue_pipeline(&db, &project, &report).await;
        process_next(db.connection(), &settings())
            .await
            .expect("the processor runs");
    }
    let row = job(&db).await;
    assert_eq!(row.confirmed_flake_count, 1);
    assert!(row.flakiness_score >= 0.6);
}

#[tokio::test]
async fn a_duplicate_payload_adds_no_rows() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let report = payload(vec![job_in("1", "test", JobStatus::Passed)]);
    queue_pipeline(&db, &project, &report).await;
    process_next(db.connection(), &settings())
        .await
        .expect("the processor runs");
    let before = job(&db).await;
    queue_pipeline(&db, &project, &report).await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(
        outcome.counts,
        Some(serde_json::json!({"passed": 0, "failed": 0, "skipped": 0, "duplicate": 1}))
    );
    assert_eq!(outcome.touched_job_ids, Vec::<i32>::new());
    assert_eq!(
        job_executions::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        1
    );
    let after = job(&db).await;
    assert_eq!(
        (after.flakiness_score, after.confirmed_flake_count),
        (before.flakiness_score, before.confirmed_flake_count)
    );
}

#[tokio::test]
async fn the_same_job_name_in_two_pipelines_is_two_jobs() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (ci_job_id, pipeline) in [
        ("1", ".github/workflows/ci.yml"),
        ("2", ".github/workflows/release.yml"),
    ] {
        let mut report = payload(vec![job_in(ci_job_id, "build", JobStatus::Passed)]);
        report.pipeline = pipeline.to_owned();
        queue_pipeline(&db, &project, &report).await;
        process_next(db.connection(), &settings())
            .await
            .expect("the processor runs");
    }
    assert_eq!(
        jobs::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        2
    );
    let pipeline_ids: Vec<i32> = jobs(&db).await.iter().map(|row| row.pipeline_id).collect();
    assert_eq!(
        pipeline_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        2
    );
}

#[tokio::test]
async fn the_same_pipeline_name_on_two_providers_is_two_pipelines() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (ci_job_id, provider) in [("1", "github"), ("2", "gitlab")] {
        let mut report = payload(vec![job_in(ci_job_id, "build", JobStatus::Passed)]);
        report.provider = provider.to_owned();
        queue_pipeline(&db, &project, &report).await;
        process_next(db.connection(), &settings())
            .await
            .expect("the processor runs");
    }
    assert_eq!(
        pipelines::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        2
    );
    assert_eq!(
        jobs::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        2
    );
}

#[tokio::test]
async fn a_feature_branch_flip_does_not_score_a_job() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (ci_job_id, status, sha) in [("1", JobStatus::Failed, "a"), ("2", JobStatus::Passed, "b")] {
        let mut report = payload(vec![job_in(ci_job_id, "test", status)]);
        report.commit_sha = sha.to_owned();
        report.branch = "feat".to_owned();
        queue_pipeline(&db, &project, &report).await;
        process_next(db.connection(), &settings())
            .await
            .expect("the processor runs");
    }
    let row = job(&db).await;
    assert_eq!(row.flakiness_score, 0.0);
    assert_eq!(row.confirmed_flake_count, 0);
}

#[tokio::test]
async fn a_main_branch_flip_scores_a_job() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (ci_job_id, status, sha) in [("1", JobStatus::Failed, "a"), ("2", JobStatus::Passed, "b")] {
        let mut report = payload(vec![job_in(ci_job_id, "test", status)]);
        report.commit_sha = sha.to_owned();
        queue_pipeline(&db, &project, &report).await;
        process_next(db.connection(), &settings())
            .await
            .expect("the processor runs");
    }
    assert!(job(&db).await.flakiness_score > 0.0);
}

#[tokio::test]
async fn a_default_branch_change_rescores_tests_and_jobs() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (ci_job_id, status, sha) in [("1", JobStatus::Failed, "a"), ("2", JobStatus::Passed, "b")] {
        let mut report = payload(vec![job_in(ci_job_id, "job-a", status)]);
        report.commit_sha = sha.to_owned();
        report.branch = "feat".to_owned();
        report.default_branch = None;
        queue_pipeline(&db, &project, &report).await;
        process_next(db.connection(), &settings())
            .await
            .expect("the processor runs");
    }
    assert!(job(&db).await.flakiness_score > 0.0);
    for (status, sha) in [("failed", "c"), ("passed", "d")] {
        queue_with(&db, &project, &make_junit(&[("t1", status)]), |b| {
            b.branch("feat").commit_sha(sha)
        })
        .await;
        process_next(db.connection(), &settings())
            .await
            .expect("the processor runs");
    }
    assert!(test(&db).await.flakiness_score > 0.0);
    let mut report = payload(vec![job_in("3", "lint", JobStatus::Passed)]);
    report.commit_sha = "e".to_owned();
    report.branch = "main".to_owned();
    report.default_branch = Some("main".to_owned());
    queue_pipeline(&db, &project, &report).await;
    process_next(db.connection(), &settings())
        .await
        .expect("the processor runs");
    assert_eq!(job(&db).await.flakiness_score, 0.0);
    assert_eq!(test(&db).await.flakiness_score, 0.0);
}

#[tokio::test]
async fn a_malformed_pipeline_body_fails_and_the_queue_moves_on() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let bad = make_report(&project, project.repo_id)
        .kind("pipeline")
        .body(b"{}")
        .create(&db)
        .await;
    let good = queue(&db, &project, &make_junit(&[("t", "passed")])).await;
    let first = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!((first.status.as_str(), first.report_id), ("failed", bad.id));
    assert!(
        first
            .error
            .expect("an error")
            .starts_with("ValidationError:")
    );
    let second = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("the next Report");
    assert_eq!(
        (second.status.as_str(), second.report_id),
        ("processed", good.id)
    );
}

#[tokio::test]
async fn same_day_pipeline_reports_share_one_history_row() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (ci_job_id, status, run_id) in [
        ("1", JobStatus::Failed, "r1"),
        ("2", JobStatus::Passed, "r2"),
    ] {
        let mut report = payload(vec![job_in(ci_job_id, "e2e", status)]);
        report.ci_run_id = CiRunId::new(run_id).expect("an id");
        queue_pipeline(&db, &project, &report).await;
        process_next(db.connection(), &settings())
            .await
            .expect("the processor runs");
    }
    let row = job(&db).await;
    let history = job_score_history::Entity::find()
        .all(db.connection())
        .await
        .expect("the history");
    assert_eq!(history.len(), 1);
    assert_eq!((history[0].executions, history[0].failures), (2, 1));
    assert_eq!(history[0].flakiness_score, row.flakiness_score);
}

#[tokio::test]
async fn the_summary_flag_defaults_false_and_flips() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let mut alls_green = job_in("1", "alls-green", JobStatus::Passed);
    alls_green.summary = true;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![alls_green, job_in("2", "test", JobStatus::Passed)]),
    )
    .await;
    process_next(db.connection(), &settings())
        .await
        .expect("the processor runs");
    let flags: BTreeMap<String, bool> = jobs(&db)
        .await
        .into_iter()
        .map(|row| (row.name, row.summary))
        .collect();
    assert!(flags["alls-green"]);
    assert!(!flags["test"]);
    queue_pipeline(
        &db,
        &project,
        &payload(vec![
            job_in("3", "alls-green", JobStatus::Passed),
            job_in("4", "test", JobStatus::Passed),
        ]),
    )
    .await;
    process_next(db.connection(), &settings())
        .await
        .expect("the processor runs");
    let row = jobs(&db)
        .await
        .into_iter()
        .find(|row| row.name == "alls-green")
        .expect("the summary Job");
    assert!(!row.summary);
}

#[tokio::test]
async fn a_job_tier_crossing_reports_job_flaky() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (ci_job_id, status, sha, run_id) in [
        ("1", JobStatus::Failed, "a", "1"),
        ("2", JobStatus::Passed, "b", "2"),
    ] {
        let mut report = payload(vec![job_in(ci_job_id, "test (ubuntu, 3.12)", status)]);
        report.commit_sha = sha.to_owned();
        report.ci_run_id = CiRunId::new(run_id).expect("an id");
        queue_pipeline(&db, &project, &report).await;
        let outcome = process_next(db.connection(), &notify_settings())
            .await
            .expect("the processor runs")
            .expect("a Report");
        if sha == "b" {
            let row = job(&db).await;
            let change = outcome.tier_changes.first().expect("a tier change");
            assert_eq!(
                (change.kind.as_str(), change.event().as_str()),
                ("job", "job.flaky")
            );
            assert_eq!(change.id, i64::from(row.id));
            assert_eq!(
                (change.repo.as_str(), change.project.as_deref()),
                ("acme/app", None)
            );
            assert_eq!(
                (change.pipeline.as_deref(), change.name.as_str()),
                (Some(".github/workflows/ci.yml"), "test (ubuntu, 3.12)")
            );
            assert_eq!(change.flakiness_score, stored_score(row.flakiness_score));
            assert_eq!(change.tier, flakeradar_core::model::Tier::Flaky);
            assert_eq!(
                (change.classname.as_deref(), change.failure_category),
                (None, None)
            );
        } else {
            assert_eq!(outcome.tier_changes, Vec::new());
        }
    }
}

#[tokio::test]
async fn a_job_recovery_reports_job_recovered() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (ci_job_id, status, sha, run_id) in [
        ("1", JobStatus::Failed, "a", "1"),
        ("2", JobStatus::Passed, "b", "2"),
    ] {
        let mut report = payload(vec![job_in(ci_job_id, "lint", status)]);
        report.commit_sha = sha.to_owned();
        report.ci_run_id = CiRunId::new(run_id).expect("an id");
        queue_pipeline(&db, &project, &report).await;
        process_next(db.connection(), &notify_settings())
            .await
            .expect("the processor runs");
    }
    let mut report = payload(vec![job_in("3", "lint", JobStatus::Passed)]);
    report.commit_sha = "c".to_owned();
    report.ci_run_id = CiRunId::new("3").expect("an id");
    queue_pipeline(&db, &project, &report).await;
    let outcome = process_next(db.connection(), &notify_settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    let change = outcome.tier_changes.first().expect("a tier change");
    assert_eq!(change.event(), "job.recovered");
    assert_eq!(change.flakiness_score, Score::ZERO);
}

#[tokio::test]
async fn a_default_branch_change_silences_the_pipeline_reports_own_crossings() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (ci_job_id, status, sha, run_id) in [
        ("1", JobStatus::Failed, "a", "1"),
        ("2", JobStatus::Passed, "b", "2"),
    ] {
        let mut report = payload(vec![job_in(ci_job_id, "lint", status)]);
        report.commit_sha = sha.to_owned();
        report.ci_run_id = CiRunId::new(run_id).expect("an id");
        report.branch = "feat".to_owned();
        report.default_branch = Some("feat".to_owned());
        queue_pipeline(&db, &project, &report).await;
        process_next(db.connection(), &notify_settings())
            .await
            .expect("the processor runs");
    }
    let mut report = payload(vec![job_in("3", "lint", JobStatus::Passed)]);
    report.commit_sha = "c".to_owned();
    report.ci_run_id = CiRunId::new("3").expect("an id");
    queue_pipeline(&db, &project, &report).await;
    let outcome = process_next(db.connection(), &notify_settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(outcome.tier_changes, Vec::new());
}

/// A `cargo-nextest` run whose flaky try failed and whose re-run passed.
///
/// Inlined next to the parser's own copy, because the recorded corpus is what the
/// parser is checked against and it is not read at run time.
/// cutover.
const NEXTEST_FLAKY_FAIL_XML: &str =
    include_str!("../../flakeradar-core/tests/fixtures/nextest_flaky_fail.xml");

#[tokio::test]
async fn a_nextest_flaky_fail_is_a_proven_flake_from_one_report() {
    // `flaky-result = "fail"` writes the flaky try as a `<failure>` marked by its
    // message and the passed re-run as a sibling, so one Report holds both tries.
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_with(&db, &project, NEXTEST_FLAKY_FAIL_XML.as_bytes(), |b| {
        b.commit_sha("777022bf")
    })
    .await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(
        outcome.counts,
        Some(serde_json::json!({
            "passed": 2, "failed": 1, "error": 0, "skipped": 0, "retried": 1
        }))
    );
    let flaky = tests(&db)
        .await
        .into_iter()
        .find(|row| {
            row.name
                .ends_with("invalidate_priority_readiness_drops_from_each_driver")
        })
        .expect("the flaky Test");
    assert_eq!(flaky.confirmed_flake_count, 1);
    assert!(flaky.flakiness_score >= 0.6);
    assert_eq!(flaky.last_status, "passed");
    let tries: Vec<(String, i32, bool)> = test_executions::Entity::find()
        .filter(test_executions::Column::TestCaseId.eq(flaky.id))
        .order_by_asc(test_executions::Column::Id)
        .all(db.connection())
        .await
        .expect("the Executions")
        .into_iter()
        .map(|row| (row.status, row.attempt, row.failure_category.is_some()))
        .collect();
    // The failed try is still classified; the converted pass is not.
    assert_eq!(
        tries,
        [
            ("failed".to_owned(), 0, true),
            ("passed".to_owned(), 1, false)
        ]
    );
}

#[tokio::test]
async fn a_report_rewrites_no_project_root_it_does_not_change() {
    // A trigger turns any `UPDATE projects` into a failure, so the processor can
    // only answer `processed` when the same root costs no write.
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    projects::Entity::update(projects::ActiveModel {
        id: sea_orm::Set(project.id),
        root: sea_orm::Set("backend".to_owned()),
        ..Default::default()
    })
    .exec(db.connection())
    .await
    .expect("the Project root");
    db.connection()
        .execute_unprepared(
            "CREATE FUNCTION no_project_update() RETURNS trigger LANGUAGE plpgsql AS \
             $$ BEGIN RAISE EXCEPTION 'projects was rewritten'; END $$",
        )
        .await
        .expect("the trigger function");
    db.connection()
        .execute_unprepared(
            "CREATE TRIGGER no_project_update BEFORE UPDATE ON projects \
             FOR EACH ROW EXECUTE FUNCTION no_project_update()",
        )
        .await
        .expect("the trigger");

    // The trigger bites: a rewrite of the row is refused, so a `processed` outcome
    // below can only mean the processor issued no `UPDATE`.
    let refused = projects::Entity::update(projects::ActiveModel {
        id: sea_orm::Set(project.id),
        root: sea_orm::Set("elsewhere".to_owned()),
        ..Default::default()
    })
    .exec(db.connection())
    .await;
    assert!(refused.is_err(), "the trigger refuses a Project rewrite");

    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.root("backend")
    })
    .await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(outcome.status, ReportStatus::Processed);
    let stored = projects::Entity::find_by_id(project.id)
        .one(db.connection())
        .await
        .expect("the Project")
        .expect("the Project exists");
    assert_eq!(stored.root, "backend");

    // A root that does change is written.
    db.connection()
        .execute_unprepared("DROP TRIGGER no_project_update ON projects")
        .await
        .expect("the trigger is dropped");
    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.root("frontend")
    })
    .await;
    process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    let stored = projects::Entity::find_by_id(project.id)
        .one(db.connection())
        .await
        .expect("the Project")
        .expect("the Project exists");
    assert_eq!(stored.root, "frontend");
}

#[tokio::test]
async fn a_report_that_changes_the_default_branch_drops_its_own_crossings() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    for (status, sha) in [("failed", "a"), ("passed", "b")] {
        queue_with(&db, &project, &make_junit(&[("t", status)]), |b| {
            b.branch("feat").commit_sha(sha)
        })
        .await;
        let outcome = process_next(db.connection(), &notify_settings())
            .await
            .expect("the processor runs")
            .expect("a Report");
        if status == "passed" {
            // The flip on `feat` is a crossing while `feat` is not the Default branch.
            assert_eq!(
                outcome
                    .tier_changes
                    .first()
                    .map(flakeradar_core::notify::TierChange::event),
                Some("test.flaky".to_owned())
            );
        }
    }
    queue_with(&db, &project, &make_junit(&[("t", "passed")]), |b| {
        b.branch("main").commit_sha("c").default_branch("main")
    })
    .await;
    let outcome = process_next(db.connection(), &notify_settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    // The new Default branch has no flips, so the Test left the flaky tier, but a
    // Report that rescores every Test must not send one event per Test.
    assert_eq!(test(&db).await.flakiness_score, 0.0);
    assert_eq!(outcome.tier_changes, Vec::new());
}

#[tokio::test]
async fn a_default_branch_change_silences_a_linked_junit_reports_job_crossings() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    // Fail then pass on either branch, so the Job is flaky under either Default.
    for (branch, failed, passed) in [("dev", "1", "2"), ("main", "3", "4")] {
        for (ci_job_id, status) in [(failed, JobStatus::Failed), (passed, JobStatus::Passed)] {
            let mut report = payload(vec![job_in(ci_job_id, "lint", status)]);
            report.commit_sha = ci_job_id.to_owned();
            report.ci_run_id = CiRunId::new(ci_job_id).expect("an id");
            report.branch = branch.to_owned();
            queue_pipeline(&db, &project, &report).await;
            process_next(db.connection(), &notify_settings())
                .await
                .expect("the processor runs");
        }
    }
    queue_with(&db, &project, &make_junit(&[("t", "failed")]), |b| {
        b.branch("dev")
            .commit_sha("z")
            .default_branch("dev")
            .ci_job_id("1")
    })
    .await;
    let outcome = process_next(db.connection(), &notify_settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    let stored = job(&db).await;
    assert_eq!(outcome.touched_job_ids, vec![stored.id]);
    // The failure is explained now, so the Job left the flaky tier, but the Default
    // branch change keeps the crossing silent.
    assert_eq!(stored.flakiness_score, 0.0);
    assert_eq!(outcome.tier_changes, Vec::new());
}

#[test]
fn the_processing_settings_debug_names_its_fields() {
    // The Debug form is what a log line carries, so it must name every field and
    // never the clock it holds.
    let text = format!("{:?}", settings());
    for field in [
        "score_window",
        "score_decay",
        "broken_streak",
        "flake_threshold",
        "max_report_retries",
        "infra_fail_share",
        "infra_min_tests",
    ] {
        assert!(text.contains(field), "{text}");
    }
    assert!(!text.contains("FixedClock"), "{text}");
}

#[tokio::test]
async fn a_report_counts_error_and_skipped_executions() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let body = b"<testsuite name=\"unit\">\
        <testcase classname=\"c\" name=\"boom\"><error message=\"kaboom\"/></testcase>\
        <testcase classname=\"c\" name=\"later\"><skipped message=\"not today\"/></testcase></testsuite>";
    queue(&db, &project, body).await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(
        outcome.counts,
        Some(serde_json::json!({
            "passed": 0, "failed": 0, "error": 1, "skipped": 1, "retried": 0
        }))
    );
}

#[tokio::test]
async fn a_line_too_large_for_its_column_fails_the_report() {
    // `line` is an `integer` column, so a runner's larger number cannot be stored.
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let body = b"<testsuite name=\"unit\">\
        <testcase classname=\"c\" name=\"t\" file=\"tests/a.py\" line=\"9999999999\"/></testsuite>";
    queue(&db, &project, body).await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    assert_eq!(outcome.status, ReportStatus::Failed);
    assert!(
        outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("line 9999999999 is out of range")),
        "{:?}",
        outcome.error
    );
}

#[tokio::test]
async fn the_last_occurrence_of_a_test_keeps_its_location() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let body = b"<testsuite name=\"unit\">\
        <testcase classname=\"tests.test_mod\" name=\"t\" file=\"old.py\" line=\"3\">\
        <failure message=\"x\"/></testcase>\
        <testcase classname=\"tests.test_mod\" name=\"t\" file=\"new.py\" line=\"7\"/></testsuite>";
    queue(&db, &project, body).await;
    drain(&db, &settings()).await;
    let row = test(&db).await;
    assert_eq!(
        (row.last_status.as_str(), row.file.as_deref(), row.line),
        ("passed", Some("new.py"), Some(7))
    );
}

#[tokio::test]
async fn a_skipped_job_counts_as_skipped() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let report = payload(vec![
        job_in("1", "lint", JobStatus::Skipped),
        job_in("2", "gate", JobStatus::Passed),
    ]);
    queue_pipeline(&db, &project, &report).await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    let counts = outcome.counts.expect("counts");
    assert_eq!(
        (counts["skipped"].as_i64(), counts["passed"].as_i64()),
        (Some(1), Some(1))
    );
}

#[tokio::test]
async fn a_job_whose_execution_is_a_duplicate_keeps_its_row() {
    // Only brand-new executions count, so a payload that repeats one Job and adds
    // another touches the added one alone.
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let first = payload(vec![job_in("1", "lint", JobStatus::Passed)]);
    queue_pipeline(&db, &project, &first).await;
    process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    let mut second = payload(vec![
        job_in("1", "lint", JobStatus::Passed),
        job_in("2", "gate", JobStatus::Failed),
    ]);
    second.commit_sha = "sha2".to_owned();
    second.ci_run_id = CiRunId::new("2").expect("an id");
    queue_pipeline(&db, &project, &second).await;
    let outcome = process_next(db.connection(), &settings())
        .await
        .expect("the processor runs")
        .expect("a Report");
    let stored = jobs(&db).await;
    let gate = stored
        .iter()
        .find(|row| row.name == "gate")
        .expect("the added Job");
    assert_eq!(outcome.touched_job_ids, vec![gate.id]);
    let counts = outcome.counts.expect("counts");
    assert_eq!(
        (counts["failed"].as_i64(), counts["passed"].as_i64()),
        (Some(1), Some(0))
    );
}

#[tokio::test]
async fn a_mass_failure_run_is_suspected_infrastructure() {
    // 21 of 25 distinct Tests failed on their last attempt: 0.84, past the share.
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue(&db, &project, &junit_with(&cases(25, 21, "failed"))).await;
    let outcome = drain(&db, &settings()).await;
    let run = run(&db, outcome[0].run_id.expect("a Run")).await;
    assert!(run.infra_suspect, "most of a large Run failed");
}

#[tokio::test]
async fn a_run_at_exactly_the_fail_share_is_suspected_infrastructure() {
    // The share is `at least`, so 20 of 25 (0.8) marks the Run too.
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue(&db, &project, &junit_with(&cases(25, 20, "failed"))).await;
    let outcome = drain(&db, &settings()).await;
    let run = run(&db, outcome[0].run_id.expect("a Run")).await;
    assert!(run.infra_suspect, "0.8 reaches the share");
}

#[tokio::test]
async fn a_run_below_the_fail_share_on_the_last_attempt_is_not_suspected() {
    // 19 of 25 failed, and a 20th Test failed then passed: only the last attempt
    // counts, so the share stays under 0.8.
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let mut report = cases(25, 19, "failed");
    // `t19` failed on an earlier attempt and passed on its last one.
    report.insert(19, ("t19".to_owned(), "failed"));
    queue(&db, &project, &junit_with(&report)).await;
    let outcome = drain(&db, &settings()).await;
    let run = run(&db, outcome[0].run_id.expect("a Run")).await;
    assert!(
        !run.infra_suspect,
        "a Test that recovered on retry is not a failing Test"
    );
    // The recovered attempt is a stored, counted Execution like any other: the
    // rule reads the last attempt, it does not drop rows.
    assert_eq!(
        outcome[0].counts,
        Some(serde_json::json!({
            "passed": 6, "failed": 20, "error": 0, "skipped": 0, "retried": 1
        }))
    );
    assert_eq!(
        test_executions::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        26
    );
}

#[tokio::test]
async fn a_small_all_failing_run_is_not_suspected_infrastructure() {
    // Every Test failed, but 19 is under the floor: a small suite has no shared
    // cause to assume.
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue(&db, &project, &junit_with(&cases(19, 19, "failed"))).await;
    let outcome = drain(&db, &settings()).await;
    let run = run(&db, outcome[0].run_id.expect("a Run")).await;
    assert!(
        !run.infra_suspect,
        "the Run is under the distinct-Test floor"
    );
}

#[tokio::test]
async fn a_run_whose_tests_error_is_suspected_infrastructure() {
    // `error` fails a Test as much as `failed` does.
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue(&db, &project, &junit_with(&cases(25, 21, "error"))).await;
    let outcome = drain(&db, &settings()).await;
    let run = run(&db, outcome[0].run_id.expect("a Run")).await;
    assert!(run.infra_suspect, "errored Tests count toward the share");
}

#[tokio::test]
async fn a_suspected_infrastructure_run_never_scores_a_proven_flake() {
    // `t0` passed and then failed on the same commit, but the failure came in the
    // mass-failure Run: its Executions stay stored and listed, and leave the
    // scoring window, so nothing is proven.
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_with(&db, &project, &make_junit(&[("t0", "passed")]), |builder| {
        builder.commit_sha("a")
    })
    .await;
    queue_with(
        &db,
        &project,
        &junit_with(&cases(25, 21, "failed")),
        |builder| builder.commit_sha("a"),
    )
    .await;
    let outcomes = drain(&db, &settings()).await;
    let second = outcomes[1].clone();
    assert!(
        run(&db, second.run_id.expect("a Run")).await.infra_suspect,
        "Report 2 is the suspected Run"
    );
    let counts = second.counts.expect("counts");
    assert_eq!(counts["failed"], 21, "the failures are still counted");

    let row = tests(&db)
        .await
        .into_iter()
        .find(|row| row.name == "t0")
        .expect("t0");
    assert_eq!((row.confirmed_flake_count, row.fail_streak), (0, 0));
    assert_eq!(row.flakiness_score, 0.0);
    assert_eq!(
        test_executions::Entity::find()
            .filter(test_executions::Column::TestCaseId.eq(row.id))
            .count(db.connection())
            .await
            .expect("a count"),
        2,
        "both of t0's Executions are stored"
    );
}

#[tokio::test]
async fn the_same_flip_in_another_project_still_proves_the_flake() {
    // The control, in a second Project of the same Repo: the same first Report,
    // but its second Report holds only `t0`, so that Run is not suspected and the
    // flip still proves the flake.
    let db = TestDb::new().await.expect("a test database");
    let mass = project(&db, "acme/app", "backend").await;
    let control = project(&db, "acme/app", "frontend").await;
    queue_with(&db, &mass, &make_junit(&[("t0", "passed")]), |builder| {
        builder.commit_sha("a")
    })
    .await;
    queue_with(
        &db,
        &mass,
        &junit_with(&cases(25, 21, "failed")),
        |builder| builder.commit_sha("a"),
    )
    .await;
    queue_with(&db, &control, &make_junit(&[("t0", "passed")]), |builder| {
        builder.commit_sha("a")
    })
    .await;
    queue_with(&db, &control, &make_junit(&[("t0", "failed")]), |builder| {
        builder.commit_sha("a")
    })
    .await;
    let outcomes = drain(&db, &settings()).await;
    assert_eq!(outcomes.len(), 4);
    let row = tests(&db)
        .await
        .into_iter()
        .find(|row| row.name == "t0" && row.project_id == control.id)
        .expect("the control's t0");
    assert_eq!(row.confirmed_flake_count, 1);
    assert!(
        row.flakiness_score >= 0.6,
        "the Proven flake floor: {}",
        row.flakiness_score
    );
}

#[tokio::test]
async fn a_suspected_infrastructure_run_never_consumes_a_window_slot() {
    // The exclusion runs inside the `LATERAL`, before the `LIMIT`: with a window of
    // two, the mass-failure Run must not push the older pair out of the window, so
    // the same-SHA flip is still proven.
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let settings = settings_with(|settings| settings.score_window = 2);
    queue_with(&db, &project, &make_junit(&[("t0", "failed")]), |builder| {
        builder.commit_sha("a")
    })
    .await;
    queue_with(&db, &project, &make_junit(&[("t0", "passed")]), |builder| {
        builder.commit_sha("a")
    })
    .await;
    queue_with(
        &db,
        &project,
        &junit_with(&cases(25, 21, "failed")),
        |builder| builder.commit_sha("b"),
    )
    .await;
    let outcomes = drain(&db, &settings).await;
    assert!(
        run(&db, outcomes[2].run_id.expect("a Run"))
            .await
            .infra_suspect,
        "the newest Report is the suspected Run"
    );
    let row = tests(&db)
        .await
        .into_iter()
        .find(|row| row.name == "t0")
        .expect("t0");
    assert_eq!(
        row.confirmed_flake_count, 1,
        "the flip is still in the window"
    );
    assert!(row.flakiness_score >= 0.6);
}
