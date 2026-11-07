//! The issue-state sync's rotation, its stop rules and the post-delete close.

use flakeradar::github::{self, Api, Verdict};
use flakeradar::settings::Secret;
use flakeradar::settings::Settings;
use flakeradar_core::model::EntityKind;
use flakeradar_db::issue_links;
use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{TimeZone as _, Utc};
use flakeradar_db::clock::SharedClock;
use flakeradar_test_support::{FixedClock, TestDb, system_clock};
use sea_orm::ConnectionTrait as _;
use sea_orm::{DbBackend, Statement, Value};
use serde_json::{Value as Json, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{
    CREATED, Held, Loopback, TOKEN, TRUNCATED, api, flaky_job, flaky_row, flaky_test,
    flaky_test_in, gh_settings, link_of, mount_close, mount_create, mount_issue, mount_state,
    noted, requested_paths, stamp, stamp_link, stamp_note, sync_batch, unstopped,
};

#[tokio::test]
async fn the_sync_stamps_a_checked_link_with_the_clock_it_was_given() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(11)).await;
    let server = MockServer::start().await;
    mount_issue(
        &server,
        "acme/app",
        11,
        ResponseTemplate::new(200).set_body_json(json!({"state": "open"})),
    )
    .await;
    let instant = Utc
        .with_ymd_and_hms(2001, 2, 3, 4, 5, 6)
        .unwrap()
        .fixed_offset();
    let clock: SharedClock = Arc::new(FixedClock::new(instant));

    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&server),
        &clock,
        &unstopped(),
    )
    .await;

    assert_eq!(
        link_of(&db, EntityKind::Test, test).await.checked_at,
        Some(instant),
        "the check was stamped with the clock the sync was given"
    );
}

#[tokio::test]
async fn the_sync_rearms_a_link_whose_issue_closed() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let still_open = flaky_test_in(&db, "acme/keep", "backend", "open").await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(12).checked()).await;
    stamp_link(
        &db,
        EntityKind::Test,
        still_open,
        stamp().number(22).checked(),
    )
    .await;
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

    sync_batch(&db, &server).await;
    let link = link_of(&db, EntityKind::Test, test).await;
    assert_eq!(
        link.number, None,
        "the number goes, so the next Report may re-file"
    );
    assert_eq!(link.filed, None);
    assert!(!link.closed);
    assert_eq!(
        link_of(&db, EntityKind::Test, still_open).await.number,
        Some(22),
        "an open issue keeps its link"
    );
}

#[tokio::test]
async fn the_sync_keeps_a_link_closed_as_not_planned() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(12).checked()).await;
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(json!({"state": "closed", "state_reason": "not_planned"})),
    )
    .await;

    sync_batch(&db, &server).await;
    let link = link_of(&db, EntityKind::Test, test).await;
    assert_eq!(
        link.number,
        Some(12),
        "the team declined this flake; nothing is filed again"
    );
    assert_eq!(link.filed, Some(true), "the link is still ours");
    assert!(link.checked(), "and the link stays in the sync rotation");
    assert!(link.closed, "and the row records why");
}

#[tokio::test]
async fn the_sync_clears_the_flag_when_a_declined_issue_reopens() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(
        &db,
        EntityKind::Test,
        test,
        stamp().number(12).checked().closed(),
    )
    .await;
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(json!({"state": "open", "state_reason": "reopened"})),
    )
    .await;

    sync_batch(&db, &server).await;
    let link = link_of(&db, EntityKind::Test, test).await;
    assert_eq!(link.number, Some(12));
    assert_eq!(link.filed, Some(true), "the link is still ours");
    assert!(!link.closed, "filing is live again");
}

#[tokio::test]
async fn the_sync_rearms_a_link_whose_issue_is_gone() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(12).checked()).await;
    let server = MockServer::start().await;
    mount_state(&server, ResponseTemplate::new(404)).await;

    sync_batch(&db, &server).await;
    let link = link_of(&db, EntityKind::Test, test).await;
    assert_eq!(link.number, None);
    assert_eq!(link.filed, None, "the flag resets with the number");
    assert!(link.checked(), "the row rotates like a checked one");
    assert!(!link.closed, "a row with no number keeps no stale flag");
}

