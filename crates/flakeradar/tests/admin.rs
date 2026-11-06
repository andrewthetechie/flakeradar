//! Admin delete of a Repo or one Project, with the cascade, the counts and the
//! issues it strands.
//!
//! The GitHub closer is proved through a stub server, so a test asserts the
//! requests it makes rather than a call count.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a fixture or an answer does not behave"
)]

use std::sync::Arc;

use axum::Router;
use axum::http::{Method, StatusCode};
use flakeradar::{app::AppState, app::build_app, github::Api};
use flakeradar_test_support::{
    Reply, TestDb, get, make_execution, make_job, make_job_execution, make_pipeline, make_project,
    make_report, make_run, make_test_case, request, request_with_headers, settings,
};

use sea_orm::ConnectionTrait as _;
use sea_orm::Statement;
use sea_orm::{DbBackend, Value};
use serde_json::{Value as Json, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The token every admin route needs.
const TOKEN: &str = "test-token";

/// The GitHub token the closer presents, which no log line may carry.
const GITHUB_TOKEN: &str = "ghs_super_secret_token";

/// An app whose GitHub client talks to `base`.
fn app_with_github(db: &TestDb, base: &str) -> Router {
    let settings = settings(&[("FLAKERADAR_API_TOKEN", TOKEN)]);
    let api = Api::new(flakeradar::settings::Secret::new(GITHUB_TOKEN), base)
        .expect("the HTTP client builds");
    build_app(
        AppState::new(db.connection().clone(), Arc::new(settings))
            .expect("the app state builds")
            .with_github(api),
    )
}

/// An app whose GitHub client is configured but points nowhere reachable.
fn app_without_github(db: &TestDb) -> Router {
    let settings = settings(&[("FLAKERADAR_API_TOKEN", TOKEN)]);
    build_app(
        AppState::new(db.connection().clone(), Arc::new(settings)).expect("the app state builds"),
    )
}

/// Sends one authorized `DELETE`.
async fn delete(app: &Router, uri: &str) -> Reply {
    request_with_headers(app, Method::DELETE, uri, &[("X-API-Key", TOKEN)]).await
}

/// The number of rows in `table`, optionally where `id = $1`.
async fn count(db: &TestDb, table: &str, id: Option<i32>) -> i64 {
    let sql = match id {
        Some(_) => format!("SELECT count(*) AS value FROM {table} WHERE id = $1"),
        None => format!("SELECT count(*) AS value FROM {table}"),
    };
    let values = match id {
        Some(id) => vec![Value::from(id)],
        None => Vec::new(),
    };
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await
        .expect("the count reads")
        .expect("a count query answers one row");
    row.try_get("", "value").expect("a count")
}

/// How many Reports still name `project_id`.
async fn project_reports(db: &TestDb, project_id: i32) -> i64 {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS value FROM reports WHERE project_id = $1",
            [Value::from(project_id)],
        ))
        .await
        .expect("the count reads")
        .expect("a count query answers one row");
    row.try_get("", "value").expect("a count")
}

/// A deferred `AFTER DELETE` trigger on `table` that raises `sqlstate` at commit.
async fn raise_on_commit(db: &TestDb, table: &str, sqlstate: &str) {
    let function = format!(
        "CREATE FUNCTION refuse_commit() RETURNS trigger AS $$ BEGIN \
         RAISE EXCEPTION 'staged commit failure' USING ERRCODE = '{sqlstate}'; END; $$ LANGUAGE plpgsql"
    );
    let trigger = format!(
        "CREATE CONSTRAINT TRIGGER refuse_commit AFTER DELETE ON {table} \
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION refuse_commit()"
    );
    for sql in [function, trigger] {
        db.connection()
            .execute_raw(Statement::from_string(DbBackend::Postgres, sql))
            .await
            .expect("the staged failure installs");
    }
}

/// A `BEFORE DELETE` trigger on `table` that raises `sqlstate`.
async fn raise_on_delete(db: &TestDb, table: &str, sqlstate: &str) {
    let function = format!(
        "CREATE FUNCTION refuse_delete() RETURNS trigger AS $$ BEGIN \
         RAISE EXCEPTION 'staged failure' USING ERRCODE = '{sqlstate}'; END; $$ LANGUAGE plpgsql"
    );
    let trigger = format!(
        "CREATE TRIGGER refuse_delete BEFORE DELETE ON {table} \
         FOR EACH ROW EXECUTE FUNCTION refuse_delete()"
    );
    for sql in [function, trigger] {
        db.connection()
            .execute_raw(Statement::from_string(DbBackend::Postgres, sql))
            .await
            .expect("the staged failure installs");
    }
}

