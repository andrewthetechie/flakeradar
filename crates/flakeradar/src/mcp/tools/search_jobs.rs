//! The `search_jobs` tool: Jobs whose name or Pipeline contains the query.
//!
//! The name, title, description, argument schema and annotations are the wire
//! contract `tests/parity` replays, so they stay literals here.

use rmcp::model::ToolAnnotations;
use serde::Deserialize;
use serde_json::{Value, json};

use flakeradar_core::identity::normalize_repo;
use flakeradar_db::reads;

use crate::mcp::{
    BoxFuture, DEFAULT_LIMIT, McpServer, ToolAnswer, ToolResult, check_limit, checked_query,
    default_limit, lax_integer, parse, read_only,
};

/// The tool's name on the wire.
pub(super) const NAME: &str = "search_jobs";
/// The title a client shows.
pub(super) const TITLE: &str = "Search Jobs";
/// The description a client shows.
pub(super) const DESCRIPTION: &str =
    "Find Jobs whose name or Pipeline (workflow file) contains `query`.";

/// Whether this tool answers a list, wrapped in `{"result": …}`.
///
/// The definition's output schema and the answer's `_meta` are both derived from
/// this one statement.
pub(super) const LISTS: bool = true;

/// The arguments `search_jobs` takes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchJobsArgs {
    /// The Repo's `owner/name`.
    repo: String,
    /// The text to look for.
    query: String,
    /// How many Jobs to answer.
    #[serde(default = "default_limit", deserialize_with = "lax_integer")]
    limit: i64,
}

/// The tool's argument schema.
pub(super) fn input_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "limit": {"default": DEFAULT_LIMIT, "type": "integer"},
            "query": {"type": "string"},
            "repo": {"type": "string"},
        },
        "required": ["repo", "query"],
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
        let args: SearchJobsArgs = parse(arguments)?;
        check_limit(args.limit)?;
        let query = checked_query(&args.query)?;
        let name = normalize_repo(&args.repo)?;
        let found = reads::search_jobs(
            server.pool(),
            &name,
            query,
            &server.read_settings(),
            args.limit,
        )
        .await?;
        ToolAnswer::list(&found)
    })
}