#[tokio::test]
async fn the_sync_keeps_a_link_on_a_permission_error_and_stamps_it() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let other = flaky_test_in(&db, "acme/b", "backend", "closed").await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(12)).await;
    stamp_link(&db, EntityKind::Test, other, stamp().number(22)).await;
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

    sync_batch(&db, &server).await;
    let link = link_of(&db, EntityKind::Test, test).await;
    assert_eq!(
        link.number,
        Some(12),
        "a permission error says nothing about the issue"
    );
    assert!(
        link.checked(),
        "but the row was answered for, so it rotates"
    );
    assert_eq!(
        link_of(&db, EntityKind::Test, other).await.number,
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
        stamp_link(&db, EntityKind::Test, test, stamp().number(11)).await;
        stamp_link(&db, EntityKind::Test, other, stamp().number(22)).await;
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

        sync_batch(&db, &server).await;
        assert_eq!(
            requested_paths(&server).await,
            vec!["/repos/acme/a/issues/11", "/repos/acme/b/issues/22"],
            "raw {raw:?}: the batch goes on"
        );
        let link = link_of(&db, EntityKind::Test, test).await;
        assert_eq!(link.number, Some(11), "raw {raw:?}: the link survives");
        assert!(link.checked(), "raw {raw:?}: the row rotates");
        assert_eq!(
            link_of(&db, EntityKind::Test, other).await.number,
            None,
            "raw {raw:?}: the other Repo still re-arms"
        );
    }
}

#[tokio::test]
async fn the_sync_stops_on_a_rate_limit_before_stamping_the_row() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(12)).await;
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(429).insert_header("retry-after", "60"),
    )
    .await;

    sync_batch(&db, &server).await;
    let link = link_of(&db, EntityKind::Test, test).await;
    assert_eq!(link.number, Some(12));
    assert!(
        !link.checked(),
        "a rate-limited row is retried first next run"
    );
}

#[tokio::test]
async fn nothing_configured_syncs_nothing() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(12)).await;
    let server = MockServer::start().await;
    let settings = Settings::from_map(&std::collections::BTreeMap::new()).expect("the defaults");

    github::sync_closed_issues(
        db.connection(),
        &settings,
        &Api::new(Secret::new(""), &server.uri()).expect("the HTTP client builds"),
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
}

#[tokio::test]
async fn a_sync_whose_rotation_list_cannot_be_read_makes_no_request() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(12)).await;
    db.connection()
        .execute_unprepared("DROP TABLE test_cases CASCADE")
        .await
        .expect("the table is dropped");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    sync_batch(&db, &server).await;

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

    sync_batch(&db, &server).await;
    let link = link_of(&db, EntityKind::Test, test).await;
    assert_eq!(link.number, Some(12), "an unknown state says nothing");
    assert_eq!(link.filed, Some(true));
    assert!(link.closed, "only a reported reopen clears the flag");
    assert!(link.checked(), "and the row still rotates");
}

#[tokio::test]
async fn the_sync_rotates_through_its_links_a_batch_at_a_time() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/app", "p1", "t1").await;
    let second = flaky_test_in(&db, "acme/app", "p2", "t2").await;
    let third = flaky_test_in(&db, "acme/app", "p3", "t3").await;
    stamp_link(&db, EntityKind::Test, first, stamp().number(11)).await;
    stamp_link(&db, EntityKind::Test, second, stamp().number(22)).await;
    stamp_link(&db, EntityKind::Test, third, stamp().number(33)).await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "open"})))
        .mount(&server)
        .await;
    let settings = gh_settings(&[("FLAKERADAR_GITHUB_SYNC_BATCH", "2")]);

    github::sync_closed_issues(
        db.connection(),
        &settings,
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/app/issues/11", "/repos/acme/app/issues/22"],
        "every stamp is NULL, so the two lowest ids go first"
    );
    assert_eq!(link_of(&db, EntityKind::Test, first).await.number, Some(11));
    assert!(
        link_of(&db, EntityKind::Test, first).await.checked(),
        "an open issue is still a check"
    );

    github::sync_closed_issues(
        db.connection(),
        &settings,
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
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
    stamp_link(&db, EntityKind::Test, first, stamp().number(11)).await;
    stamp_link(&db, EntityKind::Test, second, stamp().number(22)).await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "open"})))
        .mount(&server)
        .await;

    sync_batch(&db, &server).await;
    assert_eq!(
        requested_paths(&server).await.len(),
        2,
        "both carry a stamp now"
    );

    let fresh = flaky_test_in(&db, "acme/app", "p3", "t3").await;
    stamp_link(&db, EntityKind::Test, fresh, stamp().number(33)).await;
    let settings = gh_settings(&[("FLAKERADAR_GITHUB_SYNC_BATCH", "1")]);
    github::sync_closed_issues(
        db.connection(),
        &settings,
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&server).await[2..],
        ["/repos/acme/app/issues/33"],
        "the never-checked link goes first and is stamped"
    );
    assert!(link_of(&db, EntityKind::Test, fresh).await.checked());
}

