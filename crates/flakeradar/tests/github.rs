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
use flakeradar_db::issue_links::{self, LinkTarget};
use flakeradar_db::{ProcessOutcome, ReadSettings};
use flakeradar_test_support::{
    TestDb, make_execution, make_job, make_job_execution, make_pipeline, make_project, make_run,
    make_test_case,
};
use sea_orm::ConnectionTrait as _;
use sea_orm::EntityTrait as _;
use sea_orm::{DbBackend, Statement, Value};
use serde_json::{Value as Json, json};
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

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
    )
    .await;
    assert_eq!(
        filed.into_iter().collect::<Vec<_>>(),
        vec![below],
        "a row whose fix holds answers with a note, not an Issue"
    );
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/app/issues"]
    );
    assert_eq!(
        test_link(&db, at_streak).await.0,
        None,
        "no Issue to comment the fix on"
    );
    assert_eq!(test_link(&db, below).await.0, Some(101));
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

    let logged = logged_from(async {
        let filed =
            github::file_issues_for(db.connection(), &[test], &gh_settings(&[]), &api(&server))
                .await;
        assert!(filed.is_empty(), "a vanished row is not a link");
    })
    .await;
    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues",
            "/repos/acme/app/issues/7/comments",
            "/repos/acme/app/issues/7",
        ],
        "the new issue is commented on, then closed"
    );
    let warnings = lines_with(&logged, "closed the new issue");
    assert_eq!(warnings.len(), 1, "{logged}");
    assert!(warnings[0].contains('7'), "{logged}");
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
    assert!(text.contains("- **Explained by tests:** 0"), "{text}");
    assert!(text.contains("- **Job:** `unit`"), "{text}");
    assert!(
        text.contains("| Outcome | Commit | Branch | Attempt | Runner | Job |"),
        "{text}"
    );
    assert_eq!(job_link(&db, job).await.0, Some(8));
    assert_eq!(job_link(&db, job).await.1, Some(true));
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

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
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

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
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

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
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

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
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

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
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

    // A failed run reset the Clean streak and the stamp: the fix is noteable again.
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_fixed_noted_at = NULL WHERE id = $1",
            [Value::from(test)],
        ))
        .await
        .expect("the stamp clears");
    github::note_fixed_issues_for(db.connection(), &[test], &settings, &api(&server)).await;
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
    github::note_fixed_issues_for(db.connection(), &[test], &settings, &api(&server)).await;
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
    assert!(body.contains("**Job:** `unit`"), "{body}");
    assert!(body.contains("**Flakiness score:**"), "{body}");
    assert!(!body.contains("Unexplained failures"), "{body}");
    assert!(!body.contains("Explained by tests"), "{body}");
    let table = body
        .split("### Last 10 executions")
        .nth(1)
        .expect("the executions heading is there");
    assert!(
        table.contains("| Outcome | Commit | Branch | Attempt | Runner | Job |"),
        "{body}"
    );
    assert!(table.contains("|---|---|---|---|---|---|"), "{body}");
    assert!(!table.contains("| failed |"), "{body}");
    assert!(!table.contains("| passed |"), "{body}");
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

// --- the failure policy of a filing batch --------------------------------

/// One flaky Test named `name` in `repo`/`project`, with one failing Execution.
async fn flaky_test_in(db: &TestDb, repo: &str, project: &str, name: &str) -> i32 {
    let project = make_project()
        .repo(repo)
        .project(project)
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let test = make_test_case(&project)
        .name(name)
        .classname(format!("tests.test_{name}").as_str())
        .fingerprint(name)
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

/// One flaky github-provider Job named `name` in `repo`, under `pipeline`.
async fn flaky_job_in(db: &TestDb, repo: &str, pipeline: &str, name: &str) -> i32 {
    let pipeline = make_pipeline()
        .repo(repo)
        .provider("github")
        .name(pipeline)
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    let job = make_job(&pipeline)
        .name(name)
        .flakiness_score(0.5)
        .confirmed_flake_count(1)
        .build(db.connection())
        .await
        .expect("the Job inserts");
    make_job_execution(&job)
        .status("failed")
        .ci_job_id(name)
        .build(db.connection())
        .await
        .expect("the Job execution inserts");
    job.id
}

/// The `github_issue_*` columns of one Job.
async fn job_link(db: &TestDb, id: i32) -> (Option<i32>, Option<bool>) {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT github_issue_number, github_issue_filed FROM jobs WHERE id = $1",
            [Value::from(id)],
        ))
        .await
        .expect("the row reads")
        .expect("the Job exists");
    (
        row.try_get("", "github_issue_number").expect("a number"),
        row.try_get("", "github_issue_filed").expect("filed"),
    )
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

/// The log lines `future` writes, with a subscriber installed for this thread.
async fn logged_from(future: impl Future) -> String {
    use std::io::Write as _;
    use std::sync::{Arc, Mutex};

    /// An `io::Write` that appends to the shared buffer.
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || Sink(Arc::clone(&sink)))
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    future.await;
    drop(guard);
    let text = String::from_utf8_lossy(
        &lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .into_owned();
    let _ = std::io::stdout().flush();
    text
}

