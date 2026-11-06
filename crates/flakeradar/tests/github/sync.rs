//! The issue-state sync's rotation, its stop rules and the post-delete close.

// Rust guideline compliant 2026-07-21

use flakeradar::Settings;
use flakeradar::github::{self, Api};
use flakeradar::settings::Secret;
use flakeradar_core::EntityKind;
use flakeradar_db::issue_links;
use flakeradar_test_support::TestDb;
use sea_orm::ConnectionTrait as _;
use sea_orm::{DbBackend, Statement, Value};
use serde_json::{Value as Json, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{
    CREATED, Held, Loopback, TOKEN, TRUNCATED, api, flaky_job, flaky_test, flaky_test_in,
    gh_settings, job_checked, job_link, lines_with, logged_from, mount_close, mount_create,
    mount_issue, mount_state, noted, requested_paths, stamp_job_link, stamp_link, stamp_note,
    test_link, unstopped,
};

#[tokio::test]
async fn the_sync_rearms_a_link_whose_issue_closed() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let still_open = flaky_test_in(&db, "acme/keep", "backend", "open").await;
    stamp_link(&db, test, 12, true, false).await;
    stamp_link(&db, still_open, 22, true, false).await;
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(json!({"state": "closed", "state_reason": "completed"})),
    )
    .await;
    mount_issue(
        &server,
        "acme/keep",
        22,
        ResponseTemplate::new(200).set_body_json(json!({"state": "open"})),
    )
    .await;

    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    let (number, filed, _, closed) = test_link(&db, test).await;
    assert_eq!(
        number, None,
        "the number goes, so the next Report may re-file"
    );
    assert_eq!(filed, None);
    assert!(!closed);
    assert_eq!(
        test_link(&db, still_open).await.0,
        Some(22),
        "an open issue keeps its link"
    );
}

#[tokio::test]
async fn the_sync_keeps_a_link_closed_as_not_planned() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, true, false).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_filed = TRUE WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the filed flag writes");
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(json!({"state": "closed", "state_reason": "not_planned"})),
    )
    .await;

    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    let (number, filed, checked, closed) = test_link(&db, test).await;
    assert_eq!(
        number,
        Some(12),
        "the team declined this flake; nothing is filed again"
    );
    assert_eq!(filed, Some(true), "the link is still ours");
    assert!(checked, "and the link stays in the sync rotation");
    assert!(closed, "and the row records why");
}

#[tokio::test]
async fn the_sync_clears_the_flag_when_a_declined_issue_reopens() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, true, true).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_filed = TRUE WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the filed flag writes");
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(json!({"state": "open", "state_reason": "reopened"})),
    )
    .await;

    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    let (number, filed, _, closed) = test_link(&db, test).await;
    assert_eq!(number, Some(12));
    assert_eq!(filed, Some(true), "the link is still ours");
    assert!(!closed, "filing is live again");
}

#[tokio::test]
async fn the_sync_rearms_a_link_whose_issue_is_gone() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, true, false).await;
    let server = MockServer::start().await;
    mount_state(&server, ResponseTemplate::new(404)).await;

    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    let (number, filed, checked, closed) = test_link(&db, test).await;
    assert_eq!(number, None);
    assert_eq!(filed, None, "the flag resets with the number");
    assert!(checked, "the row rotates like a checked one");
    assert!(!closed, "a row with no number keeps no stale flag");
}

#[tokio::test]
async fn the_sync_keeps_a_link_on_a_permission_error_and_stamps_it() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let other = flaky_test_in(&db, "acme/b", "backend", "closed").await;
    stamp_link(&db, test, 12, false, false).await;
    stamp_link(&db, other, 22, false, false).await;
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(403).set_body_json(json!({"message": "Resource not accessible"})),
    )
    .await;
    mount_issue(
        &server,
        "acme/b",
        22,
        ResponseTemplate::new(200).set_body_json(json!({"state": "closed"})),
    )
    .await;

    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    let (number, _, checked, _) = test_link(&db, test).await;
    assert_eq!(
        number,
        Some(12),
        "a permission error says nothing about the issue"
    );
    assert!(checked, "but the row was answered for, so it rotates");
    assert_eq!(
        test_link(&db, other).await.0,
        None,
        "the other Repo still syncs"
    );
}