#[tokio::test]
async fn a_sync_stops_at_a_transport_error_with_one_request() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/a", "p1", "a").await;
    let second = flaky_test_in(&db, "acme/b", "p2", "b").await;
    stamp_link(&db, EntityKind::Test, first, stamp().number(1)).await;
    stamp_link(&db, EntityKind::Test, second, stamp().number(2)).await;
    // Every connection is accepted and dropped without an answer at all.
    let loopback = Loopback::dropping().await;

    let report = github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &Api::new(Secret::new(TOKEN), &loopback.base).expect("the HTTP client builds"),
        &system_clock(),
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
        !link_of(&db, EntityKind::Test, first).await.checked(),
        "a row GitHub never answered for is not stamped"
    );
    assert!(
        !link_of(&db, EntityKind::Test, second).await.checked(),
        "the batch never reached the second row"
    );
}

#[tokio::test]
async fn a_sync_rate_limit_stops_the_batch_and_keeps_the_stamp_it_made() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/a", "p1", "a").await;
    let second = flaky_test_in(&db, "acme/b", "p2", "b").await;
    stamp_link(&db, EntityKind::Test, first, stamp().number(1)).await;
    stamp_link(&db, EntityKind::Test, second, stamp().number(2)).await;

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
    let report = sync_batch(&db, &server).await;
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
        link_of(&db, EntityKind::Test, first).await.checked(),
        "the checked stamp is written"
    );
    assert!(
        !link_of(&db, EntityKind::Test, second).await.checked(),
        "the rate-limited row is never stamped"
    );
}

#[tokio::test]
async fn a_sync_continues_from_the_stamp_a_stopped_run_left() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/a", "p1", "a").await;
    let second = flaky_test_in(&db, "acme/b", "p2", "b").await;
    stamp_link(&db, EntityKind::Test, first, stamp().number(1)).await;
    stamp_link(&db, EntityKind::Test, second, stamp().number(2)).await;
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
    sync_batch(&db, &server).await;

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
        &system_clock(),
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
    stamp_link(&db, EntityKind::Test, a1, stamp().number(11)).await;
    stamp_link(&db, EntityKind::Test, a2, stamp().number(12)).await;
    stamp_link(&db, EntityKind::Test, b, stamp().number(22)).await;
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

    sync_batch(&db, &server).await;
    let mut paths = requested_paths(&server).await;
    paths.sort();
    assert_eq!(
        paths,
        vec!["/repos/acme/a/issues/11", "/repos/acme/b/issues/22"],
        "the denied Repo is skipped for the rest of the sync"
    );
    let link = link_of(&db, EntityKind::Test, a1).await;
    assert_eq!(link.number, Some(11), "a permission 403 clears nothing");
    assert!(link.checked(), "the denied row rotates like a checked one");
    let link = link_of(&db, EntityKind::Test, a2).await;
    assert_eq!(link.number, Some(12));
    assert!(link.checked(), "the skipped row rotates too");
    assert_eq!(
        link_of(&db, EntityKind::Test, b).await.number,
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
    stamp_link(&db, EntityKind::Test, a1, stamp().number(11)).await;
    stamp_link(&db, EntityKind::Test, a2, stamp().number(12)).await;
    stamp_link(&db, EntityKind::Test, b, stamp().number(22)).await;
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

    github::sync_closed_issues(
        db.connection(),
        &settings,
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/a/issues/11"],
        "the cap holds the two denied rows and the second gets no GET"
    );

    github::sync_closed_issues(
        db.connection(),
        &settings,
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/a/issues/11",
            "/repos/acme/b/issues/22",
            "/repos/acme/a/issues/11"
        ],
        "the denied rows were stamped, so the other Repo's link sorts first"
    );
    assert_eq!(link_of(&db, EntityKind::Test, a1).await.number, Some(11));
    assert_eq!(link_of(&db, EntityKind::Test, a2).await.number, Some(12));
    assert_eq!(
        link_of(&db, EntityKind::Test, b).await.number,
        None,
        "rotation reached repo B"
    );
    assert!(link_of(&db, EntityKind::Test, b).await.checked());
}

