//! The MCP server over its own transport: the token, the redirect and the tools.
//!
//! Every test but the socket ones drives the app with `tower`'s `oneshot`; the
//! requests are the JSON-RPC documents a client sends, so what is under test is
//! the wire a client sees.

#![expect(
    clippy::expect_used,
    reason = "a test that cannot build its request or read its answer has failed"
)]

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use flakeradar_test_support::{
    TestDb, make_execution, make_job, make_job_execution, make_job_score, make_pipeline,
    make_project, make_run, make_test_case, make_test_score,
};
use serde_json::{Value, json};

use common::{Reply, app, send};

/// The token every request in this file presents.
const TOKEN: &str = "mcp-test-token";

/// The protocol version every request in this file declares.
const VERSION: &str = "2026-07-28";

/// The app with the token these tests present.
fn mcp_app(db: &TestDb) -> Router {
    app(db, &[("FLAKERADAR_API_TOKEN", TOKEN)])
}

/// The `_meta` a request that declares [`VERSION`] must carry.
fn per_request_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": VERSION,
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

/// Sends one JSON-RPC request to the transport with `headers`.
async fn rpc_with(app: &Router, body: &Value, headers: &[(&str, &str)]) -> Reply {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp/")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", VERSION);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    send(
        app,
        builder
            .body(Body::from(body.to_string()))
            .expect("the request builds"),
    )
    .await
}

/// Sends one JSON-RPC request as a client holding the token.
async fn rpc(app: &Router, method: &str, name: Option<&str>, params: Value) -> Reply {
    let mut params = params;
    if let Some(fields) = params.as_object_mut() {
        let _ = fields.insert("_meta".to_owned(), per_request_meta());
    }
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let mut headers = vec![
        ("authorization", format!("Bearer {TOKEN}")),
        ("mcp-method", method.to_owned()),
    ];
    if let Some(name) = name {
        headers.push(("mcp-name", name.to_owned()));
    }
    let borrowed: Vec<(&str, &str)> = headers
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    rpc_with(app, &body, &borrowed).await
}

/// Calls one tool and answers its JSON-RPC `result`.
async fn call_result(app: &Router, tool: &str, arguments: Value) -> Value {
    let reply = rpc(
        app,
        "tools/call",
        Some(tool),
        json!({"name": tool, "arguments": arguments}),
    )
    .await;
    reply.json().get("result").cloned().unwrap_or(Value::Null)
}

/// Asserts the status the reference answers a tool whose statement failed.
///
/// fastmcp 4.1.6, the version `backend/requirements.txt` pins, carries a tool's
/// failure inside the JSON-RPC answer and answers HTTP 200: driven over its own
/// Streamable HTTP app, a tool that raises answers 200, so a failed statement
/// must never become an HTTP 500.
fn assert_answered_ok(reply: &Reply) {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "the reference answers 200 for a failed statement: {}",
        reply.text()
    );
}

/// Calls one tool and answers what a client reads as its data.
fn data(result: &Value) -> Value {
    let structured = result
        .get("structuredContent")
        .cloned()
        .unwrap_or(Value::Null);
    let unwrap = structured
        .as_object()
        .is_some_and(|fields| fields.len() == 1 && fields.contains_key("result"));
    if unwrap {
        structured.get("result").cloned().unwrap_or(Value::Null)
    } else {
        structured
    }
}

/// Calls one tool and answers its data, asserting it succeeded.
async fn call(app: &Router, tool: &str, arguments: Value) -> Value {
    let result = call_result(app, tool, arguments).await;
    assert_eq!(
        result.get("isError"),
        Some(&Value::Bool(false)),
        "{tool} refused: {}",
        text(&result)
    );
    data(&result)
}

