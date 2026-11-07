//! The `list_projects` tool: the Projects of one Repo.
//!
//! The name, title, description, argument schema and annotations are the wire
//! contract `tests/parity` replays, so they stay literals here.

use rmcp::model::ToolAnnotations;
use serde::Deserialize;
use serde_json::{Value, json};

use flakeradar_core::identity::normalize_repo;
use flakeradar_db::reads;

use crate::mcp::{BoxFuture, McpServer, ToolAnswer, ToolFailure, ToolResult, parse, read_only};

/// The tool's name on the wire.
pub(super) const NAME: &str = "list_projects";
/// The title a client shows.
pub(super) const TITLE: &str = "List Projects";
/// The description a client shows.
pub(super) const DESCRIPTION: &str =
    "List the Projects (test suites) of one Repo, e.g. repo='owner/name'.";

/// Whether this tool answers a list, wrapped in `{"result": …}`.
///
/// The definition's output schema, the answer's `_meta` and the shape the handler
/// answers in all come from this one statement, so a tool cannot declare a list
/// and answer an object.
pub(super) const LISTS: bool = true;

/// The arguments `list_projects` takes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListProjectsArgs {
    /// The Repo's `owner/name`.
    repo: String,
}

/// The tool's argument schema.
pub(super) fn input_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {"repo": {"type": "string"}},
        "required": ["repo"],
        "type": "object",
    })
}

/// The annotations a read-only tool carries.
pub(super) fn annotations() -> ToolAnnotations {
    read_only()
}

/// Runs the tool over its arguments.
pub(super) fn call(server: &McpServer, arguments: Value) -> BoxFuture<'_, ToolResult> {
    Box::pin(async move {
        let ListProjectsArgs { repo } = parse(arguments)?;
        let name = normalize_repo(&repo)?;
        for found in reads::list_repos(server.pool()).await? {
            if found.name == name {
                return ToolAnswer::shaped(LISTS, &found.projects);
            }
        }
        Err(ToolFailure::Tool(format!(
            "Unknown repo '{name}'. Call list_repos to see what exists."
        )))
    })
}
