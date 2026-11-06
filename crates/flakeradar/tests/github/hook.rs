//! The `ReportWorker` hook, which files and then notes for one finished Report.

// Rust guideline compliant 2026-07-21

use flakeradar::github;
use flakeradar_core::{EntityKind, ReportStatus};
use flakeradar_test_support::{TestDb, system_clock};
use serde_json::json;
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use crate::support::{
    api, clean_streak_of, fixed_settings, flaky_test, gh_settings, link_of, link_table,
    mount_comment, mount_create, mount_patch, note_row, noted, number_of, processed, processed_for,
    requested_calls, stage_mid_request, unstopped,
};

#[tokio::test]
async fn a_failed_report_is_never_filed_and_a_processed_one_is() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 7})),
    )
    .await;

    let mut failed = processed(&[test], &[]);
    failed.status = ReportStatus::Failed;
    github::on_report_processed(
        db.connection(),
        &failed,
        &gh_settings(&[]),
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert!(
        server
            .received_requests()
            .await
            .expect("a record")
            .is_empty()
    );

    github::on_report_processed(
        db.connection(),
        &processed(&[test], &[]),
        &gh_settings(&[]),
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(link_of(&db, EntityKind::Test, test).await.number, Some(7));
}

/// The note step skips the link this same pass filed, once per entity.
async fn the_note_step_skips_the_link_this_same_pass_filed(kind: EntityKind) {
    let db = TestDb::new().await.expect("a test database");
    let obj = note_row(&db, kind, "acme/app", "fresh", None, 2).await;
    let table = link_table(kind);
    let server = MockServer::start().await;
    let connection = db.connection().clone();
    Mock::given(any())
        .respond_with(move |request: &Request| {
            if request.url.path() == "/repos/acme/app/issues" {
                stage_mid_request(
                    &connection,
                    format!("UPDATE {table} SET clean_streak = 3 WHERE id = {obj}"),
                );
            }
            ResponseTemplate::new(201).set_body_json(json!({"number": 123}))
        })
        .mount(&server)
        .await;
    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);
    let outcome = processed_for(kind, &[obj]);

    github::on_report_processed(
        db.connection(),
        &outcome,
        &settings,
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_calls(&server).await,
        vec![("POST".to_owned(), "/repos/acme/app/issues".to_owned())],
        "the link filed in this pass is never commented on or closed"
    );
    assert_eq!(
        number_of(&db, table, obj).await,
        Some(123),
        "the filing itself stands"
    );
    assert!(
        !noted(&db, table, obj).await,
        "the fresh issue is not noted"
    );
    assert_eq!(
        clean_streak_of(&db, table, obj).await,
        3,
        "and it is noteable from the next Report on"
    );
}

#[tokio::test]
async fn the_note_step_skips_the_link_this_same_pass_filed_for_a_test() {
    the_note_step_skips_the_link_this_same_pass_filed(EntityKind::Test).await;
}

#[tokio::test]
async fn the_note_step_skips_the_link_this_same_pass_filed_for_a_job() {
    the_note_step_skips_the_link_this_same_pass_filed(EntityKind::Job).await;
}

/// A processed Report never notes the issue it filed, once per entity.
async fn a_processed_report_never_notes_the_issue_it_filed(kind: EntityKind) {
    let db = TestDb::new().await.expect("a test database");
    let live = note_row(&db, kind, "acme/app", "live", None, 0).await;
    let fresh = note_row(&db, kind, "acme/app", "fresh", None, 3).await;
    let linked = note_row(&db, kind, "acme/app", "linked", Some(88), 3).await;
    let table = link_table(kind);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 99})))
        .mount(&server)
        .await;
    mount_comment(
        &server,
        "acme/app",
        88,
        ResponseTemplate::new(201).set_body_json(json!({})),
    )
    .await;
    mount_patch(&server, "acme/app", 88, ResponseTemplate::new(200)).await;
    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);
    let outcome = processed_for(kind, &[live, fresh, linked]);

    github::on_report_processed(
        db.connection(),
        &outcome,
        &settings,
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_calls(&server).await,
        vec![
            ("POST".to_owned(), "/repos/acme/app/issues".to_owned()),
            (
                "POST".to_owned(),
                "/repos/acme/app/issues/88/comments".to_owned()
            ),
            ("PATCH".to_owned(), "/repos/acme/app/issues/88".to_owned())
        ],
        "only the old link is touched: the Issue filed this pass gets no comment and no close"
    );
    assert_eq!(
        number_of(&db, table, fresh).await,
        None,
        "a row at the note streak has no live flake to file"
    );
    assert_eq!(
        number_of(&db, table, live).await,
        Some(99),
        "the row that only met the filing gate is filed"
    );
    assert!(!noted(&db, table, live).await, "and never noted");

    github::on_report_processed(
        db.connection(),
        &outcome,
        &settings,
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_calls(&server).await.len(),
        3,
        "the next Report files no Issue and comments on none: that is the steady state"
    );
}

#[tokio::test]
async fn a_processed_report_never_notes_the_issue_it_filed_for_a_test() {
    a_processed_report_never_notes_the_issue_it_filed(EntityKind::Test).await;
}

#[tokio::test]
async fn a_processed_report_never_notes_the_issue_it_filed_for_a_job() {
    a_processed_report_never_notes_the_issue_it_filed(EntityKind::Job).await;
}