/// The text of a result's first content block.
fn text(result: &Value) -> String {
    result
        .get("content")
        .and_then(|content| content.get(0))
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Calls one tool and asserts it refused with `message`.
async fn refused(app: &Router, tool: &str, arguments: Value, message: &str) {
    let result = call_result(app, tool, arguments).await;
    assert_eq!(
        result.get("isError"),
        Some(&Value::Bool(true)),
        "{tool} did not refuse: {result}"
    );
    assert!(
        text(&result).contains(message),
        "{tool}: {:?} does not contain {message:?}",
        text(&result)
    );
}

/// The names of a list answer.
fn names(items: &Value) -> Vec<String> {
    items
        .as_array()
        .expect("the answer is a list")
        .iter()
        .map(|item| item["name"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// A date `days` before today, which the Score history filters read.
fn day(days: i64) -> chrono::NaiveDate {
    (Utc::now() - chrono::Duration::days(days)).date_naive()
}

/// The Repo, Projects, Tests and Executions the read tools are asserted over.
///
/// It is the reference suite's `_seed`: one Repo with two Projects, a flaky Test
/// in each, a suspect Test, a stable one, and two Executions of the flaky
/// frontend Test, the failing one carrying a 5000-character traceback.
async fn seed(db: &TestDb) -> i64 {
    let frontend = make_project()
        .repo("andrewthetechie/writers-app")
        .project("frontend")
        .root("frontend")
        .build(db.connection())
        .await
        .expect("the frontend Project");
    let backend = make_project()
        .repo("andrewthetechie/writers-app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the backend Project");
    let flaky = make_test_case(&frontend)
        .name("renders")
        .classname("src/app.test.ts")
        .file("src/app.test.ts")
        .line(12)
        .flakiness_score(0.7)
        .confirmed_flake_count(1)
        .build(db.connection())
        .await
        .expect("the flaky Test");
    make_test_case(&frontend)
        .name("suspect_one")
        .flakiness_score(0.1)
        .build(db.connection())
        .await
        .expect("the suspect Test");
    make_test_case(&backend)
        .name("renders")
        .classname("tests.test_views")
        .file("tests/test_views.py")
        .flakiness_score(0.4)
        .build(db.connection())
        .await
        .expect("the backend Test");
    make_test_case(&backend)
        .name("calm")
        .flakiness_score(0.0)
        .build(db.connection())
        .await
        .expect("the stable Test");
    let run = make_run(&frontend)
        .commit_sha("aaa111")
        .branch("main")
        .build(db.connection())
        .await
        .expect("the Run");
    make_execution(&flaky, &run)
        .status("failed")
        .message("expected 3")
        .details(&format!("Traceback\n{}", "x".repeat(5000)))
        .failure_category("assertion")
        .build(db.connection())
        .await
        .expect("the failing Execution");
    make_execution(&flaky, &run)
        .status("passed")
        .build(db.connection())
        .await
        .expect("the passing Execution");
    i64::from(flaky.id)
}

#[tokio::test]
async fn the_tools_are_the_eleven_the_frozen_server_served() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let reply = rpc(&app, "tools/list", None, json!({})).await;
    let tools = reply.json()["result"]["tools"].clone();
    let mut found = names(&tools);
    found.sort();
    assert_eq!(
        found,
        [
            "ci_run_failures",
            "get_job",
            "get_test",
            "link_issue",
            "list_projects",
            "list_repos",
            "search_jobs",
            "search_tests",
            "top_flaky_jobs",
            "top_flaky_tests",
            "unlink_issue",
        ]
    );
}

#[tokio::test]
async fn the_instructions_are_the_recorded_text() {
    // The instructions are the contract every client reads first, so they are
    // compared against the recorded golden rather than a copy in this file.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let reply = rpc(
        &app,
        "initialize",
        None,
        json!({
            "protocolVersion": VERSION,
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"},
        }),
    )
    .await;
    let result = reply.json()["result"].clone();
    assert_eq!(result["serverInfo"]["name"], "FlakeRadar");
    let golden: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/parity/golden/mcp/05.json"),
        )
        .expect("the recorded initialize answer is readable"),
    )
    .expect("the golden is JSON");
    assert_eq!(result["instructions"], golden["result"]["instructions"]);
}

#[tokio::test]
async fn a_request_without_a_token_is_refused_with_a_challenge() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"protocolVersion": VERSION, "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}},
    });
    let reply = rpc_with(&app, &body, &[]).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(reply.header("www-authenticate").as_deref(), Some("Bearer"));
    assert_eq!(reply.body, Vec::<u8>::new(), "the 401 body is empty");
    assert_eq!(reply.header("content-type"), None);
}

#[tokio::test]
async fn a_wrong_token_and_a_wrong_scheme_are_refused() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}});
    for authorization in [
        "Bearer wrong".to_owned(),
        format!("Basic {TOKEN}"),
        TOKEN.to_owned(),
        "Bearer ".to_owned(),
    ] {
        let reply = rpc_with(&app, &body, &[("authorization", &authorization)]).await;
        assert_eq!(
            reply.status,
            StatusCode::UNAUTHORIZED,
            "{authorization} was accepted"
        );
    }
    // The right token is accepted; the request carries the SEP-2243 headers a
    // client of this revision sends, which `rpc` adds.
    let reply = rpc(&app, "tools/list", None, json!({})).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
}

#[tokio::test]
async fn post_mcp_redirects_to_the_transport_path() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let reply = common::post(
        &app,
        "/mcp",
        &[("content-type", "application/json")],
        Body::from("{}"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(reply.header("location").as_deref(), Some("/mcp/"));
    assert_eq!(reply.body, Vec::<u8>::new());
}

#[tokio::test]
async fn two_requests_with_no_session_header_both_succeed() {
    // The transport is stateless: nothing is handed back in `Mcp-Session-Id`, and
    // two tool calls with no session header at all both answer.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    for _ in 0..2 {
        let reply = rpc(&app, "tools/list", None, json!({})).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
        assert_eq!(reply.header("mcp-session-id"), None);
        assert_eq!(
            reply.json()["result"]["tools"].as_array().map(Vec::len),
            Some(11)
        );
    }
}

#[tokio::test]
async fn list_repos_and_projects() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let _ = seed(&db).await;
    let repos = call(&app, "list_repos", json!({})).await;
    assert_eq!(names(&repos), ["andrewthetechie/writers-app"]);
    let projects = call(
        &app,
        "list_projects",
        json!({"repo": "AndrewTheTechie/Writers-App"}),
    )
    .await;
    assert_eq!(
        projects,
        json!([
            {"name": "backend", "root": ""},
            {"name": "frontend", "root": "frontend"},
        ])
    );
    refused(
        &app,
        "list_projects",
        json!({"repo": "nobody/here"}),
        "Unknown repo 'nobody/here'. Call list_repos to see what exists.",
    )
    .await;
}

