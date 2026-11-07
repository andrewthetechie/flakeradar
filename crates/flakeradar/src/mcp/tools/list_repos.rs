//! The `list_repos` tool: every Repo with its Projects.
//!
//! The name, title, description, argument schema and annotations are the wire
//! contract `tests/parity` replays, so they stay literals here.

use rmcp::model::ToolAnnotations;
use serde_json::{Value, json};

use flakeradar_db::reads;

use crate::mcp::{BoxFuture, McpServer, ToolAnswer, ToolResult, no_arguments, read_only};

/// The tool's name on the wire.
pub(super) const NAME: &str = "list_repos";
/// The title a client shows.
pub(super) const TITLE: &str = "List Repos";
/// The description a client shows.
pub(super) const DESCRIPTION: &str = "List every Repo with its Projects (name and Project root).";

/// Whether this tool answers a list, wrapped in `{"result": …}`.
///
/// The definition's output schema, the answer's `_meta` and the shape the handler
/// answers in all come from this one statement, so a tool cannot declare a list
/// and answer an object.
pub(super) const LISTS: bool = true;

/// The tool's argument schema.
pub(super) fn input_schema() -> Value {
    json!({"additionalProperties": false, "properties": {}, "type": "object"})
}

/// The annotations a read-only tool carries.
pub(super) fn annotations() -> ToolAnnotations {
    read_only()
}

/// Runs the tool over its arguments.
pub(super) fn call(server: &McpServer, arguments: Value) -> BoxFuture<'_, ToolResult> {
    Box::pin(async move {
        no_arguments(&arguments)?;
        ToolAnswer::shaped(LISTS, &reads::list_repos(server.pool()).await?)
    })
}
