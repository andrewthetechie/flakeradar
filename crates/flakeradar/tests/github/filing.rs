//! Filing a batch of issues: the gates, the statuses and the stop rules.

use flakeradar::github::{self, Api};
use flakeradar::settings::Secret;
use flakeradar::settings::Settings;
use flakeradar_core::model::EntityKind;
use flakeradar_test_support::{
    TestDb, make_execution, make_job, make_job_execution, make_pipeline, make_project, make_run,
    make_test_case, system_clock,
};
use sea_orm::ConnectionTrait as _;
use sea_orm::EntityTrait as _;
use sea_orm::{DbBackend, Statement, Value};
use serde_json::{Value as Json, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{
    CapturedLog, TOKEN, api, explain_job_execution, fail_link_stores, fixed_settings, flaky_job,
    flaky_job_in, flaky_test, flaky_test_in, gh_settings, lines_with, link_of, mount_create,
    requested_paths, stamp, stamp_link, unstopped,
};

#[tokio::test]
async fn no_token_is_a_silent_no_op() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let server = MockServer::start().await;
    let settings = Settings::from_map(&std::collections::BTreeMap::new()).expect("the defaults");
    let api = Api::new(Secret::new(""), &server.uri()).expect("the HTTP client builds");

    let filed =
        github::file_issues_for(db.connection(), &[test], &settings, &api, &unstopped()).await;
    assert!(filed.linked.is_empty());
    assert_eq!(link_of(&db, EntityKind::Test, test).await.number, None);
    assert!(
        server
            .received_requests()
            .await
            .expect("a record")
            .is_empty(),
        "the row scores below the gate, so nothing is filed"
    );
}

#[tokio::test]
async fn a_flaky_test_is_filed_in_its_own_repo_with_its_evidence() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 7})),
    )
    .await;
    let settings = gh_settings(&[]);

    let filed = github::file_issues_for(
        db.connection(),
        &[test],
        &settings,
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(filed.linked.into_iter().collect::<Vec<_>>(), vec![test]);
    let link = link_of(&db, EntityKind::Test, test).await;
    assert_eq!(link.number, Some(7));
    assert_eq!(link.filed, Some(true), "a filed link is FlakeRadar's own");
    assert!(!link.checked());
    assert!(!link.closed);

    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    let body: Json = serde_json::from_slice(&request.body).expect("the body is JSON");
    assert_eq!(
        body["title"],
        json!("[FlakeRadar] Flaky test: tests.test_cart::test_total")
    );
    assert_eq!(body["labels"], json!(["flakeradar"]));
    let body_text = body["body"].as_str().expect("a body").to_owned();
    for expected in [
        "FlakeRadar detected a flaky test: `tests.test_cart::test_total`",
        "- **Repo / project:** `acme/app` / `backend`",
        "- **Location:** `backend/tests/test_cart.py` line 12",
        "- **Permalink (last failing commit):** https://github.com/acme/app/blob/abcdef1234567890/backend/tests/test_cart.py#L12",
        "- **Flakiness score:** 0.50 (filed at ≥ 0.30)",
        "- **Proven flakes (same commit failed *and* passed):** 2",
        "- **Suite / classname:** `unit` / `tests.test_cart`",
        "- **Last failing commit:** `abcdef1234567890` on branch `main`",
        "pytest backend/tests/test_cart.py::test_total",
        "| failed | `abcdef1234` | main |",
        "Traceback (most recent call last):",
        "_Fingerprint: `abc123`_",
    ] {
        assert!(
            body_text.contains(expected),
            "the body carries {expected:?}: {body_text}"
        );
    }
    let authorization = request
        .headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .expect("the request carries the token");
    assert_eq!(authorization, format!("Bearer {TOKEN}"));
    assert_eq!(
        request
            .headers
            .get("accept")
            .and_then(|value| value.to_str().ok()),
        Some("application/vnd.github+json")
    );
    assert_eq!(
        request
            .headers
            .get("x-github-api-version")
            .and_then(|value| value.to_str().ok()),
        Some("2022-11-28")
    );
    let user_agent = request
        .headers
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .expect("GitHub requires a user agent");
    assert!(user_agent.starts_with("flakeradar/"), "{user_agent}");
}

#[tokio::test]
async fn a_test_below_the_score_gate_is_not_filed() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 7})),
    )
    .await;
    let settings = gh_settings(&[("FLAKERADAR_GITHUB_ISSUE_MIN_SCORE", "0.9")]);

    let filed = github::file_issues_for(
        db.connection(),
        &[test],
        &settings,
        &api(&server),
        &unstopped(),
    )
    .await;
    assert!(filed.linked.is_empty());
    assert!(
        server
            .received_requests()
            .await
            .expect("a record")
            .is_empty()
    );
}

#[tokio::test]
async fn the_proven_flake_gate_and_the_failure_gate_each_withhold_a_filing() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 7})),
    )
    .await;

    let flakes = gh_settings(&[("FLAKERADAR_GITHUB_ISSUE_MIN_PROVEN_FLAKES", "3")]);
    assert!(
        github::file_issues_for(
            db.connection(),
            &[test],
            &flakes,
            &api(&server),
            &unstopped()
        )
        .await
        .linked
        .is_empty()
    );
    let failures = gh_settings(&[("FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES", "2")]);
    assert!(
        github::file_issues_for(
            db.connection(),
            &[test],
            &failures,
            &api(&server),
            &unstopped()
        )
        .await
        .linked
        .is_empty()
    );
    // One failure in the window meets a gate of one.
    let one = gh_settings(&[("FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES", "1")]);
    assert_eq!(
        github::file_issues_for(db.connection(), &[test], &one, &api(&server), &unstopped())
            .await
            .linked
            .len(),
        1
    );
}

#[tokio::test]
async fn a_link_a_human_made_is_never_refiled() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, EntityKind::Test, test, stamp().number(12).checked()).await;
    let server = MockServer::start().await;
    let settings = gh_settings(&[]);

    let filed = github::file_issues_for(
        db.connection(),
        &[test],
        &settings,
        &api(&server),
        &unstopped(),
    )
    .await;
    assert!(
        filed.linked.is_empty(),
        "a row that carries a number is already linked"
    );
    assert!(
        server
            .received_requests()
            .await
            .expect("a record")
            .is_empty()
    );
}

