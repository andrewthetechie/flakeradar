//! The `unlink_issue` tool: clears a Test's or Job's issue link.
//!
//! The name, title, description, argument schema and annotations are the wire
//! contract `tests/parity` replays, so they stay literals here.

// Rust guideline compliant 2026-07-21

use rmcp::model::ToolAnnotations;
use serde::Deserialize;
use serde_json::{Value, json};

use flakeradar_db::issue_links;

use crate::mcp::{
    BoxFuture, McpServer, ToolAnswer, ToolResult, lax_optional_integer, nullable, one_target, parse,
};

/// The tool's name on the wire.
pub(super) const NAME: &str = "unlink_issue";
/// The title a client shows.
pub(super) const TITLE: &str = "Unlink Issue";
/// The description a client shows.
pub(super) const DESCRIPTION: &str =
    "Remove the GitHub issue link of a test or job (exactly one of test_id
or job_id), for example a link to the wrong issue. Returns the test or job.";

/// Whether this tool answers a list, wrapped in `{"result": …}`.
///
/// The definition's output schema, the answer's `_meta` and the shape the handler
/// answers in all come from this one statement, so a tool cannot declare a list
/// and answer an object.
pub(super) const LISTS: bool = false;

/// The arguments `unlink_issue` takes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnlinkIssueArgs {
    /// The Test to unlink, when the target is a Test.
    #[serde(default, deserialize_with = "lax_optional_integer")]
    test_id: Option<i64>,
    /// The Job to unlink, when the target is a Job.
    #[serde(default, deserialize_with = "lax_optional_integer")]
    job_id: Option<i64>,
}

/// The tool's argument schema.
pub(super) fn input_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "job_id": nullable("integer"),
            "test_id": nullable("integer"),
        },
        "type": "object",
    })
}

/// The annotations `unlink_issue` carries: destructive, repeatable, closed-world.
pub(super) fn annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(false)
        .destructive(true)
        .idempotent(true)
        .open_world(false)
}

/// Runs the tool over its arguments.
pub(super) fn call(server: &McpServer, arguments: Value) -> BoxFuture<'_, ToolResult> {
    Box::pin(async move {
        let args: UnlinkIssueArgs = parse(arguments)?;
        let (kind, id) = one_target(args.test_id, args.job_id)?;
        let linked =
            issue_links::unlink_issue(server.pool(), kind, id, &server.read_settings()).await?;
        ToolAnswer::shaped(LISTS, &linked)
    })
}
