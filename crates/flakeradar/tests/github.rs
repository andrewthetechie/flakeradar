//! The GitHub issue automation: filing, the filing gate, the sync and the notes.
//!
//! Ported from `backend/tests/test_github.py`. Every GitHub call goes to a stub
//! server, so a test asserts the request it made — method, path, body, headers —
//! and never that a function was called.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a stub or an answer does not behave"
)]

use flakeradar::Settings;
use flakeradar::github::{self, Api};
use flakeradar::{FilingGate, JobRow, RecentExecution, TestRow, settings::Secret};
use flakeradar_db::ProcessOutcome;
use flakeradar_test_support::{
    TestDb, make_execution, make_job, make_job_execution, make_pipeline, make_project, make_run,
    make_test_case,
};
use sea_orm::ConnectionTrait as _;
use sea_orm::EntityTrait as _;
use sea_orm::{DbBackend, Statement, Value};
use serde_json::{Value as Json, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The GitHub token every request presents, which no log line may carry.
const TOKEN: &str = "ghs_testtoken";

/// The settings a filing test runs with.
fn gh_settings(pairs: &[(&str, &str)]) -> Settings {
    let mut values = vec![
        ("FLAKERADAR_GITHUB_TOKEN", TOKEN),
        ("FLAKERADAR_GITHUB_ISSUE_MIN_SCORE", "0.3"),
    ];
    values.extend_from_slice(pairs);
    let map = values
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    Settings::from_map(&map).expect("the test's settings are valid")
}

/// A client pointed at the stub server.
fn api(server: &MockServer) -> Api {
    Api::new(Secret::new(TOKEN), &server.uri())
}

/// The settings a fixed-note test runs with.
fn fixed_settings(pairs: &[(&str, &str)]) -> Settings {
    let mut values = vec![
        ("FLAKERADAR_GITHUB_TOKEN", TOKEN),
        ("FLAKERADAR_GITHUB_FIXED_STREAK", "3"),
    ];
    values.extend_from_slice(pairs);
    let map = values
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    Settings::from_map(&map).expect("the test's settings are valid")
}

/// One flaky Test in `acme/app`, with one failing Execution it can show.
async fn flaky_test(db: &TestDb) -> i32 {
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .root("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let test = make_test_case(&project)
        .name("test_total")
        .classname("tests.test_cart")
        .suite("unit")
        .fingerprint("abc123")
        .flakiness_score(0.5)
        .confirmed_flake_count(2)
        .file("tests/test_cart.py")
        .line(12)
        .build(db.connection())
        .await
        .expect("the Test inserts");
    let run = make_run(&project)
        .commit_sha("abcdef1234567890")
        .branch("main")
        .build(db.connection())
        .await
        .expect("the Run inserts");
    make_execution(&test, &run)
        .status("failed")
        .message("AssertionError: expected 3")
        .details("Traceback (most recent call last):\n  ...")
        .build(db.connection())
        .await
        .expect("the Execution inserts");
    test.id
}

/// One flaky github-provider Job in `acme/app`.
async fn flaky_job(db: &TestDb) -> i32 {
    let pipeline = make_pipeline()
        .repo("acme/app")
        .provider("github")
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    let job = make_job(&pipeline)
        .name("unit")
        .flakiness_score(0.5)
        .confirmed_flake_count(1)
        .build(db.connection())
        .await
        .expect("the Job inserts");
    make_job_execution(&job)
        .status("failed")
        .build(db.connection())
        .await
        .expect("the Job execution inserts");
    job.id
}

/// Mounts the create-issue endpoint with `answer`.
async fn mount_create(server: &MockServer, answer: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(answer)
        .mount(server)
        .await;
}

/// The `github_issue_*` columns of one Test.
async fn test_link(db: &TestDb, id: i32) -> (Option<i32>, Option<bool>, bool, bool) {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT github_issue_number, github_issue_filed, github_issue_checked_at IS NOT NULL \
             AS checked, github_issue_closed FROM test_cases WHERE id = $1",
            [Value::from(id)],
        ))
        .await
        .expect("the row reads")
        .expect("the Test exists");
    (
        row.try_get("", "github_issue_number").expect("a number"),
        row.try_get("", "github_issue_filed").expect("filed"),
        row.try_get("", "checked").expect("checked"),
        row.try_get("", "github_issue_closed").expect("closed"),
    )
}