#[tokio::test]
async fn a_row_at_the_fixed_note_streak_has_no_live_flake_to_file() {
    let db = TestDb::new().await.expect("a test database");
    let at_streak = flaky_test(&db).await;
    let below = flaky_test_in(&db, "acme/app", "p2", "below").await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET clean_streak = 3 WHERE id = $1",
            [Value::from(at_streak)],
        ))
        .await
        .expect("the streak writes");
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 101})),
    )
    .await;
    let settings = fixed_settings(&[]);

    let filed = github::file_issues_for(
        db.connection(),
        &[at_streak, below],
        &settings,
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(
        filed.linked.into_iter().collect::<Vec<_>>(),
        vec![below],
        "a row whose fix holds answers with a note, not an Issue"
    );
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/app/issues"]
    );
    assert_eq!(
        link_of(&db, EntityKind::Test, at_streak).await.number,
        None,
        "no Issue to comment the fix on"
    );
    assert_eq!(
        link_of(&db, EntityKind::Test, below).await.number,
        Some(101)
    );
}

#[tokio::test]
async fn a_rate_limit_stops_the_batch_without_storing_anything() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test(&db).await;
    let project = flakeradar_db::entities::projects::Entity::find_by_id(1)
        .one(db.connection())
        .await
        .expect("the Project reads")
        .expect("the Project exists");
    let second = make_test_case(&project)
        .name("test_second")
        .flakiness_score(0.5)
        .build(db.connection())
        .await
        .expect("the second Test inserts");
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(403)
            .insert_header("x-ratelimit-remaining", "0")
            .set_body_json(json!({"message": "API rate limit exceeded"})),
    )
    .await;

    let filed = github::file_issues_for(
        db.connection(),
        &[first, second.id],
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert!(filed.linked.is_empty());
    assert_eq!(
        server.received_requests().await.expect("a record").len(),
        1,
        "a rate limit stops the batch after the first attempt"
    );
    assert_eq!(link_of(&db, EntityKind::Test, first).await.number, None);
}

#[tokio::test]
async fn a_permission_403_skips_that_repo_and_files_the_other_one() {
    let db = TestDb::new().await.expect("a test database");
    let denied = flaky_test(&db).await;
    let other_project = make_project()
        .repo("other/repo")
        .project("default")
        .build(db.connection())
        .await
        .expect("the second Project inserts");
    let allowed = make_test_case(&other_project)
        .name("test_other")
        .flakiness_score(0.5)
        .build(db.connection())
        .await
        .expect("the second Test inserts");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(json!({"message": "Resource not accessible"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/other/repo/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 9})))
        .mount(&server)
        .await;

    let filed = github::file_issues_for(
        db.connection(),
        &[denied, allowed.id],
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(
        filed.linked.into_iter().collect::<Vec<_>>(),
        vec![allowed.id]
    );
    assert_eq!(link_of(&db, EntityKind::Test, denied).await.number, None);
}

/// A bad-credentials 401 is Repo-scoped for filing, like a permission 403.
///
/// The token is refused for every Repo the same way, so a second candidate in that
/// Repo is never posted to.
#[tokio::test]
async fn a_bad_credentials_401_skips_that_repo_and_files_the_other_one() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/app", "p1", "a1").await;
    let second = flaky_test_in(&db, "acme/app", "p2", "a2").await;
    let allowed = flaky_test_in(&db, "other/repo", "default", "b").await;
    let server = MockServer::start().await;
    // Exactly one POST to the refused Repo: the second candidate is skipped, not
    // tried and failed again.
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(json!({"message": "Bad credentials"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/other/repo/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 9})))
        .mount(&server)
        .await;

    let filed = github::file_issues_for(
        db.connection(),
        &[first, second, allowed],
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(filed.linked.into_iter().collect::<Vec<_>>(), vec![allowed]);
    assert_eq!(link_of(&db, EntityKind::Test, first).await.number, None);
    assert_eq!(link_of(&db, EntityKind::Test, second).await.number, None);
}

#[tokio::test]
async fn issues_disabled_skips_the_rest_of_that_repo() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let server = MockServer::start().await;
    mount_create(&server, ResponseTemplate::new(410)).await;

    assert!(
        github::file_issues_for(
            db.connection(),
            &[test],
            &gh_settings(&[]),
            &api(&server),
            &unstopped()
        )
        .await
        .linked
        .is_empty()
    );
    assert_eq!(link_of(&db, EntityKind::Test, test).await.number, None);
}

#[tokio::test]
async fn a_201_without_a_number_never_raises_and_stores_nothing() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_string("not json"),
    )
    .await;

    assert!(
        github::file_issues_for(
            db.connection(),
            &[test],
            &gh_settings(&[]),
            &api(&server),
            &unstopped()
        )
        .await
        .linked
        .is_empty()
    );
    assert_eq!(link_of(&db, EntityKind::Test, test).await.number, None);
}

#[tokio::test]
async fn a_row_deleted_while_filing_closes_the_issue_it_just_created() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 7})),
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/7/comments"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/7"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    // The admin delete wins the race: by the time the filer stores its number the
    // row it selected no longer takes the write, so the UPDATE affects no row. A
    // trigger that suppresses the update is how the loser's statement behaves.
    db.connection()
        .execute_raw(Statement::from_string(
            DbBackend::Postgres,
            "CREATE FUNCTION drop_link() RETURNS trigger AS $$ BEGIN RETURN NULL; END; $$ \
             LANGUAGE plpgsql"
                .to_owned(),
        ))
        .await
        .expect("the vanish trigger installs");
    db.connection()
        .execute_raw(Statement::from_string(
            DbBackend::Postgres,
            "CREATE TRIGGER drop_link BEFORE UPDATE ON test_cases \
             FOR EACH ROW EXECUTE FUNCTION drop_link()"
                .to_owned(),
        ))
        .await
        .expect("the vanish trigger installs");

    let filed = github::file_issues_for(
        db.connection(),
        &[test],
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert!(filed.linked.is_empty(), "a vanished row is not a link");
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues",
            "/repos/acme/app/issues/7/comments",
            "/repos/acme/app/issues/7",
        ],
        "the new issue is commented on, then closed"
    );
}