/// The Projects the Repos listing names for one Repo.
async fn projects_of(app: &Router, repo: &str) -> Vec<String> {
    let reply = get(app, "/api/repos").await;
    reply
        .json()
        .as_array()
        .expect("repos is a list")
        .iter()
        .find(|entry| entry["name"] == json!(repo))
        .map(|entry| {
            entry["projects"]
                .as_array()
                .expect("projects is a list")
                .iter()
                .map(|project| project["name"].as_str().expect("a name").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the test walks one Project delete end to end, including what survives"
)]
async fn deleting_a_project_removes_only_its_own_rows() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let tests = [
        make_test_case(&project)
            .name("t0")
            .build(db.connection())
            .await
            .expect("a Test"),
        make_test_case(&project)
            .name("t1")
            .build(db.connection())
            .await
            .expect("a Test"),
    ];
    let run = make_run(&project)
        .commit_sha("sha0")
        .build(db.connection())
        .await
        .expect("the Run inserts");
    make_execution(&tests[0], &run)
        .status("failed")
        .build(db.connection())
        .await
        .expect("the Execution inserts");
    make_execution(&tests[1], &run)
        .status("passed")
        .build(db.connection())
        .await
        .expect("the Execution inserts");
    make_report(&project, project.repo_id)
        .commit_sha("sha1")
        .build(db.connection())
        .await
        .expect("the pending Report inserts");
    let sibling = make_project()
        .repo("acme/app")
        .project("frontend")
        .build(db.connection())
        .await
        .expect("the sibling Project inserts");
    let sibling_test = make_test_case(&sibling)
        .name("ui")
        .build(db.connection())
        .await
        .expect("the sibling Test inserts");
    let pipeline_report = make_report(&project, project.repo_id)
        .kind("pipeline")
        .build(db.connection())
        .await
        .expect("the pipeline Report inserts");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    let job = make_job(&pipeline)
        .name("test")
        .build(db.connection())
        .await
        .expect("the Job inserts");
    let app = app_without_github(&db);

    let reply = delete(&app, "/api/repos/acme/app/projects/backend").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.json(),
        json!({
            "repo": "acme/app",
            "project": "backend",
            "deleted": {"tests": 2, "executions": 2, "jobs": 0, "reports": 1},
            "github_issue_numbers": [],
        })
    );
    for (table, id) in [
        ("projects", Some(project.id)),
        ("test_cases", Some(tests[0].id)),
        ("test_runs", Some(run.id)),
    ] {
        assert_eq!(count(&db, table, id).await, 0, "{table} is gone");
    }
    assert_eq!(
        project_reports(&db, project.id).await,
        0,
        "the pending Report is gone"
    );
    assert_eq!(count(&db, "test_cases", Some(sibling_test.id)).await, 1);
    assert_eq!(count(&db, "jobs", Some(job.id)).await, 1);
    assert_eq!(count(&db, "repos", None).await, 1);
    assert_eq!(
        count(&db, "reports", Some(pipeline_report.id)).await,
        1,
        "a pipeline Report is not this Project's"
    );
    assert_eq!(projects_of(&app, "acme/app").await, vec!["frontend"]);

    let listed = get(&app, "/api/reports").await;
    let ids: Vec<i64> = listed
        .json()
        .as_array()
        .expect("reports is a list")
        .iter()
        .map(|report| report["id"].as_i64().expect("an id"))
        .collect();
    assert_eq!(ids, vec![i64::from(pipeline_report.id)]);
}

