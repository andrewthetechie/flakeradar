//! Job attribution: linked JUnit Reports explain Job failures; only unexplained count.
//!
//! The `explained_ci_job_ids` / `unexplained_failure_counts` reads, the Summary Job
//! rules (issue #9), and the way `job_history` turns an explained failure into a
//! `skipped` row before scoring sees it.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the processor misbehaves"
)]

use std::sync::Arc;

use chrono::{DateTime, FixedOffset};
use flakeradar_core::{
    dto::JobResultIn, dto::PipelineReportIn, dto::pipeline_body, model::EntityKind,
    model::JobStatus, types::CiRunAttempt, types::CiRunId,
};
use flakeradar_db::entities::{job_executions, jobs, projects, repos, test_runs};
use flakeradar_db::{
    SIBLING_MAX, explained_ci_job_ids, get_or_create_project, processing::ProcessingSettings,
    processing::job_history, processing::process_next, processing::rescore,
    summary_explaining_siblings, unexplained_failure_counts,
};
use flakeradar_test_support::{
    FixedClock, TestDb, make_execution, make_job, make_job_execution, make_junit, make_pipeline,
    make_report, make_run, make_test_case,
};
use sea_orm::{ConnectionTrait, EntityTrait, QueryOrder};

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

/// Gets or creates the Project `project` in `repo`, at the fixed instant.
async fn project(db: &TestDb, repo: &str, project: &str) -> projects::Model {
    get_or_create_project(db.connection(), repo, project, at().fixed_offset())
        .await
        .expect("a Project")
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

/// A Summary Job: one that runs no Tests and only reports its `needs`.
fn summary_job(ci_job_id: &str, name: &str, status: JobStatus) -> JobResultIn {
    JobResultIn {
        summary: true,
        ..job_in(ci_job_id, name, status)
    }
}

/// One Pipeline payload with the documented defaults.
fn payload(jobs: Vec<JobResultIn>) -> PipelineReportIn {
    PipelineReportIn {
        repo: "acme/app".to_owned(),
        provider: "github".to_owned(),
        pipeline: ".github/workflows/ci.yml".to_owned(),
        commit_sha: "s".to_owned(),
        branch: "main".to_owned(),
        default_branch: Some("main".to_owned()),
        ci_run_id: CiRunId::new("1").expect("an id"),
        ci_run_attempt: 1,
        jobs,
    }
}

/// Queues a Pipeline Report.
async fn queue_pipeline(db: &TestDb, project: &projects::Model, payload: &PipelineReportIn) -> i32 {
    make_report(project, project.repo_id)
        .kind("pipeline")
        .body(&pipeline_body(payload))
        .commit_sha(&payload.commit_sha)
        .branch(&payload.branch)
        .ci_run_id(payload.ci_run_id.as_str())
        .build(db.connection())
        .await
        .expect("a queued Pipeline Report")
        .id
}

/// Queues a JUnit Report linked to `ci_job_id`.
async fn queue_junit(
    db: &TestDb,
    project: &projects::Model,
    cases: &[(&str, &str)],
    ci_job_id: Option<&str>,
    commit_sha: &str,
) -> i32 {
    let body = make_junit(cases);
    let mut builder = make_report(project, project.repo_id)
        .body(&body)
        .commit_sha(commit_sha);
    if let Some(ci_job_id) = ci_job_id {
        builder = builder.ci_job_id(ci_job_id);
    }
    builder
        .build(db.connection())
        .await
        .expect("a queued JUnit Report")
        .id
}

/// Processes one Report and returns its outcome.
async fn process(
    db: &TestDb,
    settings: &ProcessingSettings,
) -> flakeradar_db::processing::ProcessOutcome {
    process_next(db.connection(), settings)
        .await
        .expect("the processor runs")
        .expect("a Report")
}

/// The one Job row.
async fn job(db: &TestDb) -> jobs::Model {
    jobs::Entity::find()
        .order_by_asc(jobs::Column::Name)
        .all(db.connection())
        .await
        .expect("the Jobs")
        .pop()
        .expect("one Job")
}

/// The Job named `name`.
async fn job_named(db: &TestDb, name: &str) -> jobs::Model {
    jobs::Entity::find()
        .order_by_asc(jobs::Column::Name)
        .all(db.connection())
        .await
        .expect("the Jobs")
        .into_iter()
        .find(|row| row.name == name)
        .expect("the named Job")
}

/// The one Repo row.
async fn repo(db: &TestDb) -> repos::Model {
    repos::Entity::find()
        .one(db.connection())
        .await
        .expect("the Repo")
        .expect("one Repo")
}

/// The stored statuses of `job_id`'s history, newest first, as scoring sees them.
async fn history_statuses(db: &TestDb, job_id: i32) -> Vec<String> {
    job_history(db.connection(), &[job_id], &settings())
        .await
        .expect("the history")
        .get(&job_id)
        .expect("the Job's history")
        .executions
        .iter()
        .map(|execution| execution.status.as_str().to_owned())
        .collect()
}

/// A CI run id a test states as a literal.
fn run_id(text: &str) -> CiRunId {
    CiRunId::new(text).expect("the test states a CI run id")
}

#[tokio::test]
async fn pipeline_first_then_junit_explains() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![job_in("J1", "test", JobStatus::Failed)]),
    )
    .await;
    process(&db, &settings()).await;
    queue_junit(&db, &project, &[("t1", "failed")], Some("J1"), "s").await;
    process(&db, &settings()).await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![job_in("J2", "test", JobStatus::Passed)]),
    )
    .await;
    process(&db, &settings()).await;
    let row = job(&db).await;
    // J1's failure is explained (failing t1 on the same ci_job_id), so the
    // history is [pass, skipped] and there is no flip.
    assert_eq!(row.confirmed_flake_count, 0);
    assert_eq!(row.flakiness_score, 0.0);
}