#[tokio::test]
async fn the_sync_leaves_an_unreadable_body_alone_and_stamps_it() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test_in(&db, "acme/a", "backend", "a").await;
    let other = flaky_test_in(&db, "acme/b", "backend", "b").await;
    for raw in [
        "",
        "<html>Gateway</html>",
        "[1, 2]",
        "null",
        r#"{"state": ["closed"]}"#,
    ] {
        stamp_link(&db, test, 11, false, false).await;
        stamp_link(&db, other, 22, false, false).await;
        let server = MockServer::start().await;
        mount_issue(
            &server,
            "acme/a",
            11,
            ResponseTemplate::new(200).set_body_string(raw),
        )
        .await;
        mount_issue(
            &server,
            "acme/b",
            22,
            ResponseTemplate::new(200).set_body_json(json!({"state": "closed"})),
        )
        .await;

        let logged = logged_from(github::sync_closed_issues(
            db.connection(),
            &gh_settings(&[]),
            &api(&server),
            &unstopped(),
        ))
        .await;
        assert_eq!(
            requested_paths(&server).await,
            vec!["/repos/acme/a/issues/11", "/repos/acme/b/issues/22"],
            "raw {raw:?}: the batch goes on"
        );
        let (number, _, checked, _) = test_link(&db, test).await;
        assert_eq!(number, Some(11), "raw {raw:?}: the link survives");
        assert!(checked, "raw {raw:?}: the row rotates");
        assert_eq!(
            test_link(&db, other).await.0,
            None,
            "raw {raw:?}: the other Repo still re-arms"
        );
        let warnings = lines_with(&logged, "unusable");
        assert_eq!(warnings.len(), 1, "raw {raw:?}: {logged}");
        assert!(
            warnings[0].contains("repo=acme/a") && warnings[0].contains("number=11"),
            "raw {raw:?}: {logged}"
        );
    }
}

#[tokio::test]
async fn the_sync_stops_on_a_rate_limit_before_stamping_the_row() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, false, false).await;
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(429).insert_header("retry-after", "60"),
    )
    .await;

    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    let (number, _, checked, _) = test_link(&db, test).await;
    assert_eq!(number, Some(12));
    assert!(!checked, "a rate-limited row is retried first next run");
}

#[tokio::test]
async fn nothing_configured_syncs_nothing() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, false, false).await;
    let server = MockServer::start().await;
    let settings = Settings::from_map(&std::collections::BTreeMap::new()).expect("the defaults");

    github::sync_closed_issues(
        db.connection(),
        &settings,
        &Api::new(Secret::new(""), &server.uri()).expect("the HTTP client builds"),
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
}

#[tokio::test]
async fn a_sync_whose_rotation_list_cannot_be_read_makes_no_request() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, false, false).await;
    db.connection()
        .execute_unprepared("DROP TABLE test_cases CASCADE")
        .await
        .expect("the table is dropped");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;

    assert!(
        server
            .received_requests()
            .await
            .expect("a record")
            .is_empty(),
        "a rotation list it could not read sends nothing"
    );
}

#[tokio::test]
async fn the_sync_leaves_an_unknown_issue_state_alone() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_number = 12, github_issue_filed = TRUE, \
             github_issue_closed = TRUE WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the flagged link writes");
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(json!({"state": "archived", "state_reason": "not_planned"})),
    )
    .await;

    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    let (number, filed, checked, closed) = test_link(&db, test).await;
    assert_eq!(number, Some(12), "an unknown state says nothing");
    assert_eq!(filed, Some(true));
    assert!(closed, "only a reported reopen clears the flag");
    assert!(checked, "and the row still rotates");
}

#[tokio::test]
async fn the_sync_rotates_through_its_links_a_batch_at_a_time() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/app", "p1", "t1").await;
    let second = flaky_test_in(&db, "acme/app", "p2", "t2").await;
    let third = flaky_test_in(&db, "acme/app", "p3", "t3").await;
    stamp_link(&db, first, 11, false, false).await;
    stamp_link(&db, second, 22, false, false).await;
    stamp_link(&db, third, 33, false, false).await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "open"})))
        .mount(&server)
        .await;
    let settings = gh_settings(&[("FLAKERADAR_GITHUB_SYNC_BATCH", "2")]);

    github::sync_closed_issues(db.connection(), &settings, &api(&server), &unstopped()).await;
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/app/issues/11", "/repos/acme/app/issues/22"],
        "every stamp is NULL, so the two lowest ids go first"
    );
    assert_eq!(test_link(&db, first).await.0, Some(11));
    assert!(
        test_link(&db, first).await.2,
        "an open issue is still a check"
    );

    github::sync_closed_issues(db.connection(), &settings, &api(&server), &unstopped()).await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues/11",
            "/repos/acme/app/issues/22",
            "/repos/acme/app/issues/33",
            "/repos/acme/app/issues/11",
        ],
        "the never-checked Test reaches the batch next to the oldest stamped one"
    );
}

