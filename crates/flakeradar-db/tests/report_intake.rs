//! Report intake: the pending-row builders, the JUnit duplicate identity and the
//! queue transitions.
//!
//! The behaviour is `backend/tests/test_report_intake.py`: a builder produces an
//! un-added row, the duplicate lookup reads only, and a transition changes
//! fields without committing.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when report intake misbehaves"
)]

use flakeradar_core::{dto::PipelineReportIn, dto::pipeline_body, model::ReportStatus};
use flakeradar_db::entities::{projects, reports};
use flakeradar_db::{
    DbError, NewJunitReport, NewPipelineReport, complete_report, fail_report, find_junit_duplicate,
    get_or_create_project, requeue_report, reset_for_retry, stored_ci_run_attempt,
    stored_junit_body, stored_pipeline_report,
};
use flakeradar_test_support::{TestDb, make_junit, make_report};
use sea_orm::entity::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, PaginatorTrait as _, Statement, Value};

/// A fixed instant, so a stored `created_at` never depends on the clock.
fn at() -> DateTimeWithTimeZone {
    chrono::DateTime::parse_from_rfc3339("2026-10-20T09:00:00+00:00").expect("a fixed instant")
}

/// The uploaded bytes every case uses.
fn body() -> Vec<u8> {
    make_junit(&[("t1", "failed"), ("t2", "passed")])
}

/// The attribution every identity case carries.
fn identity() -> NewJunitReport {
    NewJunitReport {
        project_id: 0,
        repo_id: 0,
        body: body(),
        commit_sha: "sha1".to_owned(),
        branch: "main".to_owned(),
        ci_run_id: "7".to_owned(),
        root: Some("pkg".to_owned()),
        line_base: Some(1),
        ci_job_id: Some("J1".to_owned()),
        ci_run_attempt: Some(2),
        pipeline: Some(".github/workflows/ci.yml".to_owned()),
        default_branch: Some("main".to_owned()),
    }
}

/// The Project the cases hang a Report on.
async fn project(db: &TestDb) -> projects::Model {
    get_or_create_project(db.connection(), "acme/app", "default", at())
        .await
        .expect("a Project")
}

/// `candidate` aimed at `project`.
fn with_project(project: &projects::Model, candidate: NewJunitReport) -> NewJunitReport {
    NewJunitReport {
        project_id: project.id,
        repo_id: project.repo_id,
        ..candidate
    }
}

/// Inserts one Report, as ingest would queue it.
async fn seed(db: &TestDb, candidate: NewJunitReport) -> reports::Model {
    reports::Entity::insert(candidate.into_active_model(at()))
        .exec_with_returning(db.connection())
        .await
        .expect("a Report")
}

/// Every stored Report id, oldest first.
async fn report_ids(db: &TestDb) -> Vec<i32> {
    let rows = reports::Entity::find()
        .all(db.connection())
        .await
        .expect("the Report rows");
    rows.into_iter().map(|report| report.id).collect()
}

/// The Pipeline Report the cases build a body from.
fn pipeline_report() -> PipelineReportIn {
    let json = serde_json::json!({
        "repo": "acme/app",
        "provider": "github",
        "pipeline": ".github/workflows/ci.yml",
        "commit_sha": "sha1",
        "branch": "main",
        "default_branch": "main",
        "ci_run_id": "1",
        "ci_run_attempt": 1,
        "jobs": [{"ci_job_id": "1", "name": "test", "status": "passed"}]
    });
    serde_json::from_value(json).expect("the fixture Report deserializes")
}

