//! `/mcp/` driven by a real MCP client over a real socket.
//!
//! Task 11's Done-when asked for an actual client, and every other MCP test in
//! this repo writes its own JSON-RPC documents. This one uses the official
//! `rmcp` SDK: its Streamable HTTP client performs the `initialize` handshake,
//! then lists the tools, reads a Test, links an Issue and unlinks it, and the
//! rows are read back from the database. Nothing here is hand-written protocol,
//! so the lifecycle a client library insists on — the handshake, the `_meta` a
//! request carries, the `mcp-*` headers — is exercised as a client sends it.
//!
//! What the client is and where it ran: `rmcp` 3.8.0's
//! `StreamableHttpClientTransport` with `transport-streamable-http-client-reqwest`
//! (the same SDK the server is built on), against `axum::serve` on a loopback
//! port, in `cargo nextest`. No desktop client or Inspector CLI was available
//! here; this is the SDK's own client.

#![expect(
    clippy::expect_used,
    reason = "a test that cannot complete the handshake or a tool call has failed"
)]

mod common;

use flakeradar_db::entities::test_cases;
use flakeradar_test_support::{TestDb, make_project, make_test_case};
use rmcp::RoleClient;
use rmcp::model::{CallToolRequestParams, JsonObject};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use sea_orm::{ColumnTrait as _, EntityTrait as _, QueryFilter as _};
use serde_json::{Value, json};

/// The token the client presents as its bearer token.
const TOKEN: &str = "mcp-client-token";

/// One Project named `frontend` with one Test named `renders`, and that Test's id.
async fn seed(db: &TestDb) -> i64 {
    let project = make_project()
        .repo("acme/app")
        .project("frontend")
        .build(db.connection())
        .await
        .expect("the Project");
    make_test_case(&project)
        .name("renders")
        .classname("src/app.test.ts")
        .flakiness_score(0.7)
        .confirmed_flake_count(1)
        .build(db.connection())
        .await
        .expect("the Test")
        .id
        .into()
}

/// Serves the app on a loopback port and connects a real client to `/mcp/`.
///
/// The handshake happens in `rmcp::serve_client`: it sends `initialize`, checks
/// the answer and sends `notifications/initialized`, so a server that cannot
/// complete the protocol fails here rather than at the first tool call.
async fn connect(db: &TestDb) -> (RunningService<RoleClient, ()>, tokio::task::JoinHandle<()>) {
    let app = common::app(db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let address = listener.local_addr().expect("the bound address");
    let serving = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let config = StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp/"))
        .auth_header(TOKEN);
    let client = rmcp::serve_client((), StreamableHttpClientTransport::from_config(config))
        .await
        .expect("the initialize handshake completes");
    (client, serving)
}

/// The tool arguments a call sends, as the client serializes them.
fn arguments(value: &Value) -> JsonObject {
    value
        .as_object()
        .cloned()
        .expect("the arguments are a JSON object")
}

/// The Issue number stored on the Test named `name`.
async fn stored_issue(db: &TestDb, name: &str) -> Option<i32> {
    test_cases::Entity::find()
        .filter(test_cases::Column::Name.eq(name))
        .one(db.connection())
        .await
        .expect("the Test reads back")
        .expect("the Test exists")
        .github_issue_number
}

#[tokio::test]
async fn a_real_mcp_client_initializes_lists_its_tools_and_reads_a_test() {
    let db = TestDb::new().await.expect("a test database is available");
    let _ = seed(&db).await;
    let (client, serving) = connect(&db).await;

    let peer = client
        .peer_info()
        .expect("a completed handshake carries the server's InitializeResult");
    let identity = peer.server_info.as_ref().expect("the server names itself");
    assert_eq!(identity.name, "FlakeRadar", "{peer:?}");
    assert!(
        peer.instructions
            .as_deref()
            .is_some_and(|instructions| !instructions.is_empty()),
        "the handshake carries the instructions an agent reads: {peer:?}"
    );

    let listed = client.list_tools(None).await.expect("tools/list answers");
    let mut names: Vec<&str> = listed.tools.iter().map(|tool| &*tool.name).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
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

    // A read tool, by the name a person would use.
    let read = client
        .call_tool(
            CallToolRequestParams::new("get_test").with_arguments(arguments(&json!({
                "repo": "acme/app",
                "project": "frontend",
                "name": "renders",
            }))),
        )
        .await
        .expect("get_test answers");
    assert_ne!(read.is_error, Some(true), "{read:?}");
    let test = read
        .structured_content
        .as_ref()
        .and_then(|structured| structured.get("test"))
        .expect("get_test carries the Test beside its text");
    assert_eq!(test["name"], json!("renders"), "{test}");
    assert_eq!(test["flakiness_score"], json!(0.7), "{test}");
    assert_eq!(test["github_issue_number"], Value::Null, "{test}");

    // A tool error is an ordinary result with `isError`, and a client library
    // surfaces it as one rather than as a protocol failure: this is the shape an
    // agent reads when it asks for a Test that is not there.
    let missing = client
        .call_tool(
            CallToolRequestParams::new("get_test")
                .with_arguments(arguments(&json!({"test_id": 999}))),
        )
        .await
        .expect("a tool error is an answer, not a protocol failure");
    assert_eq!(missing.is_error, Some(true), "{missing:?}");
    assert!(
        format!("{missing:?}").contains("999"),
        "the error names what was asked for: {missing:?}"
    );

    client.cancel().await.expect("the client stops");
    serving.abort();
}

#[tokio::test]
async fn a_real_mcp_client_links_and_unlinks_an_issue() {
    let db = TestDb::new().await.expect("a test database is available");
    let test_id = seed(&db).await;
    let (client, serving) = connect(&db).await;

    let link = client
        .call_tool(
            CallToolRequestParams::new("link_issue").with_arguments(arguments(&json!({
                "test_id": test_id,
                "number": 31,
            }))),
        )
        .await
        .expect("link_issue answers");
    assert_ne!(link.is_error, Some(true), "{link:?}");
    assert_eq!(
        stored_issue(&db, "renders").await,
        Some(31),
        "link_issue must write the Issue number the client sent"
    );

    let unlink = client
        .call_tool(
            CallToolRequestParams::new("unlink_issue")
                .with_arguments(arguments(&json!({"test_id": test_id}))),
        )
        .await
        .expect("unlink_issue answers");
    assert_ne!(unlink.is_error, Some(true), "{unlink:?}");
    assert_eq!(
        stored_issue(&db, "renders").await,
        None,
        "unlink_issue must clear the Issue number"
    );

    client.cancel().await.expect("the client stops");
    serving.abort();
}
