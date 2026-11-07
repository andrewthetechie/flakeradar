//! The fixed-note batch, its close, and every link write that clears a stamp.

use std::sync::Arc;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, FixedOffset, TimeZone as _, Utc};
use flakeradar::github::{self, Api};
use flakeradar::settings::Secret;
use flakeradar::settings::Settings;
use flakeradar_core::model::EntityKind;
use flakeradar_db::clock::SharedClock;
use flakeradar_db::issue_links::{self, linked_for_check};
use flakeradar_db::reads::ReadSettings;
use flakeradar_test_support::{FixedClock, TestDb, make_job, make_pipeline, system_clock};
use sea_orm::ConnectionTrait as _;
use sea_orm::{DbBackend, Statement, Value};
use serde_json::{Value as Json, json};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use crate::support::{
    CREATED, CapturedLog, Loopback, TOKEN, api, clean_streak_of, fixed_settings, flaky_job,
    flaky_test, flaky_test_in, gh_settings, lines_with, link_of, link_table, mount_comment,
    mount_create, mount_issue, mount_patch, mount_state, note_row, noted, requested_calls,
    requested_paths, set_clean_streak, stage_mid_request, stamp, stamp_link, stamp_note, sync,
    sync_with, unstopped,
};

/// One Test's stored fixed-note stamp.
async fn noted_at(db: &TestDb, id: i32) -> Option<DateTime<FixedOffset>> {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT github_issue_fixed_noted_at FROM test_cases WHERE id = $1",
            [Value::from(id)],
        ))
        .await
        .expect("the stamp reads")
        .expect("the Test exists");
    row.try_get("", "github_issue_fixed_noted_at")
        .expect("the stamp is a timestamp")
}

#[tokio::test]
async fn a_fixed_note_is_stamped_with_the_clock_it_was_given() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(12).checked()).await;
    set_clean_streak(&db, "test_cases", test, 5).await;
    let server = MockServer::start().await;
    mount_comment(&server, "acme/app", 12, ResponseTemplate::new(201)).await;
    let instant = Utc
        .with_ymd_and_hms(2001, 2, 3, 4, 5, 6)
        .unwrap()
        .fixed_offset();
    let clock: SharedClock = Arc::new(FixedClock::new(instant));

    let report = github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[test],
        &fixed_settings(&[]),
        &api(&server),
        &clock,
        &unstopped(),
    )
    .await;

    assert_eq!(report.stopped, None, "the batch ran to its end");
    assert_eq!(
        noted_at(&db, test).await,
        Some(instant),
        "the note was stamped with the clock the batch was given"
    );
}

#[tokio::test]
async fn the_fixed_note_comments_once_with_the_configured_sentence() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(12).checked()).await;
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
    github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[test],
        &settings,
        &api(&server),
        &system_clock(),
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
    github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[test],
        &settings,
        &api(&server),
        &system_clock(),
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
    github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[test],
        &settings,
        &api(&server),
        &system_clock(),
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
    stamp_link(&db, EntityKind::Test, test, stamp().number(12).checked()).await;
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

    github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[test],
        &fixed_settings(&[]),
        &api(&server),
        &system_clock(),
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
    stamp_link(&db, EntityKind::Test, test, stamp().number(12).checked()).await;
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
    github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[test],
        &fixed_settings(&[]),
        &api(&server),
        &system_clock(),
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
    github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[test],
        &settings,
        &api(&server),
        &system_clock(),
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
    stamp_link(&db, EntityKind::Test, test, stamp().number(12).checked()).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET clean_streak = 9 WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the streak writes");
    let server = MockServer::start().await;

    github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[test],
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
}

#[tokio::test]
async fn a_job_issue_is_noted_and_closed_with_the_same_wording() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job(&db).await;
    stamp_link(&db, EntityKind::Job, job, stamp().number(77).checked()).await;
    set_clean_streak(&db, "jobs", job, 3).await;
    let server = MockServer::start().await;
    mount_comment(&server, "acme/app", 77, ResponseTemplate::new(201)).await;
    mount_patch(&server, "acme/app", 77, ResponseTemplate::new(200)).await;
    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);

    github::note_fixed_for(
        db.connection(),
        EntityKind::Job,
        &[job],
        &settings,
        &api(&server),
        &system_clock(),
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
    assert_eq!(link_of(&db, EntityKind::Job, job).await.number, Some(77));
}

