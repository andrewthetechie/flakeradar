//! The fixed-note batch, its close, and every link write that clears a stamp.

// Rust guideline compliant 2026-07-21

use flakeradar::Settings;
use flakeradar::github::{self, Api};
use flakeradar::settings::Secret;
use flakeradar_core::EntityKind;
use flakeradar_db::ReadSettings;
use flakeradar_db::issue_links::{self, linked_for_check};
use flakeradar_test_support::{TestDb, make_job, make_pipeline};
use sea_orm::ConnectionTrait as _;
use sea_orm::{DbBackend, Statement, Value};
use serde_json::{Value as Json, json};
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use crate::support::{
    CREATED, Loopback, TOKEN, api, clean_streak_of, fixed_settings, flaky_job, flaky_test,
    flaky_test_in, gh_settings, job_checked, job_link, lines_with, link_table, logged_from,
    mount_comment, mount_create, mount_issue, mount_patch, mount_state, note_row, noted,
    requested_calls, requested_paths, set_clean_streak, stage_mid_request, stamp_job_link,
    stamp_link, stamp_note, test_link, unstopped,
};

#[tokio::test]
async fn the_fixed_note_comments_once_with_the_configured_sentence() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, true, false).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET clean_streak = 5 WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the streak writes");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    let settings = fixed_settings(&[("FLAKERADAR_PUBLIC_URL", "https://flakes.example.com/")]);
    github::note_fixed_issues_for(
        db.connection(),
        &[test],
        &settings,
        &api(&server),
        &unstopped(),
    )
    .await;

    let requests = server.received_requests().await.expect("a record");
    let body: Json = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    assert_eq!(
        body["body"],
        json!(
            "FlakeRadar: 5 passing runs since the last failure (score now 0.50). \
             The fix appears to hold.\n\nhttps://flakes.example.com/?test=1"
        )
    );

    // A second pass finds the stamp and posts nothing more.
    github::note_fixed_issues_for(
        db.connection(),
        &[test],
        &settings,
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(server.received_requests().await.expect("a record").len(), 1);

    // A failed run reset the Clean streak and the stamp: the fix is noteable again.
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_fixed_noted_at = NULL WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the stamp clears");
    github::note_fixed_issues_for(
        db.connection(),
        &[test],
        &settings,
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(server.received_requests().await.expect("a record").len(), 2);
    assert!(noted(&db, "test_cases", test).await);
}

#[tokio::test]
async fn the_fixed_note_names_the_default_branch_when_it_is_known() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, true, false).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET clean_streak = 4 WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the streak writes");
    db.connection()
        .execute_raw(Statement::from_string(
            DbBackend::Postgres,
            "UPDATE repos SET default_branch = 'main'".to_owned(),
        ))
        .await
        .expect("the Default branch writes");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    github::note_fixed_issues_for(
        db.connection(),
        &[test],
        &fixed_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    let requests = server.received_requests().await.expect("a record");
    let body: Json = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    assert_eq!(
        body["body"],
        json!(
            "FlakeRadar: 4 passing runs on `main` since the last failure (score now 0.50). \
             The fix appears to hold."
        )
    );
}

#[tokio::test]
async fn the_fixed_note_closes_the_issue_only_when_asked() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, true, false).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET clean_streak = 3 WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the streak writes");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/12"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    // CLOSE_FIXED off: a comment alone.
    github::note_fixed_issues_for(
        db.connection(),
        &[test],
        &fixed_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/app/issues/12/comments"]
    );

    // On, with the stamp cleared: that comment, then the close PATCH.
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_fixed_noted_at = NULL WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the stamp clears");
    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);
    github::note_fixed_issues_for(
        db.connection(),
        &[test],
        &settings,
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues/12/comments",
            "/repos/acme/app/issues/12/comments",
            "/repos/acme/app/issues/12"
        ]
    );

    let requests = server.received_requests().await.expect("a record");
    let close = requests
        .iter()
        .find(|request| request.method.as_str() == "PATCH")
        .expect("the issue was closed");
    let body: Json = serde_json::from_slice(&close.body).expect("the body is JSON");
    assert_eq!(
        body,
        json!({"state": "closed", "state_reason": "completed"})
    );
}