#[tokio::test]
async fn junit_first_then_pipeline_explains() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_junit(&db, &project, &[("t1", "failed")], Some("J1"), "s").await;
    process(&db, &settings()).await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![job_in("J1", "test", JobStatus::Failed)]),
    )
    .await;
    process(&db, &settings()).await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![job_in("J2", "test", JobStatus::Passed)]),
    )
    .await;
    process(&db, &settings()).await;
    assert_eq!(job(&db).await.flakiness_score, 0.0);
}

#[tokio::test]
async fn unexplained_failure_moves_score() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![job_in("J1", "test", JobStatus::Failed)]),
    )
    .await;
    process(&db, &settings()).await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![job_in("J2", "test", JobStatus::Passed)]),
    )
    .await;
    process(&db, &settings()).await;
    assert!(job(&db).await.flakiness_score > 0.0);
}

#[tokio::test]
async fn cross_repo_run_does_not_explain() {
    let db = TestDb::new().await.expect("a test database");
    project(&db, "acme/app", "backend").await;
    let other = project(&db, "other/app", "backend").await;
    queue_pipeline(
        &db,
        &project(&db, "acme/app", "backend").await,
        &payload(vec![job_in("J1", "test", JobStatus::Failed)]),
    )
    .await;
    process(&db, &settings()).await;
    queue_pipeline(
        &db,
        &project(&db, "acme/app", "backend").await,
        &payload(vec![job_in("J2", "test", JobStatus::Passed)]),
    )
    .await;
    process(&db, &settings()).await;
    // A Run in ANOTHER repo with the same ci_job_id must not explain acme/app's J1.
    queue_junit(&db, &other, &[("t1", "failed")], Some("J1"), "s").await;
    process(&db, &settings()).await;
    assert!(job(&db).await.flakiness_score > 0.0);
}

#[tokio::test]
async fn passing_tests_do_not_explain() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![job_in("J1", "test", JobStatus::Failed)]),
    )
    .await;
    process(&db, &settings()).await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![job_in("J2", "test", JobStatus::Passed)]),
    )
    .await;
    process(&db, &settings()).await;
    queue_junit(&db, &project, &[("t1", "passed")], Some("J1"), "s").await;
    process(&db, &settings()).await;
    // Only passing Tests, so the failure is still unexplained.
    assert!(job(&db).await.flakiness_score > 0.0);
}