/// The lines of `logged` that carry `needle`.
fn lines_with<'a>(logged: &'a str, needle: &str) -> Vec<&'a str> {
    logged
        .lines()
        .filter(|line| line.contains(needle))
        .collect()
}

/// The paths a stub server was asked for, in order.
async fn requested_paths(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("the stub records requests")
        .iter()
        .map(|request| request.url.path().to_owned())
        .collect()
}

#[tokio::test]
async fn a_rate_limit_classifies_only_throttling() {
    use reqwest::header::HeaderMap;

    let answer = |status: u16, headers: &[(&str, &str)], text: &str| {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
                reqwest::header::HeaderValue::from_str(value).expect("a header value"),
            );
        }
        github::Answer {
            status,
            headers: map,
            text: text.to_owned(),
        }
    };

    assert!(answer(429, &[], "").rate_limited());
    assert!(answer(403, &[("x-ratelimit-remaining", "0")], "{}").rate_limited());
    assert!(answer(403, &[("retry-after", "60")], "{}").rate_limited());
    assert!(
        answer(
            403,
            &[],
            r#"{"message": "You have exceeded a secondary rate limit"}"#
        )
        .rate_limited()
    );
    assert!(
        answer(
            403,
            &[],
            r#"{"message": "You have exceeded a Secondary Rate Limit"}"#
        )
        .rate_limited()
    );
    assert!(
        !answer(
            403,
            &[],
            r#"{"message": "Resource not accessible by personal access token"}"#
        )
        .rate_limited(),
        "a permission 403 is not throttling"
    );
    assert!(!answer(403, &[], "{}").rate_limited());
    assert!(
        !answer(403, &[], "<html>Gateway</html>").rate_limited(),
        "a non-JSON body must not raise"
    );
    assert!(!answer(403, &[], "[1, 2]").rate_limited());
    assert!(!answer(403, &[], "null").rate_limited());
    assert!(!answer(404, &[], "{}").rate_limited());
    assert!(!answer(410, &[], "{}").rate_limited());
    assert!(!answer(201, &[], r#"{"number": 5}"#).rate_limited());
}

#[tokio::test]
async fn a_linked_row_and_a_calm_row_are_both_left_alone() {
    let db = TestDb::new().await.expect("a test database");
    let filed = flaky_test_in(&db, "acme/app", "p1", "filed").await;
    stamp_link(&db, filed, 5, false, false).await;
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
    )
    .await;
    assert!(filed_ids.is_empty());
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
    stamp_link(&db, wont_fix, 11, true, true).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_filed = TRUE WHERE id = $1",
            [Value::from(wont_fix)],
        ))
        .await
        .expect("the flag writes");
    let server = MockServer::start().await;

    assert!(
        github::file_issues_for(
            db.connection(),
            &[wont_fix],
            &gh_settings(&[]),
            &api(&server)
        )
        .await
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
    let (number, filed, _, closed) = test_link(&db, wont_fix).await;
    assert_eq!(number, Some(11));
    assert_eq!(filed, Some(true));
    assert!(closed);
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

    github::file_issues_for(db.connection(), &[a, b], &gh_settings(&[]), &api(&server)).await;
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

    github::file_job_issues_for(db.connection(), &[ja, jb], &gh_settings(&[]), &api(&jobs)).await;
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

    github::file_issues_for(db.connection(), &[ta, tb], &gh_settings(&[]), &api(&server)).await;
    assert_eq!(requested_paths(&server).await.len(), 1);

    let ja = flaky_job_in(&db, "acme/c", ".github/workflows/a.yml", "a").await;
    let jb = flaky_job_in(&db, "acme/d", ".github/workflows/b.yml", "b").await;
    let jobs = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&jobs)
        .await;

    github::file_job_issues_for(db.connection(), &[ja, jb], &gh_settings(&[]), &api(&jobs)).await;
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

    let logged = logged_from(async {
        github::file_issues_for(
            db.connection(),
            &[a1, a2, b],
            &gh_settings(&[]),
            &api(&server),
        )
        .await;
    })
    .await;

    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/a/issues", "/repos/acme/b/issues"],
        "the second Test of the denied Repo is never retried"
    );
    assert_eq!(test_link(&db, a1).await.0, None);
    assert_eq!(test_link(&db, a2).await.0, None);
    assert_eq!(test_link(&db, b).await.0, Some(88));

    let warnings = lines_with(&logged, "skipped for this Repo");
    assert_eq!(warnings.len(), 1, "{logged}");
    assert!(warnings[0].contains("acme/a"), "{logged}");
    assert!(warnings[0].contains("403"), "{logged}");
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

    let logged = logged_from(async {
        github::file_job_issues_for(
            db.connection(),
            &[ja1, ja2, jb],
            &gh_settings(&[]),
            &api(&jobs),
        )
        .await;
    })
    .await;
    assert_eq!(
        requested_paths(&jobs).await,
        vec!["/repos/acme/a/issues", "/repos/acme/b/issues"]
    );
    assert_eq!(job_link(&db, ja1).await.0, None);
    assert_eq!(job_link(&db, ja2).await.0, None);
    assert_eq!(job_link(&db, jb).await.0, Some(88));
    let warnings = lines_with(&logged, "skipped for this Repo");
    assert_eq!(warnings.len(), 1, "{logged}");
    assert!(warnings[0].contains("acme/a") && warnings[0].contains("403"));
    let cut: String = body.chars().skip(300).collect();
    assert!(!warnings[0].contains(&cut));
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

        let logged = logged_from(async {
            github::file_issues_for(
                db.connection(),
                &[a1, a2, b],
                &gh_settings(&[]),
                &api(&server),
            )
            .await;
        })
        .await;
        assert_eq!(
            requested_paths(&server).await,
            vec!["/repos/acme/a/issues", "/repos/acme/b/issues"],
            "the whole Repo is skipped on {status}"
        );
        assert_eq!(test_link(&db, a1).await.0, None);
        assert_eq!(test_link(&db, a2).await.0, None);
        assert_eq!(test_link(&db, b).await.0, Some(99));
        let warnings = lines_with(&logged, "skipped for this Repo");
        assert_eq!(warnings.len(), 1, "{logged}");
        assert!(warnings[0].contains(&status.to_string()), "{logged}");
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
        let logged = logged_from(async {
            github::file_job_issues_for(
                db.connection(),
                &[ja1, ja2, jb],
                &gh_settings(&[]),
                &api(&jobs),
            )
            .await;
        })
        .await;
        assert_eq!(
            requested_paths(&jobs).await,
            vec!["/repos/acme/a/issues", "/repos/acme/b/issues"]
        );
        assert_eq!(job_link(&db, ja1).await.0, None);
        assert_eq!(job_link(&db, ja2).await.0, None);
        assert_eq!(job_link(&db, jb).await.0, Some(99));
        assert_eq!(
            lines_with(&logged, "skipped for this Repo").len(),
            1,
            "{logged}"
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

    let logged = logged_from(async {
        github::file_issues_for(db.connection(), &[a1, a2], &gh_settings(&[]), &api(&server)).await;
    })
    .await;

    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/a/issues", "/repos/acme/a/issues"],
        "a 500 says nothing about the Repo, so the second candidate is still posted"
    );
    assert_eq!(test_link(&db, a1).await.0, Some(77));
    assert_eq!(test_link(&db, a2).await.0, None);
    let failures = lines_with(&logged, "GitHub issue creation failed");
    assert_eq!(failures.len(), 1, "{logged}");
    assert!(failures[0].contains("acme/a") && failures[0].contains("500"));
    assert!(failures[0].contains("server exploded"));
    assert!(
        lines_with(&logged, "skipped for this Repo").is_empty(),
        "a 500 must not skip the Repo: {logged}"
    );
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

        let logged = logged_from(async {
            github::file_issues_for(db.connection(), &[a, b], &gh_settings(&[]), &api(&server))
                .await;
        })
        .await;

        assert_eq!(
            requested_paths(&server).await,
            vec!["/repos/acme/a/issues", "/repos/acme/b/issues"],
            "{raw}"
        );
        assert_eq!(test_link(&db, a).await.0, None, "{raw}");
        assert_eq!(test_link(&db, b).await.0, Some(91), "{raw}");
        let warnings = lines_with(&logged, "201 with no issue number");
        assert_eq!(warnings.len(), 1, "{raw}: {logged}");
        assert!(warnings[0].contains("acme/a"), "{logged}");
        assert!(warnings[0].contains(raw), "{logged}");
    }
}