#[tokio::test]
async fn the_fixed_note_is_disabled_while_the_streak_is_zero() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, true, false).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET clean_streak = 9 WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the streak writes");
    let server = MockServer::start().await;

    github::note_fixed_issues_for(
        db.connection(),
        &[test],
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
            .is_empty()
    );
}

#[tokio::test]
async fn a_job_issue_is_noted_and_closed_with_the_same_wording() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job(&db).await;
    stamp_job_link(&db, job, 77, true).await;
    set_clean_streak(&db, "jobs", job, 3).await;
    let server = MockServer::start().await;
    mount_comment(&server, "acme/app", 77, ResponseTemplate::new(201)).await;
    mount_patch(&server, "acme/app", 77, ResponseTemplate::new(200)).await;
    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);

    github::note_fixed_job_issues_for(
        db.connection(),
        &[job],
        &settings,
        &api(&server),
        &unstopped(),
    )
    .await;

    let requests = server.received_requests().await.expect("a record");
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues/77/comments",
            "/repos/acme/app/issues/77"
        ]
    );
    let body: Json = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    assert_eq!(
        body["body"],
        json!(
            "FlakeRadar: 3 passing runs since the last failure (score now 0.50). \
             The fix appears to hold."
        ),
        "a Job's note says exactly what a Test's does"
    );
    let close: Json = serde_json::from_slice(&requests[1].body).expect("the body is JSON");
    assert_eq!(
        close,
        json!({"state": "closed", "state_reason": "completed"})
    );
    assert!(noted(&db, "jobs", job).await);
    assert_eq!(job_link(&db, job).await.0, Some(77));
}

#[tokio::test]
async fn a_note_ignores_a_row_with_no_live_note() {
    let db = TestDb::new().await.expect("a test database");
    let short = flaky_test_in(&db, "acme/app", "short", "short").await;
    stamp_link(&db, short, 71, true, false).await;
    set_clean_streak(&db, "test_cases", short, 2).await;
    let unlinked = flaky_test_in(&db, "acme/app", "unlinked", "unlinked").await;
    set_clean_streak(&db, "test_cases", unlinked, 9).await;
    let declined = flaky_test_in(&db, "acme/app", "declined", "declined").await;
    stamp_link(&db, declined, 72, true, true).await;
    set_clean_streak(&db, "test_cases", declined, 9).await;
    let server = MockServer::start().await;

    github::note_fixed_issues_for(
        db.connection(),
        &[short, unlinked, declined],
        &fixed_settings(&[]),
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
        "below the streak, unlinked and declined are all silent"
    );
    assert!(!noted(&db, "test_cases", declined).await);

    let at_streak = flaky_test_in(&db, "acme/app", "streak", "streak").await;
    stamp_link(&db, at_streak, 73, true, false).await;
    set_clean_streak(&db, "test_cases", at_streak, 9).await;
    let off = fixed_settings(&[("FLAKERADAR_GITHUB_FIXED_STREAK", "0")]);
    github::note_fixed_issues_for(
        db.connection(),
        &[at_streak],
        &off,
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
        "0 ships the feature disabled"
    );
}

#[tokio::test]
async fn a_note_skips_a_job_of_another_provider() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .provider("gitlab")
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    let job = make_job(&pipeline)
        .name("unit")
        .flakiness_score(0.5)
        .build(db.connection())
        .await
        .expect("the Job inserts");
    stamp_job_link(&db, job.id, 77, true).await;
    set_clean_streak(&db, "jobs", job.id, 3).await;
    let server = MockServer::start().await;

    github::note_fixed_job_issues_for(
        db.connection(),
        &[job.id],
        &fixed_settings(&[]),
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
        "only a github Pipeline has an issue to note"
    );
}