/// Stamps one Test's link columns, as an earlier pass would have.
async fn stamp_link(db: &TestDb, id: i32, number: i32, checked: bool, closed: bool) {
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_number = $2, \
             github_issue_checked_at = CASE WHEN $3 THEN now() ELSE NULL END, \
             github_issue_closed = $4 WHERE id = $1",
            [
                Value::from(id),
                Value::from(number),
                Value::from(checked),
                Value::from(closed),
            ],
        ))
        .await
        .expect("the stamp writes");
}

/// A processed Report outcome for `tests` and `jobs`.
fn processed(tests: &[i32], jobs: &[i32]) -> ProcessOutcome {
    ProcessOutcome {
        report_id: 1,
        status: "processed".to_owned(),
        run_id: Some(1),
        counts: None,
        touched_test_ids: tests.to_vec(),
        error: None,
        touched_job_ids: jobs.to_vec(),
        tier_changes: Vec::new(),
        retry_count: 0,
    }
}

#[tokio::test]
async fn no_token_is_a_silent_no_op() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let server = MockServer::start().await;
    let settings = Settings::from_map(&std::collections::BTreeMap::new()).expect("the defaults");
    let api = Api::new(Secret::new(""), &server.uri());

    let filed = github::file_issues_for(db.connection(), &[test], &settings, &api).await;
    assert!(filed.is_empty());
    assert_eq!(test_link(&db, test).await.0, None);
    assert!(
        server
            .received_requests()
            .await
            .expect("a record")
            .is_empty()
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

    let filed = github::file_issues_for(db.connection(), &[test], &settings, &api(&server)).await;
    assert_eq!(filed.into_iter().collect::<Vec<_>>(), vec![test]);
    let (number, is_filed, checked, closed) = test_link(&db, test).await;
    assert_eq!(number, Some(7));
    assert_eq!(is_filed, Some(true), "a filed link is FlakeRadar's own");
    assert!(!checked);
    assert!(!closed);

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
    let text = body["body"].as_str().expect("a body").to_owned();
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
            text.contains(expected),
            "the body carries {expected:?}: {text}"
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

    let filed = github::file_issues_for(db.connection(), &[test], &settings, &api(&server)).await;
    assert!(filed.is_empty());
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
        github::file_issues_for(db.connection(), &[test], &flakes, &api(&server))
            .await
            .is_empty()
    );
    let failures = gh_settings(&[("FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES", "2")]);
    assert!(
        github::file_issues_for(db.connection(), &[test], &failures, &api(&server))
            .await
            .is_empty()
    );
    // One failure in the window meets a gate of one.
    let one = gh_settings(&[("FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES", "1")]);
    assert_eq!(
        github::file_issues_for(db.connection(), &[test], &one, &api(&server))
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn a_link_a_human_made_is_never_refiled() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, true, false).await;
    let server = MockServer::start().await;
    let settings = gh_settings(&[]);

    let filed = github::file_issues_for(db.connection(), &[test], &settings, &api(&server)).await;
    assert!(
        filed.is_empty(),
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
    let test = flaky_test(&db).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET clean_streak = 3 WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the streak writes");
    let server = MockServer::start().await;
    let settings = fixed_settings(&[]);

    assert!(
        github::file_issues_for(db.connection(), &[test], &settings, &api(&server))
            .await
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
    )
    .await;
    assert!(filed.is_empty());
    assert_eq!(
        server.received_requests().await.expect("a record").len(),
        1,
        "a rate limit stops the batch after the first attempt"
    );
    assert_eq!(test_link(&db, first).await.0, None);
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
    )
    .await;
    assert_eq!(filed.into_iter().collect::<Vec<_>>(), vec![allowed.id]);
    assert_eq!(test_link(&db, denied).await.0, None);
}

#[tokio::test]
async fn issues_disabled_skips_the_rest_of_that_repo() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    let server = MockServer::start().await;
    mount_create(&server, ResponseTemplate::new(410)).await;

    assert!(
        github::file_issues_for(db.connection(), &[test], &gh_settings(&[]), &api(&server))
            .await
            .is_empty()
    );
    assert_eq!(test_link(&db, test).await.0, None);
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
        github::file_issues_for(db.connection(), &[test], &gh_settings(&[]), &api(&server))
            .await
            .is_empty()
    );
    assert_eq!(test_link(&db, test).await.0, None);
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

    let filed =
        github::file_issues_for(db.connection(), &[test], &gh_settings(&[]), &api(&server)).await;
    assert!(filed.is_empty(), "a vanished row is not a link");
    let requested: Vec<String> = server
        .received_requests()
        .await
        .expect("the stub records requests")
        .iter()
        .map(|request| request.url.path().to_owned())
        .collect();
    assert!(
        requested.contains(&"/repos/acme/app/issues/7/comments".to_owned()),
        "the new issue is commented on: {requested:?}"
    );
    assert!(
        requested.contains(&"/repos/acme/app/issues/7".to_owned()),
        "and closed: {requested:?}"
    );
}

