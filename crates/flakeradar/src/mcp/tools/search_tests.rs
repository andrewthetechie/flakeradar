//! The `search_tests` tool: Tests whose name, classname or file contains the query.
//!
//! The name, title, description, argument schema and annotations are the wire
//! contract `tests/parity` replays, so they stay literals here.

use rmcp::model::ToolAnnotations;
use serde::Deserialize;
use serde_json::{Value, json};

use flakeradar_db::reads;

use crate::mcp::{
    BoxFuture, DEFAULT_LIMIT, McpServer, ToolAnswer, ToolResult, check_limit, checked_query,
    default_limit, lax_integer, nullable, parse, read_only, scope,
};

/// The tool's name on the wire.
pub(super) const NAME: &str = "search_tests";
/// The title a client shows.
pub(super) const TITLE: &str = "Search Tests";
/// The description a client shows.
pub(super) const DESCRIPTION: &str =
    "Find tests in a Repo whose name, classname or file contains `query`.";

/// Whether this tool answers a list, wrapped in `{"result": …}`.
///
/// The definition's output schema and the answer's `_meta` are both derived from
/// this one statement.
pub(super) const LISTS: bool = true;

/// The arguments `search_tests` takes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchTestsArgs {
    /// The Repo's `owner/name`.
    repo: String,
    /// The text to look for.
    query: String,
    /// The Project's name, or none for the whole Repo.
    #[serde(default)]
    project: Option<String>,
    /// How many Tests to answer.
    #[serde(default = "default_limit", deserialize_with = "lax_integer")]
    limit: i64,
}

/// The tool's argument schema.
pub(super) fn input_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "limit": {"default": DEFAULT_LIMIT, "type": "integer"},
            "project": nullable("string"),
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
        let args: SearchTestsArgs = parse(arguments)?;
        check_limit(args.limit)?;
        let query = checked_query(&args.query)?;
        let scope = scope(&args.repo, args.project.as_deref())?;
        let found = reads::search_tests(
            server.pool(),
            &scope,
            query,
            &server.read_settings(),
            args.limit,
        )
        .await?;
        ToolAnswer::list(&found)
    })
}
