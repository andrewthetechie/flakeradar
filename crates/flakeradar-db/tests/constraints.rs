//! The constraints, defaults and timestamps the schema enforces.
//!
//! The rows go through the generated entities rather than through raw inserts, so
//! a case proves the database contract and not the ORM's convenience.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the fixture or the lookup it just wrote fails"
)]

use flakeradar_db::entities::{reports, test_cases};
use flakeradar_test_support::{
    FIXTURE_INSTANT, TestDb, make_job, make_job_execution, make_pipeline, make_project,
    make_test_case,
};
use sea_orm::{ActiveModelTrait, EntityTrait, Set, entity::prelude::DateTimeWithTimeZone};

/// Asserts the insert failed on the named constraint.
fn expect_violation<T: std::fmt::Debug, E: std::fmt::Display>(
    result: Result<T, E>,
    constraint: &str,
) {
    let error = result.expect_err("the constraint rejects the row");
    let message = error.to_string();
    assert!(message.contains(constraint), "{message}");
}

#[tokio::test]
async fn project_name_is_unique_within_its_repo() {
    let db = TestDb::new().await.expect("a migrated test database");
    make_project().create(&db).await;
    make_project().repo("other/app").create(&db).await;
    expect_violation(
        make_project().build(db.connection()).await,
        "uq_projects_repo_name",
    );
}

#[tokio::test]
async fn a_test_fingerprint_is_unique_per_project() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project().create(&db).await;
    make_test_case(&project)
        .fingerprint(&"f".repeat(40))
        .create(&db)
        .await;
    expect_violation(
        make_test_case(&project)
            .name("different name")
            .fingerprint(&"f".repeat(40))
            .build(db.connection())
            .await,
        "uq_test_cases_project_fingerprint",
    );
}

#[tokio::test]
async fn a_new_report_is_pending_and_stamped_in_utc() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project().create(&db).await;
    let created_at: DateTimeWithTimeZone = FIXTURE_INSTANT.fixed_offset();
    let stored = reports::ActiveModel {
        repo_id: Set(project.repo_id),
        project_id: Set(Some(project.id)),
        commit_sha: Set("sha1".to_owned()),
        branch: Set("main".to_owned()),
        body: Set(b"<testsuites/>".to_vec()),
        created_at: Set(created_at),
        ..Default::default()
    }
    .insert(db.connection())
    .await
    .expect("the Report is stored");

    assert_eq!(stored.status, "pending");
    assert_eq!(stored.kind, "junit");
    assert_eq!(stored.created_at.offset().local_minus_utc(), 0);
}

#[tokio::test]
async fn a_junit_report_requires_a_project() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project().create(&db).await;
    expect_violation(
        reports::ActiveModel {
            repo_id: Set(project.repo_id),
            project_id: Set(None),
            commit_sha: Set("sha1".to_owned()),
            branch: Set("main".to_owned()),
            body: Set(b"<testsuites/>".to_vec()),
            created_at: Set(FIXTURE_INSTANT.fixed_offset()),
            ..Default::default()
        }
        .insert(db.connection())
        .await,
        "ck_reports_junit_has_project",
    );
}

#[tokio::test]
async fn a_pipeline_report_needs_no_project() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project().create(&db).await;
    let stored = reports::ActiveModel {
        kind: Set("pipeline".to_owned()),
        repo_id: Set(project.repo_id),
        project_id: Set(None),
        commit_sha: Set("sha1".to_owned()),
        branch: Set("main".to_owned()),
        body: Set(b"{}".to_vec()),
        created_at: Set(FIXTURE_INSTANT.fixed_offset()),
        ..Default::default()
    }
    .insert(db.connection())
    .await
    .expect("a Pipeline Report stores without a Project");
    assert_eq!(stored.kind, "pipeline");
}

#[tokio::test]
async fn a_job_name_is_unique_within_its_pipeline() {
    let db = TestDb::new().await.expect("a migrated test database");
    let pipeline = make_pipeline().create(&db).await;
    make_job(&pipeline).create(&db).await;
    expect_violation(
        make_job(&pipeline).build(db.connection()).await,
        "uq_jobs_pipeline_name",
    );
}

#[tokio::test]
async fn a_job_execution_is_unique_per_ci_job_id() {
    let db = TestDb::new().await.expect("a migrated test database");
    let pipeline = make_pipeline().create(&db).await;
    let job = make_job(&pipeline).create(&db).await;
    make_job_execution(&job).ci_job_id("42").create(&db).await;
    expect_violation(
        make_job_execution(&job)
            .ci_job_id("42")
            .build(db.connection())
            .await,
        "uq_job_executions_job_ci_job_id",
    );
}

#[tokio::test]
async fn a_test_case_stores_its_generated_row() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project().create(&db).await;
    let stored = make_test_case(&project)
        .name("test_something")
        .create(&db)
        .await;
    let found = test_cases::Entity::find_by_id(stored.id)
        .one(db.connection())
        .await
        .expect("the Test is readable")
        .expect("the Test exists");
    assert_eq!(found.fingerprint, stored.fingerprint);
    assert_eq!(found.name, "test_something");
}