#[tokio::test]
async fn a_never_checked_link_goes_before_a_stamped_one() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/app", "p1", "t1").await;
    let second = flaky_test_in(&db, "acme/app", "p2", "t2").await;
    stamp_link(&db, first, 11, false, false).await;
    stamp_link(&db, second, 22, false, false).await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "open"})))
        .mount(&server)
        .await;

    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&server).await.len(),
        2,
        "both carry a stamp now"
    );

    let fresh = flaky_test_in(&db, "acme/app", "p3", "t3").await;
    stamp_link(&db, fresh, 33, false, false).await;
    let settings = gh_settings(&[("FLAKERADAR_GITHUB_SYNC_BATCH", "1")]);
    github::sync_closed_issues(db.connection(), &settings, &api(&server), &unstopped()).await;
    assert_eq!(
        requested_paths(&server).await[2..],
        ["/repos/acme/app/issues/33"],
        "the never-checked link goes first and is stamped"
    );
    assert!(test_link(&db, fresh).await.2);
}

#[tokio::test]
async fn a_sync_stops_at_a_transport_error_with_one_request() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/a", "p1", "a").await;
    let second = flaky_test_in(&db, "acme/b", "p2", "b").await;
    stamp_link(&db, first, 1, false, false).await;
    stamp_link(&db, second, 2, false, false).await;
    // Every connection is accepted and dropped without an answer at all.
    let loopback = Loopback::dropping().await;

    let report = github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &Api::new(Secret::new(TOKEN), &loopback.base).expect("the HTTP client builds"),
        &unstopped(),
    )
    .await;

    assert_eq!(
        report.stopped,
        Some(github::BatchStop::Unreachable),
        "the batch reports why it ended"
    );
    assert_eq!(
        loopback.accepted(),
        1,
        "an unreachable GitHub ends the sync at the first call, not after every row"
    );
    assert!(
        !test_link(&db, first).await.2,
        "a row GitHub never answered for is not stamped"
    );
    assert!(
        !test_link(&db, second).await.2,
        "the batch never reached the second row"
    );
}

#[tokio::test]
async fn a_sync_rate_limit_stops_the_batch_and_keeps_the_stamp_it_made() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/a", "p1", "a").await;
    let second = flaky_test_in(&db, "acme/b", "p2", "b").await;
    stamp_link(&db, first, 1, false, false).await;
    stamp_link(&db, second, 2, false, false).await;

    let server = MockServer::start().await;
    mount_issue(
        &server,
        "acme/a",
        1,
        ResponseTemplate::new(200).set_body_json(json!({"state": "open"})),
    )
    .await;
    mount_issue(
        &server,
        "acme/b",
        2,
        ResponseTemplate::new(403).insert_header("x-ratelimit-remaining", "0"),
    )
    .await;
    let report = github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(
        report.stopped,
        Some(github::BatchStop::RateLimited),
        "the batch reports why it ended"
    );
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/a/issues/1", "/repos/acme/b/issues/2"],
        "the limit is met on the second request and stops the batch"
    );
    assert!(
        test_link(&db, first).await.2,
        "the checked stamp is written"
    );
    assert!(
        !test_link(&db, second).await.2,
        "the rate-limited row is never stamped"
    );
}

#[tokio::test]
async fn a_sync_continues_from_the_stamp_a_stopped_run_left() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/a", "p1", "a").await;
    let second = flaky_test_in(&db, "acme/b", "p2", "b").await;
    stamp_link(&db, first, 1, false, false).await;
    stamp_link(&db, second, 2, false, false).await;
    // The first run stops on a rate limit at the second row, leaving the first
    // stamped and the second not.
    let server = MockServer::start().await;
    mount_issue(
        &server,
        "acme/a",
        1,
        ResponseTemplate::new(200).set_body_json(json!({"state": "open"})),
    )
    .await;
    mount_issue(
        &server,
        "acme/b",
        2,
        ResponseTemplate::new(403).insert_header("x-ratelimit-remaining", "0"),
    )
    .await;
    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;

    // The retried row still sorts first, because nothing stamped it.
    let second_server = MockServer::start().await;
    mount_issue(
        &second_server,
        "acme/a",
        1,
        ResponseTemplate::new(200).set_body_json(json!({"state": "open"})),
    )
    .await;
    mount_issue(
        &second_server,
        "acme/b",
        2,
        ResponseTemplate::new(200).set_body_json(json!({"state": "open"})),
    )
    .await;
    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&second_server),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&second_server).await,
        vec!["/repos/acme/b/issues/2", "/repos/acme/a/issues/1"],
        "the unstamped row goes first, and the stamped one follows"
    );
}