#[tokio::test]
async fn attribution_params_round_trip_to_test_run() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let body = make_junit(&[("t1", "passed")]);
    make_report(&project, project.repo_id)
        .body(&body)
        .commit_sha("s")
        .ci_job_id("42")
        .ci_run_attempt(3)
        .pipeline(".github/workflows/ci.yml")
        .build(db.connection())
        .await
        .expect("a queued Report");
    process(&db, &settings()).await;
    let run = test_runs::Entity::find()
        .one(db.connection())
        .await
        .expect("the Run")
        .expect("one Run");
    assert_eq!(
        (
            run.ci_job_id.as_deref(),
            run.ci_run_attempt,
            run.pipeline.as_deref()
        ),
        (Some("42"), Some(3), Some(".github/workflows/ci.yml"))
    );
}

#[tokio::test]
async fn explained_ci_job_ids_agrees_with_scoring() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![job_in("J1", "test", JobStatus::Failed)]),
    )
    .await;
    process(&db, &settings()).await;
    queue_junit(&db, &project, &[("t1", "failed")], Some("J1"), "s").await;
    process(&db, &settings()).await;

    let row = job(&db).await;
    let repo = repo(&db).await;
    assert_eq!(
        explained_ci_job_ids(db.connection(), repo.id, &["J1".to_owned()])
            .await
            .expect("the explained ids"),
        std::collections::BTreeSet::from(["J1".to_owned()])
    );
    // The scoring history agrees: J1 is fed to scoring as skipped.
    assert_eq!(history_statuses(&db, row.id).await, vec!["skipped"]);

    // A ci_job_id with only passing Tests is not explained.
    queue_junit(&db, &project, &[("t1", "passed")], Some("J2"), "s").await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![job_in("J2", "test", JobStatus::Failed)]),
    )
    .await;
    process(&db, &settings()).await;
    process(&db, &settings()).await;
    assert!(
        explained_ci_job_ids(db.connection(), repo.id, &["J2".to_owned()])
            .await
            .expect("the explained ids")
            .is_empty()
    );
}

#[tokio::test]
async fn unexplained_failure_counts_over_the_newest_window() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_junit(&db, &project, &[("t1", "failed")], Some("J1"), "s").await;
    process(&db, &settings()).await;
    // Oldest to newest: explained failure, unexplained failure, pass, unexplained failure.
    for (ci_job_id, status) in [
        ("J1", JobStatus::Failed),
        ("J2", JobStatus::Failed),
        ("J3", JobStatus::Passed),
        ("J4", JobStatus::Failed),
    ] {
        queue_pipeline(
            &db,
            &project,
            &payload(vec![job_in(ci_job_id, "test", status)]),
        )
        .await;
        process(&db, &settings()).await;
    }
    let job = job(&db).await;
    assert_eq!(
        unexplained_failure_counts(db.connection(), &[job.id], 10)
            .await
            .expect("the counts"),
        std::collections::BTreeMap::from([(job.id, 2)])
    );
    assert_eq!(
        unexplained_failure_counts(db.connection(), &[job.id], 2)
            .await
            .expect("the counts"),
        std::collections::BTreeMap::from([(job.id, 1)])
    );
    assert!(
        unexplained_failure_counts(db.connection(), &[], 10)
            .await
            .expect("the counts")
            .is_empty()
    );
}

#[tokio::test]
async fn explained_failure_does_not_break_clean_streak() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let job = make_job(&pipeline)
        .name("e2e")
        .build(db.connection())
        .await
        .expect("a Job");
    for (ci_job_id, status) in [("J1", "passed"), ("J2", "failed"), ("J3", "passed")] {
        make_job_execution(&job)
            .ci_job_id(ci_job_id)
            .status(status)
            .branch("main")
            .build(db.connection())
            .await
            .expect("a Job execution");
    }
    // Explain J2: a failing Test in the same repo with the same ci_job_id.
    let test_case = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("a Test");
    let run = make_run(&project)
        .ci_job_id("J2")
        .build(db.connection())
        .await
        .expect("a Run");
    make_execution(&test_case, &run)
        .status("failed")
        .build(db.connection())
        .await
        .expect("an Execution");

    rescore(
        db.connection(),
        EntityKind::Job,
        &[job.id],
        None,
        &settings(),
    )
    .await
    .expect("the rescore");
    let stored = job_named(&db, "e2e").await;
    assert_eq!(stored.clean_streak, 2);
}