#[tokio::test]
async fn a_note_ignores_a_row_with_no_live_note() {
    let db = TestDb::new().await.expect("a test database");
    let short = flaky_test_in(&db, "acme/app", "short", "short").await;
    stamp_link(&db, EntityKind::Test, short, stamp().number(71).checked()).await;
    set_clean_streak(&db, "test_cases", short, 2).await;
    let unlinked = flaky_test_in(&db, "acme/app", "unlinked", "unlinked").await;
    set_clean_streak(&db, "test_cases", unlinked, 9).await;
    let declined = flaky_test_in(&db, "acme/app", "declined", "declined").await;
    stamp_link(
        &db,
        EntityKind::Test,
        declined,
        stamp().number(72).checked().closed(),
    )
    .await;
    set_clean_streak(&db, "test_cases", declined, 9).await;
    let server = MockServer::start().await;

    github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[short, unlinked, declined],
        &fixed_settings(&[]),
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
            .is_empty(),
        "below the streak, unlinked and declined are all silent"
    );
    assert!(!noted(&db, "test_cases", declined).await);

    let at_streak = flaky_test_in(&db, "acme/app", "streak", "streak").await;
    stamp_link(
        &db,
        EntityKind::Test,
        at_streak,
        stamp().number(73).checked(),
    )
    .await;
    set_clean_streak(&db, "test_cases", at_streak, 9).await;
    let off = fixed_settings(&[("FLAKERADAR_GITHUB_FIXED_STREAK", "0")]);
    github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[at_streak],
        &off,
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
        .create(&db)
        .await;
    let job = make_job(&pipeline)
        .name("unit")
        .flakiness_score(0.5)
        .create(&db)
        .await;
    stamp_link(&db, EntityKind::Job, job.id, stamp().number(77).checked()).await;
    set_clean_streak(&db, "jobs", job.id, 3).await;
    let server = MockServer::start().await;

    github::note_fixed_for(
        db.connection(),
        EntityKind::Job,
        &[job.id],
        &fixed_settings(&[]),
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
            .is_empty(),
        "only a github Pipeline has an issue to note"
    );
}

#[tokio::test]
async fn a_note_close_that_fails_keeps_the_stamp() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test_in(&db, "acme/app", "linked", "linked").await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(77).checked()).await;
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

    let report = github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[test],
        &settings,
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues/77/comments",
            "/repos/acme/app/issues/77"
        ]
    );
    assert!(noted(&db, "test_cases", test).await);
    assert!(
        !link_of(&db, EntityKind::Test, test).await.closed,
        "the close failed, so the issue is still open"
    );
    assert_eq!(
        (report.stopped, report.skipped.len()),
        (None, 0),
        "a 500 on one close is neither a stop nor a skipped Repo"
    );

    github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[test],
        &settings,
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&server).await.len(),
        2,
        "a failed close must not buy a second comment"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn stopping_waits_for_the_close_and_reads_its_answer() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test_in(&db, "acme/app", "linked", "linked").await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(77).checked()).await;
    set_clean_streak(&db, "test_cases", test, 3).await;
    let (arrived, arrived_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let arrived = Mutex::new(Some(arrived));
    let release_rx = Mutex::new(release_rx);
    let server = MockServer::start().await;
    mount_comment(&server, "acme/app", 77, ResponseTemplate::new(201)).await;
    // The close is held open, so the stop lands while its answer is still on the
    // way. It answers a rate limit, which is a verdict the batch can only report
    // by reading the answer: a close whose future was dropped reports the stop
    // instead.
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/77"))
        .respond_with(move |_request: &Request| {
            let waiting = arrived
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(arrived) = waiting {
                let _ = arrived.send(());
            }
            let _ = release_rx
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .recv_timeout(Duration::from_secs(30));
            ResponseTemplate::new(429).insert_header("x-ratelimit-remaining", "0")
        })
        .mount(&server)
        .await;

    let stop = CancellationToken::new();
    let stopping = stop.clone();
    let closing = {
        let connection = db.connection().clone();
        let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);
        let api = api(&server);
        let ids = vec![test];
        tokio::spawn(async move {
            github::note_fixed_for(
                &connection,
                EntityKind::Test,
                &ids,
                &settings,
                &api,
                &system_clock(),
                &stopping,
            )
            .await
        })
    };
    arrived_rx.await.expect("the close PATCH is in flight");
    stop.cancel();
    release_tx.send(()).expect("the stub is released");
    let report = closing.await.expect("the batch finishes");

    // The stamp is written before the close, and dropping the close would leave a
    // fixed issue open with nobody to retry it, so the PATCH runs through its
    // answer and the batch stops after it.
    assert!(
        noted(&db, "test_cases", test).await,
        "the comment landed and is recorded"
    );
    assert_eq!(
        report.stopped,
        Some(github::BatchStop::RateLimited),
        "the close's own answer ended the batch, so it was read rather than dropped"
    );
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues/77/comments",
            "/repos/acme/app/issues/77"
        ]
    );
}