#[tokio::test]
async fn a_store_failure_leaves_the_issue_open_and_stops_the_batch() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/app", "p1", "one").await;
    let second = flaky_test_in(&db, "acme/app", "p2", "two").await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 7})),
    )
    .await;
    // The store itself fails: a trigger that raises is what a broken write does, and
    // a savepoint cannot save a statement the server refuses.
    fail_link_stores(&db, "test_cases").await;

    let (filed, logged) = CapturedLog::during_call(github::file_issues_for(
        db.connection(),
        &[first, second],
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    ))
    .await;
    assert!(filed.linked.is_empty(), "a failed store links nothing");
    assert_eq!(
        filed.stopped,
        Some(github::BatchStop::StoreFailed),
        "the batch reports why it ended"
    );
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/app/issues"],
        "the issue that was created stays open: no comment and no close"
    );
    assert_eq!(link_of(&db, EntityKind::Test, first).await.number, None);
    assert_eq!(link_of(&db, EntityKind::Test, second).await.number, None);
    let errors = lines_with(&logged, "could not store its link");
    assert_eq!(errors.len(), 1, "{logged}");
    assert!(errors[0].contains("acme/app"), "{logged}");
    assert!(errors[0].contains('7'), "{logged}");
}

#[tokio::test]
async fn a_network_error_never_raises() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    // Nothing listens on this port.
    let api = Api::new(Secret::new(TOKEN), "http://127.0.0.1:1").expect("the HTTP client builds");

    let filed = github::file_issues_for(
        db.connection(),
        &[test],
        &gh_settings(&[]),
        &api,
        &unstopped(),
    )
    .await;
    assert!(filed.linked.is_empty());
    assert_eq!(
        filed.stopped,
        Some(github::BatchStop::Unreachable),
        "the batch reports that nothing answered"
    );
    assert_eq!(link_of(&db, EntityKind::Test, test).await.number, None);
}

#[tokio::test]
async fn the_configured_label_is_used() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 7})),
    )
    .await;
    let settings = gh_settings(&[("FLAKERADAR_GITHUB_ISSUE_LABEL", "flaky-test")]);

    github::file_issues_for(
        db.connection(),
        &[test],
        &settings,
        &api(&server),
        &unstopped(),
    )
    .await;
    let requests = server.received_requests().await.expect("a record");
    let body: Json = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    assert_eq!(body["labels"], json!(["flaky-test"]));
}

#[tokio::test]
async fn a_flaky_job_is_filed_with_its_pipeline_and_unexplained_failures() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job(&db).await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 8})),
    )
    .await;

    let filed = github::file_job_issues_for(
        db.connection(),
        &[job],
        &gh_settings(&[]),
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(filed.linked.into_iter().collect::<Vec<_>>(), vec![job]);
    let requests = server.received_requests().await.expect("a record");
    let body: Json = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    assert_eq!(
        body["title"],
        json!("[FlakeRadar] Flaky CI job: .github/workflows/ci.yml / unit")
    );
    let text = body["body"].as_str().expect("a body");
    assert!(
        text.contains("FlakeRadar detected a flaky CI job: `.github/workflows/ci.yml` / `unit`"),
        "{text}"
    );
    assert!(text.contains("- **Unexplained failures:** 1"), "{text}");
    assert!(text.contains("- **Explained by tests:** 0"), "{text}");
    assert!(text.contains("- **Job:** `unit`"), "{text}");
    assert!(
        text.contains("| Outcome | Commit | Branch | Attempt | Runner | Job |"),
        "{text}"
    );
    assert_eq!(link_of(&db, EntityKind::Job, job).await.number, Some(8));
    assert_eq!(link_of(&db, EntityKind::Job, job).await.filed, Some(true));
}

#[tokio::test]
async fn a_job_of_another_provider_is_never_filed() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .provider("gitlab")
        .name(".gitlab-ci.yml")
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    let job = make_job(&pipeline)
        .flakiness_score(0.9)
        .build(db.connection())
        .await
        .expect("the Job inserts");
    let server = MockServer::start().await;

    assert!(
        github::file_job_issues_for(
            db.connection(),
            &[job.id],
            &gh_settings(&[]),
            &api(&server),
            &system_clock(),
            &unstopped()
        )
        .await
        .linked
        .is_empty()
    );
    assert!(
        server
            .received_requests()
            .await
            .expect("a record")
            .is_empty()
    );
}

#[tokio::test]
async fn a_candidate_read_that_fails_logs_and_reports_an_empty_batch() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let job = flaky_job(&db).await;
    db.connection()
        .execute_unprepared("DROP TABLE test_cases, jobs CASCADE")
        .await
        .expect("the tables are dropped");
    let api = Api::new(Secret::new(TOKEN), "http://127.0.0.1:1").expect("the HTTP client builds");

    let fired = github::file_issues_for(
        db.connection(),
        &[test],
        &gh_settings(&[]),
        &api,
        &unstopped(),
    )
    .await;
    assert!(
        fired.linked.is_empty(),
        "a batch it could not read files nothing"
    );
    let fired = github::file_job_issues_for(
        db.connection(),
        &[job],
        &gh_settings(&[]),
        &api,
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert!(fired.linked.is_empty());

    // The note step reads the same two tables, and reports an empty batch rather
    // than raising.
    let settings = fixed_settings(&[]);
    for report in [
        github::note_fixed_issues_for(
            db.connection(),
            &[test],
            &settings,
            &api,
            &system_clock(),
            &unstopped(),
        )
        .await,
        github::note_fixed_job_issues_for(
            db.connection(),
            &[job],
            &settings,
            &api,
            &system_clock(),
            &unstopped(),
        )
        .await,
    ] {
        assert_eq!(report, github::BatchReport::default());
    }
}

#[tokio::test]
async fn a_test_whose_executions_cannot_be_read_is_still_filed_without_them() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    db.connection()
        .execute_unprepared("DROP TABLE test_executions CASCADE")
        .await
        .expect("the table is dropped");
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 7})),
    )
    .await;

    let fired = github::file_issues_for(
        db.connection(),
        &[test],
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;

    assert_eq!(fired.linked.len(), 1, "the Test is filed");
    assert_eq!(link_of(&db, EntityKind::Test, test).await.number, Some(7));
}

#[tokio::test]
async fn the_filing_gate_reads_the_configured_minimums() {
    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_ISSUE_MIN_SCORE", "0.75")]);
    let gate = github::filing_gate(&settings);
    assert!((gate.min_score - 0.75).abs() < f64::EPSILON);
    assert_eq!(gate.fixed_streak, Some(3));

    let off = github::filing_gate(&gh_settings(&[]));
    assert_eq!(off.fixed_streak, None, "0 disables the fixed-note signal");
}