#[tokio::test]
async fn top_flaky_tests_lists_worst_first() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let _ = seed(&db).await;
    let wide = call(
        &app,
        "top_flaky_tests",
        json!({"repo": "andrewthetechie/writers-app"}),
    )
    .await;
    let found: Vec<(String, String, String)> = wide
        .as_array()
        .expect("the answer is a list")
        .iter()
        .map(|test| {
            (
                test["project"].as_str().unwrap_or_default().to_owned(),
                test["name"].as_str().unwrap_or_default().to_owned(),
                test["tier"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                "frontend".to_owned(),
                "renders".to_owned(),
                "flaky".to_owned()
            ),
            (
                "backend".to_owned(),
                "renders".to_owned(),
                "flaky".to_owned()
            ),
            (
                "frontend".to_owned(),
                "suspect_one".to_owned(),
                "suspect".to_owned()
            ),
        ]
    );
    let flaky_only = call(
        &app,
        "top_flaky_tests",
        json!({"repo": "andrewthetechie/writers-app", "include_suspect": false}),
    )
    .await;
    assert_eq!(flaky_only.as_array().map(Vec::len), Some(2));
    let in_file = call(
        &app,
        "top_flaky_tests",
        json!({"repo": "andrewthetechie/writers-app", "file": "test_views"}),
    )
    .await;
    let files: Vec<&str> = in_file
        .as_array()
        .expect("the answer is a list")
        .iter()
        .map(|test| test["file"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(files, ["tests/test_views.py"]);
    refused(
        &app,
        "top_flaky_tests",
        json!({"repo": "x", "limit": 5}),
        "repo must look like",
    )
    .await;
}

#[tokio::test]
async fn top_flaky_tests_filters_by_category() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project");
    for (name, score, category) in [
        ("A", 0.8, "timing"),
        ("B", 0.2, "network"),
        ("C", 0.5, "timing"),
    ] {
        make_test_case(&project)
            .name(name)
            .flakiness_score(score)
            .failure_category(category)
            .build(db.connection())
            .await
            .expect("a Test");
    }
    let network = call(
        &app,
        "top_flaky_tests",
        json!({"repo": "acme/app", "category": "network"}),
    )
    .await;
    assert_eq!(names(&network), ["B"]);
    refused(
        &app,
        "top_flaky_tests",
        json!({"repo": "acme/app", "category": "nope"}),
        "category must be one of: network, environment, timing, assertion, other",
    )
    .await;
}

/// Two Tests and two Jobs of one Repo: one became flaky this week, one a month ago.
async fn seed_new_within_days(db: &TestDb) {
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project");
    let crossed = make_test_case(&project)
        .name("crossed")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("crossed");
    let old_flaky = make_test_case(&project)
        .name("old_flaky")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("old_flaky");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the Pipeline");
    let job_crossed = make_job(&pipeline)
        .name("crossed")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the crossed Job");
    let job_old = make_job(&pipeline)
        .name("old_flaky")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the old Job");
    for (entity, days, score) in [
        (&crossed, 20, 0.1),
        (&crossed, 3, 0.6),
        (&old_flaky, 30, 0.6),
    ] {
        make_test_score(entity, day(days), score)
            .build(db.connection())
            .await
            .expect("a Score row");
    }
    for (entity, days, score) in [
        (&job_crossed, 20, 0.1),
        (&job_crossed, 3, 0.6),
        (&job_old, 30, 0.6),
    ] {
        make_job_score(entity, day(days), score)
            .build(db.connection())
            .await
            .expect("a Job Score row");
    }
}

#[tokio::test]
async fn new_within_days_keeps_what_became_flaky_recently() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    seed_new_within_days(&db).await;

    let tests_week = call(
        &app,
        "top_flaky_tests",
        json!({"repo": "acme/app", "new_within_days": 7}),
    )
    .await;
    assert_eq!(names(&tests_week), ["crossed"]);
    let tests_48h = call(
        &app,
        "top_flaky_tests",
        json!({"repo": "acme/app", "new_within_days": 2}),
    )
    .await;
    assert_eq!(tests_48h, json!([]));
    let jobs_week = call(
        &app,
        "top_flaky_jobs",
        json!({"repo": "acme/app", "new_within_days": 7}),
    )
    .await;
    assert_eq!(names(&jobs_week), ["crossed"]);
    let jobs_48h = call(
        &app,
        "top_flaky_jobs",
        json!({"repo": "acme/app", "new_within_days": 2}),
    )
    .await;
    assert_eq!(jobs_48h, json!([]));
    for arguments in [json!({"repo": "acme/app", "new_within_days": 0})] {
        refused(
            &app,
            "top_flaky_tests",
            arguments,
            "new_within_days must be between 1 and 90",
        )
        .await;
    }
    refused(
        &app,
        "top_flaky_jobs",
        json!({"repo": "acme/app", "new_within_days": 91}),
        "new_within_days must be between 1 and 90",
    )
    .await;
}