#[tokio::test]
async fn deleting_a_repo_removes_everything_in_it() {
    let db = TestDb::new().await.expect("a test database");
    let backend = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let tests = [
        make_test_case(&backend)
            .name("t0")
            .build(db.connection())
            .await
            .expect("a Test"),
        make_test_case(&backend)
            .name("t1")
            .build(db.connection())
            .await
            .expect("a Test"),
    ];
    let runs = [
        make_run(&backend)
            .commit_sha("sha0")
            .build(db.connection())
            .await
            .expect("a Run"),
        make_run(&backend)
            .commit_sha("sha1")
            .build(db.connection())
            .await
            .expect("a Run"),
    ];
    for (index, run) in runs.iter().enumerate() {
        make_execution(&tests[index % 2], run)
            .status("failed")
            .build(db.connection())
            .await
            .expect("the Execution inserts");
    }
    let frontend = make_project()
        .repo("acme/app")
        .project("frontend")
        .build(db.connection())
        .await
        .expect("the sibling Project inserts");
    make_test_case(&frontend)
        .name("ui")
        .build(db.connection())
        .await
        .expect("the sibling Test inserts");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    let job = make_job(&pipeline)
        .name("test")
        .build(db.connection())
        .await
        .expect("the Job inserts");
    make_job_execution(&job)
        .status("failed")
        .build(db.connection())
        .await
        .expect("the Job execution inserts");
    make_report(&backend, backend.repo_id)
        .commit_sha("sha1")
        .build(db.connection())
        .await
        .expect("the pending Report inserts");
    make_report(&backend, backend.repo_id)
        .kind("pipeline")
        .build(db.connection())
        .await
        .expect("the pipeline Report inserts");
    let app = app_without_github(&db);

    let reply = delete(&app, "/api/repos/acme/app").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.json(),
        json!({
            "repo": "acme/app",
            "deleted": {"tests": 3, "executions": 2, "jobs": 1, "reports": 2},
            "github_issue_numbers": [],
        })
    );
    for table in [
        "repos",
        "projects",
        "test_cases",
        "test_runs",
        "test_executions",
        "pipelines",
        "jobs",
        "job_executions",
        "reports",
    ] {
        assert_eq!(count(&db, table, None).await, 0, "{table} is empty");
    }
    assert_eq!(get(&app, "/api/repos").await.json(), json!([]));
}

#[tokio::test]
async fn a_project_delete_reports_its_tests_issue_numbers_and_strands_no_job() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    make_test_case(&project)
        .name("t0")
        .github_issue_number(123)
        .build(db.connection())
        .await
        .expect("the Test inserts");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    let job = make_job(&pipeline)
        .github_issue_number(456)
        .build(db.connection())
        .await
        .expect("the Job inserts");
    let app = app_without_github(&db);

    let reply = delete(&app, "/api/repos/acme/app/projects/backend").await;
    assert_eq!(reply.json()["github_issue_numbers"], json!([123]));
    assert_eq!(count(&db, "jobs", Some(job.id)).await, 1);
}

#[tokio::test]
async fn a_repo_delete_reports_both_scopes_sorted_and_deduplicated() {
    let db = TestDb::new().await.expect("a test database");
    let backend = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    make_test_case(&backend)
        .github_issue_number(123)
        .build(db.connection())
        .await
        .expect("the Test inserts");
    let frontend = make_project()
        .repo("acme/app")
        .project("frontend")
        .build(db.connection())
        .await
        .expect("the sibling Project inserts");
    make_test_case(&frontend)
        .name("ui")
        .github_issue_number(123)
        .build(db.connection())
        .await
        .expect("the Test inserts");
    make_test_case(&frontend)
        .name("ui2")
        .build(db.connection())
        .await
        .expect("the Test inserts");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    make_job(&pipeline)
        .github_issue_number(456)
        .build(db.connection())
        .await
        .expect("the Job inserts");
    let app = app_without_github(&db);

    let reply = delete(&app, "/api/repos/acme/app").await;
    let body = reply.json();
    assert_eq!(body["github_issue_numbers"], json!([123, 456]));
    assert_eq!(
        body["deleted"],
        json!({"tests": 3, "executions": 0, "jobs": 1, "reports": 0})
    );
}

/// Mounts the comment and close endpoints an admin delete calls.
async fn mount_closer(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/123/comments"))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/456/comments"))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/123"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/456"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(server)
        .await;
}

/// One FlakeRadar-filed Test issue and one filed Job issue, both in `acme/app`.
async fn seed_filed_issues(db: &TestDb) {
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let test = make_test_case(&project)
        .name("t0")
        .github_issue_number(123)
        .build(db.connection())
        .await
        .expect("the Test inserts");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    let job = make_job(&pipeline)
        .github_issue_number(456)
        .build(db.connection())
        .await
        .expect("the Job inserts");
    for (table, id) in [("test_cases", test.id), ("jobs", job.id)] {
        db.connection()
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!("UPDATE {table} SET github_issue_filed = TRUE WHERE id = $1"),
                [Value::from(id)],
            ))
            .await
            .expect("the provenance writes");
    }
}