#[tokio::test]
async fn a_note_close_that_fails_keeps_the_stamp() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test_in(&db, "acme/app", "linked", "linked").await;
    stamp_link(&db, test, 77, true, false).await;
    set_clean_streak(&db, "test_cases", test, 3).await;
    let server = MockServer::start().await;
    mount_comment(&server, "acme/app", 77, ResponseTemplate::new(201)).await;
    mount_patch(
        &server,
        "acme/app",
        77,
        ResponseTemplate::new(500).set_body_string("boom"),
    )
    .await;
    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);

    let logged = logged_from(github::note_fixed_issues_for(
        db.connection(),
        &[test],
        &settings,
        &api(&server),
        &unstopped(),
    ))
    .await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues/77/comments",
            "/repos/acme/app/issues/77"
        ]
    );
    assert!(noted(&db, "test_cases", test).await);
    assert_eq!(
        lines_with(&logged, "close after noting the fix failed").len(),
        1,
        "{logged}"
    );

    github::note_fixed_issues_for(
        db.connection(),
        &[test],
        &settings,
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&server).await.len(),
        2,
        "a failed close must not buy a second comment"
    );
}

#[tokio::test]
async fn a_note_rate_limit_stops_the_batch() {
    for (status, headers) in [(429, None), (403, Some(("x-ratelimit-remaining", "0")))] {
        let db = TestDb::new().await.expect("a test database");
        let first = flaky_test_in(&db, "acme/a", "a", "a").await;
        let second = flaky_test_in(&db, "acme/b", "b", "b").await;
        stamp_link(&db, first, 71, true, false).await;
        stamp_link(&db, second, 72, true, false).await;
        set_clean_streak(&db, "test_cases", first, 3).await;
        set_clean_streak(&db, "test_cases", second, 3).await;
        let server = MockServer::start().await;
        let mut answer = ResponseTemplate::new(status);
        if let Some((name, value)) = headers {
            answer = answer.insert_header(name, value);
        }
        Mock::given(method("POST"))
            .respond_with(answer)
            .mount(&server)
            .await;

        github::note_fixed_issues_for(
            db.connection(),
            &[first, second],
            &fixed_settings(&[]),
            &api(&server),
            &unstopped(),
        )
        .await;
        assert_eq!(
            requested_paths(&server).await,
            vec!["/repos/acme/a/issues/71/comments"],
            "status {status}: the batch stops after the first request"
        );
        assert!(
            !noted(&db, "test_cases", second).await,
            "status {status}: the second Repo is never touched"
        );
    }

    // A throttled close PATCH stops it too, after the comment landed.
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/a", "a", "a").await;
    let second = flaky_test_in(&db, "acme/b", "b", "b").await;
    stamp_link(&db, first, 71, true, false).await;
    stamp_link(&db, second, 72, true, false).await;
    set_clean_streak(&db, "test_cases", first, 3).await;
    set_clean_streak(&db, "test_cases", second, 3).await;
    let server = MockServer::start().await;
    mount_comment(&server, "acme/a", 71, ResponseTemplate::new(201)).await;
    mount_patch(
        &server,
        "acme/a",
        71,
        ResponseTemplate::new(429).insert_header("x-ratelimit-remaining", "0"),
    )
    .await;
    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);
    let logged = logged_from(github::note_fixed_issues_for(
        db.connection(),
        &[first, second],
        &settings,
        &api(&server),
        &unstopped(),
    ))
    .await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/a/issues/71/comments",
            "/repos/acme/a/issues/71"
        ],
        "{logged}"
    );
    assert!(noted(&db, "test_cases", first).await, "the comment landed");
    assert!(!noted(&db, "test_cases", second).await);
}