#[tokio::test]
async fn search_tests_matches_name_classname_and_file() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let _ = seed(&db).await;
    let by_file = call(
        &app,
        "search_tests",
        json!({"repo": "andrewthetechie/writers-app", "query": "APP.TEST"}),
    )
    .await;
    assert_eq!(names(&by_file), ["renders"]);
    let by_name = call(
        &app,
        "search_tests",
        json!({"repo": "andrewthetechie/writers-app", "query": "calm"}),
    )
    .await;
    assert_eq!(names(&by_name), ["calm"], "a stable Test is searchable");
}

#[tokio::test]
async fn get_test_by_id_and_by_name() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let flaky = seed(&db).await;
    let by_id = call(&app, "get_test", json!({"test_id": flaky})).await;
    let by_name = call(
        &app,
        "get_test",
        json!({"repo": "andrewthetechie/writers-app", "project": "frontend", "name": "renders"}),
    )
    .await;
    assert_eq!(by_id, by_name);
    assert_eq!(by_id["test"]["repo"], "andrewthetechie/writers-app");
    assert_eq!(
        by_id["location"]["url"],
        "https://github.com/andrewthetechie/writers-app/blob/aaa111/frontend/src/app.test.ts#L12"
    );
    assert_eq!(by_id["last_failing_sha"], "aaa111");
    assert_eq!(by_id["latest_failure"]["message"], "expected 3");
    let details = by_id["latest_failure"]["details"]
        .as_str()
        .expect("the details are text");
    assert_eq!(
        details.chars().count(),
        4096 + "\n…[truncated]".chars().count()
    );
    let statuses: Vec<&str> = by_id["executions"]
        .as_array()
        .expect("the executions are a list")
        .iter()
        .map(|execution| execution["status"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(statuses, ["passed", "failed"]);
    for execution in by_id["executions"].as_array().expect("a list") {
        assert!(execution.get("attempt").is_some());
        assert!(execution.get("failure_category").is_some());
        assert!(
            execution.get("details").is_none(),
            "an Execution carries no traceback"
        );
    }
    assert!(by_id.get("score_history").is_some());
    assert!(by_id["test"].get("clean_streak").is_some());
    refused(
        &app,
        "get_test",
        json!({"repo": "andrewthetechie/writers-app", "project": "frontend", "name": "nope"}),
        "No test named 'nope'",
    )
    .await;
    let nothing = call_result(&app, "get_test", json!({})).await;
    assert_eq!(nothing["isError"], true);
}

#[tokio::test]
async fn get_test_narrows_an_ambiguous_name_by_classname() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project");
    let first = make_test_case(&project)
        .name("dup")
        .classname("a")
        .build(db.connection())
        .await
        .expect("the first Test");
    let second = make_test_case(&project)
        .name("dup")
        .classname("b")
        .build(db.connection())
        .await
        .expect("the second Test");
    let narrowed = call(
        &app,
        "get_test",
        json!({"repo": "acme/app", "project": "backend", "name": "dup", "classname": "b"}),
    )
    .await;
    assert_eq!(narrowed["test"]["id"], second.id);
    let result = call_result(
        &app,
        "get_test",
        json!({"repo": "acme/app", "project": "backend", "name": "dup"}),
    )
    .await;
    assert_eq!(result["isError"], true);
    let message = text(&result);
    assert!(message.contains("2 tests are named 'dup'"), "{message}");
    assert!(
        message.contains(&format!("test_id [{}, {}]", first.id, second.id)),
        "{message}"
    );
}

#[tokio::test]
async fn a_test_with_no_failures_has_no_latest_failure() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project");
    let steady = make_test_case(&project)
        .name("steady")
        .build(db.connection())
        .await
        .expect("the Test");
    let run = make_run(&project)
        .build(db.connection())
        .await
        .expect("the Run");
    make_execution(&steady, &run)
        .status("passed")
        .build(db.connection())
        .await
        .expect("the Execution");
    let data = call(&app, "get_test", json!({"test_id": steady.id})).await;
    assert_eq!(data["latest_failure"], Value::Null);
    assert_eq!(data["last_failing_sha"], Value::Null);
}

/// The Repo the Job tools are asserted over: an infra-flaky Job, an explained
/// Job, and a Test whose failing Run explains one of its executions.
async fn seed_jobs(db: &TestDb) -> (i32, i32, i32) {
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .name("ci.yml")
        .build(db.connection())
        .await
        .expect("the Pipeline");
    let e2e = make_job(&pipeline)
        .name("e2e (ubuntu-latest)")
        .flakiness_score(0.4)
        .build(db.connection())
        .await
        .expect("the e2e Job");
    make_job_execution(&e2e)
        .status("failed")
        .ci_job_id("e2e-1")
        .build(db.connection())
        .await
        .expect("the unexplained execution");
    let explained_job = make_job(&pipeline)
        .name("test")
        .flakiness_score(0.1)
        .build(db.connection())
        .await
        .expect("the explained Job");
    let test = make_test_case(&project)
        .name("t1")
        .build(db.connection())
        .await
        .expect("the Test");
    let run = make_run(&project)
        .commit_sha("s")
        .ci_job_id("ex-1")
        .build(db.connection())
        .await
        .expect("the Run");
    make_execution(&test, &run)
        .status("failed")
        .build(db.connection())
        .await
        .expect("the failing Execution");
    make_job_execution(&explained_job)
        .status("failed")
        .ci_job_id("ex-1")
        .build(db.connection())
        .await
        .expect("the explained execution");
    make_job_execution(&explained_job)
        .status("passed")
        .ci_job_id("ex-2")
        .build(db.connection())
        .await
        .expect("the passing execution");
    (e2e.id, explained_job.id, test.id)
}