#[tokio::test]
async fn unexplained_failure_breaks_clean_streak() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let job = make_job(&pipeline)
        .name("e2e")
        .build(db.connection())
        .await
        .expect("a Job");
    for (ci_job_id, status) in [("J1", "passed"), ("J2", "failed"), ("J3", "passed")] {
        make_job_execution(&job)
            .ci_job_id(ci_job_id)
            .status(status)
            .branch("main")
            .build(db.connection())
            .await
            .expect("a Job execution");
    }
    rescore(
        db.connection(),
        EntityKind::Job,
        &[job.id],
        None,
        &settings(),
    )
    .await
    .expect("the rescore");
    assert_eq!(job_named(&db, "e2e").await.clean_streak, 1);
}

#[tokio::test]
async fn old_explained_failures_beyond_the_window_do_not_change_the_score() {
    let db = TestDb::new().await.expect("a test database");
    let settings = ProcessingSettings {
        score_window: 3,
        ..settings()
    };
    let project = project(&db, "acme/app", "backend").await;
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let twin = make_job(&pipeline)
        .name("recent-only")
        .build(db.connection())
        .await
        .expect("a Job");
    let full = make_job(&pipeline)
        .name("with-old-explained")
        .build(db.connection())
        .await
        .expect("a Job");
    // 6 old explained failures on `full`: a failing Test run with the same ci_job_id explains each.
    let test_case = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("a Test");
    for index in 0..6 {
        make_job_execution(&full)
            .status("failed")
            .ci_job_id(&format!("OLD{index}"))
            .branch("main")
            .commit_sha(&format!("o{index}"))
            .build(db.connection())
            .await
            .expect("a Job execution");
        let run = make_run(&project)
            .ci_job_id(&format!("OLD{index}"))
            .build(db.connection())
            .await
            .expect("a Run");
        make_execution(&test_case, &run)
            .status("failed")
            .build(db.connection())
            .await
            .expect("an Execution");
    }
    // Both Jobs then share the same newest 3 (unexplained) rows: failed, passed, failed.
    for job in [&twin, &full] {
        for (index, status) in ["failed", "passed", "failed"].iter().enumerate() {
            make_job_execution(job)
                .status(status)
                .ci_job_id(&format!("NEW{}-{index}", job.id))
                .branch("main")
                .commit_sha(&format!("n{index}"))
                .build(db.connection())
                .await
                .expect("a Job execution");
        }
    }

    rescore(
        db.connection(),
        EntityKind::Job,
        &[twin.id, full.id],
        None,
        &settings,
    )
    .await
    .expect("the rescore");
    let twin = job_named(&db, "recent-only").await;
    let full = job_named(&db, "with-old-explained").await;
    assert_eq!(full.flakiness_score, twin.flakiness_score);
    assert!(full.flakiness_score > 0.0);
    assert_eq!(full.confirmed_flake_count, twin.confirmed_flake_count);
    assert_eq!(full.clean_streak, twin.clean_streak);
    assert_eq!(
        unexplained_failure_counts(db.connection(), &[twin.id, full.id], 3)
            .await
            .expect("the counts"),
        std::collections::BTreeMap::from([(twin.id, 2), (full.id, 2)])
    );
}

#[tokio::test]
async fn job_default_branch_window_survives_newer_feature_rows() {
    let db = TestDb::new().await.expect("a test database");
    let settings = ProcessingSettings {
        score_window: 3,
        ..settings()
    };
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    db.connection()
        .execute_unprepared("UPDATE repos SET default_branch = 'main'")
        .await
        .expect("the Default branch");
    let job = make_job(&pipeline)
        .name("e2e")
        .build(db.connection())
        .await
        .expect("a Job");
    for (index, status) in ["failed", "passed", "failed"].iter().enumerate() {
        make_job_execution(&job)
            .status(status)
            .ci_job_id(&format!("M{index}"))
            .branch("main")
            .commit_sha(&format!("m{index}"))
            .build(db.connection())
            .await
            .expect("a Job execution");
    }
    for index in 0..10 {
        make_job_execution(&job)
            .status("passed")
            .ci_job_id(&format!("F{index}"))
            .branch("feat")
            .commit_sha(&format!("f{index}"))
            .build(db.connection())
            .await
            .expect("a Job execution");
    }

    rescore(db.connection(), EntityKind::Job, &[job.id], None, &settings)
        .await
        .expect("the rescore");
    // The 3 newest main flips still score despite 10 newer feat rows.
    assert!(job_named(&db, "e2e").await.flakiness_score > 0.0);
}