#[tokio::test]
async fn a_note_rate_limit_stops_the_batch() {
    for (status, headers) in [(429, None), (403, Some(("x-ratelimit-remaining", "0")))] {
        let db = TestDb::new().await.expect("a test database");
        let first = flaky_test_in(&db, "acme/a", "a", "a").await;
        let second = flaky_test_in(&db, "acme/b", "b", "b").await;
        stamp_link(&db, EntityKind::Test, first, stamp().number(71).checked()).await;
        stamp_link(&db, EntityKind::Test, second, stamp().number(72).checked()).await;
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

        github::note_fixed_for(
            db.connection(),
            EntityKind::Test,
            &[first, second],
            &fixed_settings(&[]),
            &api(&server),
            &system_clock(),
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
    stamp_link(&db, EntityKind::Test, first, stamp().number(71).checked()).await;
    stamp_link(&db, EntityKind::Test, second, stamp().number(72).checked()).await;
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
    let report = github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[first, second],
        &settings,
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/a/issues/71/comments",
            "/repos/acme/a/issues/71"
        ],
    );
    assert!(noted(&db, "test_cases", first).await, "the comment landed");
    assert!(!noted(&db, "test_cases", second).await);
    assert_eq!(
        report.stopped,
        Some(github::BatchStop::RateLimited),
        "the 429 on a close ends the batch"
    );
}

#[tokio::test]
async fn a_note_permission_403_skips_only_that_repo() {
    let db = TestDb::new().await.expect("a test database");
    let a1 = flaky_test_in(&db, "acme/a", "a1", "a1").await;
    let a2 = flaky_test_in(&db, "acme/a", "a2", "a2").await;
    let other = flaky_test_in(&db, "acme/b", "b", "b").await;
    stamp_link(&db, EntityKind::Test, a1, stamp().number(71).checked()).await;
    stamp_link(&db, EntityKind::Test, a2, stamp().number(73).checked()).await;
    stamp_link(&db, EntityKind::Test, other, stamp().number(72).checked()).await;
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

    github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[a1, other, a2],
        &fixed_settings(&[]),
        &api(&server),
        &system_clock(),
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
    stamp_link(&db, EntityKind::Test, a, stamp().number(71).checked()).await;
    stamp_link(&db, EntityKind::Test, b, stamp().number(72).checked()).await;
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
    let report = github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[a, b],
        &settings,
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
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
    assert_eq!(
        report.skipped,
        std::collections::BTreeSet::from(["acme/a".to_owned()]),
        "only the Repo whose close was denied is skipped"
    );
}

#[tokio::test]
async fn a_note_network_error_ends_the_batch() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test_in(&db, "acme/app", "linked", "linked").await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(77).checked()).await;
    set_clean_streak(&db, "test_cases", test, 3).await;
    // Nothing listens on this port.
    let api = Api::new(Secret::new(TOKEN), "http://127.0.0.1:1").expect("the HTTP client builds");

    let report = github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[test],
        &fixed_settings(&[]),
        &api,
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert!(!noted(&db, "test_cases", test).await);
    assert_eq!(
        report.stopped,
        Some(github::BatchStop::Unreachable),
        "the batch ends at the endpoint it could not reach"
    );
}