#[tokio::test]
async fn a_sync_definitive_client_error_keeps_the_link_and_skips_the_repo() {
    for status in [401, 410] {
        let db = TestDb::new().await.expect("a test database");
        let a1 = flaky_test_in(&db, "acme/a", "p1", "a1").await;
        let a2 = flaky_test_in(&db, "acme/a", "p2", "a2").await;
        let b = flaky_test_in(&db, "acme/b", "p3", "closed").await;
        stamp_link(&db, EntityKind::Test, a1, stamp().number(11)).await;
        stamp_link(&db, EntityKind::Test, a2, stamp().number(12)).await;
        stamp_link(&db, EntityKind::Test, b, stamp().number(22)).await;
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

        sync_batch(&db, &server).await;
        assert_eq!(
            requested_paths(&server).await,
            vec!["/repos/acme/a/issues/11", "/repos/acme/b/issues/22"],
            "status {status}: the Repo is skipped, the other Repo still syncs"
        );
        assert_eq!(
            link_of(&db, EntityKind::Test, a1).await.number,
            Some(11),
            "status {status}"
        );
        assert!(
            link_of(&db, EntityKind::Test, a1).await.checked(),
            "status {status}: the row rotates"
        );
        assert_eq!(link_of(&db, EntityKind::Test, a2).await.number, Some(12));
        assert!(
            link_of(&db, EntityKind::Test, a2).await.checked(),
            "status {status}"
        );
        assert_eq!(
            link_of(&db, EntityKind::Test, b).await.number,
            None,
            "status {status}"
        );
    }
}

/// One row of `kind` whose link says FlakeRadar filed issue `number`.
async fn linked_row(db: &TestDb, kind: EntityKind, number: i32) -> i32 {
    let id = flaky_row(db, kind, "acme/app", &format!("{number}")).await;
    stamp_link(db, kind, id, stamp().number(number)).await;
    id
}

/// GitHub no longer has the issue: the link is dropped so the next Report may
/// re-file, and the row keeps its check stamp.
async fn gone_issue_rearms_its_link(kind: EntityKind) {
    let db = TestDb::new().await.expect("a test database");
    let id = linked_row(&db, kind, 11).await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/11"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message": "Gone"})))
        .mount(&server)
        .await;

    sync_batch(&db, &server).await;

    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/app/issues/11"],
        "the linked issue is the only one checked"
    );
    let link = link_of(&db, kind, id).await;
    assert_eq!(link.number, None, "a gone issue re-arms");
    assert!(link.checked(), "and the check stamp is kept");
    assert!(!link.closed, "and the declined flag is not set");
}

#[tokio::test]
async fn a_gone_test_issue_rearms_its_link() {
    gone_issue_rearms_its_link(EntityKind::Test).await;
}

#[tokio::test]
async fn a_gone_job_issue_rearms_its_link() {
    gone_issue_rearms_its_link(EntityKind::Job).await;
}

/// An answer that says nothing about the issue (a 500): the link and its stamp
/// stand, and the batch reports no skip.
async fn an_unusable_answer_keeps_the_link(kind: EntityKind) {
    let db = TestDb::new().await.expect("a test database");
    let id = linked_row(&db, kind, 11).await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/11"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"message": "Server error"})))
        .mount(&server)
        .await;

    let report = sync_batch(&db, &server).await;

    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/app/issues/11"],
        "the issue is checked again"
    );
    assert!(
        report.skipped.is_empty(),
        "{kind:?}: an unexpected status is nobody's fault: {report:?}"
    );
    let link = link_of(&db, kind, id).await;
    assert_eq!(link.number, Some(11), "the answer keeps the link");
    assert!(link.checked(), "and its stamp");
}