#[tokio::test]
async fn top_flaky_jobs_lists_worst_first() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let _ = seed_jobs(&db).await;
    let wide = call(&app, "top_flaky_jobs", json!({"repo": "acme/app"})).await;
    assert_eq!(names(&wide), ["e2e (ubuntu-latest)", "test"]);
    let flaky_only = call(
        &app,
        "top_flaky_jobs",
        json!({"repo": "acme/app", "include_suspect": false}),
    )
    .await;
    assert_eq!(
        names(&flaky_only),
        ["e2e (ubuntu-latest)"],
        "test is suspect"
    );
    refused(
        &app,
        "top_flaky_jobs",
        json!({"repo": "acme/app", "limit": 0}),
        "limit must be between 1 and 100",
    )
    .await;
}

#[tokio::test]
async fn search_jobs_matches_a_name_and_a_pipeline() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let _ = seed_jobs(&db).await;
    let by_name = call(
        &app,
        "search_jobs",
        json!({"repo": "acme/app", "query": "e2e"}),
    )
    .await;
    assert_eq!(names(&by_name), ["e2e (ubuntu-latest)"]);
    let by_pipeline = call(
        &app,
        "search_jobs",
        json!({"repo": "acme/app", "query": "ci.yml"}),
    )
    .await;
    let mut found = names(&by_pipeline);
    found.sort();
    assert_eq!(found, ["e2e (ubuntu-latest)", "test"]);
}

#[tokio::test]
async fn get_job_by_id_and_by_name_with_explained_failures() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let (_, explained_job, _) = seed_jobs(&db).await;
    let by_id = call(&app, "get_job", json!({"job_id": explained_job})).await;
    let by_name = call(&app, "get_job", json!({"repo": "acme/app", "name": "test"})).await;
    assert_eq!(by_id, by_name);
    assert!(by_id.get("score_history").is_some());
    assert!(by_id["job"].get("clean_streak").is_some());
    let executions = by_id["executions"].as_array().expect("a list");
    assert_eq!(executions.len(), 2);
    let mut outcomes: Vec<(String, String)> = executions
        .iter()
        .map(|execution| {
            (
                execution["ci_job_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                execution["outcome"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    outcomes.sort();
    assert_eq!(
        outcomes,
        [
            ("ex-1".to_owned(), "explained".to_owned()),
            ("ex-2".to_owned(), "passed".to_owned()),
        ]
    );
    let explained = executions
        .iter()
        .find(|execution| execution["ci_job_id"] == "ex-1")
        .expect("the explained execution");
    assert_eq!(names(&explained["explained_by"]), ["t1"]);
    assert_eq!(explained["explained_by_jobs"], json!([]));
    let mut fields: Vec<&str> = explained
        .as_object()
        .expect("the execution is an object")
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        [
            "branch",
            "ci_job_id",
            "ci_run_attempt",
            "commit_sha",
            "created_at",
            "explained_by",
            "explained_by_jobs",
            "outcome",
            "runner_labels",
            "runner_name",
            "status",
            "url",
        ]
    );
    // Python's `isoformat()` writes `+00:00`; Rust writes the six-digit `Z`
    // spelling differences.md records, so the assertion is on that contract.
    let created_at = explained["created_at"]
        .as_str()
        .expect("created_at is text");
    assert!(created_at.ends_with('Z'), "{created_at}");
    refused(
        &app,
        "get_job",
        json!({"repo": "acme/app", "name": "nope"}),
        "No job named 'nope'",
    )
    .await;
}

#[tokio::test]
async fn get_job_groups_failures_by_runner() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let pipeline = make_pipeline()
        .repo("acme/app")
        .name("ci.yml")
        .build(db.connection())
        .await
        .expect("the Pipeline");
    let job = make_job(&pipeline)
        .name("test")
        .build(db.connection())
        .await
        .expect("the Job");
    for index in 0..2 {
        make_job_execution(&job)
            .status("failed")
            .ci_job_id(&format!("sh-{index}"))
            .runner_name(&format!("self-hosted-{index}"))
            .runner_labels(&["self-hosted"])
            .build(db.connection())
            .await
            .expect("a self-hosted execution");
    }
    make_job_execution(&job)
        .status("passed")
        .ci_job_id("gh-1")
        .runner_labels(&["ubuntu-latest"])
        .build(db.connection())
        .await
        .expect("the hosted execution");
    let data = call(&app, "get_job", json!({"job_id": job.id})).await;
    // A runner name seen once is not grouped, so the label set is the item.
    assert_eq!(
        data["runners"],
        json!([
            {"labels": ["self-hosted"], "runner_name": null, "executions": 2, "unexplained_failures": 2},
            {"labels": ["ubuntu-latest"], "runner_name": null, "executions": 1, "unexplained_failures": 0},
        ])
    );
}

#[tokio::test]
async fn get_job_narrows_an_ambiguous_name_by_pipeline() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let first_pipeline = make_pipeline()
        .repo("acme/app")
        .name("a.yml")
        .build(db.connection())
        .await
        .expect("the first Pipeline");
    let second_pipeline = make_pipeline()
        .repo("acme/app")
        .name("b.yml")
        .build(db.connection())
        .await
        .expect("the second Pipeline");
    let first = make_job(&first_pipeline)
        .name("build")
        .build(db.connection())
        .await
        .expect("the first Job");
    let second = make_job(&second_pipeline)
        .name("build")
        .build(db.connection())
        .await
        .expect("the second Job");
    let result = call_result(
        &app,
        "get_job",
        json!({"repo": "acme/app", "name": "build"}),
    )
    .await;
    assert_eq!(result["isError"], true);
    let message = text(&result);
    assert!(message.contains(&first.id.to_string()), "{message}");
    assert!(message.contains(&second.id.to_string()), "{message}");
    assert!(message.contains("pass pipeline"), "{message}");
    let by_pipeline = call(
        &app,
        "get_job",
        json!({"repo": "acme/app", "name": "build", "pipeline": "b.yml"}),
    )
    .await;
    assert_eq!(by_pipeline["job"]["id"], second.id);
}