#[tokio::test]
async fn a_transport_error_on_a_fixed_note_close_ends_the_batch() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/a", "linked", "linked").await;
    let second = flaky_test_in(&db, "acme/b", "other", "other").await;
    stamp_link(&db, EntityKind::Test, first, stamp().number(71).checked()).await;
    stamp_link(&db, EntityKind::Test, second, stamp().number(72).checked()).await;
    set_clean_streak(&db, "test_cases", first, 3).await;
    set_clean_streak(&db, "test_cases", second, 3).await;
    // The first comment lands, then every later connection is dropped: the close
    // of the first issue is the transport failure that ends the batch.
    let loopback = Loopback::answering(CREATED.to_owned()).await;
    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);

    let report = github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[first, second],
        &settings,
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
    stamp_link(&db, EntityKind::Test, test, stamp().number(77).checked()).await;
    set_clean_streak(&db, "test_cases", test, 3).await;
    // The comment lands, then the close's connection is dropped before it answers.
    let loopback = Loopback::answering(CREATED.to_owned()).await;
    let api = Api::new(Secret::new(TOKEN), &loopback.base).expect("the HTTP client builds");
    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);

    let report = github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[test],
        &settings,
        &api,
        &system_clock(),
        &unstopped(),
    )
    .await;

    assert_eq!(
        loopback.accepted(),
        2,
        "the comment landed and the close was attempted"
    );
    assert_eq!(
        report.stopped,
        Some(github::BatchStop::Unreachable),
        "the batch ends at the close it could not reach"
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
    stamp_link(&db, EntityKind::Test, bad, stamp().number(71).checked()).await;
    stamp_link(&db, EntityKind::Test, other, stamp().number(72).checked()).await;
    set_clean_streak(&db, "test_cases", bad, 3).await;
    set_clean_streak(&db, "test_cases", other, 3).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string("nope"))
        .mount(&server)
        .await;

    let report = github::note_fixed_for(
        db.connection(),
        EntityKind::Test,
        &[bad, other],
        &fixed_settings(&[]),
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
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
        report.skipped,
        std::collections::BTreeSet::new(),
        "a 500 is nobody's Repo-wide fault"
    );
}

/// The fixed-note entry point of one entity.
async fn note_entity(db: &TestDb, kind: EntityKind, ids: &[i32], settings: &Settings, api: &Api) {
    note_entity_value(db, kind, ids, settings, api).await;
}