#[tokio::test]
async fn a_network_error_never_raises() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    // Nothing listens on this port.
    let api = Api::new(Secret::new(TOKEN), "http://127.0.0.1:1");

    let filed = github::file_issues_for(db.connection(), &[test], &gh_settings(&[]), &api).await;
    assert!(filed.is_empty());
    assert_eq!(test_link(&db, test).await.0, None);
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

    github::file_issues_for(db.connection(), &[test], &settings, &api(&server)).await;
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

    let filed =
        github::file_job_issues_for(db.connection(), &[job], &gh_settings(&[]), &api(&server))
            .await;
    assert_eq!(filed.into_iter().collect::<Vec<_>>(), vec![job]);
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
    assert!(
        text.contains("| Outcome | Commit | Branch | Attempt | Runner | Job |"),
        "{text}"
    );
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
        github::file_job_issues_for(db.connection(), &[job.id], &gh_settings(&[]), &api(&server))
            .await
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

/// Mounts the issue-state read of `acme/app` issue 12 with `answer`.
async fn mount_state(server: &MockServer, answer: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12"))
        .respond_with(answer)
        .mount(server)
        .await;
}

#[tokio::test]
async fn the_sync_rearms_a_link_whose_issue_closed() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, true, false).await;
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(json!({"state": "closed", "state_reason": "completed"})),
    )
    .await;

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
    let (number, filed, _, closed) = test_link(&db, test).await;
    assert_eq!(
        number, None,
        "the number goes, so the next Report may re-file"
    );
    assert_eq!(filed, None);
    assert!(!closed);
}

#[tokio::test]
async fn the_sync_keeps_a_link_closed_as_not_planned() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, true, false).await;
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(json!({"state": "closed", "state_reason": "not_planned"})),
    )
    .await;

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
    let (number, _, _, closed) = test_link(&db, test).await;
    assert_eq!(
        number,
        Some(12),
        "the team declined this flake; nothing is filed again"
    );
    assert!(closed, "and the row records why");
}

#[tokio::test]
async fn the_sync_clears_the_flag_when_a_declined_issue_reopens() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, true, true).await;
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(json!({"state": "open", "state_reason": "reopened"})),
    )
    .await;

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
    let (number, _, _, closed) = test_link(&db, test).await;
    assert_eq!(number, Some(12));
    assert!(!closed, "filing is live again");
}

#[tokio::test]
async fn the_sync_rearms_a_link_whose_issue_is_gone() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, true, false).await;
    let server = MockServer::start().await;
    mount_state(&server, ResponseTemplate::new(404)).await;

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
    assert_eq!(test_link(&db, test).await.0, None);
}

#[tokio::test]
async fn the_sync_keeps_a_link_on_a_permission_error_and_stamps_it() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, false, false).await;
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(403).set_body_json(json!({"message": "Resource not accessible"})),
    )
    .await;

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
    let (number, _, checked, _) = test_link(&db, test).await;
    assert_eq!(
        number,
        Some(12),
        "a permission error says nothing about the issue"
    );
    assert!(checked, "but the row was answered for, so it rotates");
}