#[tokio::test]
async fn a_file_without_a_line_still_gets_a_permalink() {
    let db = TestDb::new().await.expect("a test database");
    let row_id = flaky_test_in(&db, "acme/app", "backend", "t_file").await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET file = 'tests/test_x.py', line = NULL WHERE id = $1",
            [Value::from(row_id)],
        ))
        .await
        .expect("the location writes");
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;

    github::file_issues_for(db.connection(), &[row_id], &gh_settings(&[]), &api(&server)).await;
    let requests = server.received_requests().await.expect("a record");
    assert_eq!(requests.len(), 1);
    let body: Json = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    let text = body["body"].as_str().expect("a body");
    assert!(
        text.contains("https://github.com/acme/app/blob/abcdef1234567890/tests/test_x.py"),
        "the permalink is still useful without an anchor: {text}"
    );
    assert!(!text.contains("#L"), "{text}");
    assert!(text.contains("`tests/test_x.py`"), "{text}");
}

#[tokio::test]
async fn a_test_with_only_passed_executions_gets_no_failure_evidence() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let quiet = make_test_case(&project)
        .name("t_passed")
        .classname("tests.test_x")
        .flakiness_score(0.6)
        .confirmed_flake_count(1)
        .file("tests/test_x.py")
        .line(7)
        .build(db.connection())
        .await
        .expect("the Test inserts");
    let run = make_run(&project)
        .commit_sha("deadbeef")
        .branch("main")
        .build(db.connection())
        .await
        .expect("the Run inserts");
    make_execution(&quiet, &run)
        .status("passed")
        .build(db.connection())
        .await
        .expect("the Execution inserts");
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;

    github::file_issues_for(
        db.connection(),
        &[quiet.id],
        &gh_settings(&[]),
        &api(&server),
    )
    .await;
    let requests = server.received_requests().await.expect("a record");
    assert_eq!(requests.len(), 1);
    let body: Json = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    let text = body["body"].as_str().expect("a body");
    assert!(!text.contains("Last failing commit"), "{text}");
    assert!(!text.contains("Permalink"), "{text}");
    assert!(!text.contains("### Sample failure"), "{text}");
    assert!(
        text.contains("### Last 10 executions"),
        "the body is not empty, it only lacks a failure: {text}"
    );
    assert!(text.contains("| passed | `deadbeef` | main |"), "{text}");
    assert_eq!(test_link(&db, quiet.id).await.0, Some(77));
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
        github::file_issues_for(db.connection(), &[under], &settings, &api(&server))
            .await
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
        github::file_issues_for(db.connection(), &[over], &settings, &api(&server))
            .await
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
        github::file_issues_for(db.connection(), &[test], &settings, &api(&server))
            .await
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
        github::file_issues_for(db.connection(), &[test], &settings, &api(&server))
            .await
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

    let filed =
        github::file_issues_for(db.connection(), &[old.id, new.id], &settings, &api(&server)).await;
    assert_eq!(
        filed.into_iter().collect::<Vec<_>>(),
        vec![new.id],
        "the failures outside the newest window do not count"
    );
    assert_eq!(test_link(&db, old.id).await.0, None);
    assert_eq!(test_link(&db, new.id).await.0, Some(91));
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

    github::file_issues_for(db.connection(), &ids, &gh_settings(&[]), &api(&server)).await;
    assert_eq!(
        requested_paths(&server).await.len(),
        1,
        "a huge id list is read in chunks instead of one oversized bind"
    );
    assert_eq!(test_link(&db, test).await.0, Some(77));
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

    let filed =
        github::file_issues_for(db.connection(), &[a, b], &gh_settings(&[]), &api(&server)).await;
    assert_eq!(filed.into_iter().collect::<Vec<_>>(), vec![b]);
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
    assert_eq!(test_link(&db, a).await.0, None);
    let (number, is_filed, _, _) = test_link(&db, b).await;
    assert_eq!(number, Some(88));
    assert_eq!(is_filed, Some(true));
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

    let logged = logged_from(async {
        github::file_job_issues_for(db.connection(), &[job], &gh_settings(&[]), &api(&server))
            .await;
    })
    .await;

    assert_eq!(
        requested_paths(&server).await,
        vec![
            "/repos/acme/app/issues",
            "/repos/acme/app/issues/77/comments",
            "/repos/acme/app/issues/77",
        ]
    );
    assert_eq!(job_link(&db, job).await.0, None);
    let warnings = lines_with(&logged, "closed the new issue");
    assert_eq!(warnings.len(), 1, "{logged}");
    assert!(warnings[0].contains("77"), "{logged}");
}