#[tokio::test]
async fn a_permission_403_skips_the_later_rows_of_that_repo() {
    let db = TestDb::new().await.expect("a test database");
    let a1 = flaky_test_in(&db, "acme/a", "p1", "a1").await;
    let a2 = flaky_test_in(&db, "acme/a", "p2", "a2").await;
    let b = flaky_test_in(&db, "acme/b", "p3", "b").await;
    stamp_link(&db, a1, 11, false, false).await;
    stamp_link(&db, a2, 12, false, false).await;
    stamp_link(&db, b, 22, false, false).await;
    let server = MockServer::start().await;
    mount_issue(
        &server,
        "acme/a",
        11,
        ResponseTemplate::new(403)
            .set_body_json(json!({"message": "Resource not accessible by personal access token"})),
    )
    .await;
    mount_issue(&server, "acme/a", 12, ResponseTemplate::new(500)).await;
    mount_issue(
        &server,
        "acme/b",
        22,
        ResponseTemplate::new(200).set_body_json(json!({"state": "closed"})),
    )
    .await;

    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    let mut paths = requested_paths(&server).await;
    paths.sort();
    assert_eq!(
        paths,
        vec!["/repos/acme/a/issues/11", "/repos/acme/b/issues/22"],
        "the denied Repo is skipped for the rest of the sync"
    );
    let (a1_number, _, a1_checked, _) = test_link(&db, a1).await;
    assert_eq!(a1_number, Some(11), "a permission 403 clears nothing");
    assert!(a1_checked, "the denied row rotates like a checked one");
    let (a2_number, _, a2_checked, _) = test_link(&db, a2).await;
    assert_eq!(a2_number, Some(12));
    assert!(a2_checked, "the skipped row rotates too");
    assert_eq!(
        test_link(&db, b).await.0,
        None,
        "the other Repo still syncs"
    );
}

#[tokio::test]
async fn the_sync_rotates_past_a_denied_repo() {
    let db = TestDb::new().await.expect("a test database");
    let a1 = flaky_test_in(&db, "acme/a", "p1", "a1").await;
    let a2 = flaky_test_in(&db, "acme/a", "p2", "a2").await;
    let b = flaky_test_in(&db, "acme/b", "p3", "b").await;
    stamp_link(&db, a1, 11, false, false).await;
    stamp_link(&db, a2, 12, false, false).await;
    stamp_link(&db, b, 22, false, false).await;
    let server = MockServer::start().await;
    mount_issue(
        &server,
        "acme/a",
        11,
        ResponseTemplate::new(403)
            .set_body_json(json!({"message": "Resource not accessible by personal access token"})),
    )
    .await;
    mount_issue(&server, "acme/a", 12, ResponseTemplate::new(500)).await;
    mount_issue(
        &server,
        "acme/b",
        22,
        ResponseTemplate::new(200).set_body_json(json!({"state": "closed"})),
    )
    .await;
    let settings = gh_settings(&[("FLAKERADAR_GITHUB_SYNC_BATCH", "2")]);

    github::sync_closed_issues(db.connection(), &settings, &api(&server), &unstopped()).await;
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/a/issues/11"],
        "the cap holds the two denied rows and the second gets no GET"
    );

    github::sync_closed_issues(db.connection(), &settings, &api(&server), &unstopped()).await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/a/issues/11",
            "/repos/acme/b/issues/22",
            "/repos/acme/a/issues/11"
        ],
        "the denied rows were stamped, so the other Repo's link sorts first"
    );
    assert_eq!(test_link(&db, a1).await.0, Some(11));
    assert_eq!(test_link(&db, a2).await.0, Some(12));
    assert_eq!(test_link(&db, b).await.0, None, "rotation reached repo B");
    assert!(test_link(&db, b).await.2);
}

#[tokio::test]
async fn a_sync_definitive_client_error_keeps_the_link_and_skips_the_repo() {
    for status in [401, 410] {
        let db = TestDb::new().await.expect("a test database");
        let a1 = flaky_test_in(&db, "acme/a", "p1", "a1").await;
        let a2 = flaky_test_in(&db, "acme/a", "p2", "a2").await;
        let b = flaky_test_in(&db, "acme/b", "p3", "closed").await;
        stamp_link(&db, a1, 11, false, false).await;
        stamp_link(&db, a2, 12, false, false).await;
        stamp_link(&db, b, 22, false, false).await;
        let server = MockServer::start().await;
        mount_issue(
            &server,
            "acme/a",
            11,
            ResponseTemplate::new(status).set_body_json(json!({"message": "no"})),
        )
        .await;
        mount_issue(&server, "acme/a", 12, ResponseTemplate::new(200)).await;
        mount_issue(
            &server,
            "acme/b",
            22,
            ResponseTemplate::new(200).set_body_json(json!({"state": "closed"})),
        )
        .await;

        github::sync_closed_issues(
            db.connection(),
            &gh_settings(&[]),
            &api(&server),
            &unstopped(),
        )
        .await;
        assert_eq!(
            requested_paths(&server).await,
            vec!["/repos/acme/a/issues/11", "/repos/acme/b/issues/22"],
            "status {status}: the Repo is skipped, the other Repo still syncs"
        );
        assert_eq!(test_link(&db, a1).await.0, Some(11), "status {status}");
        assert!(
            test_link(&db, a1).await.2,
            "status {status}: the row rotates"
        );
        assert_eq!(test_link(&db, a2).await.0, Some(12));
        assert!(test_link(&db, a2).await.2, "status {status}");
        assert_eq!(test_link(&db, b).await.0, None, "status {status}");
    }
}