#[test]
fn a_digest_is_the_sha256_of_the_bytes() {
    let candidate = NewJunitReport {
        body: b"abc".to_vec(),
        ..identity()
    };
    assert_eq!(
        candidate.body_sha256(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    // A different body is a different digest.
    let other = NewJunitReport {
        body: make_junit(&[("t1", "passed")]),
        ..candidate.clone()
    };
    assert_ne!(other.body_sha256(), candidate.body_sha256());
}

#[test]
fn the_junit_row_carries_the_body_and_its_digest() {
    let candidate = identity();
    let digest = candidate.body_sha256();
    let row = candidate.into_active_model(at());
    assert_eq!(row.kind, sea_orm::Set("junit".to_owned()));
    assert_eq!(row.project_id, sea_orm::Set(Some(0)));
    assert_eq!(row.body, sea_orm::Set(body()));
    assert_eq!(row.body_sha256, sea_orm::Set(Some(digest)));
    assert_eq!(
        row.status,
        sea_orm::Set(ReportStatus::Pending.as_str().to_owned())
    );
    assert_eq!(row.created_at, sea_orm::Set(at()));
    assert_eq!(row.retry_count, sea_orm::Set(0));
    // The ids stay unset, so the database assigns them.
    assert!(
        matches!(row.id, sea_orm::ActiveValue::NotSet),
        "{:?}",
        row.id
    );
}

#[test]
fn the_pipeline_row_has_no_project_and_no_digest() {
    let report = pipeline_report();
    let row = NewPipelineReport {
        repo_id: 3,
        body: pipeline_body(&report),
        commit_sha: "sha1".to_owned(),
        branch: "main".to_owned(),
        ci_run_id: "1".to_owned(),
        ci_run_attempt: 1,
        pipeline: Some(".github/workflows/ci.yml".to_owned()),
        default_branch: Some("main".to_owned()),
    }
    .into_active_model(at());
    assert_eq!(row.kind, sea_orm::Set("pipeline".to_owned()));
    assert_eq!(row.project_id, sea_orm::Set(None));
    assert_eq!(row.repo_id, sea_orm::Set(3));
    // Only the JUnit duplicate check reads a digest; Pipeline rows never get one.
    assert_eq!(row.body_sha256, sea_orm::Set(None));
    assert_eq!(
        row.status,
        sea_orm::Set(ReportStatus::Pending.as_str().to_owned())
    );
}

#[test]
fn the_pipeline_body_is_the_canonical_json_of_the_report() {
    let report = pipeline_report();
    let body = pipeline_body(&report);
    assert_eq!(
        body,
        serde_json::to_vec(&report).expect("the Report serializes")
    );
    let read_back: PipelineReportIn =
        serde_json::from_slice(&body).expect("the body is the Report");
    assert_eq!(read_back, report);
}

#[tokio::test]
async fn the_duplicate_lookup_returns_the_stored_report() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let stored = seed(&db, with_project(&project, identity())).await;
    let candidate = with_project(&project, identity());

    assert_eq!(
        find_junit_duplicate(db.connection(), &candidate)
            .await
            .expect("the lookup runs")
            .map(|found| found.id),
        Some(stored.id)
    );
    // The lookup reads only: no second row.
    assert_eq!(report_ids(&db).await, vec![stored.id]);
}

#[tokio::test]
async fn the_duplicate_lookup_finds_nothing_on_an_empty_table() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let candidate = with_project(&project, identity());
    assert_eq!(
        find_junit_duplicate(db.connection(), &candidate)
            .await
            .expect("the lookup runs"),
        None
    );
}

#[tokio::test]
async fn the_duplicate_lookup_keys_on_each_identity_column() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    seed(&db, with_project(&project, identity())).await;

    let other_commit = with_project(
        &project,
        NewJunitReport {
            commit_sha: "other-sha".to_owned(),
            ..identity()
        },
    );
    let other_run = with_project(
        &project,
        NewJunitReport {
            ci_run_id: "8".to_owned(),
            ..identity()
        },
    );
    let other_attempt = with_project(
        &project,
        NewJunitReport {
            ci_run_attempt: Some(3),
            ..identity()
        },
    );
    let other_job = with_project(
        &project,
        NewJunitReport {
            ci_job_id: Some("J2".to_owned()),
            ..identity()
        },
    );
    let other_line_base = with_project(
        &project,
        NewJunitReport {
            line_base: Some(0),
            ..identity()
        },
    );
    for candidate in [
        other_commit,
        other_run,
        other_attempt,
        other_job,
        other_line_base,
    ] {
        assert_eq!(
            find_junit_duplicate(db.connection(), &candidate)
                .await
                .expect("the lookup runs"),
            None,
            "{candidate:?}"
        );
    }
}

#[tokio::test]
async fn the_duplicate_lookup_keys_on_the_project() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let other = get_or_create_project(db.connection(), "acme/app", "other", at())
        .await
        .expect("another Project");
    seed(&db, with_project(&project, identity())).await;
    assert_eq!(
        find_junit_duplicate(db.connection(), &with_project(&other, identity()))
            .await
            .expect("the lookup runs"),
        None
    );
}

#[tokio::test]
async fn the_duplicate_lookup_keys_on_the_body_bytes() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    seed(&db, with_project(&project, identity())).await;
    let other_body = with_project(
        &project,
        NewJunitReport {
            body: make_junit(&[("t1", "passed")]),
            ..identity()
        },
    );
    assert_eq!(
        find_junit_duplicate(db.connection(), &other_body)
            .await
            .expect("the lookup runs"),
        None
    );
}

#[tokio::test]
async fn a_stored_null_line_base_reads_as_one() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let stored = seed(
        &db,
        with_project(
            &project,
            NewJunitReport {
                line_base: None,
                ..identity()
            },
        ),
    )
    .await;
    assert_eq!(
        find_junit_duplicate(db.connection(), &with_project(&project, identity()))
            .await
            .expect("the lookup runs")
            .map(|found| found.id),
        Some(stored.id)
    );
    // A corrected re-upload repeats its own line_base, so it is a new Report.
    let zero = with_project(
        &project,
        NewJunitReport {
            line_base: Some(0),
            ..identity()
        },
    );
    assert_eq!(
        find_junit_duplicate(db.connection(), &zero)
            .await
            .expect("the lookup runs"),
        None
    );
}

