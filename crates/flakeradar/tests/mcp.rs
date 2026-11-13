//! The MCP server over its own transport: the token, the redirect and the tools.
//!
//! Every test but the socket ones drives the app with `tower`'s `oneshot`; the
//! requests are the JSON-RPC documents a client sends, so what is under test is
//! the wire a client sees.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test that cannot build its request or read its answer has failed"
)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use flakeradar_test_support::{
    Reply, TestDb, app, make_execution, make_job, make_job_execution, make_job_score,
    make_pipeline, make_project, make_run, make_test_case, make_test_score, post, send,
};
use serde_json::{Value, json};

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

/// Asserts the status a tool whose statement failed answers.
///
/// A tool's failure is carried inside the JSON-RPC answer with HTTP 200, recorded
/// from a real client, so a failed statement must never become an HTTP 500.
fn assert_answered_ok(reply: &Reply) {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "a failed statement is still an answered request: {}",
        reply.text()
    );
}

/// Asserts a tool failure is an `isError` result, and answers the text it carries.
///
/// A tool that raises, a name no tool has and arguments that do not validate all
/// answer HTTP 200 with a JSON-RPC *result* — never a JSON-RPC `error`, because
/// the request itself was well formed — whose only content is one text block,
/// whose `isError` is true, and which carries no structured content field at all.
/// (A refusal does the same but with an explicit `null`, golden `mcp/09`.)
fn error_result_text(reply: &Reply) -> String {
    assert_answered_ok(reply);
    let body = reply.json();
    assert!(
        body.get("error").is_none(),
        "a tool failure is an isError result, not a JSON-RPC error: {body}"
    );
    let result = body.get("result").expect("a failed call answers a result");
    assert_eq!(result.get("isError"), Some(&Value::Bool(true)), "{body}");
    assert!(
        result.get("structuredContent").is_none(),
        "a failed call sends no structured content: {body}"
    );
    result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| blocks.first())
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str)
        .expect("the error result carries its message as text")
        .to_owned()
}