#[tokio::test]
async fn the_sync_rotates_past_an_unreadable_link() {
    let db = TestDb::new().await.expect("a test database");
    let bad = flaky_test_in(&db, "acme/a", "p1", "a").await;
    let second = flaky_test_in(&db, "acme/b", "p2", "b").await;
    let third = flaky_test_in(&db, "acme/c", "p3", "c").await;
    stamp_link(&db, bad, 11, false, false).await;
    stamp_link(&db, second, 22, false, false).await;
    stamp_link(&db, third, 33, false, false).await;
    let server = MockServer::start().await;
    mount_issue(
        &server,
        "acme/a",
        11,
        ResponseTemplate::new(200).set_body_string("<html>Gateway</html>"),
    )
    .await;
    mount_issue(
        &server,
        "acme/b",
        22,
        ResponseTemplate::new(200).set_body_json(json!({"state": "open"})),
    )
    .await;
    mount_issue(
        &server,
        "acme/c",
        33,
        ResponseTemplate::new(200).set_body_json(json!({"state": "open"})),
    )
    .await;
    let settings = gh_settings(&[("FLAKERADAR_GITHUB_SYNC_BATCH", "2")]);

    github::sync_closed_issues(db.connection(), &settings, &api(&server), &unstopped()).await;
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/a/issues/11", "/repos/acme/b/issues/22"]
    );
    assert_eq!(test_link(&db, bad).await.0, Some(11));
    assert!(test_link(&db, bad).await.2, "the bad row rotates");

    github::sync_closed_issues(db.connection(), &settings, &api(&server), &unstopped()).await;
    assert_eq!(
        requested_paths(&server).await[2..],
        ["/repos/acme/c/issues/33", "/repos/acme/a/issues/11"],
        "the never-checked Test is reached instead of queueing behind the bad link"
    );
    assert!(test_link(&db, third).await.2);
}

#[tokio::test]
async fn a_gone_issue_rearms_its_link_but_any_other_answer_keeps_it() {
    for entity in ["test", "job"] {
        let db = TestDb::new().await.expect("a test database");
        let (id, expected) = if entity == "test" {
            let id = flaky_test(&db).await;
            stamp_link(&db, id, 11, false, false).await;
            (id, 11)
        } else {
            let id = flaky_job(&db).await;
            stamp_job_link(&db, id, 5, false).await;
            (id, 5)
        };
        let route = format!("/repos/acme/app/issues/{expected}");

        let gone = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route.as_str()))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message": "Gone"})))
            .mount(&gone)
            .await;
        let logged = logged_from(github::sync_closed_issues(
            db.connection(),
            &gh_settings(&[]),
            &api(&gone),
            &unstopped(),
        ))
        .await;
        assert_eq!(requested_paths(&gone).await, vec![route.clone()]);
        let only = lines_with(&logged, "issue gone");
        assert_eq!(only.len(), 1, "{entity}: {logged}");
        assert!(only[0].contains("repo=acme/app"), "{logged}");
        assert!(only[0].contains(&format!("number={expected}")), "{logged}");
        assert!(only[0].contains(entity), "{logged}");
        if entity == "test" {
            let (number, _, checked, closed) = test_link(&db, id).await;
            assert_eq!(number, None, "a gone issue re-arms");
            assert!(checked);
            assert!(!closed);
        } else {
            assert_eq!(job_link(&db, id).await.0, None);
            assert!(job_checked(&db, id).await);
        }

        if entity == "test" {
            stamp_link(&db, id, 11, false, false).await;
        } else {
            stamp_job_link(&db, id, 5, false).await;
        }
        let broken = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route.as_str()))
            .respond_with(
                ResponseTemplate::new(500).set_body_json(json!({"message": "Server error"})),
            )
            .mount(&broken)
            .await;
        let logged = logged_from(github::sync_closed_issues(
            db.connection(),
            &gh_settings(&[]),
            &api(&broken),
            &unstopped(),
        ))
        .await;
        assert_eq!(requested_paths(&broken).await, vec![route]);
        assert!(
            lines_with(&logged, "acme/app").is_empty(),
            "{entity}: an unexpected status is nobody's fault: {logged}"
        );
        if entity == "test" {
            assert_eq!(test_link(&db, id).await.0, Some(expected));
            assert!(test_link(&db, id).await.2);
        } else {
            assert_eq!(job_link(&db, id).await.0, Some(expected));
            assert!(job_checked(&db, id).await);
        }
    }
}

