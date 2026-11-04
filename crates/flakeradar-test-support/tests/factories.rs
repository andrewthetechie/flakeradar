//! The row builders, against a real database.
//!
//! Every builder's defaults come from `backend/tests/factories.py`, so a port of
//! a Python test can build the same rows without restating them. Each test here
//! states the defaults it relies on and proves that a setter stores its value.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the fixture it just built cannot be read back"
)]

use std::fmt::Write as _;

use chrono::{TimeZone as _, Utc};
use flakeradar_test_support::{
    TestDb, make_execution, make_job, make_job_execution, make_job_score, make_pipeline,
    make_project, make_run, make_test_case, make_test_score,
};
use sea_orm::{DatabaseConnection, EntityTrait};
use sha1::{Digest as _, Sha1};

/// The lowercase hexadecimal form of `bytes`.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::new();
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The Test row with `id`, read back from the database.
async fn stored_test_case(
    db: &DatabaseConnection,
    id: i32,
) -> flakeradar_db::entities::test_cases::Model {
    flakeradar_db::entities::test_cases::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("the Test is readable")
        .expect("the Test exists")
}

#[tokio::test]
async fn a_project_and_its_repo_have_pythons_defaults() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    assert_eq!(project.name, "default");
    assert_eq!(project.root, "");
    let repo = flakeradar_db::entities::repos::Entity::find_by_id(project.repo_id)
        .one(db.connection())
        .await
        .expect("the Repo is readable")
        .expect("the Repo exists");
    assert_eq!(repo.name, "acme/app");
}

#[tokio::test]
async fn a_project_builder_stores_the_repo_project_and_root_it_was_given() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project()
        .repo("other/app")
        .project("backend")
        .root("backend")
        .build(db.connection())
        .await
        .expect("a Project");
    assert_eq!(project.name, "backend");
    assert_eq!(project.root, "backend");
    let repo = flakeradar_db::entities::repos::Entity::find_by_id(project.repo_id)
        .one(db.connection())
        .await
        .expect("the Repo is readable")
        .expect("the Repo exists");
    assert_eq!(repo.name, "other/app");
}

#[tokio::test]
async fn a_test_has_pythons_defaults_and_a_fingerprint_from_its_identity() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let test_case = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("a Test");
    assert_eq!(test_case.name, "t1");
    assert_eq!(test_case.classname, "tests.test_mod");
    assert_eq!(test_case.suite, "unit");
    let expected = hex(&Sha1::digest(b"unit::tests.test_mod::t1"));
    assert_eq!(test_case.fingerprint, expected);
    assert_eq!(test_case.flakiness_score, 0.0);
    assert_eq!(test_case.confirmed_flake_count, 0);
    assert_eq!(test_case.clean_streak, 0);
    assert_eq!(test_case.fail_streak, 0);
    assert_eq!(test_case.last_status, "passed");
    assert!(!test_case.quarantined);
    assert_eq!(test_case.github_issue_number, None);
}

#[tokio::test]
async fn a_test_builder_stores_its_name_classname_suite_and_issue_number() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let built = make_test_case(&project)
        .name("test_login")
        .classname("tests.test_login")
        .suite("e2e")
        .github_issue_number(123)
        .build(db.connection())
        .await
        .expect("a Test");
    let test_case = stored_test_case(db.connection(), built.id).await;
    assert_eq!(test_case.name, "test_login");
    assert_eq!(test_case.classname, "tests.test_login");
    assert_eq!(test_case.suite, "e2e");
    assert_eq!(test_case.github_issue_number, Some(123));
    let expected = hex(&Sha1::digest(b"e2e::tests.test_login::test_login"));
    assert_eq!(test_case.fingerprint, expected);
}

#[tokio::test]
async fn a_test_builder_keeps_a_fingerprint_it_was_given() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let test_case = make_test_case(&project)
        .fingerprint("0123456789abcdef0123456789abcdef01234567")
        .build(db.connection())
        .await
        .expect("a Test");
    assert_eq!(
        test_case.fingerprint,
        "0123456789abcdef0123456789abcdef01234567"
    );
}

#[tokio::test]
async fn a_run_has_pythons_defaults_and_stores_what_it_is_given() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let run = make_run(&project)
        .build(db.connection())
        .await
        .expect("a Run");
    assert_eq!(run.commit_sha, "sha1");
    assert_eq!(run.branch, "main");
    assert_eq!(run.ci_run_id, "");
    assert_eq!(run.ci_job_id, None);
    assert_eq!(run.ci_run_attempt, None);
    assert_eq!(run.pipeline, None);

    let created_at = Utc
        .with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
        .single()
        .expect("a fixed time")
        .fixed_offset();
    let built = make_run(&project)
        .commit_sha("sha2")
        .branch("feature/x")
        .ci_run_id("9001")
        .ci_job_id("42")
        .ci_run_attempt(2)
        .pipeline(".github/workflows/ci.yml")
        .created_at(created_at)
        .build(db.connection())
        .await
        .expect("a Run");
    assert_eq!(built.commit_sha, "sha2");
    assert_eq!(built.branch, "feature/x");
    assert_eq!(built.ci_run_id, "9001");
    assert_eq!(built.ci_job_id.as_deref(), Some("42"));
    assert_eq!(built.ci_run_attempt, Some(2));
    assert_eq!(built.pipeline.as_deref(), Some(".github/workflows/ci.yml"));
    assert_eq!(built.created_at, created_at);
}