/// Asserts a tool-execution failure is an `isError` result, and answers the text
/// after the `Error calling tool '<tool>': ` prefix.
fn assert_raised(reply: &Reply, tool: &str) -> String {
    let text = error_result_text(reply);
    let prefix = format!("Error calling tool '{tool}': ");
    assert!(
        text.starts_with(&prefix),
        "the message starts with {prefix:?}: {text}"
    );
    text.strip_prefix(&prefix)
        .expect("the assertion above found the prefix")
        .to_owned()
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

/// The one message's text of a prompt answer.
fn prompt_text(body: &Value) -> String {
    body.get("result")
        .and_then(|result| result.get("messages"))
        .and_then(Value::as_array)
        .and_then(|messages| messages.first())
        .and_then(|message| message.get("content"))
        .and_then(|content| content.get("text"))
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

/// Asserts a request answered JSON-RPC `-32602` (invalid params) and no result.
///
/// The request routed and its protocol version was accepted, so a refused argument
/// is the protocol's own error answer rather than a tool `isError` result: only a
/// prompt can answer one, because a tool's bad arguments are a result.
fn assert_invalid_params(reply: &Reply) {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "an invalid params refusal is an answered request: {}",
        reply.text()
    );
    let body = reply.json();
    assert!(
        body.get("result").is_none(),
        "a refusal carries no result: {body}"
    );
    assert_eq!(
        body.get("error").and_then(|error| error.get("code")),
        Some(&json!(-32602)),
        "{body}"
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
/// It is the read suite's seed: one Repo with two Projects, a flaky Test
/// in each, a suspect Test, a stable one, and two Executions of the flaky
/// frontend Test, the failing one carrying a 5000-character traceback.
async fn seed(db: &TestDb) -> i64 {
    let frontend = make_project()
        .repo("andrewthetechie/writers-app")
        .project("frontend")
        .root("frontend")
        .create(db)
        .await;
    let backend = make_project()
        .repo("andrewthetechie/writers-app")
        .project("backend")
        .create(db)
        .await;
    let flaky = make_test_case(&frontend)
        .name("renders")
        .classname("src/app.test.ts")
        .file("src/app.test.ts")
        .line(12)
        .flakiness_score(0.7)
        .confirmed_flake_count(1)
        .create(db)
        .await;
    make_test_case(&frontend)
        .name("suspect_one")
        .flakiness_score(0.1)
        .create(db)
        .await;
    make_test_case(&backend)
        .name("renders")
        .classname("tests.test_views")
        .file("tests/test_views.py")
        .flakiness_score(0.4)
        .create(db)
        .await;
    make_test_case(&backend)
        .name("calm")
        .flakiness_score(0.0)
        .create(db)
        .await;
    let run = make_run(&frontend)
        .commit_sha("aaa111")
        .branch("main")
        .create(db)
        .await;
    make_execution(&flaky, &run)
        .status("failed")
        .message("expected 3")
        .details(&format!("Traceback\n{}", "x".repeat(5000)))
        .failure_category("assertion")
        .create(db)
        .await;
    make_execution(&flaky, &run)
        .status("passed")
        .create(db)
        .await;
    i64::from(flaky.id)
}

#[tokio::test]
async fn the_tools_are_the_eleven_the_wire_declares() {
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
async fn the_prompts_are_the_one_the_wire_declares() {
    // The prompt is a fixed template, so an unseeded database answers it: the list
    // and the text depend on no row.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let reply = rpc(&app, "prompts/list", None, json!({})).await;
    let prompts = reply.json()["result"]["prompts"].clone();
    assert_eq!(names(&prompts), ["investigate_flaky_test"]);
    let prompt = prompts
        .as_array()
        .and_then(|prompts| prompts.first())
        .cloned()
        .expect("one prompt is listed");
    assert!(
        prompt["title"]
            .as_str()
            .is_some_and(|title| !title.is_empty()),
        "the prompt carries a title: {prompt}"
    );
    assert!(
        prompt["description"]
            .as_str()
            .is_some_and(|description| !description.is_empty()),
        "the prompt carries a description: {prompt}"
    );
    assert_eq!(
        prompt["arguments"],
        json!([
            {
                "name": "test_id",
                "description": "The Test's id, from top_flaky_tests or search_tests",
                "required": true,
            },
            {
                "name": "repo",
                "description": "The Repo's owner/name",
            },
        ]),
        "only `test_id` is required: {prompt}"
    );
}

#[tokio::test]
async fn the_prompt_answers_the_walkthrough_of_one_test() {
    // Test id 42 names no row, and the answer is still the whole walkthrough: the
    // prompt is a template, so building it reads nothing and calls no model.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let reply = rpc(
        &app,
        "prompts/get",
        Some("investigate_flaky_test"),
        json!({
            "name": "investigate_flaky_test",
            "arguments": {"test_id": "42", "repo": "acme/app"},
        }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let body = reply.json();
    assert!(body.get("error").is_none(), "{body}");
    let messages = body["result"]["messages"].clone();
    let message = messages
        .as_array()
        .and_then(|messages| {
            assert_eq!(messages.len(), 1, "one message: {body}");
            messages.first().cloned()
        })
        .expect("the answer carries its messages");
    assert_eq!(message["role"], "user", "{body}");
    let text = message["content"]["text"]
        .as_str()
        .expect("the message is text");
    for wanted in [
        "42",
        "acme/app",
        "get_test",
        "score_history",
        "clean_streak",
        "failure_category",
        "link_issue",
    ] {
        assert!(text.contains(wanted), "the prompt names {wanted}: {text}");
    }
    // The steps are in the order the issue gives them.
    let mut before: Option<usize> = None;
    for step in [
        "get_test",
        "score_history",
        "failure_category",
        "link_issue",
    ] {
        let found = text.find(step);
        assert!(
            found.is_some_and(|at| before.is_none_or(|then| at > then)),
            "the prompt names {step} after the step before it: {text}"
        );
        before = found;
    }
}

#[tokio::test]
async fn a_prompt_that_names_no_repo_walks_through_the_test_alone() {
    // `repo` is optional in both spellings a client sends: left out, and sent as
    // JSON null. Its subject is then the Test, with no Repo to look in.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    for arguments in [
        json!({"test_id": "42"}),
        json!({"test_id": "42", "repo": null}),
    ] {
        let reply = rpc(
            &app,
            "prompts/get",
            Some("investigate_flaky_test"),
            json!({"name": "investigate_flaky_test", "arguments": arguments}),
        )
        .await;
        let body = reply.json();
        assert!(body.get("error").is_none(), "{body}");
        let text = prompt_text(&body);
        assert!(text.contains("the flaky Test 42"), "the subject: {text}");
        assert!(!text.contains("in the Repo"), "no Repo was sent: {text}");
    }
}

#[tokio::test]
async fn a_repo_that_is_not_a_string_is_invalid_params() {
    // As for `test_id`, an argument of the wrong kind is the protocol's own refusal:
    // a number is never read as a Repo name.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    for arguments in [
        json!({"test_id": "42", "repo": 5}),
        json!({"test_id": "42", "repo": ["acme/app"]}),
    ] {
        let reply = rpc(
            &app,
            "prompts/get",
            Some("investigate_flaky_test"),
            json!({"name": "investigate_flaky_test", "arguments": arguments}),
        )
        .await;
        assert_invalid_params(&reply);
    }
}

#[tokio::test]
async fn a_prompt_name_no_prompt_has_is_invalid_params() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let reply = rpc(
        &app,
        "prompts/get",
        Some("no_such_prompt"),
        json!({"name": "no_such_prompt", "arguments": {}}),
    )
    .await;
    assert_invalid_params(&reply);
}

#[tokio::test]
async fn a_test_id_that_is_no_positive_integer_is_invalid_params() {
    // A prompt argument arrives as a string, so the check is the string's: nothing
    // here reaches the database, and nothing here coerces a bad id into a query.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    for arguments in [
        json!({}),
        json!({"test_id": "abc"}),
        json!({"test_id": "0"}),
        json!({"test_id": "-7"}),
        json!({"test_id": 42}),
    ] {
        let reply = rpc(
            &app,
            "prompts/get",
            Some("investigate_flaky_test"),
            json!({"name": "investigate_flaky_test", "arguments": arguments}),
        )
        .await;
        assert_invalid_params(&reply);
    }
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
    let reply = post(
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
        .create(&db)
        .await;
    for (name, score, category) in [
        ("A", 0.8, "timing"),
        ("B", 0.2, "network"),
        ("C", 0.5, "timing"),
    ] {
        make_test_case(&project)
            .name(name)
            .flakiness_score(score)
            .failure_category(category)
            .create(&db)
            .await;
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
        .create(db)
        .await;
    let crossed = make_test_case(&project)
        .name("crossed")
        .flakiness_score(0.6)
        .create(db)
        .await;
    let old_flaky = make_test_case(&project)
        .name("old_flaky")
        .flakiness_score(0.6)
        .create(db)
        .await;
    let pipeline = make_pipeline().repo("acme/app").create(db).await;
    let job_crossed = make_job(&pipeline)
        .name("crossed")
        .flakiness_score(0.6)
        .create(db)
        .await;
    let job_old = make_job(&pipeline)
        .name("old_flaky")
        .flakiness_score(0.6)
        .create(db)
        .await;
    for (entity, days, score) in [
        (&crossed, 20, 0.1),
        (&crossed, 3, 0.6),
        (&old_flaky, 30, 0.6),
    ] {
        make_test_score(entity, day(days), score).create(db).await;
    }
    for (entity, days, score) in [
        (&job_crossed, 20, 0.1),
        (&job_crossed, 3, 0.6),
        (&job_old, 30, 0.6),
    ] {
        make_job_score(entity, day(days), score).create(db).await;
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
        .create(&db)
        .await;
    let first = make_test_case(&project)
        .name("dup")
        .classname("a")
        .create(&db)
        .await;
    let second = make_test_case(&project)
        .name("dup")
        .classname("b")
        .create(&db)
        .await;
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
        .create(&db)
        .await;
    let steady = make_test_case(&project).name("steady").create(&db).await;
    let run = make_run(&project).create(&db).await;
    make_execution(&steady, &run)
        .status("passed")
        .create(&db)
        .await;
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
        .create(db)
        .await;
    let pipeline = make_pipeline()
        .repo("acme/app")
        .name("ci.yml")
        .create(db)
        .await;
    let e2e = make_job(&pipeline)
        .name("e2e (ubuntu-latest)")
        .flakiness_score(0.4)
        .create(db)
        .await;
    make_job_execution(&e2e)
        .status("failed")
        .ci_job_id("e2e-1")
        .create(db)
        .await;
    let explained_job = make_job(&pipeline)
        .name("test")
        .flakiness_score(0.1)
        .create(db)
        .await;
    let test = make_test_case(&project).name("t1").create(db).await;
    let run = make_run(&project)
        .commit_sha("s")
        .ci_job_id("ex-1")
        .create(db)
        .await;
    make_execution(&test, &run)
        .status("failed")
        .create(db)
        .await;
    make_job_execution(&explained_job)
        .status("failed")
        .ci_job_id("ex-1")
        .create(db)
        .await;
    make_job_execution(&explained_job)
        .status("passed")
        .ci_job_id("ex-2")
        .create(db)
        .await;
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
    // The REST leaderboard's own bound, so a tool cannot ask for a page no
    // client could page to. A tool takes no `page`, so no page can overflow here.
    for limit in [0, 101] {
        refused(
            &app,
            "top_flaky_jobs",
            json!({"repo": "acme/app", "limit": limit}),
            "limit must be between 1 and 100",
        )
        .await;
    }
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
    // The recorded timestamps write `+00:00`; this server writes the six-digit `Z`
    // spelling, so the assertion is on that contract.
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
        .create(&db)
        .await;
    let job = make_job(&pipeline).name("test").create(&db).await;
    for index in 0..2 {
        make_job_execution(&job)
            .status("failed")
            .ci_job_id(&format!("sh-{index}"))
            .runner_name(&format!("self-hosted-{index}"))
            .runner_labels(&["self-hosted"])
            .create(&db)
            .await;
    }
    make_job_execution(&job)
        .status("passed")
        .ci_job_id("gh-1")
        .runner_labels(&["ubuntu-latest"])
        .create(&db)
        .await;
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
        .create(&db)
        .await;
    let second_pipeline = make_pipeline()
        .repo("acme/app")
        .name("b.yml")
        .create(&db)
        .await;
    let first = make_job(&first_pipeline).name("build").create(&db).await;
    let second = make_job(&second_pipeline).name("build").create(&db).await;
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
        .create(&db)
        .await;
    let test = make_test_case(&project)
        .name("t1")
        .flakiness_score(0.5)
        .create(&db)
        .await;
    let run = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    make_execution(&test, &run)
        .status("failed")
        .message("boom")
        .create(&db)
        .await;
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
        .create(&db)
        .await;
    let test = make_test_case(&project).name("t1").create(&db).await;
    let run = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    make_execution(&test, &run)
        .status("failed")
        .create(&db)
        .await;
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
async fn every_argument_and_not_found_message_is_the_published_text() {
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
async fn re_linking_a_declined_issue_reports_the_stored_closed_flag() {
    // The sync kept the link and flagged the row because GitHub reported the issue
    // closed as not planned; the tool must answer that flag, not a hard-coded false.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let flaky = seed(&db).await;
    let linked = call(&app, "link_issue", json!({"number": 31, "test_id": flaky})).await;
    assert_eq!(linked["github_issue_closed"], false);
    sea_orm::ConnectionTrait::execute_unprepared(
        db.connection(),
        "UPDATE test_cases SET github_issue_closed = TRUE",
    )
    .await
    .expect("the declined flag writes");

    let relinked = call(&app, "link_issue", json!({"number": 31, "test_id": flaky})).await;
    assert_eq!(
        relinked["github_issue_closed"], true,
        "the tool reports the stored flag"
    );
}

#[tokio::test]
async fn link_and_unlink_a_job_issue() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let pipeline = make_pipeline().repo("acme/app").create(&db).await;
    let job = make_job(&pipeline).name("e2e").create(&db).await;
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
    let pipeline = make_pipeline().repo("acme/app").create(&db).await;
    let job = make_job(&pipeline).name("e2e").create(&db).await;
    let gitlab_pipeline = make_pipeline()
        .repo("acme/app")
        .provider("gitlab")
        .name(".gitlab-ci.yml")
        .create(&db)
        .await;
    let gitlab = make_job(&gitlab_pipeline).name("gl").create(&db).await;
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
async fn a_tool_argument_takes_the_documented_spellings() {
    // A tool's arguments are read with the same lax rules the request bodies use,
    // so `"2"`, `2.0` and `true` are all the integer 2, `1`, `1.0` and `"TRUE"`
    // are all `true`, and an argument the tool does not declare is a validation
    // error rather than something to ignore. The refusal's text is a recorded
    // difference; what is asserted here is whether the call ran at all.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let accepted = [
        ("top_flaky_tests", json!({"repo": "acme/app", "limit": "2"})),
        ("top_flaky_tests", json!({"repo": "acme/app", "limit": 2.0})),
        (
            "top_flaky_tests",
            json!({"repo": "acme/app", "limit": true}),
        ),
        (
            "top_flaky_tests",
            json!({"repo": "acme/app", "include_suspect": 1}),
        ),
        (
            "top_flaky_tests",
            json!({"repo": "acme/app", "include_suspect": 0}),
        ),
        (
            "top_flaky_tests",
            json!({"repo": "acme/app", "include_suspect": 1.0}),
        ),
        (
            "top_flaky_tests",
            json!({"repo": "acme/app", "include_suspect": "TRUE"}),
        ),
        (
            "top_flaky_tests",
            json!({"repo": "acme/app", "new_within_days": "7"}),
        ),
        (
            "top_flaky_tests",
            json!({"repo": "acme/app", "new_within_days": null}),
        ),
        ("top_flaky_jobs", json!({"repo": "acme/app", "limit": "2"})),
        (
            "search_tests",
            json!({"repo": "acme/app", "query": "x", "limit": "2"}),
        ),
        (
            "search_jobs",
            json!({"repo": "acme/app", "query": "x", "limit": 2.0}),
        ),
        (
            "ci_run_failures",
            json!({"repo": "acme/app", "ci_run_id": "1", "ci_run_attempt": "1"}),
        ),
        (
            "link_issue",
            json!({"number": "7", "test_id": "1", "replace": "true"}),
        ),
        ("unlink_issue", json!({"test_id": "1"})),
        ("get_job", json!({"job_id": "1", "executions_limit": 2.0})),
        ("get_test", json!({"test_id": "1", "executions_limit": "2"})),
    ];
    for (tool, arguments) in accepted {
        let result = call_result(&app, tool, arguments.clone()).await;
        assert!(
            !text(&result).starts_with("1 validation error for call"),
            "{tool} {arguments} was validated as documented: {}",
            text(&result)
        );
    }
    let refused = [
        ("top_flaky_tests", json!({"repo": "acme/app", "limit": 2.5})),
        (
            "top_flaky_tests",
            json!({"repo": "acme/app", "limit": "many"}),
        ),
        (
            "top_flaky_tests",
            json!({"repo": "acme/app", "include_suspect": null}),
        ),
        (
            "top_flaky_tests",
            json!({"repo": "acme/app", "include_suspect": 2}),
        ),
        ("list_repos", json!({"extra": 1})),
        ("list_projects", json!({"repo": "acme/app", "extra": 1})),
        ("top_flaky_tests", json!({"repo": "acme/app", "unknown": 1})),
    ];
    for (tool, arguments) in refused {
        let result = call_result(&app, tool, arguments.clone()).await;
        assert_eq!(
            result.get("isError"),
            Some(&Value::Bool(true)),
            "{tool} {arguments} must be refused"
        );
        assert!(
            text(&result).starts_with("1 validation error for call"),
            "{tool} {arguments} was not a validation error: {}",
            text(&result)
        );
    }
}

#[tokio::test]
async fn an_unknown_tool_and_an_unreadable_argument_are_error_results() {
    // Both are HTTP 200 `isError` results: the request routed and its JSON-RPC was
    // well formed, so only the tool could not be run.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let unknown = rpc(
        &app,
        "tools/call",
        Some("no_such_tool"),
        json!({"name": "no_such_tool", "arguments": {}}),
    )
    .await;
    assert_eq!(error_result_text(&unknown), "Unknown tool: 'no_such_tool'");
    let unreadable = rpc(
        &app,
        "tools/call",
        Some("top_flaky_tests"),
        json!({"name": "top_flaky_tests", "arguments": {"repo": "acme/app", "limit": "many"}}),
    )
    .await;
    let text = error_result_text(&unreadable);
    assert!(
        text.starts_with("1 validation error for call[top_flaky_tests]\nlimit\n  "),
        "the text opens with the header and the field: {text:?}"
    );
    // The diagnosis is the recorded sentence, without the type, value and
    // documentation-URL tail that went with it.
    assert!(
        text.ends_with("Input should be a valid integer, unable to parse string as an integer"),
        "the diagnosis is the recorded one: {text:?}"
    );
    let missing = rpc(
        &app,
        "tools/call",
        Some("top_flaky_tests"),
        json!({"name": "top_flaky_tests", "arguments": {}}),
    )
    .await;
    let text = error_result_text(&missing);
    // The field is named; only the diagnosis's own wording (`Missing required
    // argument`) is a recorded difference.
    assert!(
        text.starts_with("1 validation error for call[top_flaky_tests]\nrepo\n  "),
        "a missing field is named: {text:?}"
    );
    assert!(
        text.contains("missing field `repo`"),
        "the missing field is named: {text:?}"
    );
}

#[tokio::test]
async fn a_body_that_is_not_json_is_a_protocol_error() {
    // The one case kept out of the result: a `POST /mcp/` whose body is not JSON
    // answers 400 with JSON-RPC -32700 and the same serde message this server
    // gives, because both parse with serde.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let reply = send(&app, raw_post(Body::from("{not json"))).await;
    assert_eq!(
        reply.status,
        StatusCode::BAD_REQUEST,
        "a body that is no JSON is refused: {}",
        reply.text()
    );
    let body = reply.json();
    assert!(body.get("result").is_none(), "{body}");
    assert_eq!(body.get("id"), Some(&Value::Null), "{body}");
    assert_eq!(
        body.get("error").and_then(|error| error.get("code")),
        Some(&json!(-32700)),
        "{body}"
    );
    assert_eq!(
        body.get("error").and_then(|error| error.get("message")),
        Some(&json!(
            "Parse error: key must be a string at line 1 column 2"
        )),
        "{body}"
    );
    // An empty body is the same case: nothing to parse.
    let empty = send(&app, raw_post(Body::from(""))).await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST, "{}", empty.text());
    let body = empty.json();
    assert_eq!(
        body.get("error").and_then(|error| error.get("message")),
        Some(&json!(
            "Parse error: EOF while parsing a value at line 1 column 0"
        )),
        "{body}"
    );
}

#[tokio::test]
async fn a_body_that_fails_while_it_is_read_is_a_protocol_error() {
    // A body the server cannot read is a body it cannot parse, so it gets the
    // same 400 and -32700 rather than reaching the transport half-read.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let stream = futures_util::stream::iter(vec![
        Ok(axum::body::Bytes::from_static(b"{")),
        Err(std::io::Error::other("the client went away")),
    ]);
    let reply = send(&app, raw_post(Body::from_stream(stream))).await;
    assert_eq!(
        reply.status,
        StatusCode::BAD_REQUEST,
        "a body that cannot be read cannot be parsed: {}",
        reply.text()
    );
    let body = reply.json();
    assert_eq!(
        body.get("error").and_then(|error| error.get("code")),
        Some(&json!(-32700)),
        "{body}"
    );
    assert!(
        body.get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .is_some_and(|message| message.starts_with("Parse error: ")),
        "{body}"
    );
}

/// A `POST /mcp/` as a client holding the token sends it, with `body`.
fn raw_post(body: Body) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/mcp/")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("mcp-protocol-version", VERSION)
        .body(body)
        .expect("the request builds")
}