#[tokio::test]
async fn a_note_permission_403_skips_only_that_repo() {
    let db = TestDb::new().await.expect("a test database");
    let a1 = flaky_test_in(&db, "acme/a", "a1", "a1").await;
    let a2 = flaky_test_in(&db, "acme/a", "a2", "a2").await;
    let other = flaky_test_in(&db, "acme/b", "b", "b").await;
    stamp_link(&db, a1, 71, true, false).await;
    stamp_link(&db, a2, 73, true, false).await;
    stamp_link(&db, other, 72, true, false).await;
    for id in [a1, a2, other] {
        set_clean_streak(&db, "test_cases", id, 3).await;
    }
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/a/issues/71/comments"))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(
                json!({"message": "Resource not accessible by personal access token"}),
            ),
        )
        .mount(&server)
        .await;
    mount_comment(&server, "acme/b", 72, ResponseTemplate::new(201)).await;
    mount_comment(&server, "acme/a", 73, ResponseTemplate::new(201)).await;

    github::note_fixed_issues_for(
        db.connection(),
        &[a1, other, a2],
        &fixed_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/a/issues/71/comments",
            "/repos/acme/b/issues/72/comments",
        ],
        "the whole Repo is skipped for the rest of the batch"
    );
    assert!(noted(&db, "test_cases", other).await);
    assert!(!noted(&db, "test_cases", a2).await);

    // A permission 403 on the close costs that Repo's later closes, not the notes.
    let db = TestDb::new().await.expect("a test database");
    let a = flaky_test_in(&db, "acme/a", "a", "a").await;
    let b = flaky_test_in(&db, "acme/b", "b", "b").await;
    stamp_link(&db, a, 71, true, false).await;
    stamp_link(&db, b, 72, true, false).await;
    set_clean_streak(&db, "test_cases", a, 3).await;
    set_clean_streak(&db, "test_cases", b, 3).await;
    let server = MockServer::start().await;
    mount_comment(&server, "acme/a", 71, ResponseTemplate::new(201)).await;
    mount_comment(&server, "acme/b", 72, ResponseTemplate::new(201)).await;
    mount_patch(
        &server,
        "acme/a",
        71,
        ResponseTemplate::new(403).set_body_json(json!({"message": "Resource not accessible"})),
    )
    .await;
    mount_patch(&server, "acme/b", 72, ResponseTemplate::new(200)).await;
    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);
    let logged = logged_from(github::note_fixed_issues_for(
        db.connection(),
        &[a, b],
        &settings,
        &api(&server),
        &unstopped(),
    ))
    .await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/a/issues/71/comments",
            "/repos/acme/a/issues/71",
            "/repos/acme/b/issues/72/comments",
            "/repos/acme/b/issues/72",
        ]
    );
    assert!(noted(&db, "test_cases", a).await);
    assert!(noted(&db, "test_cases", b).await);
    assert!(
        lines_with(&logged, "issue close skipped for this Repo").len() == 1,
        "{logged}"
    );
}

#[tokio::test]
async fn a_note_network_error_is_logged_and_never_raises() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test_in(&db, "acme/app", "linked", "linked").await;
    stamp_link(&db, test, 77, true, false).await;
    set_clean_streak(&db, "test_cases", test, 3).await;
    // Nothing listens on this port.
    let api = Api::new(Secret::new(TOKEN), "http://127.0.0.1:1").expect("the HTTP client builds");

    let logged = logged_from(github::note_fixed_issues_for(
        db.connection(),
        &[test],
        &fixed_settings(&[]),
        &api,
        &unstopped(),
    ))
    .await;
    assert!(!noted(&db, "test_cases", test).await);
    assert_eq!(
        lines_with(&logged, "unreachable, skipping fixed-note comments").len(),
        1,
        "{logged}"
    );
}

#[tokio::test]
async fn a_transport_error_on_a_fixed_note_close_ends_the_batch() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/a", "linked", "linked").await;
    let second = flaky_test_in(&db, "acme/b", "other", "other").await;
    stamp_link(&db, first, 71, true, false).await;
    stamp_link(&db, second, 72, true, false).await;
    set_clean_streak(&db, "test_cases", first, 3).await;
    set_clean_streak(&db, "test_cases", second, 3).await;
    // The first comment lands, then every later connection is dropped: the close
    // of the first issue is the transport failure that ends the batch.
    let loopback = Loopback::answering(CREATED.to_owned()).await;
    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);

    let report = github::note_fixed_issues_for(
        db.connection(),
        &[first, second],
        &settings,
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
        2,
        "the second Test's comment is not posted after the close gave no answer"
    );
    assert!(
        noted(&db, "test_cases", first).await,
        "the first comment landed before the close was tried"
    );
    assert!(
        !noted(&db, "test_cases", second).await,
        "the batch ended before the second comment"
    );
}

#[tokio::test]
async fn a_fixed_note_close_whose_connection_dies_keeps_the_stamp() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test_in(&db, "acme/app", "linked", "linked").await;
    stamp_link(&db, test, 77, true, false).await;
    set_clean_streak(&db, "test_cases", test, 3).await;
    // The comment lands, then the close's connection is dropped before it answers.
    let loopback = Loopback::answering(CREATED.to_owned()).await;
    let api = Api::new(Secret::new(TOKEN), &loopback.base).expect("the HTTP client builds");
    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);

    let logged = logged_from(github::note_fixed_issues_for(
        db.connection(),
        &[test],
        &settings,
        &api,
        &unstopped(),
    ))
    .await;

    assert_eq!(
        loopback.accepted(),
        2,
        "the comment landed and the close was attempted"
    );
    assert_eq!(
        lines_with(&logged, "GitHub unreachable, skipping the close").len(),
        1,
        "{logged}"
    );
    assert!(
        noted(&db, "test_cases", test).await,
        "the comment already landed, so the row keeps its stamp"
    );
}