#[tokio::test]
async fn summary_job_explained_by_a_failed_sibling() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![
            job_in("J1", "test", JobStatus::Failed),
            summary_job("J2", "alls-green", JobStatus::Failed),
        ]),
    )
    .await;
    process(&db, &settings()).await;

    let repo = repo(&db).await;
    // The non-summary Job has no Run, so it stays unexplained; the summary Job is explained.
    assert_eq!(
        explained_ci_job_ids(
            db.connection(),
            repo.id,
            &["J1".to_owned(), "J2".to_owned()]
        )
        .await
        .expect("the explained ids"),
        std::collections::BTreeSet::from(["J2".to_owned()])
    );
    let summary = job_named(&db, "alls-green").await;
    assert_eq!(history_statuses(&db, summary.id).await, vec!["skipped"]);
}

#[tokio::test]
async fn summary_job_without_a_failed_sibling_is_unexplained() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![summary_job("J1", "alls-green", JobStatus::Failed)]),
    )
    .await;
    process(&db, &settings()).await;

    let repo = repo(&db).await;
    assert!(
        explained_ci_job_ids(db.connection(), repo.id, &["J1".to_owned()])
            .await
            .expect("the explained ids")
            .is_empty()
    );
    let summary = job(&db).await;
    assert_eq!(history_statuses(&db, summary.id).await, vec!["failed"]);
}

#[tokio::test]
async fn summary_jobs_do_not_explain_each_other() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![
            summary_job("J1", "alls-green-a", JobStatus::Failed),
            summary_job("J2", "alls-green-b", JobStatus::Failed),
        ]),
    )
    .await;
    process(&db, &settings()).await;

    let repo = repo(&db).await;
    assert!(
        explained_ci_job_ids(
            db.connection(),
            repo.id,
            &["J1".to_owned(), "J2".to_owned()]
        )
        .await
        .expect("the explained ids")
        .is_empty()
    );
}

#[tokio::test]
async fn summary_job_explained_when_the_sibling_failure_is_a_flaky_test() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![
            job_in("J1", "test-playwright", JobStatus::Failed),
            summary_job("J2", "alls-green-playwright", JobStatus::Failed),
        ]),
    )
    .await;
    process(&db, &settings()).await;
    queue_junit(&db, &project, &[("t1", "failed")], Some("J1"), "s").await;
    process(&db, &settings()).await;

    let repo = repo(&db).await;
    assert_eq!(
        explained_ci_job_ids(
            db.connection(),
            repo.id,
            &["J1".to_owned(), "J2".to_owned()]
        )
        .await
        .expect("the explained ids"),
        std::collections::BTreeSet::from(["J1".to_owned(), "J2".to_owned()])
    );
}

#[tokio::test]
async fn summary_job_is_not_explained_by_another_attempt() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let mut first = payload(vec![
        job_in("J1", "test", JobStatus::Failed),
        summary_job("J2", "alls-green", JobStatus::Failed),
    ]);
    first.ci_run_id = CiRunId::new("100").expect("an id");
    first.ci_run_attempt = 1;
    queue_pipeline(&db, &project, &first).await;
    process(&db, &settings()).await;
    let mut second = payload(vec![
        job_in("J3", "test", JobStatus::Passed),
        summary_job("J4", "alls-green", JobStatus::Failed),
    ]);
    second.ci_run_id = CiRunId::new("100").expect("an id");
    second.ci_run_attempt = 2;
    queue_pipeline(&db, &project, &second).await;
    process(&db, &settings()).await;

    let repo = repo(&db).await;
    assert_eq!(
        explained_ci_job_ids(
            db.connection(),
            repo.id,
            &["J2".to_owned(), "J4".to_owned()]
        )
        .await
        .expect("the explained ids"),
        std::collections::BTreeSet::from(["J2".to_owned()])
    );
}

#[tokio::test]
async fn summary_job_failure_does_not_move_its_score() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![
            job_in("J1", "test", JobStatus::Failed),
            summary_job("J2", "alls-green", JobStatus::Failed),
        ]),
    )
    .await;
    process(&db, &settings()).await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![
            job_in("J3", "test", JobStatus::Passed),
            summary_job("J4", "alls-green", JobStatus::Passed),
        ]),
    )
    .await;
    process(&db, &settings()).await;

    let summary = job_named(&db, "alls-green").await;
    assert_eq!(summary.flakiness_score, 0.0);
    assert_eq!(summary.confirmed_flake_count, 0);
    assert_eq!(summary.clean_streak, 1);
}