/// Drops one Test's link, as an unlink would.
async fn unlink_test(db: &TestDb, id: i32) {
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_number = NULL, github_issue_filed = NULL, \
             github_issue_checked_at = NULL, github_issue_closed = FALSE WHERE id = $1",
            [Value::from(id)],
        ))
        .await
        .expect("the unlink writes");
}

#[tokio::test]
async fn a_linked_row_and_a_calm_row_are_both_left_alone() {
    let db = TestDb::new().await.expect("a test database");
    let filed = flaky_test_in(&db, "acme/app", "p1", "filed").await;
    stamp_link(&db, EntityKind::Test, filed, stamp().number(5)).await;
    let calm = flaky_test_in(&db, "acme/other", "p2", "calm").await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET flakiness_score = 0.1 WHERE id = $1",
            [Value::from(calm)],
        ))
        .await
        .expect("the score writes");
    let server = MockServer::start().await;

    let filed_ids = github::file_issues_for(
        db.connection(),
        &[filed, calm],
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert!(filed_ids.linked.is_empty());
    assert!(
        server
            .received_requests()
            .await
            .expect("a record")
            .is_empty()
    );
}

#[tokio::test]
async fn a_kept_not_planned_link_is_never_refiled() {
    let db = TestDb::new().await.expect("a test database");
    let wont_fix = flaky_test_in(&db, "acme/app", "backend", "wont_fix").await;
    stamp_link(
        &db,
        EntityKind::Test,
        wont_fix,
        stamp().number(11).checked().closed(),
    )
    .await;
    let server = MockServer::start().await;

    assert!(
        github::file_issues_for(
            db.connection(),
            &[wont_fix],
            &gh_settings(&[]),
            &api(&server),
            &unstopped(),
        )
        .await
        .linked
        .is_empty()
    );
    assert!(
        server
            .received_requests()
            .await
            .expect("a record")
            .is_empty(),
        "a kept link is still the dedupe"
    );
    let link = link_of(&db, EntityKind::Test, wont_fix).await;
    assert_eq!(link.number, Some(11));
    assert_eq!(link.filed, Some(true));
    assert!(link.closed);
}

#[tokio::test]
async fn a_rate_limit_stops_the_test_and_job_batches() {
    let db = TestDb::new().await.expect("a test database");
    let a = flaky_test_in(&db, "acme/a", "p1", "a").await;
    let b = flaky_test_in(&db, "acme/b", "p2", "b").await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("x-ratelimit-remaining", "0")
                .set_body_json(json!({"message": "API rate limit exceeded"})),
        )
        .mount(&server)
        .await;

    github::file_issues_for(
        db.connection(),
        &[a, b],
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(requested_paths(&server).await.len(), 1);

    let ja = flaky_job_in(&db, "acme/c", ".github/workflows/a.yml", "a").await;
    let jb = flaky_job_in(&db, "acme/d", ".github/workflows/b.yml", "b").await;
    let jobs = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("x-ratelimit-remaining", "0")
                .set_body_json(json!({"message": "API rate limit exceeded"})),
        )
        .mount(&jobs)
        .await;

    github::file_job_issues_for(
        db.connection(),
        &[ja, jb],
        &gh_settings(&[]),
        &api(&jobs),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(requested_paths(&jobs).await.len(), 1);
}

#[tokio::test]
async fn a_429_stops_the_test_and_job_batches() {
    let db = TestDb::new().await.expect("a test database");
    let ta = flaky_test_in(&db, "acme/a", "p1", "a").await;
    let tb = flaky_test_in(&db, "acme/b", "p2", "b").await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    github::file_issues_for(
        db.connection(),
        &[ta, tb],
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(requested_paths(&server).await.len(), 1);

    let ja = flaky_job_in(&db, "acme/c", ".github/workflows/a.yml", "a").await;
    let jb = flaky_job_in(&db, "acme/d", ".github/workflows/b.yml", "b").await;
    let jobs = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&jobs)
        .await;

    github::file_job_issues_for(
        db.connection(),
        &[ja, jb],
        &gh_settings(&[]),
        &api(&jobs),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert_eq!(requested_paths(&jobs).await.len(), 1);
}

#[tokio::test]
async fn a_permission_403_costs_only_the_denied_repo() {
    let db = TestDb::new().await.expect("a test database");
    let a1 = flaky_test_in(&db, "acme/a", "p1", "a1").await;
    let a2 = flaky_test_in(&db, "acme/a", "p2", "a2").await;
    let b = flaky_test_in(&db, "acme/b", "p3", "b").await;
    let detail = format!(
        "Resource not accessible by personal access token{}",
        "x".repeat(400)
    );
    let body = serde_json::to_string(&json!({ "message": detail })).expect("a JSON body");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/a/issues"))
        .respond_with(ResponseTemplate::new(403).set_body_string(body.clone()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/b/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 88})))
        .mount(&server)
        .await;

    let (report, logged) = CapturedLog::during_call(github::file_issues_for(
        db.connection(),
        &[a1, a2, b],
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    ))
    .await;

    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/a/issues", "/repos/acme/b/issues"],
        "the second Test of the denied Repo is never retried"
    );
    assert_eq!(link_of(&db, EntityKind::Test, a1).await.number, None);
    assert_eq!(link_of(&db, EntityKind::Test, a2).await.number, None);
    assert_eq!(link_of(&db, EntityKind::Test, b).await.number, Some(88));

    assert_eq!(
        report.skipped,
        std::collections::BTreeSet::from(["acme/a".to_owned()]),
        "the denied Repo is the one the batch skipped"
    );
    // The cap on a GitHub body is this project's own logging rule, so it is the one
    // part of this case asserted from the log.
    let warnings = lines_with(&logged, "skipped for this Repo");
    assert_eq!(warnings.len(), 1, "{logged}");
    let kept: String = body.chars().take(300).collect();
    let cut: String = body.chars().skip(300).collect();
    assert!(
        warnings[0].contains(&kept),
        "only the first 300 characters are logged: {logged}"
    );
    assert!(!warnings[0].contains(&cut), "the rest is cut: {logged}");

    let ja1 = flaky_job_in(&db, "acme/a", ".github/workflows/a1.yml", "a1").await;
    let ja2 = flaky_job_in(&db, "acme/a", ".github/workflows/a2.yml", "a2").await;
    let jb = flaky_job_in(&db, "acme/b", ".github/workflows/b.yml", "b").await;
    let jobs = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/a/issues"))
        .respond_with(ResponseTemplate::new(403).set_body_string(body.clone()))
        .mount(&jobs)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/b/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 88})))
        .mount(&jobs)
        .await;

    let (report, logged) = CapturedLog::during_call(github::file_job_issues_for(
        db.connection(),
        &[ja1, ja2, jb],
        &gh_settings(&[]),
        &api(&jobs),
        &system_clock(),
        &unstopped(),
    ))
    .await;
    assert_eq!(
        requested_paths(&jobs).await,
        vec!["/repos/acme/a/issues", "/repos/acme/b/issues"]
    );
    assert_eq!(link_of(&db, EntityKind::Job, ja1).await.number, None);
    assert_eq!(link_of(&db, EntityKind::Job, ja2).await.number, None);
    assert_eq!(link_of(&db, EntityKind::Job, jb).await.number, Some(88));
    assert_eq!(
        report.skipped,
        std::collections::BTreeSet::from(["acme/a".to_owned()])
    );
    let warnings = lines_with(&logged, "skipped for this Repo");
    assert_eq!(warnings.len(), 1, "{logged}");
    let cut: String = body.chars().skip(300).collect();
    assert!(!warnings[0].contains(&cut), "the rest is cut: {logged}");
}