#[tokio::test]
async fn the_sync_leaves_an_unreadable_body_alone_and_stamps_it() {
    let db = TestDb::new().await.expect("a test database");
    let test = flaky_test(&db).await;
    stamp_link(&db, test, 12, false, false).await;
    let server = MockServer::start().await;
    mount_state(
        &server,
        ResponseTemplate::new(200).set_body_string("not json"),
    )
    .await;

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
    let (number, _, checked, _) = test_link(&db, test).await;
    assert_eq!(number, Some(12));
    assert!(
        checked,
        "an unusable answer must not pin the front of the rotation"
    );
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

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
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
        &Api::new(Secret::new(""), &server.uri()),
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
        .expect(1)
        .mount(&server)
        .await;

    let settings = fixed_settings(&[("FLAKERADAR_PUBLIC_URL", "https://flakes.example.com/")]);
    github::note_fixed_issues_for(db.connection(), &[test], &settings, &api(&server)).await;

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
    github::note_fixed_issues_for(db.connection(), &[test], &settings, &api(&server)).await;
    assert_eq!(server.received_requests().await.expect("a record").len(), 1);
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
        .expect(1)
        .mount(&server)
        .await;

    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", "true")]);
    github::note_fixed_issues_for(db.connection(), &[test], &settings, &api(&server)).await;

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

    github::note_fixed_issues_for(db.connection(), &[test], &gh_settings(&[]), &api(&server)).await;
    assert!(
        server
            .received_requests()
            .await
            .expect("a record")
            .is_empty()
    );
}

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
    failed.status = "failed".to_owned();
    github::on_report_processed(db.connection(), &failed, &gh_settings(&[]), &api(&server)).await;
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
    )
    .await;
    assert_eq!(test_link(&db, test).await.0, Some(7));
}

#[tokio::test]
async fn a_field_the_body_prices_differently_is_still_the_reference_sentence() {
    // The run hint falls back when no file was reported, and a Test with no line
    // still gets a permalink without an anchor.
    let test = TestRow {
        id: 1,
        classname: "tests.test_cart".to_owned(),
        name: "test_total".to_owned(),
        suite: String::new(),
        file: None,
        line: None,
        fingerprint: "abc".to_owned(),
        flakiness_score: 0.5,
        confirmed_flake_count: 0,
    };
    let recent = [RecentExecution {
        status: "passed".to_owned(),
        message: String::new(),
        details: None,
        commit_sha: "abcdef1234567890".to_owned(),
        branch: "main".to_owned(),
        created_at: chrono::Utc::now(),
    }];
    let settings = gh_settings(&[]);
    let body = github::test_issue_body(&test, &recent, "acme/app", "backend", "", &settings);
    assert!(
        body.contains(
            "# Re-run `tests.test_cart::test_total` via your project's test runner (no file path reported)."
        ),
        "{body}"
    );
    assert!(
        body.contains("- **Location:** (not reported by the test runner)"),
        "{body}"
    );
    assert!(
        body.contains("- **Suite / classname:** `(none)` / `tests.test_cart`"),
        "{body}"
    );
    assert!(
        !body.contains("### Sample failure"),
        "a passing Test shows no failure"
    );
}

#[tokio::test]
async fn the_run_hint_follows_the_file_extension() {
    for (path, expected) in [
        (
            Some("tests/test_cart.py"),
            "pytest tests/test_cart.py::test_total",
        ),
        (
            Some("src/cart.test.ts"),
            "npx vitest run src/cart.test.ts -t \"test_total\"",
        ),
        (Some("cart_test.go"), "go test ./... -run test_total"),
        (Some("CartTest.java"), "mvn test -Dtest=CartTest#test_total"),
        (
            Some("cart_spec.rb"),
            "bundle exec rspec cart_spec.rb -e \"test_total\"",
        ),
        (
            Some("cart_test.rs"),
            "# Re-run `CartTest::test_total` via your project's test runner (cart_test.rs).",
        ),
        (
            None,
            "# Re-run `CartTest::test_total` via your project's test runner (no file path reported).",
        ),
    ] {
        let hint = github::run_hint(path, "CartTest", "test_total");
        assert_eq!(hint, expected, "{path:?}");
    }
}

#[tokio::test]
async fn a_job_body_without_history_still_carries_the_table_heading() {
    let job = JobRow {
        id: 3,
        name: "unit".to_owned(),
        flakiness_score: 0.5,
        confirmed_flake_count: 1,
    };
    let body = github::job_issue_body(&job, "ci", "acme/app", None, &gh_settings(&[]));
    assert!(
        body.contains("| Outcome | Commit | Branch | Attempt | Runner | Job |"),
        "{body}"
    );
    assert!(!body.contains("- **Unexplained failures:**"), "{body}");
}

#[tokio::test]
async fn the_filing_gate_reads_the_configured_minimums() {
    let settings = fixed_settings(&[("FLAKERADAR_GITHUB_ISSUE_MIN_SCORE", "0.75")]);
    let gate = FilingGate::from_settings(&settings);
    assert!((gate.min_score - 0.75).abs() < f64::EPSILON);
    assert_eq!(gate.fixed_streak, Some(3));

    let off = FilingGate::from_settings(&gh_settings(&[]));
    assert_eq!(off.fixed_streak, None, "0 disables the fixed-note signal");
}