#[tokio::test]
async fn an_unusable_note_comment_status_leaves_the_row_unstamped() {
    let db = TestDb::new().await.expect("a test database");
    let bad = flaky_test_in(&db, "acme/a", "bad", "bad").await;
    let other = flaky_test_in(&db, "acme/b", "ok", "ok").await;
    stamp_link(&db, bad, 71, true, false).await;
    stamp_link(&db, other, 72, true, false).await;
    set_clean_streak(&db, "test_cases", bad, 3).await;
    set_clean_streak(&db, "test_cases", other, 3).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string("nope"))
        .mount(&server)
        .await;

    let logged = logged_from(github::note_fixed_issues_for(
        db.connection(),
        &[bad, other],
        &fixed_settings(&[]),
        &api(&server),
        &unstopped(),
    ))
    .await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/a/issues/71/comments",
            "/repos/acme/b/issues/72/comments",
        ],
        "a 500 is nobody's Repo-wide fault"
    );
    assert!(!noted(&db, "test_cases", bad).await);
    assert!(!noted(&db, "test_cases", other).await);
    assert_eq!(
        lines_with(&logged, "fixed-note comment failed").len(),
        2,
        "{logged}"
    );
}

/// The fixed-note entry point of one entity.
async fn note_entity(db: &TestDb, entity: &str, ids: &[i32], settings: &Settings, api: &Api) {
    if entity == "test" {
        github::note_fixed_issues_for(db.connection(), ids, settings, api, &unstopped()).await;
    } else {
        github::note_fixed_job_issues_for(db.connection(), ids, settings, api, &unstopped()).await;
    }
}

#[tokio::test]
async fn link_writes_reset_the_closed_flag() {
    let db = TestDb::new().await.expect("a test database");
    let flagged = flaky_test_in(&db, "acme/app", "flagged", "flagged").await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_number = 11, github_issue_filed = TRUE, \
             github_issue_closed = TRUE WHERE id = $1",
            [Value::from(flagged)],
        ))
        .await
        .expect("the fixture link writes");

    issue_links::unlink_issue(
        db.connection(),
        EntityKind::Test,
        i64::from(flagged),
        &ReadSettings::default(),
    )
    .await
    .expect("the unlink succeeds");
    assert_eq!(test_link(&db, flagged).await.0, None, "no number is kept");
    assert_eq!(
        test_link(&db, flagged).await.1,
        None,
        "an unlinked row is nobody's"
    );
    assert!(
        !test_link(&db, flagged).await.3,
        "unlink re-arms filing, so the declined flag goes too"
    );

    let relinked = flaky_test_in(&db, "acme/app", "p2", "relinked").await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_number = 11, github_issue_closed = TRUE WHERE id = $1",
            [Value::from(relinked)],
        ))
        .await
        .expect("the fixture link writes");
    issue_links::store_filed_link(db.connection(), EntityKind::Test, i64::from(relinked), 44)
        .await
        .expect("the fresh link writes");
    assert_eq!(
        test_link(&db, relinked).await.0,
        Some(44),
        "a fresh link is stored"
    );
    assert_eq!(
        test_link(&db, relinked).await.1,
        Some(true),
        "ours to close"
    );
    assert!(
        !test_link(&db, relinked).await.3,
        "a fresh link points at an open issue"
    );
}