/// A failing Test uploaded by the same job attempt, so the Job's failure is explained.
async fn explain_job_execution(db: &TestDb, ci_job_id: &str) {
    let project = make_project()
        .repo("acme/app")
        .project(&format!("explained-{ci_job_id}"))
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let test = make_test_case(&project)
        .name(&format!("t-{ci_job_id}"))
        .flakiness_score(0.0)
        .confirmed_flake_count(0)
        .build(db.connection())
        .await
        .expect("the Test inserts");
    let run = make_run(&project)
        .commit_sha("s")
        .ci_job_id(ci_job_id)
        .build(db.connection())
        .await
        .expect("the Run inserts");
    make_execution(&test, &run)
        .status("failed")
        .build(db.connection())
        .await
        .expect("the Execution inserts");
}

/// The body of the one issue a job filing posted to `path`.
async fn only_issue_body(server: &MockServer) -> String {
    let requests = server.received_requests().await.expect("a record");
    let request = requests.first().expect("exactly one issue is filed");
    assert_eq!(requests.len(), 1, "exactly one issue is filed");
    let body: Json = serde_json::from_slice(&request.body).expect("the body is JSON");
    body.get("body")
        .and_then(Json::as_str)
        .expect("a body")
        .to_owned()
}

#[tokio::test]
async fn a_job_body_lists_its_worst_runner_groups_first() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job_in(
        &db,
        "acme/app",
        ".github/workflows/ci.yml",
        "e2e (ubuntu-latest)",
    )
    .await;
    let pipeline = flakeradar_db::entities::jobs::Entity::find_by_id(job)
        .one(db.connection())
        .await
        .expect("the Job reads")
        .expect("the Job exists");
    for _ in 0..2 {
        make_job_execution(&pipeline)
            .status("failed")
            .runner_labels(&["self-hosted", "gpu"])
            .build(db.connection())
            .await
            .expect("the Job execution inserts");
    }
    make_job_execution(&pipeline)
        .status("failed")
        .runner_labels(&["ubuntu-latest"])
        .build(db.connection())
        .await
        .expect("the Job execution inserts");
    make_job_execution(&pipeline)
        .status("passed")
        .runner_labels(&["windows-latest"])
        .build(db.connection())
        .await
        .expect("the Job execution inserts");
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;

    github::file_job_issues_for(db.connection(), &[job], &gh_settings(&[]), &api(&server)).await;
    let body = only_issue_body(&server).await;
    assert!(body.contains("**Failures by runner:**"), "{body}");
    let gpu = "- `gpu, self-hosted`: 2 unexplained of 2 executions";
    let ubuntu = "- `ubuntu-latest`: 1 unexplained of 1 executions";
    assert!(body.contains(gpu), "{body}");
    assert!(body.contains(ubuntu), "{body}");
    assert!(
        body.find(gpu) < body.find(ubuntu),
        "the worst group comes first: {body}"
    );
    assert!(
        !body.contains("windows-latest"),
        "a group with no unexplained failure is not listed: {body}"
    );
}