/// The same, answering what the batch reported.
async fn note_entity_value(
    db: &TestDb,
    kind: EntityKind,
    ids: &[i32],
    settings: &Settings,
    api: &Api,
) -> github::BatchReport {
    match kind {
        EntityKind::Test => {
            github::note_fixed_for(
                db.connection(),
                EntityKind::Test,
                ids,
                settings,
                api,
                &system_clock(),
                &unstopped(),
            )
            .await
        }
        EntityKind::Job => {
            github::note_fixed_for(
                db.connection(),
                EntityKind::Job,
                ids,
                settings,
                api,
                &system_clock(),
                &unstopped(),
            )
            .await
        }
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
    assert_eq!(
        link_of(&db, EntityKind::Test, flagged).await.number,
        None,
        "no number is kept"
    );
    assert_eq!(
        link_of(&db, EntityKind::Test, flagged).await.filed,
        None,
        "an unlinked row is nobody's"
    );
    assert!(
        !link_of(&db, EntityKind::Test, flagged).await.closed,
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
        link_of(&db, EntityKind::Test, relinked).await.number,
        Some(44),
        "a fresh link is stored"
    );
    assert_eq!(
        link_of(&db, EntityKind::Test, relinked).await.filed,
        Some(true),
        "ours to close"
    );
    assert!(
        !link_of(&db, EntityKind::Test, relinked).await.closed,
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
    sync_with(&db, &settings, &server).await;
    assert!(
        link_of(&db, EntityKind::Job, job).await.checked(),
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
    assert_eq!(link_of(&db, EntityKind::Job, job).await.number, Some(44));
    assert!(
        !link_of(&db, EntityKind::Job, job).await.checked(),
        "a replaced link is checked first"
    );
    sync_with(&db, &settings, &server).await;
    assert!(
        link_of(&db, EntityKind::Job, job).await.checked(),
        "the new link is checked"
    );

    issue_links::unlink_issue(db.connection(), EntityKind::Job, i64::from(job), &reads)
        .await
        .expect("the unlink succeeds");
    assert_eq!(
        link_of(&db, EntityKind::Job, job).await.number,
        None,
        "no number is left"
    );
    assert!(
        !link_of(&db, EntityKind::Job, job).await.checked(),
        "with no number the stamp has nothing to rotate"
    );
}

/// The scenario of `a_deleted_row_costs_only_that_fixed_note`, once per entity.
async fn a_deleted_row_costs_only_that_fixed_note(kind: EntityKind) {
    let db = TestDb::new().await.expect("a test database");
    let doomed = note_row(&db, kind, "acme/app", "doomed", Some(77), 3).await;
    let kept = note_row(&db, kind, "acme/b", "kept", Some(78), 3).await;
    let server = MockServer::start().await;
    let table = link_table(kind);
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

    note_entity(&db, kind, &[doomed, kept], &settings, &api(&server)).await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues/77/comments",
            "/repos/acme/b/issues/78/comments"
        ],
        "the vanished row costs one note, not the batch"
    );
    // The row is gone, so the only thing left to read is the one that survived.
    assert!(noted(&db, table, kept).await, "the surviving note lands");
}

#[tokio::test]
async fn a_deleted_row_costs_only_that_fixed_note_for_a_test() {
    a_deleted_row_costs_only_that_fixed_note(EntityKind::Test).await;
}

#[tokio::test]
async fn a_deleted_row_costs_only_that_fixed_note_for_a_job() {
    a_deleted_row_costs_only_that_fixed_note(EntityKind::Job).await;
}

/// The scenario of `a_clean_streak_reset_mid_post_is_not_undone_by_the_stamp`, once per entity.
async fn a_clean_streak_reset_mid_post_is_not_undone_by_the_stamp(kind: EntityKind) {
    let db = TestDb::new().await.expect("a test database");
    let obj = note_row(&db, kind, "acme/app", "linked", Some(77), 3).await;
    let server = MockServer::start().await;
    let table = link_table(kind);
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

    let (report, logged) = CapturedLog::during_call(Box::pin(note_entity_value(
        &db,
        kind,
        &[obj],
        &settings,
        &api(&server),
    )))
    .await;
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
    assert_eq!(
        report.skipped,
        std::collections::BTreeSet::new(),
        "a stamp that could not land costs no Repo"
    );
    // The stamp's refusal names the streak it found, which no returned value
    // carries, so the line stays a log assertion.
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
    note_entity(&db, kind, &[obj], &settings, &api(&second)).await;
    assert_eq!(
        requested_calls(&second).await,
        vec![
            (
                "POST".to_owned(),
                "/repos/acme/app/issues/77/comments".to_owned()
            ),
            ("PATCH".to_owned(), "/repos/acme/app/issues/77".to_owned())
        ],
        "the re-armed note is commented on and closed once"
    );
    assert!(noted(&db, table, obj).await, "the fresh note stamps");
}

#[tokio::test]
async fn a_clean_streak_reset_mid_post_is_not_undone_by_the_stamp_for_a_test() {
    a_clean_streak_reset_mid_post_is_not_undone_by_the_stamp(EntityKind::Test).await;
}

#[tokio::test]
async fn a_clean_streak_reset_mid_post_is_not_undone_by_the_stamp_for_a_job() {
    a_clean_streak_reset_mid_post_is_not_undone_by_the_stamp(EntityKind::Job).await;
}

#[tokio::test]
async fn an_unlink_clears_both_halves_and_a_rearm_keeps_its_stamp() {
    let db = TestDb::new().await.expect("a test database");
    let unlinked = flaky_test_in(&db, "acme/app", "p1", "t1").await;
    let rearmed = flaky_test_in(&db, "acme/app", "p2", "t2").await;
    stamp_link(&db, EntityKind::Test, unlinked, stamp().number(11)).await;
    stamp_link(&db, EntityKind::Test, rearmed, stamp().number(22)).await;

    issue_links::unlink_issue(
        db.connection(),
        EntityKind::Test,
        i64::from(unlinked),
        &ReadSettings::default(),
    )
    .await
    .expect("the unlink succeeds");
    let link = link_of(&db, EntityKind::Test, unlinked).await;
    assert_eq!(link.number, None, "unlink drops the number");
    assert_eq!(link.filed, None, "and the provenance");
    assert!(!link.checked(), "and the check stamp");
    assert!(!link.closed);

    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(json!({"state": "closed", "state_reason": "completed"})),
    )
    .await;
    sync(&db, &server).await;
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/app/issues/22"],
        "the unlinked row is out of the rotation"
    );
    let link = link_of(&db, EntityKind::Test, rearmed).await;
    assert_eq!(link.number, None, "the re-arm clears the number");
    assert!(link.checked(), "and keeps the stamp it just wrote");
    assert!(
        !link_of(&db, EntityKind::Test, unlinked).await.checked(),
        "and leaves the unlinked row alone"
    );
}