#[tokio::test]
async fn job_link_change_and_unlink_reset_the_check_stamp() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job(&db).await;
    let server = MockServer::start().await;
    for number in [301, 44] {
        mount_issue(
            &server,
            "acme/app",
            number,
            ResponseTemplate::new(200).set_body_json(json!({"state": "open"})),
        )
        .await;
    }
    let settings = gh_settings(&[("FLAKERADAR_GITHUB_SYNC_BATCH", "2")]);
    let reads = ReadSettings::default();

    issue_links::link_issue(
        db.connection(),
        EntityKind::Job,
        i64::from(job),
        301,
        None,
        false,
        &reads,
    )
    .await
    .expect("the link succeeds");
    github::sync_closed_issues(db.connection(), &settings, &api(&server), &unstopped()).await;
    assert!(
        job_checked(&db, job).await,
        "an open issue is still a check"
    );

    issue_links::link_issue(
        db.connection(),
        EntityKind::Job,
        i64::from(job),
        44,
        None,
        true,
        &reads,
    )
    .await
    .expect("the replace succeeds");
    assert_eq!(job_link(&db, job).await.0, Some(44));
    assert!(
        !job_checked(&db, job).await,
        "a replaced link is checked first"
    );
    github::sync_closed_issues(db.connection(), &settings, &api(&server), &unstopped()).await;
    assert!(job_checked(&db, job).await, "the new link is checked");

    issue_links::unlink_issue(db.connection(), EntityKind::Job, i64::from(job), &reads)
        .await
        .expect("the unlink succeeds");
    assert_eq!(job_link(&db, job).await.0, None, "no number is left");
    assert!(
        !job_checked(&db, job).await,
        "with no number the stamp has nothing to rotate"
    );
}

#[tokio::test]
async fn a_deleted_row_costs_only_that_fixed_note() {
    for entity in ["test", "job"] {
        let db = TestDb::new().await.expect("a test database");
        let doomed = note_row(&db, entity, "acme/app", "doomed", Some(77), 3).await;
        let kept = note_row(&db, entity, "acme/b", "kept", Some(78), 3).await;
        let server = MockServer::start().await;
        let table = link_table(entity);
        let connection = db.connection().clone();
        Mock::given(any())
            .respond_with(move |request: &Request| {
                if request.url.path() == "/repos/acme/app/issues/77/comments" {
                    stage_mid_request(
                        &connection,
                        format!("DELETE FROM {table} WHERE id = {doomed}"),
                    );
                }
                ResponseTemplate::new(201).set_body_json(json!({"number": 99}))
            })
            .mount(&server)
            .await;
        let settings = fixed_settings(&[]);

        let logged = logged_from(note_entity(
            &db,
            entity,
            &[doomed, kept],
            &settings,
            &api(&server),
        ))
        .await;
        assert_eq!(
            requested_paths(&server).await,
            vec![
                "/repos/acme/app/issues/77/comments",
                "/repos/acme/b/issues/78/comments"
            ],
            "the vanished row costs one note, not the batch"
        );
        assert!(noted(&db, table, kept).await, "the surviving note lands");
        assert!(
            lines_with(&logged, "deleted (admin delete) while noting").len() == 1,
            "the lost note is reported: {logged}"
        );
    }
}

#[tokio::test]
async fn a_clean_streak_reset_mid_post_is_not_undone_by_the_stamp() {
    for entity in ["test", "job"] {
        let db = TestDb::new().await.expect("a test database");
        let obj = note_row(&db, entity, "acme/app", "linked", Some(77), 3).await;
        let server = MockServer::start().await;
        let table = link_table(entity);
        let connection = db.connection().clone();
        Mock::given(any())
            .respond_with(move |request: &Request| {
                if request.method.as_str() == "POST" {
                    stage_mid_request(
                        &connection,
                        format!(
                            "UPDATE {table} SET clean_streak = 0, \
                             github_issue_fixed_noted_at = NULL WHERE id = {obj}"
                        ),
                    );
                }
                ResponseTemplate::new(201).set_body_json(json!({"number": 99}))
            })
            .mount(&server)
            .await;
        let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);

        let logged = logged_from(note_entity(&db, entity, &[obj], &settings, &api(&server))).await;
        assert_eq!(
            requested_calls(&server).await,
            vec![(
                "POST".to_owned(),
                "/repos/acme/app/issues/77/comments".to_owned()
            )],
            "a fix that did not hold is neither noted twice nor closed"
        );
        assert_eq!(
            clean_streak_of(&db, table, obj).await,
            0,
            "the reset stands"
        );
        assert!(
            !noted(&db, table, obj).await,
            "the reset must not be undone by a stamp"
        );
        assert!(
            lines_with(&logged, "stopped being noteable").len() == 1,
            "the skipped stamp is reported: {logged}"
        );

        // The re-armed note really does come once the streak is back.
        set_clean_streak(&db, table, obj, 3).await;
        let second = MockServer::start().await;
        mount_comment(
            &second,
            "acme/app",
            77,
            ResponseTemplate::new(201).set_body_json(json!({"number": 99})),
        )
        .await;
        mount_patch(&second, "acme/app", 77, ResponseTemplate::new(200)).await;
        note_entity(&db, entity, &[obj], &settings, &api(&second)).await;
        assert_eq!(
            requested_calls(&second).await,
            vec![
                (
                    "POST".to_owned(),
                    "/repos/acme/app/issues/77/comments".to_owned()
                ),
                ("PATCH".to_owned(), "/repos/acme/app/issues/77".to_owned())
            ]
        );
        assert!(noted(&db, table, obj).await, "the fresh note stamps");
    }
}