#[tokio::test]
async fn a_job_body_caps_the_runner_groups_at_three() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job_in(
        &db,
        "acme/app",
        ".github/workflows/many.yml",
        "many-runners",
    )
    .await;
    let row = flakeradar_db::entities::jobs::Entity::find_by_id(job)
        .one(db.connection())
        .await
        .expect("the Job reads")
        .expect("the Job exists");
    for (label, failures) in [("a-lab", 3), ("b-lab", 2), ("c-lab", 1), ("d-lab", 1)] {
        for _ in 0..failures {
            make_job_execution(&row)
                .status("failed")
                .runner_labels(&[label])
                .build(db.connection())
                .await
                .expect("the Job execution inserts");
        }
    }
    make_job_execution(&row)
        .status("passed")
        .runner_labels(&["d-lab"])
        .build(db.connection())
        .await
        .expect("the Job execution inserts");
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;

    github::file_job_issues_for(db.connection(), &[job], &gh_settings(&[]), &api(&server)).await;
    let body = only_issue_body(&server).await;
    for expected in [
        "- `a-lab`: 3 unexplained of 3 executions",
        "- `b-lab`: 2 unexplained of 2 executions",
        "- `c-lab`: 1 unexplained of 1 executions",
    ] {
        assert!(body.contains(expected), "{expected}: {body}");
    }
    assert!(!body.contains("d-lab"), "only three groups: {body}");
}

#[tokio::test]
async fn a_job_body_omits_the_runner_section_without_an_unexplained_failure() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job_in(
        &db,
        "acme/app",
        ".github/workflows/explained.yml",
        "explained-only",
    )
    .await;
    // The one failed execution the fixture made is explained by a failing Test the
    // same job attempt uploaded, so the Job has no unexplained failure at all.
    explain_job_execution(&db, "explained-only").await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;

    github::file_job_issues_for(db.connection(), &[job], &gh_settings(&[]), &api(&server)).await;
    let body = only_issue_body(&server).await;
    assert!(!body.contains("Failures by runner"), "{body}");
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
        github::file_job_issues_for(db.connection(), &[job], &gh_settings(&[]), &api(&server))
            .await
            .len(),
        1
    );
    assert!(
        github::file_job_issues_for(db.connection(), &[job], &gh_settings(&[]), &api(&server))
            .await
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
    let api = Api::new(Secret::new(""), &server.uri());

    assert!(
        github::file_job_issues_for(db.connection(), &[job], &settings, &api)
            .await
            .is_empty()
    );
    assert!(
        requested_paths(&server).await.is_empty(),
        "no token is a silent no-op"
    );
    assert_eq!(job_link(&db, job).await.0, None);
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
        github::file_job_issues_for(db.connection(), &[all_explained], &settings, &api(&server))
            .await
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
        github::file_job_issues_for(db.connection(), &[mixed], &settings, &api(&server))
            .await
            .len(),
        1,
        "two unexplained failures meet the gate"
    );
}

// --- the sync's rotation and its failure policy ---------------------------

/// Stamps one Job's link columns, as an earlier pass would have.
async fn stamp_job_link(db: &TestDb, id: i32, number: i32, checked: bool) {
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE jobs SET github_issue_number = $2, \
             github_issue_checked_at = CASE WHEN $3 THEN now() ELSE NULL END \
             WHERE id = $1",
            [Value::from(id), Value::from(number), Value::from(checked)],
        ))
        .await
        .expect("the stamp writes");
}

/// Whether one Job's link carries a check stamp.
async fn job_checked(db: &TestDb, id: i32) -> bool {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT github_issue_checked_at IS NOT NULL AS checked FROM jobs WHERE id = $1",
            [Value::from(id)],
        ))
        .await
        .expect("the row reads")
        .expect("the Job exists");
    row.try_get("", "checked").expect("checked")
}