/// The scenario of `an_unlink_after_a_rearm_clears_the_kept_stamp`, once per entity.
async fn an_unlink_after_a_rearm_clears_the_kept_stamp(kind: EntityKind) {
    let db = TestDb::new().await.expect("a test database");
    let (id, number) = match kind {
        EntityKind::Test => {
            let id = flaky_test(&db).await;
            stamp_link(&db, EntityKind::Test, id, stamp().number(300)).await;
            (id, 300)
        }
        EntityKind::Job => {
            let id = flaky_job(&db).await;
            stamp_link(&db, EntityKind::Job, id, stamp().number(302)).await;
            (id, 302)
        }
    };
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({"state": "closed"})),
    )
    .await;
    sync(&db, &server).await;
    assert_eq!(
        requested_paths(&server).await,
        vec![format!("/repos/acme/app/issues/{number}")],
        "the linked {kind} is what the sync checks"
    );
    let rearmed = link_of(&db, kind, id).await;
    assert_eq!(rearmed.number, None, "the re-arm clears the number");
    assert!(rearmed.checked(), "the re-arm keeps the stamp it wrote");

    issue_links::unlink_issue(
        db.connection(),
        kind,
        i64::from(id),
        &ReadSettings::default(),
    )
    .await
    .expect("the unlink succeeds");
    let cleared = link_of(&db, kind, id).await;
    assert_eq!(cleared.filed, None, "unlink drops the provenance");
    assert_eq!(cleared.number, None, "unlink drops the number");
    assert!(!cleared.checked(), "unlink drops the stamp the re-arm kept");
    assert!(
        linked_for_check(db.connection(), kind, 10)
            .await
            .expect("the rotation reads")
            .is_empty(),
        "the pair is fully reset"
    );

    let before = requested_paths(&server).await.len();
    sync(&db, &server).await;
    assert_eq!(
        requested_paths(&server).await.len(),
        before,
        "the unlinked {kind} is out of the rotation for good"
    );
}

#[tokio::test]
async fn an_unlink_after_a_rearm_clears_the_kept_stamp_for_a_test() {
    an_unlink_after_a_rearm_clears_the_kept_stamp(EntityKind::Test).await;
}

#[tokio::test]
async fn an_unlink_after_a_rearm_clears_the_kept_stamp_for_a_job() {
    an_unlink_after_a_rearm_clears_the_kept_stamp(EntityKind::Job).await;
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
    let link = link_of(&db, EntityKind::Test, test).await;
    assert_eq!(link.number, None);
    assert_eq!(link.filed, None, "no provenance without a number");
    assert!(!link.closed, "the declined flag goes with the link");

    issue_links::unlink_issue(
        db.connection(),
        EntityKind::Job,
        i64::from(job),
        &ReadSettings::default(),
    )
    .await
    .expect("the Job unlink succeeds");
    let link = link_of(&db, EntityKind::Job, job).await;
    assert_eq!(link.number, None);
    assert_eq!(link.filed, None);

    let filing = MockServer::start().await;
    mount_create(
        &filing,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;
    github::file_for(
        db.connection(),
        EntityKind::Test,
        &[test],
        &gh_settings(&[]),
        &api(&filing),
        &system_clock(),
        &unstopped(),
    )
    .await;
    let link = link_of(&db, EntityKind::Test, test).await;
    assert_eq!(link.number, Some(77), "the filer files the re-armed Test");
    assert!(!link.closed, "a fresh link starts unflagged");

    let resync = MockServer::start().await;
    mount_state(
        &resync,
        ResponseTemplate::new(200).set_body_json(json!({"state": "open"})),
    )
    .await;
    sync(&db, &resync).await;
    assert_eq!(
        requested_paths(&resync).await,
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
    stamp_link(&db, EntityKind::Job, job, stamp().number(40).checked()).await;
    for id in [filed, relinked] {
        stamp_note(&db, EntityKind::Test, id).await;
    }
    stamp_note(&db, EntityKind::Test, unlinked).await;
    stamp_note(&db, EntityKind::Job, job).await;

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
    assert_eq!(link_of(&db, EntityKind::Job, job).await.number, Some(41));
    assert!(!noted(&db, "jobs", job).await);

    issue_links::unlink_issue(
        db.connection(),
        EntityKind::Test,
        i64::from(unlinked),
        &ReadSettings::default(),
    )
    .await
    .expect("the unlink succeeds");
    assert_eq!(link_of(&db, EntityKind::Test, unlinked).await.number, None);
    assert!(
        !noted(&db, "test_cases", unlinked).await,
        "with no Issue a stamp would note nothing"
    );
}