#[tokio::test]
async fn the_duplicate_lookup_matches_a_null_attribution_with_a_null() {
    // An upload with no ci_job_id or ci_run_attempt is a duplicate of a stored
    // row that also has none: SQLAlchemy rendered `column == None` as IS NULL.
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let anonymous = NewJunitReport {
        ci_job_id: None,
        ci_run_attempt: None,
        ..identity()
    };
    let stored = seed(&db, with_project(&project, anonymous.clone())).await;
    assert_eq!(
        find_junit_duplicate(db.connection(), &with_project(&project, anonymous))
            .await
            .expect("the lookup runs")
            .map(|found| found.id),
        Some(stored.id)
    );
    // A named attribution is not the same upload as an anonymous one.
    assert_eq!(
        find_junit_duplicate(db.connection(), &with_project(&project, identity()))
            .await
            .expect("the lookup runs"),
        None
    );
}

#[tokio::test]
async fn the_duplicate_lookup_returns_the_newest_stored_report() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let older = seed(&db, with_project(&project, identity())).await;
    let newer = seed(&db, with_project(&project, identity())).await;
    assert!(newer.id > older.id);
    assert_eq!(
        find_junit_duplicate(db.connection(), &with_project(&project, identity()))
            .await
            .expect("the lookup runs")
            .map(|found| found.id),
        Some(newer.id)
    );
}

#[tokio::test]
async fn the_seeded_report_is_what_ingest_would_have_queued() {
    // The factory builds through the same identity, so a seeded row carries its
    // digest and the duplicate check matches it.
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let stored = make_report(&project, project.repo_id)
        .body(&body())
        .build(db.connection())
        .await
        .expect("a Report");
    assert_eq!(stored.kind, "junit");
    assert_eq!(report_ids(&db).await, vec![stored.id]);

    let candidate = with_project(&project, identity());
    // The factory row carries no digest, so it is not the same upload until it
    // has one: this is what the ingest route writes.
    assert_eq!(
        find_junit_duplicate(db.connection(), &candidate)
            .await
            .expect("the lookup runs"),
        None
    );
    let with_digest = seed(&db, candidate.clone()).await;
    assert_eq!(
        find_junit_duplicate(db.connection(), &candidate)
            .await
            .expect("the lookup runs")
            .map(|found| found.id),
        Some(with_digest.id)
    );
}

#[tokio::test]
async fn a_pipeline_row_is_never_a_duplicate() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let pipeline = NewPipelineReport {
        repo_id: project.repo_id,
        body: pipeline_body(&pipeline_report()),
        commit_sha: "p".to_owned(),
        branch: "main".to_owned(),
        ci_run_id: "1".to_owned(),
        ci_run_attempt: 2,
        pipeline: Some(".github/workflows/ci.yml".to_owned()),
        default_branch: Some("main".to_owned()),
    }
    .into_active_model(at());
    reports::Entity::insert(pipeline)
        .exec(db.connection())
        .await
        .expect("a Pipeline Report");
    assert_eq!(
        find_junit_duplicate(db.connection(), &with_project(&project, identity()))
            .await
            .expect("the lookup runs"),
        None
    );
}

#[tokio::test]
async fn the_stored_junit_body_reads_a_null_line_base_as_one() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    for (line_base, expected) in [(None, 1), (Some(0), 0), (Some(1), 1)] {
        let stored = seed(
            &db,
            with_project(
                &project,
                NewJunitReport {
                    line_base,
                    ..identity()
                },
            ),
        )
        .await;
        let (stored_bytes, stored_base) = stored_junit_body(&stored);
        assert_eq!(stored_bytes, body().as_slice());
        assert_eq!(stored_base, expected);
    }
}

#[tokio::test]
async fn the_stored_pipeline_report_is_the_inverse_of_the_body() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let report = pipeline_report();
    let stored = make_report(&project, project.repo_id)
        .body(&pipeline_body(&report))
        .kind("pipeline")
        .build(db.connection())
        .await
        .expect("a Pipeline Report");
    assert_eq!(
        stored_pipeline_report(&stored).expect("the stored Report"),
        report
    );
}

#[tokio::test]
async fn a_stored_body_that_is_not_a_report_is_reported() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let stored = make_report(&project, project.repo_id)
        .body(b"not json")
        .kind("pipeline")
        .build(db.connection())
        .await
        .expect("a Pipeline Report");
    let error = stored_pipeline_report(&stored).expect_err("the body is not a Report");
    assert!(matches!(error, DbError::StoredReportBody { .. }), "{error}");
    assert!(
        error
            .to_string()
            .contains("not the Report it was built from"),
        "{error}"
    );
}