#[tokio::test]
async fn an_unlink_clears_both_halves_and_a_rearm_keeps_its_stamp() {
    let db = TestDb::new().await.expect("a test database");
    let unlinked = flaky_test_in(&db, "acme/app", "p1", "t1").await;
    let rearmed = flaky_test_in(&db, "acme/app", "p2", "t2").await;
    stamp_link(&db, unlinked, 11, false, false).await;
    stamp_link(&db, rearmed, 22, false, false).await;

    issue_links::unlink_issue(
        db.connection(),
        EntityKind::Test,
        i64::from(unlinked),
        &ReadSettings::default(),
    )
    .await
    .expect("the unlink succeeds");
    let (number, filed, checked, closed) = test_link(&db, unlinked).await;
    assert_eq!(number, None, "unlink drops the number");
    assert_eq!(filed, None, "and the provenance");
    assert!(!checked, "and the check stamp");
    assert!(!closed);

    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(json!({"state": "closed", "state_reason": "completed"})),
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
        vec!["/repos/acme/app/issues/22"],
        "the unlinked row is out of the rotation"
    );
    let (number, _, checked, _) = test_link(&db, rearmed).await;
    assert_eq!(number, None, "the re-arm clears the number");
    assert!(checked, "and keeps the stamp it just wrote");
    assert!(
        !test_link(&db, unlinked).await.2,
        "and leaves the unlinked row alone"
    );
}

#[tokio::test]
async fn an_unlink_after_a_rearm_clears_the_kept_stamp_on_a_test_and_a_job() {
    for entity in ["test", "job"] {
        let db = TestDb::new().await.expect("a test database");
        let (id, number, target) = if entity == "test" {
            let id = flaky_test(&db).await;
            stamp_link(&db, id, 300, false, false).await;
            (id, 300, EntityKind::Test)
        } else {
            let id = flaky_job(&db).await;
            stamp_job_link(&db, id, 302, false).await;
            (id, 302, EntityKind::Job)
        };
        let server = MockServer::start().await;
        mount_state(
            &server,
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
            vec![format!("/repos/acme/app/issues/{number}")]
        );
        let rearmed = if entity == "test" {
            let (number, _, checked, _) = test_link(&db, id).await;
            assert_eq!(number, None);
            checked
        } else {
            assert_eq!(job_link(&db, id).await.0, None);
            job_checked(&db, id).await
        };
        assert!(rearmed, "the re-arm keeps the stamp it wrote");

        issue_links::unlink_issue(
            db.connection(),
            target,
            i64::from(id),
            &ReadSettings::default(),
        )
        .await
        .expect("the unlink succeeds");
        let cleared = if entity == "test" {
            let (number, filed, checked, _) = test_link(&db, id).await;
            assert_eq!(filed, None);
            assert_eq!(number, None);
            checked
        } else {
            assert_eq!(job_link(&db, id).await.0, None);
            job_checked(&db, id).await
        };
        assert!(!cleared, "unlink drops the stamp the re-arm kept");
        assert!(
            linked_for_check(db.connection(), target, 10)
                .await
                .expect("the rotation reads")
                .is_empty(),
            "the pair is fully reset"
        );

        let before = requested_paths(&server).await.len();
        github::sync_closed_issues(
            db.connection(),
            &gh_settings(&[]),
            &api(&server),
            &unstopped(),
        )
        .await;
        assert_eq!(
            requested_paths(&server).await.len(),
            before,
            "the unlinked {entity} is out of the rotation for good"
        );
    }
}