#[tokio::test]
async fn get_test_lists_the_jobs_its_executions_ran_in() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let (_, _, test) = seed_jobs(&db).await;
    let data = call(&app, "get_test", json!({"test_id": test})).await;
    let jobs = data["jobs"].as_array().expect("the jobs are a list");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["pipeline"], "ci.yml");
    assert_eq!(jobs[0]["name"], "test");
}

#[tokio::test]
async fn ci_run_failures_lists_the_failing_tests_of_one_attempt() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project");
    let test = make_test_case(&project)
        .name("t1")
        .flakiness_score(0.5)
        .build(db.connection())
        .await
        .expect("the Test");
    let run = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .build(db.connection())
        .await
        .expect("the Run");
    make_execution(&test, &run)
        .status("failed")
        .message("boom")
        .build(db.connection())
        .await
        .expect("the Execution");
    let data = call(
        &app,
        "ci_run_failures",
        json!({"repo": "ACME/App", "ci_run_id": "100"}),
    )
    .await;
    assert_eq!(data["repo"], "acme/app");
    assert_eq!(data["ci_run_attempt"], 1);
    let failures = data["failures"].as_array().expect("a list");
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0]["name"], "t1");
    assert_eq!(failures[0]["tier"], "flaky");
    assert_eq!(failures[0]["message"], "boom");
    assert_eq!(data["failed_jobs"], json!([]));
    let reports = data["reports"].as_object().expect("the report counts");
    let mut counted: Vec<&str> = reports.keys().map(String::as_str).collect();
    counted.sort_unstable();
    assert_eq!(counted, ["failed", "pending", "processed"]);
}

#[tokio::test]
async fn ci_run_failures_refuses_what_it_cannot_read() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the Project");
    let test = make_test_case(&project)
        .name("t1")
        .build(db.connection())
        .await
        .expect("the Test");
    let run = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .build(db.connection())
        .await
        .expect("the Run");
    make_execution(&test, &run)
        .status("failed")
        .build(db.connection())
        .await
        .expect("the Execution");
    refused(
        &app,
        "ci_run_failures",
        json!({"repo": "acme/app", "ci_run_id": "nope"}),
        "CI run not found",
    )
    .await;
    refused(
        &app,
        "ci_run_failures",
        json!({"repo": "acme/app", "ci_run_id": "100", "ci_run_attempt": 0}),
        "ci_run_attempt must be 1 or more",
    )
    .await;
    refused(
        &app,
        "ci_run_failures",
        json!({"repo": "acme/app", "ci_run_id": "100", "ci_run_attempt": 5}),
        "CI run attempt not found",
    )
    .await;
    refused(
        &app,
        "ci_run_failures",
        json!({"repo": "nobody/here", "ci_run_id": "100"}),
        "Repo not found",
    )
    .await;
    let bad_repo = call_result(
        &app,
        "ci_run_failures",
        json!({"repo": "nope", "ci_run_id": "100"}),
    )
    .await;
    assert_eq!(bad_repo["isError"], true);
}

#[tokio::test]
async fn every_argument_and_not_found_message_is_the_reference_text() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    for (tool, arguments, message) in [
        (
            "search_tests",
            json!({"repo": "acme/app", "query": "   "}),
            "query must not be empty",
        ),
        (
            "get_test",
            json!({"test_id": 999_999}),
            "No test with id 999999.",
        ),
        (
            "search_jobs",
            json!({"repo": "acme/app", "query": ""}),
            "query must not be empty",
        ),
        (
            "get_job",
            json!({"repo": "acme/app"}),
            "Pass job_id, or repo + name",
        ),
        (
            "get_job",
            json!({"job_id": 999_999}),
            "No job with id 999999.",
        ),
        (
            "ci_run_failures",
            json!({"repo": "acme/app", "ci_run_id": "  "}),
            "ci_run_id must not be empty",
        ),
    ] {
        refused(&app, tool, arguments, message).await;
    }
}