/// Mounts `GET /repos/{repo}/issues/{number}` answering `answer`.
async fn mount_issue(server: &MockServer, repo: &str, number: i32, answer: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(format!("/repos/{repo}/issues/{number}")))
        .respond_with(answer)
        .mount(server)
        .await;
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

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
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

    github::sync_closed_issues(db.connection(), &settings, &api(&server)).await;
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

    github::sync_closed_issues(db.connection(), &settings, &api(&server)).await;
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

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
    assert_eq!(
        requested_paths(&server).await.len(),
        2,
        "both carry a stamp now"
    );

    let fresh = flaky_test_in(&db, "acme/app", "p3", "t3").await;
    stamp_link(&db, fresh, 33, false, false).await;
    let settings = gh_settings(&[("FLAKERADAR_GITHUB_SYNC_BATCH", "1")]);
    github::sync_closed_issues(db.connection(), &settings, &api(&server)).await;
    assert_eq!(
        requested_paths(&server).await[2..],
        ["/repos/acme/app/issues/33"],
        "the never-checked link goes first and is stamped"
    );
    assert!(test_link(&db, fresh).await.2);
}

#[tokio::test]
async fn a_sync_rate_limit_stops_the_batch_and_keeps_the_stamps_it_made() {
    let db = TestDb::new().await.expect("a test database");
    let first = flaky_test_in(&db, "acme/a", "p1", "a").await;
    let second = flaky_test_in(&db, "acme/b", "p2", "b").await;
    stamp_link(&db, first, 1, false, false).await;
    stamp_link(&db, second, 2, false, false).await;

    // An unreachable GitHub never raises: nothing is stamped, and the batch ends.
    let offline = Api::new(Secret::new(TOKEN), "http://127.0.0.1:1");
    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &offline).await;
    assert!(!test_link(&db, first).await.2);

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
    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/a/issues/1", "/repos/acme/b/issues/2"],
        "the limit is met on the second request and stops the batch"
    );
    assert!(test_link(&db, first).await.2, "the checked stamp commits");
    assert!(
        !test_link(&db, second).await.2,
        "the rate-limited row is never stamped"
    );

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
    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&second_server)).await;
    assert_eq!(
        requested_paths(&second_server).await,
        vec!["/repos/acme/b/issues/2", "/repos/acme/a/issues/1"],
        "rotation continues from the committed stamp"
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

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
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

    github::sync_closed_issues(db.connection(), &settings, &api(&server)).await;
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/a/issues/11"],
        "the cap holds the two denied rows and the second gets no GET"
    );

    github::sync_closed_issues(db.connection(), &settings, &api(&server)).await;
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

        github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&server)).await;
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

    github::sync_closed_issues(db.connection(), &settings, &api(&server)).await;
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/a/issues/11", "/repos/acme/b/issues/22"]
    );
    assert_eq!(test_link(&db, bad).await.0, Some(11));
    assert!(test_link(&db, bad).await.2, "the bad row rotates");

    github::sync_closed_issues(db.connection(), &settings, &api(&server)).await;
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
    github::file_job_issues_for(db.connection(), &[job], &gh_settings(&[]), &api(&server)).await;
    assert_eq!(job_link(&db, job).await.0, Some(77));

    let sync_server = MockServer::start().await;
    mount_issue(
        &sync_server,
        "acme/app",
        77,
        ResponseTemplate::new(200).set_body_json(json!({"state": "closed"})),
    )
    .await;
    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&sync_server)).await;
    assert_eq!(job_link(&db, job).await.0, None);

    let refile = MockServer::start().await;
    mount_create(
        &refile,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;
    assert_eq!(
        github::file_job_issues_for(db.connection(), &[job], &gh_settings(&[]), &api(&refile))
            .await
            .len(),
        1,
        "the re-armed Job is filed again"
    );
    assert_eq!(job_link(&db, job).await.0, Some(77));
    assert!(!job_checked(&db, job).await, "a new link is checked first");
}

// --- closing the issues of a deleted Repo --------------------------------

/// Mounts a comment `POST` and a close `PATCH` for one issue.
async fn mount_close(server: &MockServer, number: i32, status: u16, is_patch: bool) {
    let matcher = if is_patch {
        method("PATCH")
    } else {
        method("POST")
    };
    let route = if is_patch {
        format!("/repos/acme/app/issues/{number}")
    } else {
        format!("/repos/acme/app/issues/{number}/comments")
    };
    Mock::given(matcher)
        .and(path(route))
        .respond_with(ResponseTemplate::new(status))
        .mount(server)
        .await;
}