#[tokio::test]
async fn the_job_sync_rearms_a_closed_issue_and_the_filer_files_again() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job(&db).await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;
    github::file_job_issues_for(
        db.connection(),
        &[job],
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(job_link(&db, job).await.0, Some(77));

    let sync_server = MockServer::start().await;
    mount_issue(
        &sync_server,
        "acme/app",
        77,
        ResponseTemplate::new(200).set_body_json(json!({"state": "closed"})),
    )
    .await;
    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&sync_server),
        &unstopped(),
    )
    .await;
    assert_eq!(job_link(&db, job).await.0, None);

    let refile = MockServer::start().await;
    mount_create(
        &refile,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;
    assert_eq!(
        github::file_job_issues_for(
            db.connection(),
            &[job],
            &gh_settings(&[]),
            &api(&refile),
            &unstopped()
        )
        .await
        .linked
        .len(),
        1,
        "the re-armed Job is filed again"
    );
    assert_eq!(job_link(&db, job).await.0, Some(77));
    assert!(!job_checked(&db, job).await, "a new link is checked first");
}

#[tokio::test]
async fn deleting_a_repo_comments_then_closes_each_issue() {
    let server = MockServer::start().await;
    for number in [123, 456] {
        mount_close(&server, number, 201, false).await;
        mount_close(&server, number, 200, true).await;
    }

    github::close_issues_after_removal("acme/app", &[123, 456], &api(&server)).await;
    let requests = server.received_requests().await.expect("a record");
    let calls: Vec<(String, String)> = requests
        .iter()
        .map(|request| {
            (
                request.method.as_str().to_owned(),
                request.url.path().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        calls,
        vec![
            (
                "POST".to_owned(),
                "/repos/acme/app/issues/123/comments".to_owned()
            ),
            ("PATCH".to_owned(), "/repos/acme/app/issues/123".to_owned()),
            (
                "POST".to_owned(),
                "/repos/acme/app/issues/456/comments".to_owned()
            ),
            ("PATCH".to_owned(), "/repos/acme/app/issues/456".to_owned()),
        ],
        "each issue is commented on, then closed"
    );
    let comment: Json = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    let text = comment["body"].as_str().expect("a body");
    assert!(text.contains("admin") && text.contains("delet"), "{text}");
    let close: Json = serde_json::from_slice(&requests[1].body).expect("the body is JSON");
    assert_eq!(close, json!({"state": "closed"}));

    // No numbers and no token are both silent.
    let empty = MockServer::start().await;
    github::close_issues_after_removal("acme/app", &[], &api(&empty)).await;
    github::close_issues_after_removal(
        "acme/app",
        &[123],
        &Api::new(Secret::new(""), &empty.uri()).expect("the HTTP client builds"),
    )
    .await;
    assert!(
        empty
            .received_requests()
            .await
            .expect("a record")
            .is_empty()
    );

    // A gone issue is not closed: the comment's 404 ends that issue.
    let gone = MockServer::start().await;
    mount_close(&gone, 123, 404, false).await;
    mount_close(&gone, 456, 201, false).await;
    mount_close(&gone, 456, 200, true).await;
    github::close_issues_after_removal("acme/app", &[123, 456], &api(&gone)).await;
    assert_eq!(
        requested_paths(&gone).await,
        vec![
            "/repos/acme/app/issues/123/comments",
            "/repos/acme/app/issues/456/comments",
            "/repos/acme/app/issues/456",
        ],
        "the gone issue gets no close PATCH"
    );
}

#[tokio::test]
async fn a_failed_post_delete_request_costs_only_that_issue() {
    // A 500 on the comment.
    let failed_comment = MockServer::start().await;
    mount_close(&failed_comment, 123, 500, false).await;
    mount_close(&failed_comment, 456, 201, false).await;
    mount_close(&failed_comment, 456, 200, true).await;
    github::close_issues_after_removal("acme/app", &[123, 456], &api(&failed_comment)).await;
    assert_eq!(
        requested_paths(&failed_comment).await,
        vec![
            "/repos/acme/app/issues/123/comments",
            "/repos/acme/app/issues/456/comments",
            "/repos/acme/app/issues/456",
        ]
    );

    // An issue deleted between the comment and the close PATCH is skipped, with a log line.
    let gone_close = MockServer::start().await;
    mount_close(&gone_close, 123, 201, false).await;
    mount_close(&gone_close, 123, 404, true).await;
    mount_close(&gone_close, 456, 201, false).await;
    mount_close(&gone_close, 456, 200, true).await;
    let logged = logged_from(github::close_issues_after_removal(
        "acme/app",
        &[123, 456],
        &api(&gone_close),
    ))
    .await;
    assert_eq!(
        requested_paths(&gone_close).await,
        vec![
            "/repos/acme/app/issues/123/comments",
            "/repos/acme/app/issues/123",
            "/repos/acme/app/issues/456/comments",
            "/repos/acme/app/issues/456",
        ]
    );
    assert!(
        !lines_with(&logged, "already gone after an admin delete").is_empty(),
        "{logged}"
    );

    // A 5xx on the close PATCH is a warning, and the batch goes on.
    let failed_close = MockServer::start().await;
    mount_close(&failed_close, 123, 201, false).await;
    mount_close(&failed_close, 123, 500, true).await;
    mount_close(&failed_close, 456, 201, false).await;
    mount_close(&failed_close, 456, 200, true).await;
    let logged = logged_from(github::close_issues_after_removal(
        "acme/app",
        &[123, 456],
        &api(&failed_close),
    ))
    .await;
    assert_eq!(
        requested_paths(&failed_close).await,
        vec![
            "/repos/acme/app/issues/123/comments",
            "/repos/acme/app/issues/123",
            "/repos/acme/app/issues/456/comments",
            "/repos/acme/app/issues/456",
        ]
    );
    assert_eq!(
        lines_with(&logged, "close after an admin delete failed").len(),
        1,
        "{logged}"
    );
}

#[tokio::test]
async fn the_post_delete_close_never_raises_when_github_is_unreachable() {
    // Nothing listens on this port.
    let api = Api::new(Secret::new(TOKEN), "http://127.0.0.1:1").expect("the HTTP client builds");
    let logged = logged_from(github::close_issues_after_removal("acme/app", &[123], &api)).await;
    assert_eq!(
        lines_with(&logged, "close skipped for one issue").len(),
        1,
        "{logged}"
    );
}

#[tokio::test]
async fn a_rate_limit_on_either_post_delete_request_stops_the_batch() {
    let comment_limit = MockServer::start().await;
    mount_close(&comment_limit, 123, 429, false).await;
    mount_close(&comment_limit, 456, 201, false).await;
    github::close_issues_after_removal("acme/app", &[123, 456], &api(&comment_limit)).await;
    assert_eq!(
        requested_paths(&comment_limit).await,
        vec!["/repos/acme/app/issues/123/comments"],
        "a throttled comment stops the batch"
    );

    let close_limit = MockServer::start().await;
    mount_close(&close_limit, 123, 201, false).await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/123"))
        .respond_with(ResponseTemplate::new(429).insert_header("x-ratelimit-remaining", "0"))
        .mount(&close_limit)
        .await;
    mount_close(&close_limit, 456, 201, false).await;
    let logged = logged_from(github::close_issues_after_removal(
        "acme/app",
        &[123, 456],
        &api(&close_limit),
    ))
    .await;
    assert_eq!(
        requested_paths(&close_limit).await,
        vec![
            "/repos/acme/app/issues/123/comments",
            "/repos/acme/app/issues/123",
        ],
        "a throttled close PATCH stops the batch"
    );
    assert!(
        !lines_with(&logged, "stopping the post-delete close batch").is_empty(),
        "{logged}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_link_replaced_during_the_sync_get_survives() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test_in(&db, "acme/app", "linked", "linked").await;
    stamp_link(&db, test, 11, true, false).await;
    let (arrived, arrived_rx) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/11"))
        .respond_with(Held::new(arrived, released, json!({"state": "closed"})))
        .mount(&server)
        .await;

    // While the sync's GET is in flight, someone re-links the Test to a different
    // issue. The answer to the old number must not touch the new link.
    let relink = async {
        arrived_rx.await.expect("the sync's GET arrives");
        issue_links::store_filed_link(db.connection(), EntityKind::Test, i64::from(test), 99)
            .await
            .expect("the link is replaced");
        release.send(()).expect("the GET is released");
    };
    let settings = gh_settings(&[]);
    let api = api(&server);
    let stop = unstopped();
    tokio::join!(
        github::sync_closed_issues(db.connection(), &settings, &api, &stop),
        relink
    );

    let (number, _, checked, _) = test_link(&db, test).await;
    assert_eq!(
        number,
        Some(99),
        "the link replaced during the GET stands, whatever the old answer said"
    );
    assert!(
        !checked,
        "the old number's answer does not stamp the new link as checked"
    );
}

#[tokio::test]
async fn a_body_that_dies_after_the_status_line_is_a_transport_error() {
    // The status line and headers arrive, then the connection dies mid-body, so
    // the answer is unusable: the step must fail rather than read a short text
    // and report what it guessed from it.
    let loopback = Loopback::answering(TRUNCATED.to_owned()).await;
    let api = Api::new(Secret::new(TOKEN), &loopback.base).expect("the HTTP client builds");

    let logged = logged_from(github::close_issues_after_removal("acme/app", &[123], &api)).await;

    assert_eq!(
        loopback.accepted(),
        1,
        "a dead body costs the issue before a close is attempted"
    );
    let skipped = lines_with(&logged, "post-delete close skipped for one issue");
    assert_eq!(skipped.len(), 1, "{logged}");
    assert!(
        skipped
            .first()
            .is_some_and(|line| line.contains("GitHub request failed")),
        "the mapped failure is the transport error: {logged}"
    );
    assert!(
        lines_with(&logged, "comment after an admin delete failed").is_empty()
            && lines_with(&logged, "closed an issue after an admin delete").is_empty(),
        "neither a status failure nor a success is reported: {logged}"
    );
}

#[tokio::test]
async fn a_post_delete_close_whose_connection_dies_costs_that_issue() {
    // The comment lands, then the close's connection is dropped before it
    // answers: that one issue is lost and the batch goes on.
    let loopback = Loopback::answering(CREATED.to_owned()).await;
    let api = Api::new(Secret::new(TOKEN), &loopback.base).expect("the HTTP client builds");

    let logged = logged_from(github::close_issues_after_removal("acme/app", &[123], &api)).await;

    assert_eq!(
        loopback.accepted(),
        2,
        "the comment landed and the close was attempted"
    );
    assert_eq!(
        lines_with(&logged, "post-delete close skipped for one issue").len(),
        1,
        "{logged}"
    );
    assert!(
        lines_with(&logged, "closed an issue after an admin delete").is_empty(),
        "a close that died is never reported as done: {logged}"
    );
}

#[tokio::test]
async fn a_test_sync_rearms_its_closed_issue_and_the_filer_files_again() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 300, true, false).await;
    let sync_server = MockServer::start().await;
    mount_issue(
        &sync_server,
        "acme/app",
        300,
        ResponseTemplate::new(200).set_body_json(json!({"state": "closed"})),
    )
    .await;

    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&sync_server),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&sync_server).await,
        vec!["/repos/acme/app/issues/300"]
    );
    assert_eq!(test_link(&db, test).await.0, None);

    let filing = MockServer::start().await;
    mount_create(
        &filing,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;
    let filed = github::file_issues_for(
        db.connection(),
        &[test],
        &gh_settings(&[]),
        &api(&filing),
        &unstopped(),
    )
    .await;
    assert_eq!(filed.linked.into_iter().collect::<Vec<_>>(), vec![test]);
    assert_eq!(test_link(&db, test).await.0, Some(77));
    assert!(!test_link(&db, test).await.2, "a new link is checked first");
}