#[tokio::test]
async fn the_annotations_say_which_tools_write() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let reply = rpc(&app, "tools/list", None, json!({})).await;
    let tools = reply.json()["result"]["tools"].clone();
    for tool in tools.as_array().expect("the tools are a list") {
        let name = tool["name"].as_str().unwrap_or_default();
        let annotations = &tool["annotations"];
        if name == "link_issue" {
            assert_eq!(annotations["readOnlyHint"], false);
            assert_eq!(annotations["destructiveHint"], false);
            assert_eq!(annotations["idempotentHint"], true);
            assert_eq!(annotations["openWorldHint"], false);
            continue;
        }
        if name == "unlink_issue" {
            assert_eq!(annotations["readOnlyHint"], false);
            assert_eq!(annotations["destructiveHint"], true);
            assert_eq!(annotations["idempotentHint"], true);
            assert_eq!(annotations["openWorldHint"], false);
            continue;
        }
        assert_eq!(annotations["readOnlyHint"], true, "{name}");
    }
}

#[tokio::test]
async fn link_and_unlink_a_test_issue() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let flaky = seed(&db).await;
    let linked = call(&app, "link_issue", json!({"number": 31, "test_id": flaky})).await;
    assert_eq!(linked["id"], flaky);
    assert_eq!(linked["github_issue_number"], 31);
    assert_eq!(
        linked["github_issue_url"],
        "https://github.com/andrewthetechie/writers-app/issues/31"
    );
    let got = call(&app, "get_test", json!({"test_id": flaky})).await;
    assert_eq!(got["test"]["github_issue_number"], 31);
    assert_eq!(got["test"]["github_issue_closed"], false);
    let unlinked = call(&app, "unlink_issue", json!({"test_id": flaky})).await;
    assert_eq!(unlinked["github_issue_number"], Value::Null);
}

#[tokio::test]
async fn link_and_unlink_a_job_issue() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the Pipeline");
    let job = make_job(&pipeline)
        .name("e2e")
        .build(db.connection())
        .await
        .expect("the Job");
    let linked = call(&app, "link_issue", json!({"number": 9, "job_id": job.id})).await;
    assert_eq!(linked["id"], job.id);
    assert_eq!(linked["github_issue_number"], 9);
    assert_eq!(
        linked["github_issue_url"],
        "https://github.com/acme/app/issues/9"
    );
    assert_eq!(linked["github_issue_closed"], false);
    let unlinked = call(&app, "unlink_issue", json!({"job_id": job.id})).await;
    assert_eq!(unlinked["github_issue_number"], Value::Null);
}

#[tokio::test]
async fn link_issue_refuses_what_the_rules_refuse() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let flaky = seed(&db).await;
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the Pipeline");
    let job = make_job(&pipeline)
        .name("e2e")
        .build(db.connection())
        .await
        .expect("the Job");
    let gitlab_pipeline = make_pipeline()
        .repo("acme/app")
        .provider("gitlab")
        .name(".gitlab-ci.yml")
        .build(db.connection())
        .await
        .expect("the GitLab Pipeline");
    let gitlab = make_job(&gitlab_pipeline)
        .name("gl")
        .build(db.connection())
        .await
        .expect("the GitLab Job");
    refused(
        &app,
        "link_issue",
        json!({"number": 1}),
        "Pass exactly one of test_id or job_id.",
    )
    .await;
    refused(
        &app,
        "link_issue",
        json!({"number": 1, "test_id": flaky, "job_id": job.id}),
        "Pass exactly one of test_id or job_id.",
    )
    .await;
    refused(
        &app,
        "link_issue",
        json!({"number": 1, "test_id": 99_999}),
        "Test not found",
    )
    .await;
    refused(
        &app,
        "link_issue",
        json!({"number": 1, "job_id": 99_999}),
        "Job not found",
    )
    .await;
    refused(
        &app,
        "link_issue",
        json!({"number": 0, "test_id": flaky}),
        "number must be between 1 and 2147483647",
    )
    .await;
    refused(
        &app,
        "link_issue",
        json!({"number": 1, "job_id": gitlab.id}),
        "Only Jobs of GitHub Pipelines can link a GitHub issue",
    )
    .await;
    refused(
        &app,
        "unlink_issue",
        json!({}),
        "Pass exactly one of test_id or job_id.",
    )
    .await;
    refused(
        &app,
        "unlink_issue",
        json!({"test_id": 99_999}),
        "Test not found",
    )
    .await;
}

#[tokio::test]
async fn link_issue_refuses_a_conflict_unless_it_may_replace() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let flaky = seed(&db).await;
    let first = call(&app, "link_issue", json!({"number": 31, "test_id": flaky})).await;
    assert_eq!(first["github_issue_number"], 31);
    refused(
        &app,
        "link_issue",
        json!({"number": 32, "test_id": flaky}),
        "Test already links issue #31; pass replace=true to replace it",
    )
    .await;
    let replaced = call(
        &app,
        "link_issue",
        json!({"number": 32, "test_id": flaky, "replace": true}),
    )
    .await;
    assert_eq!(replaced["github_issue_number"], 32);
    let again = call(&app, "link_issue", json!({"number": 32, "test_id": flaky})).await;
    assert_eq!(again["github_issue_number"], 32);
}