#[tokio::test]
async fn a_repo_whose_issues_are_disabled_is_skipped_for_the_rest_of_the_batch() {
    let db = TestDb::new().await.expect("a test database");
    let a1 = flaky_test_in(&db, "acme/a", "p1", "a1").await;
    let a2 = flaky_test_in(&db, "acme/a", "p2", "a2").await;
    let b = flaky_test_in(&db, "acme/b", "p3", "b").await;
    for status in [404, 410] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/a/issues"))
            .respond_with(
                ResponseTemplate::new(status).set_body_json(json!({"message": "Not Found"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/b/issues"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 99})))
            .mount(&server)
            .await;

        let report = github::file_issues_for(
            db.connection(),
            &[a1, a2, b],
            &gh_settings(&[]),
            &api(&server),
            &unstopped(),
        )
        .await;
        assert_eq!(
            requested_paths(&server).await,
            vec!["/repos/acme/a/issues", "/repos/acme/b/issues"],
            "the whole Repo is skipped on {status}"
        );
        assert_eq!(link_of(&db, EntityKind::Test, a1).await.number, None);
        assert_eq!(link_of(&db, EntityKind::Test, a2).await.number, None);
        assert_eq!(link_of(&db, EntityKind::Test, b).await.number, Some(99));
        assert_eq!(
            report.skipped,
            std::collections::BTreeSet::from(["acme/a".to_owned()]),
            "the Repo whose Issues are off is skipped on {status}"
        );
        unlink_test(&db, b).await;

        let ja1 = flaky_job_in(
            &db,
            "acme/a",
            &format!(".github/workflows/a1-{status}.yml"),
            "a1",
        )
        .await;
        let ja2 = flaky_job_in(
            &db,
            "acme/a",
            &format!(".github/workflows/a2-{status}.yml"),
            "a2",
        )
        .await;
        let jb = flaky_job_in(
            &db,
            "acme/b",
            &format!(".github/workflows/b-{status}.yml"),
            "b",
        )
        .await;
        let jobs = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/a/issues"))
            .respond_with(
                ResponseTemplate::new(status).set_body_json(json!({"message": "Not Found"})),
            )
            .mount(&jobs)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/b/issues"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 99})))
            .mount(&jobs)
            .await;
        let report = github::file_job_issues_for(
            db.connection(),
            &[ja1, ja2, jb],
            &gh_settings(&[]),
            &api(&jobs),
            &system_clock(),
            &unstopped(),
        )
        .await;
        assert_eq!(
            requested_paths(&jobs).await,
            vec!["/repos/acme/a/issues", "/repos/acme/b/issues"]
        );
        assert_eq!(link_of(&db, EntityKind::Job, ja1).await.number, None);
        assert_eq!(link_of(&db, EntityKind::Job, ja2).await.number, None);
        assert_eq!(link_of(&db, EntityKind::Job, jb).await.number, Some(99));
        assert_eq!(
            report.skipped,
            std::collections::BTreeSet::from(["acme/a".to_owned()]),
            "the Job batch skips the same Repo on {status}"
        );
    }
}

#[tokio::test]
async fn an_unusable_status_costs_one_issue_and_the_batch_goes_on() {
    let db = TestDb::new().await.expect("a test database");
    let a1 = flaky_test_in(&db, "acme/a", "p1", "a1").await;
    let a2 = flaky_test_in(&db, "acme/a", "p2", "a2").await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/a/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 77})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/a/issues"))
        .respond_with(ResponseTemplate::new(500).set_body_string("server exploded"))
        .mount(&server)
        .await;

    let (report, logged) = CapturedLog::during_call(github::file_issues_for(
        db.connection(),
        &[a1, a2],
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    ))
    .await;

    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/a/issues", "/repos/acme/a/issues"],
        "a 500 says nothing about the Repo, so the second candidate is still posted"
    );
    assert_eq!(link_of(&db, EntityKind::Test, a1).await.number, Some(77));
    assert_eq!(link_of(&db, EntityKind::Test, a2).await.number, None);
    assert!(
        report.skipped.is_empty(),
        "a 500 must not skip the Repo: {report:?}"
    );
    assert_eq!(
        report.linked,
        std::collections::BTreeSet::from([a1]),
        "only the issue that was created is linked"
    );
    // The failure's own sentence is what an operator reads, and nothing else
    // carries it, so this one line stays a log assertion.
    let failures = lines_with(&logged, "GitHub issue creation failed");
    assert_eq!(failures.len(), 1, "{logged}");
    assert!(failures[0].contains("acme/a") && failures[0].contains("500"));
    assert!(failures[0].contains("server exploded"));
}