#[tokio::test]
async fn deleting_a_repo_comments_then_closes_each_issue() {
    let server = MockServer::start().await;
    for number in [123, 456] {
        mount_close(&server, number, 201, false).await;
        mount_close(&server, number, 200, true).await;
    }

    github::close_issues_for_deleted_repo(
        "acme/app",
        &[123, 456],
        &gh_settings(&[]),
        &api(&server),
    )
    .await;
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
    github::close_issues_for_deleted_repo("acme/app", &[], &gh_settings(&[]), &api(&empty)).await;
    github::close_issues_for_deleted_repo(
        "acme/app",
        &[123],
        &gh_settings(&[]),
        &Api::new(Secret::new(""), &empty.uri()),
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
    github::close_issues_for_deleted_repo("acme/app", &[123, 456], &gh_settings(&[]), &api(&gone))
        .await;
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
    github::close_issues_for_deleted_repo(
        "acme/app",
        &[123, 456],
        &gh_settings(&[]),
        &api(&failed_comment),
    )
    .await;
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
    let logged = logged_from(github::close_issues_for_deleted_repo(
        "acme/app",
        &[123, 456],
        &gh_settings(&[]),
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
    let logged = logged_from(github::close_issues_for_deleted_repo(
        "acme/app",
        &[123, 456],
        &gh_settings(&[]),
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
    let api = Api::new(Secret::new(TOKEN), "http://127.0.0.1:1");
    let logged = logged_from(github::close_issues_for_deleted_repo(
        "acme/app",
        &[123],
        &gh_settings(&[]),
        &api,
    ))
    .await;
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
    github::close_issues_for_deleted_repo(
        "acme/app",
        &[123, 456],
        &gh_settings(&[]),
        &api(&comment_limit),
    )
    .await;
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
    let logged = logged_from(github::close_issues_for_deleted_repo(
        "acme/app",
        &[123, 456],
        &gh_settings(&[]),
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

// --- the fixed-note batch's failure policy -------------------------------

/// Sets one Test's or Job's Clean streak.
async fn set_clean_streak(db: &TestDb, table: &str, id: i32, streak: i32) {
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("UPDATE {table} SET clean_streak = $2 WHERE id = $1"),
            [Value::from(id), Value::from(streak)],
        ))
        .await
        .expect("the streak writes");
}

/// Whether one Test or Job carries the fixed-note stamp.
async fn noted(db: &TestDb, table: &str, id: i32) -> bool {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "SELECT github_issue_fixed_noted_at IS NOT NULL AS noted FROM {table} WHERE id = $1"
            ),
            [Value::from(id)],
        ))
        .await
        .expect("the row reads")
        .expect("the row exists");
    row.try_get("", "noted").expect("noted")
}

/// Mounts the comment endpoint of issue `number`.
async fn mount_comment(server: &MockServer, repo: &str, number: i32, answer: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(format!("/repos/{repo}/issues/{number}/comments")))
        .respond_with(answer)
        .mount(server)
        .await;
}

/// Mounts the close endpoint of issue `number`.
async fn mount_patch(server: &MockServer, repo: &str, number: i32, answer: ResponseTemplate) {
    Mock::given(method("PATCH"))
        .and(path(format!("/repos/{repo}/issues/{number}")))
        .respond_with(answer)
        .mount(server)
        .await;
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

    github::note_fixed_job_issues_for(db.connection(), &[job], &settings, &api(&server)).await;

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
    github::note_fixed_issues_for(db.connection(), &[at_streak], &off, &api(&server)).await;
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

    github::note_fixed_issues_for(db.connection(), &[test], &settings, &api(&server)).await;
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
    let api = Api::new(Secret::new(TOKEN), "http://127.0.0.1:1");

    let logged = logged_from(github::note_fixed_issues_for(
        db.connection(),
        &[test],
        &fixed_settings(&[]),
        &api,
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

    github::sync_closed_issues(db.connection(), &gh_settings(&[]), &api(&sync_server)).await;
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
    let filed =
        github::file_issues_for(db.connection(), &[test], &gh_settings(&[]), &api(&filing)).await;
    assert_eq!(filed.into_iter().collect::<Vec<_>>(), vec![test]);
    assert_eq!(test_link(&db, test).await.0, Some(77));
    assert!(!test_link(&db, test).await.2, "a new link is checked first");
}

#[tokio::test]
async fn a_linked_test_and_a_linked_job_are_never_filed() {
    let db = TestDb::new().await.expect("a test database");
    let linked = flaky_test(&db).await;
    let other = flaky_test_in(&db, "acme/other", "backend", "other").await;
    stamp_link(&db, linked, 300, true, false).await;
    let job = flaky_job(&db).await;
    stamp_job_link(&db, job, 301, true).await;
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
    )
    .await;
    assert_eq!(filed.into_iter().collect::<Vec<_>>(), vec![other]);
    assert_eq!(
        requested_paths(&server).await,
        vec!["/repos/acme/other/issues"],
        "the linked Test is skipped and the other Repo is filed"
    );
    assert_eq!(test_link(&db, linked).await.0, Some(300));

    let jobs =
        github::file_job_issues_for(db.connection(), &[job], &gh_settings(&[]), &api(&server))
            .await;
    assert!(jobs.is_empty(), "a linked Job is never filed");
    assert!(
        server.received_requests().await.expect("a record").len() == 1,
        "no request for the linked Job"
    );
    assert_eq!(job_link(&db, job).await.0, Some(301));
}

// --- link writes and the races a slow request can stage --------------------

/// Runs one statement on the test's own pool from inside a stub's synchronous answer.
///
/// The fixed-note step POSTs the comment and stamps the row afterwards, so the
/// only place a concurrent admin delete or rescore can commit is while the
/// request is in flight. `Respond::respond` is synchronous and sqlx drives its
/// own socket thread, so blocking on the statement here completes even though
/// the stub's worker thread holds the runtime.
fn stage_mid_request(db: &sea_orm::DatabaseConnection, sql: String) {
    block_on(db.execute_raw(Statement::from_string(DbBackend::Postgres, sql)))
        .expect("the staged statement commits");
}

/// Drives one future to completion on the current thread.
///
/// The stub's `Respond::respond` is synchronous, so a mid-request database change
/// has to drive its query here. sqlx runs each connection's socket on its own
/// thread, so this thread can block without the reactor it is holding.
fn block_on<F: Future>(future: F) -> F::Output {
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    /// Wakes the thread that parked on one pending poll.
    struct Unpark(std::thread::Thread);

    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    let mut future = pin!(future);
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::park();
    }
}

/// The table that holds one entity's link columns.
fn link_table(entity: &str) -> &'static str {
    if entity == "test" {
        "test_cases"
    } else {
        "jobs"
    }
}

/// A Test (`test`) or Job (`job`) named `name` in `repo`, linked to `number`
/// when one is given, and at `streak` passing runs.
async fn note_row(
    db: &TestDb,
    entity: &str,
    repo: &str,
    name: &str,
    number: Option<i32>,
    streak: i32,
) -> i32 {
    let id = if entity == "test" {
        flaky_test_in(db, repo, name, name).await
    } else {
        flaky_job_in(db, repo, &format!(".github/workflows/{name}.yml"), name).await
    };
    if let Some(number) = number {
        if entity == "test" {
            stamp_link(db, id, number, false, false).await;
        } else {
            stamp_job_link(db, id, number, false).await;
        }
    }
    set_clean_streak(db, link_table(entity), id, streak).await;
    id
}

/// The fixed-note entry point of one entity.
async fn note_entity(db: &TestDb, entity: &str, ids: &[i32], settings: &Settings, api: &Api) {
    if entity == "test" {
        github::note_fixed_issues_for(db.connection(), ids, settings, api).await;
    } else {
        github::note_fixed_job_issues_for(db.connection(), ids, settings, api).await;
    }
}

/// The method and path of every request a stub server was asked for, in order.
async fn requested_calls(server: &MockServer) -> Vec<(String, String)> {
    server
        .received_requests()
        .await
        .expect("the stub records requests")
        .iter()
        .map(|request| {
            (
                request.method.as_str().to_owned(),
                request.url.path().to_owned(),
            )
        })
        .collect()
}

/// One Test's or Job's Clean streak.
async fn clean_streak_of(db: &TestDb, table: &str, id: i32) -> i32 {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT clean_streak FROM {table} WHERE id = $1"),
            [Value::from(id)],
        ))
        .await
        .expect("the row reads")
        .expect("the row exists");
    row.try_get("", "clean_streak").expect("a streak")
}