#[tokio::test]
async fn an_unknown_tool_and_an_unreadable_argument_are_protocol_errors() {
    // A refusal is an `isError` result; a request the server cannot route at all
    // is a JSON-RPC error, which is what a client's protocol layer sees.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let unknown = rpc(
        &app,
        "tools/call",
        Some("no_such_tool"),
        json!({"name": "no_such_tool", "arguments": {}}),
    )
    .await;
    let body = unknown.json();
    assert!(
        body.get("result").is_none(),
        "an unknown tool answered a result: {body}"
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("unknown tool 'no_such_tool'"),
        "{body}"
    );
    let unreadable = rpc(
        &app,
        "tools/call",
        Some("top_flaky_tests"),
        json!({"name": "top_flaky_tests", "arguments": {"repo": "acme/app", "limit": "many"}}),
    )
    .await;
    let body = unreadable.json();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("invalid arguments"),
        "{body}"
    );
}

#[tokio::test]
async fn a_broken_database_is_a_protocol_error_rather_than_a_refusal() {
    // A tool whose statement fails has not refused anything: the server could not
    // answer, which the client must see as an error rather than as data.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let _ = seed(&db).await;
    sea_orm::ConnectionTrait::execute_unprepared(db.connection(), "DROP TABLE repos CASCADE")
        .await
        .expect("the table is dropped");
    let reply = rpc(
        &app,
        "tools/call",
        Some("list_repos"),
        json!({"name": "list_repos", "arguments": {}}),
    )
    .await;
    assert_answered_ok(&reply);
    let body = reply.json();
    assert!(
        body.get("result").is_none(),
        "a dropped table answered a result: {body}"
    );
    assert!(body.get("error").is_some(), "{body}");
}

#[tokio::test]
async fn a_failing_read_of_one_ci_run_is_a_protocol_error() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let _ = seed(&db).await;
    sea_orm::ConnectionTrait::execute_unprepared(db.connection(), "DROP TABLE test_runs CASCADE")
        .await
        .expect("the table is dropped");
    let reply = rpc(
        &app,
        "tools/call",
        Some("ci_run_failures"),
        json!({
            "name": "ci_run_failures",
            "arguments": {"repo": "andrewthetechie/writers-app", "ci_run_id": "1"},
        }),
    )
    .await;
    assert_answered_ok(&reply);
    let body = reply.json();
    assert!(body.get("error").is_some(), "{body}");
}

#[tokio::test]
async fn a_non_finite_score_is_answered_as_null_not_as_a_failure() {
    // `flakiness_score` is a `double`, so a NaN score is storable even though
    // nothing this server writes makes one. `serde_json` serializes a
    // non-finite float as `null` rather than failing, so the answer is still a
    // well-formed result a client can read, and no answer this file builds can
    // reach its "does not serialize" arm.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let _ = seed(&db).await;
    sea_orm::ConnectionTrait::execute_unprepared(
        db.connection(),
        "UPDATE test_cases SET flakiness_score = 'NaN'",
    )
    .await
    .expect("the NaN score is written");
    let reply = rpc(
        &app,
        "tools/call",
        Some("top_flaky_tests"),
        json!({
            "name": "top_flaky_tests",
            "arguments": {"repo": "andrewthetechie/writers-app"},
        }),
    )
    .await;
    let body = reply.json();
    assert!(body.get("error").is_none(), "{body}");
    let tests = body["result"]["structuredContent"]["result"]
        .as_array()
        .expect("the answer is a list");
    assert_eq!(tests.len(), 4, "{body}");
    for test in tests {
        assert_eq!(test["flakiness_score"], Value::Null, "{test}");
    }
}

#[tokio::test]
async fn a_failing_link_is_a_protocol_error() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let flaky = seed(&db).await;
    sea_orm::ConnectionTrait::execute_unprepared(db.connection(), "DROP TABLE test_cases CASCADE")
        .await
        .expect("the table is dropped");
    let reply = rpc(
        &app,
        "tools/call",
        Some("link_issue"),
        json!({"name": "link_issue", "arguments": {"number": 7, "test_id": flaky}}),
    )
    .await;
    assert_answered_ok(&reply);
    let body = reply.json();
    assert!(body.get("error").is_some(), "{body}");
}

#[tokio::test]
async fn the_transport_answers_over_a_real_socket() {
    // Everything else in this file drives the router with `oneshot`; this binds a
    // port and speaks HTTP to it, so the framing, the headers and the stateless
    // transport are proved the way a client meets them.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let address = listener.local_addr().expect("the bound address");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = reqwest::Client::new();
    let url = format!("http://{address}/mcp/");
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "list_repos",
            "arguments": {},
            "_meta": per_request_meta(),
        },
    });
    for attempt in 0..2 {
        let response = client
            .post(&url)
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("mcp-protocol-version", VERSION)
            .header("mcp-method", "tools/call")
            .header("mcp-name", "list_repos")
            .header("accept", "application/json, text/event-stream")
            .json(&body)
            .send()
            .await
            .expect("the server answers");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("mcp-session-id"),
            None,
            "attempt {attempt} was given a session"
        );
        let answer: Value = response.json().await.expect("the answer is JSON");
        assert_eq!(answer["result"]["isError"], false, "{answer}");
    }
    server.abort();
}