#[tokio::test]
async fn a_broken_database_is_an_error_result_rather_than_a_protocol_error() {
    // A tool's own failure goes inside the answer, not beside it:
    // `Error calling tool 'list_repos': <what failed>`, with `isError` true.
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
    let message = assert_raised(&reply, "list_repos");
    assert!(
        message.contains(r#"relation "repos" does not exist"#),
        "the driver's own message reaches the client: {message}"
    );
}

#[tokio::test]
async fn a_failing_read_of_one_ci_run_is_an_error_result() {
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
    let _ = assert_raised(&reply, "ci_run_failures");
}

#[tokio::test]
async fn a_stored_score_that_is_no_score_raises_the_tool() {
    // `flakiness_score` is a `double precision`, so a hand-written row can hold
    // `NaN`, which this port used to serve as JSON `null` under a field its own
    // schema calls a number. A score is a finite number from 0 to 1, so the answer
    // is now the tool error a failing query gets, and the row says so.
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
    let message = assert_raised(&reply, "top_flaky_tests");
    assert!(
        message.contains("flakiness_score"),
        "the refusal names the column: {message}"
    );
}

#[tokio::test]
async fn a_failing_link_is_an_error_result() {
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
    let _ = assert_raised(&reply, "link_issue");
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

#[tokio::test]
async fn a_request_that_is_not_a_post_reaches_the_transport_unchanged() {
    // The JSON check reads a POST body only. A method this transport does not
    // serve is refused by the transport, with the 405 and the plain-text body a
    // `PUT` gets.
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let reply = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri("/mcp/")
            .header("host", "localhost")
            .header("accept", "text/event-stream")
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("mcp-protocol-version", VERSION)
            .body(Body::empty())
            .expect("the request builds"),
    )
    .await;
    assert_eq!(
        reply.status,
        StatusCode::METHOD_NOT_ALLOWED,
        "{}",
        reply.text()
    );
    assert_eq!(reply.text(), "Method Not Allowed");
}