#[tokio::test]
async fn an_execution_has_pythons_defaults_and_stores_what_it_is_given() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let test_case = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("a Test");
    let run = make_run(&project)
        .build(db.connection())
        .await
        .expect("a Run");

    let execution = make_execution(&test_case, &run)
        .build(db.connection())
        .await
        .expect("an Execution");
    assert_eq!(execution.status, "passed");
    assert_eq!(execution.message, "");
    assert_eq!(execution.details, "");
    assert_eq!(execution.attempt, 0);
    assert_eq!(execution.duration, 0.0);
    assert_eq!(execution.failure_category, None);

    let created_at = Utc
        .with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
        .single()
        .expect("a fixed time")
        .fixed_offset();
    let built = make_execution(&test_case, &run)
        .status("failed")
        .message("assert 1 == 2")
        .details("trace")
        .attempt(1)
        .failure_category("assertion")
        .created_at(created_at)
        .build(db.connection())
        .await
        .expect("an Execution");
    assert_eq!(built.status, "failed");
    assert_eq!(built.message, "assert 1 == 2");
    assert_eq!(built.details, "trace");
    assert_eq!(built.attempt, 1);
    assert_eq!(built.failure_category.as_deref(), Some("assertion"));
    assert_eq!(built.created_at, created_at);
}

#[tokio::test]
async fn a_pipeline_and_its_job_have_pythons_defaults() {
    let db = TestDb::new().await.expect("a migrated test database");
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    assert_eq!(pipeline.name, ".github/workflows/ci.yml");
    assert_eq!(pipeline.provider, "github");

    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("a Job");
    assert_eq!(job.name, "test");
    assert!(!job.summary);
    assert_eq!(job.flakiness_score, 0.0);
    assert_eq!(job.last_status, "passed");
    assert_eq!(job.github_issue_number, None);

    let other = make_pipeline()
        .repo("other/app")
        .provider("gitlab")
        .name(".gitlab-ci.yml")
        .build(db.connection())
        .await
        .expect("a Pipeline");
    assert_eq!(other.provider, "gitlab");
    assert_eq!(other.name, ".gitlab-ci.yml");

    let named = make_job(&pipeline)
        .name("e2e")
        .summary(true)
        .github_issue_number(456)
        .build(db.connection())
        .await
        .expect("a Job");
    assert_eq!(named.name, "e2e");
    assert!(named.summary);
    assert_eq!(named.github_issue_number, Some(456));
}

#[tokio::test]
async fn a_job_execution_has_pythons_defaults_and_a_fresh_ci_job_id() {
    let db = TestDb::new().await.expect("a migrated test database");
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("a Job");

    let first = make_job_execution(&job)
        .build(db.connection())
        .await
        .expect("a Job execution");
    let second = make_job_execution(&job)
        .build(db.connection())
        .await
        .expect("a Job execution");
    assert_eq!(first.status, "passed");
    assert_eq!(first.commit_sha, "sha1");
    assert_eq!(first.branch, "main");
    assert_eq!(first.ci_run_id, "1");
    assert_eq!(first.ci_run_attempt, 1);
    assert!(
        first.ci_job_id != second.ci_job_id,
        "each Job execution gets its own CI job id"
    );

    let created_at = Utc
        .with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
        .single()
        .expect("a fixed time")
        .fixed_offset();
    let built = make_job_execution(&job)
        .status("failed")
        .ci_job_id("999")
        .commit_sha("sha2")
        .branch("feature/x")
        .ci_run_id("9001")
        .ci_run_attempt(2)
        .created_at(created_at)
        .build(db.connection())
        .await
        .expect("a Job execution");
    assert_eq!(built.status, "failed");
    assert_eq!(built.ci_job_id, "999");
    assert_eq!(built.commit_sha, "sha2");
    assert_eq!(built.branch, "feature/x");
    assert_eq!(built.ci_run_id, "9001");
    assert_eq!(built.ci_run_attempt, 2);
    assert_eq!(built.created_at, created_at);
}

#[tokio::test]
async fn a_job_execution_refuses_a_status_postgres_would_accept() {
    let db = TestDb::new().await.expect("a migrated test database");
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("a Job");
    // A Job execution has no CHECK on its outcome, so the fixture refuses a
    // status the server never writes instead of storing a row no reader expects.
    let outcome = make_job_execution(&job)
        .status("flaky")
        .build(db.connection())
        .await;
    assert!(
        matches!(&outcome, Err(flakeradar_test_support::TestError::Fixture(message)) if message.contains("bad job status")),
        "the fixture refuses an impossible outcome: {outcome:?}"
    );
}

#[tokio::test]
async fn a_score_history_row_is_one_day_for_one_owner() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let test_case = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("a Test");
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("a Job");
    let day = Utc
        .with_ymd_and_hms(2026, 1, 2, 0, 0, 0)
        .single()
        .expect("a fixed time")
        .date_naive();

    let test_score = make_test_score(&test_case, day, 0.5)
        .build(db.connection())
        .await
        .expect("a Test Score history row");
    assert_eq!(test_score.day, day);
    assert_eq!(test_score.flakiness_score, 0.5);
    assert_eq!(test_score.confirmed_flake_count, 0);
    assert_eq!(test_score.executions, 0);
    assert_eq!(test_score.failures, 0);

    let job_score = make_job_score(&job, day, 0.25)
        .build(db.connection())
        .await
        .expect("a Job Score history row");
    assert_eq!(job_score.day, day);
    assert_eq!(job_score.flakiness_score, 0.25);
    assert_eq!(job_score.confirmed_flake_count, 0);
    assert_eq!(job_score.executions, 0);
    assert_eq!(job_score.failures, 0);
}