#[tokio::test]
async fn completing_a_report_records_its_counts_and_its_run() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let mut stored = seed(&db, with_project(&project, identity())).await;
    complete_report(&mut stored, at(), Some(7), serde_json::json!({"passed": 1}));
    assert_eq!(stored.status, ReportStatus::Processed.as_str());
    assert_eq!(stored.counts, Some(serde_json::json!({"passed": 1})));
    assert_eq!(stored.run_id, Some(7));
    assert_eq!(stored.error, None);
    assert_eq!(stored.processed_at, Some(at()));
    // Field-only: no retry budget spent.
    assert_eq!(stored.retry_count, 0);
}

#[tokio::test]
async fn failing_a_report_records_the_error_and_no_retry() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let mut stored = seed(&db, with_project(&project, identity())).await;
    fail_report(&mut stored, at(), "ValidationError: bad body");
    assert_eq!(stored.status, ReportStatus::Failed.as_str());
    assert_eq!(stored.error.as_deref(), Some("ValidationError: bad body"));
    assert_eq!(stored.processed_at, Some(at()));
    assert_eq!(stored.retry_count, 0);
    assert_eq!(stored.counts, None);
    assert_eq!(stored.run_id, None);
}

#[tokio::test]
async fn requeueing_a_report_spends_one_retry() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let mut stored = seed(&db, with_project(&project, identity())).await;
    stored.error = Some("old".to_owned());
    stored.processed_at = Some(at());
    requeue_report(&mut stored, Some("transient"));
    assert_eq!(stored.status, ReportStatus::Pending.as_str());
    assert_eq!(stored.retry_count, 1);
    assert_eq!(stored.error.as_deref(), Some("transient"));
    assert_eq!(stored.processed_at, None);

    // A requeue with no error clears it, and spends another retry.
    requeue_report(&mut stored, None);
    assert_eq!(stored.retry_count, 2);
    assert_eq!(stored.error, None);
}

#[tokio::test]
async fn a_manual_retry_starts_the_budget_again() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let mut stored = seed(&db, with_project(&project, identity())).await;
    stored.status = ReportStatus::Failed.as_str().to_owned();
    stored.error = Some("boom".to_owned());
    stored.processed_at = Some(at());
    stored.retry_count = 3;
    reset_for_retry(&mut stored);
    assert_eq!(stored.status, ReportStatus::Pending.as_str());
    assert_eq!(stored.retry_count, 0);
    assert_eq!(stored.error, None);
    assert_eq!(stored.processed_at, None);
}

#[tokio::test]
async fn an_absurd_attempt_number_saturates() {
    assert_eq!(stored_ci_run_attempt(2), 2);
    assert_eq!(stored_ci_run_attempt(1), 1);
    assert_eq!(stored_ci_run_attempt(i64::MAX), i32::MAX);
    assert_eq!(stored_ci_run_attempt(i64::from(i32::MAX) + 1), i32::MAX);
}

#[tokio::test]
async fn the_transitions_never_commit_anything_themselves() {
    // The row is loaded, changed in memory and left alone: the caller owns the
    // transaction, so the database still holds the queued values.
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let mut stored = seed(&db, with_project(&project, identity())).await;
    complete_report(&mut stored, at(), Some(7), serde_json::json!({}));
    let reread = reports::Entity::find_by_id(stored.id)
        .one(db.connection())
        .await
        .expect("the Report query")
        .expect("the Report");
    assert_eq!(reread.status, ReportStatus::Pending.as_str());
    assert_eq!(reread.run_id, None);
    assert_eq!(
        reports::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        1
    );
}

#[tokio::test]
async fn a_report_body_round_trips_through_the_binary_column() {
    // The body is a bytea column, so a report with a NUL byte in it comes back
    // unchanged: a text column would truncate it.
    let db = TestDb::new().await.expect("a migrated test database");
    let project = project(&db).await;
    let stored = seed(
        &db,
        with_project(
            &project,
            NewJunitReport {
                body: b"<testsuite>\x00\xff</testsuite>".to_vec(),
                ..identity()
            },
        ),
    )
    .await;
    let (body, line_base) = stored_junit_body(&stored);
    assert_eq!(body, b"<testsuite>\x00\xff</testsuite>");
    assert_eq!(line_base, 1);
    let counted: i64 = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS count FROM reports WHERE body = $1",
            [Value::from(b"<testsuite>\x00\xff</testsuite>".to_vec())],
        ))
        .await
        .expect("the count query runs")
        .expect("the count query returns a row")
        .try_get("", "count")
        .expect("a count");
    assert_eq!(counted, 1);
}