#[tokio::test]
async fn a_malformed_201_body_keeps_the_batch_and_stores_nothing() {
    let db = TestDb::new().await.expect("a test database");
    for (index, raw) in [
        "<html>Bad Gateway</html>",
        "[1, 2]",
        "null",
        r#"{"number": "not-a-number"}"#,
        "{}",
    ]
    .into_iter()
    .enumerate()
    {
        let a = flaky_test_in(&db, "acme/a", &format!("p{index}a"), "a").await;
        let b = flaky_test_in(&db, "acme/b", &format!("p{index}b"), "b").await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/a/issues"))
            .respond_with(ResponseTemplate::new(201).set_body_string(raw))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/b/issues"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 91})))
            .mount(&server)
            .await;

        let (report, logged) = CapturedLog::during_call(github::file_issues_for(
            db.connection(),
            &[a, b],
            &gh_settings(&[]),
            &api(&server),
            &unstopped(),
        ))
        .await;

        assert_eq!(
            requested_paths(&server).await,
            vec!["/repos/acme/a/issues", "/repos/acme/b/issues"],
            "{raw}"
        );
        assert_eq!(
            link_of(&db, EntityKind::Test, a).await.number,
            None,
            "{raw}"
        );
        assert_eq!(
            link_of(&db, EntityKind::Test, b).await.number,
            Some(91),
            "{raw}"
        );
        assert_eq!(
            report.linked,
            std::collections::BTreeSet::from([b]),
            "{raw}: the row with no issue number is not a link"
        );
        // The body GitHub answered with is what an operator needs to see, and no
        // returned value carries it, so this line stays a log assertion.
        let warnings = lines_with(&logged, "201 with no issue number");
        assert_eq!(warnings.len(), 1, "{raw}: {logged}");
        assert!(warnings[0].contains(raw), "{logged}");
    }
}

#[tokio::test]
async fn two_proven_flakes_meet_the_proven_flake_gate() {
    let db = TestDb::new().await.expect("a test database");
    let under = flaky_test_in(&db, "acme/app", "p1", "under").await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET confirmed_flake_count = 1 WHERE id = $1",
            [Value::from(under)],
        ))
        .await
        .expect("the count writes");
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;
    let settings = gh_settings(&[("FLAKERADAR_GITHUB_ISSUE_MIN_PROVEN_FLAKES", "2")]);

    assert!(
        github::file_issues_for(
            db.connection(),
            &[under],
            &settings,
            &api(&server),
            &unstopped()
        )
        .await
        .linked
        .is_empty(),
        "one proven flake is under a gate of two"
    );
    let over = flaky_test_in(&db, "acme/app", "p2", "over").await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET confirmed_flake_count = 2 WHERE id = $1",
            [Value::from(over)],
        ))
        .await
        .expect("the count writes");
    assert_eq!(
        github::file_issues_for(
            db.connection(),
            &[over],
            &settings,
            &api(&server),
            &unstopped()
        )
        .await
        .linked
        .len(),
        1,
        "two proven flakes meet it"
    );
}

#[tokio::test]
async fn a_second_failure_meets_the_min_failures_gate() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test_in(&db, "acme/app", "backend", "t").await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;
    let settings = gh_settings(&[("FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES", "2")]);

    assert!(
        github::file_issues_for(
            db.connection(),
            &[test],
            &settings,
            &api(&server),
            &unstopped()
        )
        .await
        .linked
        .is_empty(),
        "one failure in the window is under a gate of two"
    );

    let project = flakeradar_db::entities::projects::Entity::find_by_id(1)
        .one(db.connection())
        .await
        .expect("the Project reads")
        .expect("the Project exists");
    let run = make_run(&project)
        .commit_sha("cafe1234")
        .branch("main")
        .build(db.connection())
        .await
        .expect("the Run inserts");
    let row = flakeradar_db::entities::test_cases::Entity::find_by_id(test)
        .one(db.connection())
        .await
        .expect("the Test reads")
        .expect("the Test exists");
    make_execution(&row, &run)
        .status("failed")
        .details("boom2")
        .build(db.connection())
        .await
        .expect("the Execution inserts");

    assert_eq!(
        github::file_issues_for(
            db.connection(),
            &[test],
            &settings,
            &api(&server),
            &unstopped()
        )
        .await
        .linked
        .len(),
        1,
        "a second failure in the window meets the gate"
    );
}

#[tokio::test]
async fn only_the_newest_window_counts_a_failure() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/win")
        .project("p")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let old = make_test_case(&project)
        .name("old_failures")
        .flakiness_score(0.5)
        .confirmed_flake_count(0)
        .build(db.connection())
        .await
        .expect("the Test inserts");
    let new = make_test_case(&project)
        .name("new_failures")
        .flakiness_score(0.5)
        .confirmed_flake_count(0)
        .build(db.connection())
        .await
        .expect("the Test inserts");
    let run = make_run(&project)
        .build(db.connection())
        .await
        .expect("the Run inserts");
    let old_statuses = ["failed", "failed", "passed", "passed", "passed"];
    let new_statuses = ["failed", "passed", "failed", "passed", "failed"];
    for (old_status, new_status) in old_statuses.into_iter().zip(new_statuses) {
        make_execution(&old, &run)
            .status(old_status)
            .build(db.connection())
            .await
            .expect("the Execution inserts");
        make_execution(&new, &run)
            .status(new_status)
            .build(db.connection())
            .await
            .expect("the Execution inserts");
    }
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/win/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 91})))
        .mount(&server)
        .await;
    let settings = gh_settings(&[
        ("FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES", "1"),
        ("FLAKERADAR_SCORE_WINDOW", "3"),
    ]);

    let filed = github::file_issues_for(
        db.connection(),
        &[old.id, new.id],
        &settings,
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(
        filed.linked.into_iter().collect::<Vec<_>>(),
        vec![new.id],
        "the failures outside the newest window do not count"
    );
    assert_eq!(link_of(&db, EntityKind::Test, old.id).await.number, None);
    assert_eq!(
        link_of(&db, EntityKind::Test, new.id).await.number,
        Some(91)
    );
}