#[tokio::test]
async fn unexplained_failure_counts_excludes_summary_explained() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![
            job_in("J1", "test", JobStatus::Failed),
            summary_job("J2", "alls-green", JobStatus::Failed),
        ]),
    )
    .await;
    process(&db, &settings()).await;

    let summary = job_named(&db, "alls-green").await;
    let other = job_named(&db, "test").await;
    assert!(
        unexplained_failure_counts(db.connection(), &[summary.id], 10)
            .await
            .expect("the counts")
            .is_empty()
    );
    assert_eq!(
        unexplained_failure_counts(db.connection(), &[other.id], 10)
            .await
            .expect("the counts"),
        std::collections::BTreeMap::from([(other.id, 1)])
    );
}

#[tokio::test]
async fn summary_job_with_an_empty_ci_run_id_is_unexplained() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    let mut first = payload(vec![job_in("J1", "test", JobStatus::Failed)]);
    first.ci_run_id = CiRunId::empty();
    queue_pipeline(&db, &project, &first).await;
    process(&db, &settings()).await;
    let mut second = payload(vec![summary_job("J2", "alls-green", JobStatus::Failed)]);
    second.ci_run_id = CiRunId::empty();
    queue_pipeline(&db, &project, &second).await;
    process(&db, &settings()).await;

    let repo = repo(&db).await;
    assert!(
        explained_ci_job_ids(db.connection(), repo.id, &["J2".to_owned()])
            .await
            .expect("the explained ids")
            .is_empty()
    );
    let summary = job_named(&db, "alls-green").await;
    assert_eq!(history_statuses(&db, summary.id).await, vec!["failed"]);
}

#[tokio::test]
async fn summary_flag_flip_rescores_without_a_new_execution() {
    let db = TestDb::new().await.expect("a test database");
    let project = project(&db, "acme/app", "backend").await;
    queue_pipeline(
        &db,
        &project,
        &payload(vec![
            job_in("J1", "test", JobStatus::Passed),
            job_in("J2", "alls-green", JobStatus::Passed),
        ]),
    )
    .await;
    process(&db, &settings()).await;
    let mut second = payload(vec![
        job_in("J3", "test", JobStatus::Failed),
        job_in("J4", "alls-green", JobStatus::Failed),
    ]);
    second.commit_sha = "b".to_owned();
    queue_pipeline(&db, &project, &second).await;
    process(&db, &settings()).await;

    let summary = job_named(&db, "alls-green").await;
    // Not yet a summary Job, so the failure counts.
    assert!(summary.flakiness_score > 0.0);

    // The same executions are re-reported, now with the marker: no new execution
    // is inserted, so only the flag change can trigger the correcting rescore.
    let mut third = payload(vec![
        job_in("J3", "test", JobStatus::Failed),
        summary_job("J4", "alls-green", JobStatus::Failed),
    ]);
    third.commit_sha = "b".to_owned();
    queue_pipeline(&db, &project, &third).await;
    process(&db, &settings()).await;

    let summary = job_named(&db, "alls-green").await;
    assert!(summary.summary);
    assert_eq!(summary.flakiness_score, 0.0);
    assert_eq!(summary.confirmed_flake_count, 0);
}

#[tokio::test]
async fn a_job_execution_with_an_unknown_status_refuses_to_score() {
    // `job_history` parses the stored status, so a row the schema accepted but the
    // scoring code does not know fails loudly rather than scoring as a pass.
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("a Job");
    job_executions::Entity::insert(job_executions::ActiveModel {
        job_id: sea_orm::Set(job.id),
        ci_job_id: sea_orm::Set("J1".to_owned()),
        ci_run_id: sea_orm::Set("1".to_owned()),
        ci_run_attempt: sea_orm::Set(1),
        commit_sha: sea_orm::Set("sha1".to_owned()),
        branch: sea_orm::Set("main".to_owned()),
        status: sea_orm::Set("mystery".to_owned()),
        url: sea_orm::Set(String::new()),
        runner_name: sea_orm::Set(String::new()),
        runner_labels: sea_orm::Set(serde_json::json!([])),
        started_at: sea_orm::Set(None),
        completed_at: sea_orm::Set(None),
        created_at: sea_orm::Set(at()),
        ..Default::default()
    })
    .exec(db.connection())
    .await
    .expect("a Job execution the processor never wrote");
    let error = job_history(db.connection(), &[job.id], &settings())
        .await
        .expect_err("an unknown status");
    assert!(
        error
            .to_string()
            .contains("unknown Execution status mystery")
    );
}