#[tokio::test]
async fn an_unlink_rearms_a_declined_link_and_the_filer_files_it_again() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let job = flaky_job(&db).await;
    for (table, id, number) in [("test_cases", test, 11), ("jobs", job, 22)] {
        db.connection()
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!(
                    "UPDATE {table} SET github_issue_number = $2, github_issue_filed = TRUE, \
                     github_issue_closed = TRUE WHERE id = $1"
                ),
                [Value::from(id), Value::from(number)],
            ))
            .await
            .expect("the declined link writes");
    }

    issue_links::unlink_issue(
        db.connection(),
        EntityKind::Test,
        i64::from(test),
        &ReadSettings::default(),
    )
    .await
    .expect("the Test unlink succeeds");
    let (number, filed, _, closed) = test_link(&db, test).await;
    assert_eq!(number, None);
    assert_eq!(filed, None, "no provenance without a number");
    assert!(!closed, "the declined flag goes with the link");

    issue_links::unlink_issue(
        db.connection(),
        EntityKind::Job,
        i64::from(job),
        &ReadSettings::default(),
    )
    .await
    .expect("the Job unlink succeeds");
    let (number, filed) = job_link(&db, job).await;
    assert_eq!(number, None);
    assert_eq!(filed, None);

    let filing = MockServer::start().await;
    mount_create(
        &filing,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;
    github::file_issues_for(
        db.connection(),
        &[test],
        &gh_settings(&[]),
        &api(&filing),
        &unstopped(),
    )
    .await;
    let (number, _, _, closed) = test_link(&db, test).await;
    assert_eq!(number, Some(77), "the filer files the re-armed Test");
    assert!(!closed, "a fresh link starts unflagged");

    let sync = MockServer::start().await;
    mount_state(
        &sync,
        ResponseTemplate::new(200).set_body_json(json!({"state": "open"})),
    )
    .await;
    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(&sync),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&sync).await,
        vec!["/repos/acme/app/issues/77"],
        "only the fresh link remains in the rotation"
    );
}

#[tokio::test]
async fn every_link_write_clears_the_fixed_note_stamp() {
    let db = TestDb::new().await.expect("a test database");
    let filed = flaky_test_in(&db, "acme/app", "p1", "t1").await;
    let relinked = flaky_test_in(&db, "acme/app", "p2", "t2").await;
    let unlinked = flaky_test_in(&db, "acme/app", "p3", "t3").await;
    let job = flaky_job(&db).await;
    stamp_job_link(&db, job, 40, true).await;
    for (table, id) in [("test_cases", filed), ("test_cases", relinked)] {
        stamp_note(&db, table, id).await;
    }
    stamp_note(&db, "test_cases", unlinked).await;
    stamp_note(&db, "jobs", job).await;

    issue_links::store_filed_link(db.connection(), EntityKind::Test, i64::from(filed), 11)
        .await
        .expect("the filer stores what GitHub created");
    assert!(!noted(&db, "test_cases", filed).await);

    issue_links::link_issue(
        db.connection(),
        EntityKind::Test,
        i64::from(relinked),
        22,
        None,
        false,
        &ReadSettings::default(),
    )
    .await
    .expect("the number from elsewhere links");
    assert!(!noted(&db, "test_cases", relinked).await);

    issue_links::link_issue(
        db.connection(),
        EntityKind::Job,
        i64::from(job),
        41,
        None,
        true,
        &ReadSettings::default(),
    )
    .await
    .expect("the replaced Job link writes");
    assert_eq!(job_link(&db, job).await.0, Some(41));
    assert!(!noted(&db, "jobs", job).await);

    issue_links::unlink_issue(
        db.connection(),
        EntityKind::Test,
        i64::from(unlinked),
        &ReadSettings::default(),
    )
    .await
    .expect("the unlink succeeds");
    assert_eq!(test_link(&db, unlinked).await.0, None);
    assert!(
        !noted(&db, "test_cases", unlinked).await,
        "with no Issue a stamp would note nothing"
    );
}