#[tokio::test]
async fn the_failure_gate_counts_the_overall_window_only() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("p")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    // A known Default branch, so a union with the Default-branch window would add
    // rows the overall window does not hold.
    db.connection()
        .execute_raw(Statement::from_string(
            DbBackend::Postgres,
            "UPDATE repos SET default_branch = 'main' WHERE name = 'acme/app'".to_owned(),
        ))
        .await
        .expect("the Default branch writes");
    let test = make_test_case(&project)
        .name("old_main_failures")
        .flakiness_score(0.5)
        .confirmed_flake_count(0)
        .build(db.connection())
        .await
        .expect("the Test inserts");
    let main = make_run(&project)
        .commit_sha("main1")
        .branch("main")
        .build(db.connection())
        .await
        .expect("the Run inserts");
    // Five older failures on the Default branch, all outside the newest five.
    for _ in 0..5 {
        make_execution(&test, &main)
            .status("failed")
            .details("boom")
            .build(db.connection())
            .await
            .expect("the Execution inserts");
    }
    let feature = make_run(&project)
        .commit_sha("feat1")
        .branch("feat")
        .build(db.connection())
        .await
        .expect("the Run inserts");
    // The newest five executions hold one failure and four passes, on a feature
    // branch: the gate counts this window alone, so a gate of two is missed.
    for status in ["failed", "passed", "passed", "passed", "passed"] {
        make_execution(&test, &feature)
            .status(status)
            .build(db.connection())
            .await
            .expect("the Execution inserts");
    }
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 91})),
    )
    .await;
    let settings = gh_settings(&[
        ("FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES", "2"),
        ("FLAKERADAR_SCORE_WINDOW", "5"),
    ]);

    let filed = github::file_issues_for(
        db.connection(),
        &[test.id],
        &settings,
        &api(&server),
        &unstopped(),
    )
    .await;
    assert!(
        filed.linked.is_empty(),
        "one failure in the overall window is under a gate of two"
    );
    assert!(
        requested_paths(&server).await.is_empty(),
        "no issue is filed for a Test the score gate refuses"
    );
}

#[tokio::test]
async fn a_job_history_that_cannot_be_read_is_logged_and_the_issue_still_files() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job(&db).await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 8})),
    )
    .await;
    // The Job's evidence cannot be read, so the body is filed without it.
    db.connection()
        .execute_unprepared("DROP TABLE job_executions CASCADE")
        .await
        .expect("the Job executions table is dropped");

    let logged = CapturedLog::during(async {
        let filed = github::file_job_issues_for(
            db.connection(),
            &[job],
            &gh_settings(&[]),
            &api(&server),
            &system_clock(),
            &unstopped(),
        )
        .await;
        assert_eq!(
            filed.linked.into_iter().collect::<Vec<_>>(),
            vec![job],
            "a Job it could not read the evidence for is still filed"
        );
    })
    .await;
    // J13's requirement is that the dropped error is not swallowed, and a log line
    // is the only place it can appear: nothing in the answer carries it.
    let warnings = lines_with(&logged, "could not read a Job's Executions");
    assert_eq!(warnings.len(), 1, "the dropped error is logged: {logged}");
    assert!(warnings[0].contains("acme/app"), "{logged}");
    assert!(
        warnings[0].contains(&format!("entity_id={job}")),
        "{logged}"
    );
}

#[tokio::test]
async fn a_huge_id_list_stays_under_the_bind_limit() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test_in(&db, "acme/app", "backend", "t").await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;
    let mut ids = vec![test];
    ids.extend(10_000_000..10_040_000);

    github::file_issues_for(
        db.connection(),
        &ids,
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(
        requested_paths(&server).await.len(),
        1,
        "a huge id list is read in chunks instead of one oversized bind"
    );
    assert_eq!(link_of(&db, EntityKind::Test, test).await.number, Some(77));
}

/// Installs a trigger that makes an `UPDATE` of `table` row `id` affect no row.
async fn vanish_on_update(db: &TestDb, table: &str, id: i32) {
    db.connection()
        .execute_raw(Statement::from_string(
            DbBackend::Postgres,
            "CREATE FUNCTION vanish_link() RETURNS trigger AS $$ BEGIN \
             IF NEW.id = $1 THEN RETURN NULL; END IF; RETURN NEW; END; $$ LANGUAGE plpgsql"
                .replace("$1", &id.to_string()),
        ))
        .await
        .expect("the vanish function installs");
    db.connection()
        .execute_raw(Statement::from_string(
            DbBackend::Postgres,
            format!(
                "CREATE TRIGGER vanish_link BEFORE UPDATE ON {table} \
                 FOR EACH ROW EXECUTE FUNCTION vanish_link()"
            ),
        ))
        .await
        .expect("the vanish trigger installs");
}

#[tokio::test]
async fn a_row_deleted_while_filing_leaves_the_rest_of_the_batch_filed() {
    let db = TestDb::new().await.expect("a test database");
    let a = flaky_test_in(&db, "acme/app", "p1", "a").await;
    let b = flaky_test_in(&db, "acme/app", "p2", "b").await;
    vanish_on_update(&db, "test_cases", a).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 77})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 88})),
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/77/comments"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/77"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let filed = github::file_issues_for(
        db.connection(),
        &[a, b],
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(filed.linked.into_iter().collect::<Vec<_>>(), vec![b]);
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues",
            "/repos/acme/app/issues/77/comments",
            "/repos/acme/app/issues/77",
            "/repos/acme/app/issues",
        ],
        "the raced filing costs only itself"
    );
    assert_eq!(link_of(&db, EntityKind::Test, a).await.number, None);
    let link = link_of(&db, EntityKind::Test, b).await;
    assert_eq!(link.number, Some(88));
    assert_eq!(link.filed, Some(true));
}

#[tokio::test]
async fn the_job_row_deleted_while_filing_closes_the_new_issue() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job_in(&db, "acme/app", ".github/workflows/ci.yml", "unit").await;
    vanish_on_update(&db, "jobs", job).await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/77/comments"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/77"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    github::file_job_issues_for(
        db.connection(),
        &[job],
        &gh_settings(&[]),
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;

    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues",
            "/repos/acme/app/issues/77/comments",
            "/repos/acme/app/issues/77",
        ],
        "the issue the vanished Job stranded is commented on and closed"
    );
    assert_eq!(link_of(&db, EntityKind::Job, job).await.number, None);
}