#[tokio::test]
async fn summary_explaining_siblings_lists_the_failed_jobs_of_each_pair() {
    // The read API lists the Jobs behind a Summary Job's explanation, so the query
    // is checked here: one pair, a second attempt, an empty run ID, a passed
    // execution, a Summary Job and another Repo must all be left out or kept apart.
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let lint = make_job(&pipeline)
        .name("lint")
        .build(db.connection())
        .await
        .expect("a Job");
    let gate = make_job(&pipeline)
        .name("gate")
        .summary(true)
        .build(db.connection())
        .await
        .expect("a Summary Job");
    for (job, status, run_id, attempt) in [
        (&lint, "failed", "7", 1),
        (&lint, "passed", "7", 1),
        (&gate, "failed", "7", 1),
        (&lint, "failed", "7", 2),
        (&lint, "failed", "8", 1),
    ] {
        make_job_execution(job)
            .status(status)
            .ci_run_id(run_id)
            .ci_run_attempt(attempt)
            .build(db.connection())
            .await
            .expect("a Job execution");
    }
    let other = make_pipeline()
        .repo("other/app")
        .build(db.connection())
        .await
        .expect("a Pipeline in another Repo");
    let foreign = make_job(&other)
        .name("lint")
        .build(db.connection())
        .await
        .expect("a Job");
    make_job_execution(&foreign)
        .status("failed")
        .ci_run_id("7")
        .ci_run_attempt(1)
        .build(db.connection())
        .await
        .expect("a Job execution");
    let repo = repos::Entity::find()
        .one(db.connection())
        .await
        .expect("the Repo")
        .expect("the Repo exists");

    let empty = summary_explaining_siblings(
        db.connection(),
        repo.id,
        &[CiRunAttempt::owned(run_id(""), 1)],
    )
    .await
    .expect("the sibling query runs");
    assert!(empty.is_empty(), "an empty ci_run_id explains nothing");

    let explained = summary_explaining_siblings(
        db.connection(),
        repo.id,
        &[
            CiRunAttempt::owned(run_id("7"), 1),
            CiRunAttempt::owned(run_id("8"), 1),
            CiRunAttempt::owned(run_id("7"), 1),
        ],
    )
    .await
    .expect("the sibling query runs");
    let siblings = explained
        .get(&CiRunAttempt::owned(run_id("7"), 1))
        .expect("the pair with a failure");
    assert_eq!(
        siblings
            .iter()
            .map(|sibling| (sibling.name.as_str(), sibling.pipeline.as_str()))
            .collect::<Vec<_>>(),
        [("lint", ".github/workflows/ci.yml")]
    );
    assert_eq!(
        explained
            .get(&CiRunAttempt::owned(run_id("8"), 1))
            .map(Vec::len),
        Some(1),
        "a pair of its own keeps its own failures"
    );
    assert_eq!(explained.len(), 2);
}

#[tokio::test]
async fn summary_explaining_siblings_lists_at_most_the_cap_per_pair() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let lint = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("a Job");
    for _ in 0..=SIBLING_MAX {
        make_job_execution(&lint)
            .status("failed")
            .ci_run_id("7")
            .ci_run_attempt(1)
            .build(db.connection())
            .await
            .expect("a Job execution");
    }
    let repo = repos::Entity::find()
        .one(db.connection())
        .await
        .expect("the Repo")
        .expect("the Repo exists");
    let explained = summary_explaining_siblings(
        db.connection(),
        repo.id,
        &[CiRunAttempt::owned(run_id("7"), 1)],
    )
    .await
    .expect("the sibling query runs");
    assert_eq!(
        explained
            .get(&CiRunAttempt::owned(run_id("7"), 1))
            .map(Vec::len),
        Some(SIBLING_MAX)
    );
}