#[tokio::test]
async fn an_unusable_answer_keeps_a_test_link() {
    an_unusable_answer_keeps_the_link(EntityKind::Test).await;
}

#[tokio::test]
async fn an_unusable_answer_keeps_a_job_link() {
    an_unusable_answer_keeps_the_link(EntityKind::Job).await;
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
    github::file_for(
        db.connection(),
        EntityKind::Job,
        &[job],
        &gh_settings(&[]),
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(link_of(&db, EntityKind::Job, job).await.number, Some(77));

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
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(link_of(&db, EntityKind::Job, job).await.number, None);

    let refile = MockServer::start().await;
    mount_create(
        &refile,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;
    assert_eq!(
        github::file_for(
            db.connection(),
            EntityKind::Job,
            &[job],
            &gh_settings(&[]),
            &api(&refile),
            &system_clock(),
            &unstopped()
        )
        .await
        .linked
        .len(),
        1,
        "the re-armed Job is filed again"
    );
    assert_eq!(link_of(&db, EntityKind::Job, job).await.number, Some(77));
    assert!(
        !link_of(&db, EntityKind::Job, job).await.checked(),
        "a new link is checked first"
    );
}

#[tokio::test]
async fn deleting_a_repo_comments_then_closes_each_issue() {
    let server = MockServer::start().await;
    for number in [123, 456] {
        mount_close(&server, number, 201, false).await;
        mount_close(&server, number, 200, true).await;
    }

    let report = github::close_issues_after_removal("acme/app", &[123, 456], &api(&server)).await;
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
    assert_eq!(
        report.closed,
        BTreeSet::from([123, 456]),
        "the batch reports both issues closed"
    );
    assert!(
        report.refused.is_empty() && report.stopped.is_none(),
        "nothing was refused and nothing ended the batch: {report:?}"
    );
    let comment: Json = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    let text = comment["body"].as_str().expect("a body");
    assert!(text.contains("admin") && text.contains("delet"), "{text}");
    let close: Json = serde_json::from_slice(&requests[1].body).expect("the body is JSON");
    assert_eq!(close, json!({"state": "closed"}));

    // No numbers and no token are both silent.
    let empty = MockServer::start().await;
    let silent = github::close_issues_after_removal("acme/app", &[], &api(&empty)).await;
    let unconfigured = github::close_issues_after_removal(
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
    assert_eq!(
        (silent, unconfigured),
        (
            github::RemovalReport::default(),
            github::RemovalReport::default()
        ),
        "a batch with nothing to do reports nothing"
    );

    // A gone issue is not closed: the comment's 404 ends that issue.
    let gone = MockServer::start().await;
    mount_close(&gone, 123, 404, false).await;
    mount_close(&gone, 456, 201, false).await;
    mount_close(&gone, 456, 200, true).await;
    let report = github::close_issues_after_removal("acme/app", &[123, 456], &api(&gone)).await;
    assert_eq!(
        requested_paths(&gone).await,
        vec![
            "/repos/acme/app/issues/123/comments",
            "/repos/acme/app/issues/456/comments",
            "/repos/acme/app/issues/456",
        ],
        "the gone issue gets no close PATCH"
    );
    assert_eq!(
        report.closed,
        BTreeSet::from([456]),
        "only the issue that still existed was closed"
    );
    assert_eq!(
        report.refused.get(&123),
        Some(&Verdict::NotFound),
        "the 404 says the issue is gone: {report:?}"
    );
}

#[tokio::test]
async fn a_failed_post_delete_request_costs_only_that_issue() {
    // A 500 on the comment.
    let failed_comment = MockServer::start().await;
    mount_close(&failed_comment, 123, 500, false).await;
    mount_close(&failed_comment, 456, 201, false).await;
    mount_close(&failed_comment, 456, 200, true).await;
    let report =
        github::close_issues_after_removal("acme/app", &[123, 456], &api(&failed_comment)).await;
    assert_eq!(
        requested_paths(&failed_comment).await,
        vec![
            "/repos/acme/app/issues/123/comments",
            "/repos/acme/app/issues/456/comments",
            "/repos/acme/app/issues/456",
        ]
    );
    assert_eq!(
        report.closed,
        BTreeSet::from([456]),
        "the issue whose comment failed is the only one left open"
    );
    assert_eq!(
        report.refused.get(&123),
        Some(&Verdict::Other),
        "a 500 is one issue's failure, not the Repo's: {report:?}"
    );

    // An issue deleted between the comment and the close PATCH is skipped.
    let gone_close = MockServer::start().await;
    mount_close(&gone_close, 123, 201, false).await;
    mount_close(&gone_close, 123, 404, true).await;
    mount_close(&gone_close, 456, 201, false).await;
    mount_close(&gone_close, 456, 200, true).await;
    let report =
        github::close_issues_after_removal("acme/app", &[123, 456], &api(&gone_close)).await;
    assert_eq!(
        requested_paths(&gone_close).await,
        vec![
            "/repos/acme/app/issues/123/comments",
            "/repos/acme/app/issues/123",
            "/repos/acme/app/issues/456/comments",
            "/repos/acme/app/issues/456",
        ]
    );
    assert_eq!(
        report.closed,
        BTreeSet::from([456]),
        "an issue deleted before its close is not reported closed"
    );
    assert_eq!(
        report.refused.get(&123),
        Some(&Verdict::NotFound),
        "the close's 404 says the issue is already gone: {report:?}"
    );

    // A 5xx on the close PATCH is a warning, and the batch goes on.
    let failed_close = MockServer::start().await;
    mount_close(&failed_close, 123, 201, false).await;
    mount_close(&failed_close, 123, 500, true).await;
    mount_close(&failed_close, 456, 201, false).await;
    mount_close(&failed_close, 456, 200, true).await;
    let report =
        github::close_issues_after_removal("acme/app", &[123, 456], &api(&failed_close)).await;
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
        report.closed,
        BTreeSet::from([456]),
        "a failed close costs its own issue and the batch goes on"
    );
    assert_eq!(
        report.refused.get(&123),
        Some(&Verdict::Other),
        "{report:?}"
    );
}

#[tokio::test]
async fn the_post_delete_close_reports_the_issue_nothing_answered() {
    // Nothing listens on this port.
    let api = Api::new(Secret::new(TOKEN), "http://127.0.0.1:1").expect("the HTTP client builds");
    let report = github::close_issues_after_removal("acme/app", &[123], &api).await;
    assert_eq!(
        report.refused.get(&123),
        Some(&Verdict::Unreachable),
        "the issue whose comment got no answer is the one it lost: {report:?}"
    );
    assert!(
        report.closed.is_empty(),
        "nothing is reported closed for an endpoint nothing answered"
    );
}

#[tokio::test]
async fn a_rate_limit_on_either_post_delete_request_stops_the_batch() {
    let comment_limit = MockServer::start().await;
    mount_close(&comment_limit, 123, 429, false).await;
    mount_close(&comment_limit, 456, 201, false).await;
    let report =
        github::close_issues_after_removal("acme/app", &[123, 456], &api(&comment_limit)).await;
    assert_eq!(
        requested_paths(&comment_limit).await,
        vec!["/repos/acme/app/issues/123/comments"],
        "a throttled comment stops the batch"
    );
    assert_eq!(
        report.stopped,
        Some(github::BatchStop::RateLimited),
        "a throttled comment ends the batch: {report:?}"
    );
    assert!(
        report.closed.is_empty(),
        "nothing is closed when the first comment is throttled"
    );

    let close_limit = MockServer::start().await;
    mount_close(&close_limit, 123, 201, false).await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/123"))
        .respond_with(ResponseTemplate::new(429).insert_header("x-ratelimit-remaining", "0"))
        .mount(&close_limit)
        .await;
    mount_close(&close_limit, 456, 201, false).await;
    let report =
        github::close_issues_after_removal("acme/app", &[123, 456], &api(&close_limit)).await;
    assert_eq!(
        requested_paths(&close_limit).await,
        vec![
            "/repos/acme/app/issues/123/comments",
            "/repos/acme/app/issues/123",
        ],
        "a throttled close PATCH stops the batch"
    );
    assert_eq!(
        report.stopped,
        Some(github::BatchStop::RateLimited),
        "a throttled close ends the batch: {report:?}"
    );
    assert!(
        report.closed.is_empty(),
        "the throttled close itself is not reported closed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_link_replaced_during_the_sync_get_survives() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test_in(&db, "acme/app", "linked", "linked").await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(11).checked()).await;
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
    let clock = system_clock();
    let stop = unstopped();
    tokio::join!(
        github::sync_closed_issues(db.connection(), &settings, &api, &clock, &stop),
        relink
    );

    let link = link_of(&db, EntityKind::Test, test).await;
    assert_eq!(
        link.number,
        Some(99),
        "the link replaced during the GET stands, whatever the old answer said"
    );
    assert!(
        !link.checked(),
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

    let report = github::close_issues_after_removal("acme/app", &[123], &api).await;

    assert_eq!(
        loopback.accepted(),
        1,
        "a dead body costs the issue before a close is attempted"
    );
    assert_eq!(
        report.refused.get(&123),
        Some(&Verdict::Unreachable),
        "the answer without a body is the transport error, not a status: {report:?}"
    );
    assert!(
        report.closed.is_empty() && report.stopped.is_none(),
        "neither a close nor an end of the batch came of it: {report:?}"
    );
}

#[tokio::test]
async fn a_post_delete_close_whose_connection_dies_costs_that_issue() {
    // The comment lands, then the close's connection is dropped before it
    // answers: that one issue is lost and the batch goes on.
    let loopback = Loopback::answering(CREATED.to_owned()).await;
    let api = Api::new(Secret::new(TOKEN), &loopback.base).expect("the HTTP client builds");

    let report = github::close_issues_after_removal("acme/app", &[123], &api).await;

    assert_eq!(
        loopback.accepted(),
        2,
        "the comment landed and the close was attempted"
    );
    assert_eq!(
        report.refused.get(&123),
        Some(&Verdict::Unreachable),
        "the dead close is the one issue the batch lost: {report:?}"
    );
    assert!(
        report.closed.is_empty(),
        "a close that died is never reported as done: {report:?}"
    );
}

#[tokio::test]
async fn a_test_sync_rearms_its_closed_issue_and_the_filer_files_again() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(300).checked()).await;
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
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&sync_server).await,
        vec!["/repos/acme/app/issues/300"]
    );
    assert_eq!(link_of(&db, EntityKind::Test, test).await.number, None);

    let filing = MockServer::start().await;
    mount_create(
        &filing,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;
    let filed = github::file_for(
        db.connection(),
        EntityKind::Test,
        &[test],
        &gh_settings(&[]),
        &api(&filing),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(filed.linked.into_iter().collect::<Vec<_>>(), vec![test]);
    assert_eq!(link_of(&db, EntityKind::Test, test).await.number, Some(77));
    assert!(
        !link_of(&db, EntityKind::Test, test).await.checked(),
        "a new link is checked first"
    );
}

#[tokio::test]
async fn the_sync_rearm_clears_the_fixed_note_stamp_but_a_declined_link_keeps_it() {
    let db = TestDb::new().await.expect("a test database");
    let closed = flaky_test_in(&db, "acme/app", "p1", "t1").await;
    let gone = flaky_test_in(&db, "acme/app", "p2", "t2").await;
    let declined = flaky_test_in(&db, "acme/app", "p3", "t3").await;
    for (id, number) in [(closed, 11), (gone, 22), (declined, 33)] {
        stamp_link(&db, EntityKind::Test, id, stamp().number(number).checked()).await;
        stamp_note(&db, EntityKind::Test, id).await;
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

    sync_batch(&db, &server).await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues/11",
            "/repos/acme/app/issues/22",
            "/repos/acme/app/issues/33"
        ]
    );
    for id in [closed, gone] {
        let link = link_of(&db, EntityKind::Test, id).await;
        assert_eq!(link.number, None, "the closed or gone link re-arms");
        assert!(!noted(&db, "test_cases", id).await);
    }
    let link = link_of(&db, EntityKind::Test, declined).await;
    assert_eq!(link.number, Some(33), "a declined link is kept");
    assert!(link.closed);
    assert!(
        noted(&db, "test_cases", declined).await,
        "the same Issue keeps the comment it already earned"
    );
}