/// One Test's or Job's stored issue number.
async fn number_of(db: &TestDb, table: &str, id: i32) -> Option<i32> {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT github_issue_number FROM {table} WHERE id = $1"),
            [Value::from(id)],
        ))
        .await
        .expect("the row reads")
        .expect("the row exists");
    row.try_get("", "github_issue_number").expect("a number")
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
        LinkTarget::Test,
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
    issue_links::store_filed_link(db.connection(), LinkTarget::Test, i64::from(relinked), 44)
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
        LinkTarget::Job,
        i64::from(job),
        301,
        None,
        false,
        &reads,
    )
    .await
    .expect("the link succeeds");
    github::sync_closed_issues(db.connection(), &settings, &api(&server)).await;
    assert!(
        job_checked(&db, job).await,
        "an open issue is still a check"
    );

    issue_links::link_issue(
        db.connection(),
        LinkTarget::Job,
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
    github::sync_closed_issues(db.connection(), &settings, &api(&server)).await;
    assert!(job_checked(&db, job).await, "the new link is checked");

    issue_links::unlink_issue(db.connection(), LinkTarget::Job, i64::from(job), &reads)
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
async fn the_note_step_skips_the_link_this_same_pass_filed() {
    for entity in ["test", "job"] {
        let db = TestDb::new().await.expect("a test database");
        let obj = note_row(&db, entity, "acme/app", "fresh", None, 2).await;
        let table = link_table(entity);
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
        let outcome = if entity == "test" {
            processed(&[obj], &[])
        } else {
            processed(&[], &[obj])
        };

        github::on_report_processed(db.connection(), &outcome, &settings, &api(&server)).await;
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
        assert!(!noted(&db, table, obj).await);
        assert_eq!(
            clean_streak_of(&db, table, obj).await,
            3,
            "and it is noteable from the next Report on"
        );
    }
}

#[tokio::test]
async fn a_processed_report_never_notes_the_issue_it_filed() {
    for entity in ["test", "job"] {
        let db = TestDb::new().await.expect("a test database");
        let live = note_row(&db, entity, "acme/app", "live", None, 0).await;
        let fresh = note_row(&db, entity, "acme/app", "fresh", None, 3).await;
        let linked = note_row(&db, entity, "acme/app", "linked", Some(88), 3).await;
        let table = link_table(entity);
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
        let outcome = if entity == "test" {
            processed(&[live, fresh, linked], &[])
        } else {
            processed(&[], &[live, fresh, linked])
        };

        github::on_report_processed(db.connection(), &outcome, &settings, &api(&server)).await;
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

        github::on_report_processed(db.connection(), &outcome, &settings, &api(&server)).await;
        assert_eq!(
            requested_calls(&server).await.len(),
            3,
            "the next Report files no Issue and comments on none: that is the steady state"
        );
    }
}