#[tokio::test]
async fn the_sync_rearm_clears_the_fixed_note_stamp_but_a_declined_link_keeps_it() {
    let db = TestDb::new().await.expect("a test database");
    let closed = flaky_test_in(&db, "acme/app", "p1", "t1").await;
    let gone = flaky_test_in(&db, "acme/app", "p2", "t2").await;
    let declined = flaky_test_in(&db, "acme/app", "p3", "t3").await;
    for (id, number) in [(closed, 11), (gone, 22), (declined, 33)] {
        stamp_link(&db, id, number, true, false).await;
        stamp_note(&db, "test_cases", id).await;
    }
    let server = MockServer::start().await;
    mount_issue(
        &server,
        "acme/app",
        11,
        ResponseTemplate::new(200)
            .set_body_json(json!({"state": "closed", "state_reason": "completed"})),
    )
    .await;
    mount_issue(
        &server,
        "acme/app",
        22,
        ResponseTemplate::new(404).set_body_json(json!({"message": "Not Found"})),
    )
    .await;
    mount_issue(
        &server,
        "acme/app",
        33,
        ResponseTemplate::new(200)
            .set_body_json(json!({"state": "closed", "state_reason": "not_planned"})),
    )
    .await;

    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues/11",
            "/repos/acme/app/issues/22",
            "/repos/acme/app/issues/33"
        ]
    );
    for id in [closed, gone] {
        let (number, _, _, _) = test_link(&db, id).await;
        assert_eq!(number, None, "the closed or gone link re-arms");
        assert!(!noted(&db, "test_cases", id).await);
    }
    let (number, _, _, is_closed) = test_link(&db, declined).await;
    assert_eq!(number, Some(33), "a declined link is kept");
    assert!(is_closed);
    assert!(
        noted(&db, "test_cases", declined).await,
        "the same Issue keeps the comment it already earned"
    );
}