#[tokio::test]
async fn a_repo_delete_comments_on_and_closes_every_issue_flakeradar_filed() {
    let db = TestDb::new().await.expect("a test database");
    let server = MockServer::start().await;
    mount_closer(&server).await;
    seed_filed_issues(&db).await;
    let app = app_with_github(&db, &server.uri());

    let reply = delete(&app, "/api/repos/acme/app").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.json(),
        json!({
            "repo": "acme/app",
            "deleted": {"tests": 1, "executions": 0, "jobs": 1, "reports": 0},
            "github_issue_numbers": [123, 456],
        })
    );
    // `expect(1)` on every stub is checked when the server drops.
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert_eq!(requests.len(), 4);
    let comment = requests
        .iter()
        .find(|request| request.url.path() == "/repos/acme/app/issues/123/comments")
        .expect("the comment was posted");
    let body: Json = serde_json::from_slice(&comment.body).expect("the comment body is JSON");
    assert_eq!(
        body["body"],
        json!(
            "FlakeRadar deleted the tracked flaky test or CI job this issue was filed for via the \
             admin delete API, so the permalinks in this issue no longer resolve. Closing it \
             automatically; if the test or job is uploaded again, FlakeRadar files a fresh issue."
        )
    );
    let authorization = comment
        .headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .expect("the request carries the token header");
    assert_eq!(authorization, format!("Bearer {GITHUB_TOKEN}"));
}

#[tokio::test]
async fn a_confirming_stub_proves_the_token_never_reaches_a_log_line() {
    use std::io::Write as _;
    use std::sync::Mutex;

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

    let db = TestDb::new().await.expect("a test database");
    let server = MockServer::start().await;
    // A failing answer, so every log path of a refused close runs.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Bad credentials"))
        .mount(&server)
        .await;
    seed_filed_issues(&db).await;
    let app = app_with_github(&db, &server.uri());

    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || Sink(Arc::clone(&sink)))
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    let reply = delete(&app, "/api/repos/acme/app").await;
    drop(guard);

    assert_eq!(reply.status, StatusCode::OK);
    let logged = String::from_utf8_lossy(
        &lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .into_owned();
    assert!(!logged.is_empty(), "the refused close logged something");
    assert!(
        !logged.contains(GITHUB_TOKEN),
        "no log line carries the token: {logged}"
    );
    assert!(
        !logged.contains(&server.uri()),
        "no log line carries the API base URL: {logged}"
    );
    let _ = std::io::stdout().flush();
}

#[tokio::test]
async fn a_project_delete_closes_only_its_own_tests_issues() {
    let db = TestDb::new().await.expect("a test database");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/123/comments"))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/123"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    seed_filed_issues(&db).await;
    let app = app_with_github(&db, &server.uri());

    let reply = delete(&app, "/api/repos/acme/app/projects/backend").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["github_issue_numbers"], json!([123]));
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert_eq!(
        requests.len(),
        2,
        "the Job's issue is not this route's to close"
    );
}

#[tokio::test]
async fn an_issue_a_human_linked_is_reported_but_never_closed() {
    let db = TestDb::new().await.expect("a test database");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/123/comments"))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/123"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let filed = make_test_case(&project)
        .name("filed")
        .github_issue_number(123)
        .build(db.connection())
        .await
        .expect("the filed Test inserts");
    make_test_case(&project)
        .name("linked")
        .github_issue_number(124)
        .build(db.connection())
        .await
        .expect("the linked Test inserts");
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_filed = TRUE WHERE id = $1",
            [Value::from(filed.id)],
        ))
        .await
        .expect("the provenance writes");
    let app = app_with_github(&db, &server.uri());

    let reply = delete(&app, "/api/repos/acme/app/projects/backend").await;
    assert_eq!(reply.json()["github_issue_numbers"], json!([123, 124]));
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert!(
        requests
            .iter()
            .all(|request| !request.url.path().contains("/issues/124")),
        "a human-linked issue is never commented on or closed"
    );
}

