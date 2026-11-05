//! The MCP server over its own transport: the token, the redirect and the tools.
//!
//! Every test drives the app with `tower`'s `oneshot`, so no test binds a port.
//! The requests are the JSON-RPC documents a client sends, which is the contract
//! a client sees.

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test that cannot build its request or read its answer has failed"
)]

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use flakeradar_test_support::TestDb;
use serde_json::{Value, json};

use common::{Reply, app, send, settings};

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

/// Sends one JSON-RPC request to the MCP transport.
async fn rpc(app: &Router, method: &str, name: Option<&str>, params: Value) -> Reply {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp/")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("mcp-protocol-version", VERSION)
        .header("mcp-method", method);
    if let Some(name) = name {
        builder = builder.header("mcp-name", name);
    }
    let mut params = params;
    if let Some(fields) = params.as_object_mut() {
        let _ = fields.insert("_meta".to_owned(), per_request_meta());
    }
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    send(
        app,
        builder
            .body(Body::from(body.to_string()))
            .expect("the request builds"),
    )
    .await
}

/// Calls one tool and answers its JSON-RPC `result`.
async fn call(app: &Router, tool: &str, arguments: Value) -> Value {
    let reply = rpc(
        app,
        "tools/call",
        Some(tool),
        json!({"name": tool, "arguments": arguments}),
    )
    .await;
    reply.json()
}

#[tokio::test]
async fn probe_initialize() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let reply = rpc(
        &app,
        "initialize",
        None,
        json!({
            "protocolVersion": VERSION,
            "capabilities": {},
            "clientInfo": {"name": "probe", "version": "0"},
        }),
    )
    .await;
    assert_eq!(reply.status.as_u16(), 200, "{}", reply.text());
    assert_eq!(reply.json(), json!(null), "{}", reply.text());
}

#[tokio::test]
async fn probe_tools_list() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let reply = rpc(&app, "tools/list", None, json!({})).await;
    assert_eq!(reply.status.as_u16(), 200, "{}", reply.text());
    assert_eq!(reply.json(), json!(null), "{}", reply.text());
}

#[tokio::test]
async fn probe_call() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = mcp_app(&db);
    let reply = call(&app, "list_repos", json!({})).await;
    assert_eq!(reply, json!(null));
    let _ = settings(&[]);
}