#[tokio::test]
async fn a_job_that_was_reported_is_not_filed_twice() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job_in(&db, "acme/app", ".github/workflows/ci.yml", "unit").await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;

    assert_eq!(
        github::file_job_issues_for(
            db.connection(),
            &[job],
            &gh_settings(&[]),
            &api(&server),
            &system_clock(),
            &unstopped()
        )
        .await
        .linked
        .len(),
        1
    );
    assert!(
        github::file_job_issues_for(
            db.connection(),
            &[job],
            &gh_settings(&[]),
            &api(&server),
            &system_clock(),
            &unstopped()
        )
        .await
        .linked
        .is_empty()
    );
    assert_eq!(requested_paths(&server).await.len(), 1);
}

#[tokio::test]
async fn a_job_filing_without_a_token_is_silent() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job_in(&db, "acme/app", ".github/workflows/ci.yml", "unit").await;
    let server = MockServer::start().await;
    let settings = Settings::from_map(&std::collections::BTreeMap::new()).expect("the defaults");
    let api = Api::new(Secret::new(""), &server.uri()).expect("the HTTP client builds");

    assert!(
        github::file_job_issues_for(
            db.connection(),
            &[job],
            &settings,
            &api,
            &system_clock(),
            &unstopped()
        )
        .await
        .linked
        .is_empty()
    );
    assert!(
        requested_paths(&server).await.is_empty(),
        "no token is a silent no-op"
    );
    assert_eq!(link_of(&db, EntityKind::Job, job).await.number, None);
}

#[tokio::test]
async fn only_unexplained_job_failures_meet_the_min_failures_gate() {
    let db = TestDb::new().await.expect("a test database");
    let all_explained =
        flaky_job_in(&db, "acme/app", ".github/workflows/a.yml", "all-explained").await;
    let explained_row = flakeradar_db::entities::jobs::Entity::find_by_id(all_explained)
        .one(db.connection())
        .await
        .expect("the Job reads")
        .expect("the Job exists");
    for index in 0..3 {
        let ci_job_id = format!("ex-{index}");
        make_job_execution(&explained_row)
            .status("failed")
            .ci_job_id(&ci_job_id)
            .build(db.connection())
            .await
            .expect("the Job execution inserts");
        explain_job_execution(&db, &ci_job_id).await;
    }
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;
    let settings = gh_settings(&[("FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES", "2")]);

    assert!(
        github::file_job_issues_for(
            db.connection(),
            &[all_explained],
            &settings,
            &api(&server),
            &system_clock(),
            &unstopped()
        )
        .await
        .linked
        .is_empty(),
        "three explained failures are none of them unexplained"
    );

    let mixed = flaky_job_in(&db, "acme/app", ".github/workflows/b.yml", "mixed").await;
    let mixed_row = flakeradar_db::entities::jobs::Entity::find_by_id(mixed)
        .one(db.connection())
        .await
        .expect("the Job reads")
        .expect("the Job exists");
    for ci_job_id in ["mix-1", "mix-2", "mix-3"] {
        make_job_execution(&mixed_row)
            .status("failed")
            .ci_job_id(ci_job_id)
            .build(db.connection())
            .await
            .expect("the Job execution inserts");
    }
    explain_job_execution(&db, "mix-1").await;

    assert_eq!(
        github::file_job_issues_for(
            db.connection(),
            &[mixed],
            &settings,
            &api(&server),
            &system_clock(),
            &unstopped()
        )
        .await
        .linked
        .len(),
        1,
        "two unexplained failures meet the gate"
    );
}

#[tokio::test]
async fn the_job_failure_gate_counts_the_overall_window_only() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .provider("github")
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    // A known Default branch, so a union with the Default-branch window would add
    // rows the overall window does not hold.
    db.connection()
        .execute_raw(Statement::from_string(
            DbBackend::Postgres,
            "UPDATE repos SET default_branch = 'main' WHERE name = 'acme/app'".to_owned(),
        ))
        .await
        .expect("the Default branch writes");
    let job = make_job(&pipeline)
        .name("old_main_failures")
        .flakiness_score(0.5)
        .confirmed_flake_count(0)
        .build(db.connection())
        .await
        .expect("the Job inserts");
    // Five older unexplained failures on the Default branch, outside the newest five.
    for _ in 0..5 {
        make_job_execution(&job)
            .status("failed")
            .branch("main")
            .build(db.connection())
            .await
            .expect("the Job execution inserts");
    }
    // The newest five hold one failure and four passes, on a feature branch.
    for status in ["failed", "passed", "passed", "passed", "passed"] {
        make_job_execution(&job)
            .status(status)
            .branch("feat")
            .build(db.connection())
            .await
            .expect("the Job execution inserts");
    }
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 91})),
    )
    .await;
    let settings = gh_settings(&[
        ("FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES", "2"),
        ("FLAKERADAR_SCORE_WINDOW", "5"),
    ]);

    let filed = github::file_job_issues_for(
        db.connection(),
        &[job.id],
        &settings,
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert!(
        filed.linked.is_empty(),
        "one unexplained failure in the overall window is under a gate of two"
    );
    assert!(
        requested_paths(&server).await.is_empty(),
        "no issue is filed for a Job the score gate refuses"
    );
}

#[tokio::test]
async fn a_linked_test_and_a_linked_job_are_never_filed() {
    let db = TestDb::new().await.expect("a test database");
    let linked = flaky_test(&db).await;
    let other = flaky_test_in(&db, "acme/other", "backend", "other").await;
    stamp_link(&db, EntityKind::Test, linked, stamp().number(300).checked()).await;
    let job = flaky_job(&db).await;
    stamp_link(&db, EntityKind::Job, job, stamp().number(301).checked()).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/other/issues"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"number": 88})))
        .mount(&server)
        .await;

    let filed = github::file_issues_for(
        db.connection(),
        &[linked, other],
        &gh_settings(&[]),
        &api(&server),
        &unstopped(),
    )
    .await;
    assert_eq!(filed.linked.into_iter().collect::<Vec<_>>(), vec![other]);
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/other/issues"],
        "the linked Test is skipped and the other Repo is filed"
    );
    assert_eq!(
        link_of(&db, EntityKind::Test, linked).await.number,
        Some(300)
    );

    let jobs = github::file_job_issues_for(
        db.connection(),
        &[job],
        &gh_settings(&[]),
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    assert!(jobs.linked.is_empty(), "a linked Job is never filed");
    assert!(
        server.received_requests().await.expect("a record").len() == 1,
        "no request for the linked Job"
    );
    assert_eq!(link_of(&db, EntityKind::Job, job).await.number, Some(301));
}