#[tokio::test]
async fn a_delete_that_did_not_happen_closes_nothing() {
    let db = TestDb::new().await.expect("a test database");
    let server = MockServer::start().await;
    seed_filed_issues(&db).await;
    let app = app_with_github(&db, &server.uri());

    assert_eq!(
        delete(&app, "/api/repos/acme/none").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        delete(&app, "/api/repos/acme/app/projects/none")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        delete(&app, "/api/repos/ac!me/app").await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    raise_on_delete(&db, "repos", "40P01").await;
    assert_eq!(
        delete(&app, "/api/repos/acme/app").await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert!(requests.is_empty(), "no refused delete reaches GitHub");
}

#[tokio::test]
async fn a_project_delete_refuses_a_name_the_identity_rules_refuse() {
    let db = TestDb::new().await.expect("a test database");
    make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let app = app_without_github(&db);

    let refused = delete(&app, "/api/repos/acme/app%20name/projects/backend").await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        refused.json()["detail"]
            .as_str()
            .expect("a detail string")
            .contains("app name"),
        "{refused:?}"
    );
    assert_eq!(count(&db, "repos", None).await, 1, "nothing was deleted");
}

#[tokio::test]
async fn the_admin_routes_need_the_token() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let _ = project;
    let app = app_without_github(&db);

    for uri in [
        "/api/repos/acme/app",
        "/api/repos/acme/app/projects/backend",
    ] {
        assert_eq!(
            request(&app, Method::DELETE, uri).await.status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request_with_headers(&app, Method::DELETE, uri, &[("X-API-Key", "wrong")])
                .await
                .status,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(count(&db, "repos", None).await, 1);
}

#[tokio::test]
async fn unknown_names_are_404s_that_name_what_is_missing() {
    let db = TestDb::new().await.expect("a test database");
    make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let app = app_without_github(&db);

    let missing_repo = delete(&app, "/api/repos/acme/none").await;
    assert_eq!(missing_repo.status, StatusCode::NOT_FOUND);
    assert_eq!(missing_repo.json()["detail"], json!("Repo not found"));

    let missing_project = delete(&app, "/api/repos/acme/app/projects/none").await;
    assert_eq!(missing_project.status, StatusCode::NOT_FOUND);
    assert_eq!(missing_project.json()["detail"], json!("Project not found"));

    let unknown_repo = delete(&app, "/api/repos/acme/none/projects/backend").await;
    assert_eq!(unknown_repo.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown_repo.json()["detail"], json!("Repo not found"));
    assert_eq!(count(&db, "repos", None).await, 1);
}

#[tokio::test]
async fn malformed_names_are_refused_before_anything_is_deleted() {
    let db = TestDb::new().await.expect("a test database");
    make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let app = app_without_github(&db);

    let bad_repo = delete(&app, "/api/repos/ac!me/app").await;
    assert_eq!(bad_repo.status, StatusCode::UNPROCESSABLE_ENTITY);
    let detail = bad_repo.json()["detail"]
        .as_str()
        .expect("a detail")
        .to_owned();
    assert!(detail.contains("owner/name"), "{detail}");

    let bad_project = delete(&app, "/api/repos/acme/app/projects/bad*project").await;
    assert_eq!(bad_project.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(count(&db, "repos", None).await, 1);
}

#[tokio::test]
async fn the_names_are_normalized_before_the_lookup() {
    let db = TestDb::new().await.expect("a test database");
    make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let app = app_without_github(&db);

    let reply = delete(&app, "/api/repos/ACME/App").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["repo"], json!("acme/app"));
    assert_eq!(count(&db, "repos", None).await, 0);
}

#[tokio::test]
async fn a_deadlock_with_the_processor_is_a_retryable_503() {
    let db = TestDb::new().await.expect("a test database");
    make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    raise_on_delete(&db, "repos", "40P01").await;
    let app = app_without_github(&db);

    let reply = delete(&app, "/api/repos/acme/app").await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        reply.json()["detail"],
        json!("Delete raced with the report processor; retry.")
    );
    // The aborted transaction left the whole Repo behind for a retry.
    assert_eq!(count(&db, "repos", None).await, 1);
    assert_eq!(count(&db, "projects", None).await, 1);
}

#[tokio::test]
async fn a_serialization_failure_is_the_same_retryable_503() {
    let db = TestDb::new().await.expect("a test database");
    make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    raise_on_delete(&db, "projects", "40001").await;
    let app = app_without_github(&db);

    let reply = delete(&app, "/api/repos/acme/app/projects/backend").await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(count(&db, "projects", None).await, 1);
}

#[tokio::test]
async fn any_other_database_error_is_a_500() {
    for (table, path) in [
        ("repos", "/api/repos/acme/app"),
        ("projects", "/api/repos/acme/app/projects/backend"),
    ] {
        let db = TestDb::new().await.expect("a test database");
        make_project()
            .repo("acme/app")
            .project("backend")
            .build(db.connection())
            .await
            .expect("the Project inserts");
        raise_on_delete(&db, table, "23503").await;
        let app = app_without_github(&db);

        let reply = delete(&app, path).await;
        assert_eq!(
            reply.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{path} propagates a non-deadlock error"
        );
        assert_eq!(count(&db, "repos", None).await, 1);
        assert_eq!(count(&db, "projects", None).await, 1);
    }
}

/// Drives one future to completion on the current thread.
///
/// A stub's `Respond::respond` is synchronous, so a mid-request database read
/// has to drive its query here. sqlx runs each connection's socket on its own
/// thread, so this thread can block without the reactor it is holding.
fn block_on<F: Future>(future: F) -> F::Output {
    use std::pin::pin;
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

#[tokio::test]
async fn the_issues_close_only_after_the_commit() {
    use std::sync::Mutex;

    let db = TestDb::new().await.expect("a test database");
    let server = MockServer::start().await;
    seed_filed_issues(&db).await;
    let visible = Arc::new(Mutex::new(Vec::<i64>::new()));
    let seen = Arc::clone(&visible);
    let connection = db.connection().clone();
    Mock::given(method("POST"))
        .respond_with(move |_request: &wiremock::Request| {
            let connection = connection.clone();
            let count = block_on(async move {
                let row = connection
                    .query_one_raw(Statement::from_string(
                        DbBackend::Postgres,
                        "SELECT count(*) AS value FROM repos",
                    ))
                    .await
                    .expect("the count reads")
                    .expect("a count query answers one row");
                row.try_get::<i64>("", "value").expect("a count")
            });
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(count);
            ResponseTemplate::new(201)
        })
        .mount(&server)
        .await;
    let app = app_with_github(&db, &server.uri());

    let reply = delete(&app, "/api/repos/acme/app").await;
    assert_eq!(reply.status, StatusCode::OK);
    let counts = visible
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert!(!counts.is_empty(), "the close ran");
    assert!(
        counts.iter().all(|count| *count == 0),
        "the Repo was already deleted when the close ran: {counts:?}"
    );
}

#[tokio::test]
async fn a_deadlock_at_commit_is_the_same_retryable_503() {
    for (table, path, sqlstate) in [
        ("repos", "/api/repos/acme/app", "40P01"),
        ("projects", "/api/repos/acme/app/projects/backend", "40001"),
    ] {
        let db = TestDb::new().await.expect("a test database");
        make_project()
            .repo("acme/app")
            .project("backend")
            .build(db.connection())
            .await
            .expect("the Project inserts");
        raise_on_commit(&db, table, sqlstate).await;
        let app = app_without_github(&db);

        let reply = delete(&app, path).await;
        assert_eq!(
            reply.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{path}/{sqlstate}"
        );
        assert_eq!(
            reply.json()["detail"],
            json!("Delete raced with the report processor; retry.")
        );
        assert_eq!(count(&db, "repos", None).await, 1);
        assert_eq!(count(&db, "projects", None).await, 1);
    }

    for (table, path) in [
        ("repos", "/api/repos/acme/app"),
        ("projects", "/api/repos/acme/app/projects/backend"),
    ] {
        let db = TestDb::new().await.expect("a test database");
        make_project()
            .repo("acme/app")
            .project("backend")
            .build(db.connection())
            .await
            .expect("the Project inserts");
        raise_on_commit(&db, table, "23503").await;
        let app = app_without_github(&db);

        let reply = delete(&app, path).await;
        assert_eq!(
            reply.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "a non-deadlock commit failure on {path} propagates"
        );
        assert_eq!(count(&db, "repos", None).await, 1);
        assert_eq!(count(&db, "projects", None).await, 1);
    }
}
