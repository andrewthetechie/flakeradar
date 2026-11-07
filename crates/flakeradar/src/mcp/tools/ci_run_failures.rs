//! The `ci_run_failures` tool: the failing Tests and failed Jobs of one CI run attempt.
//!
//! The name, title, description, argument schema and annotations are the wire
//! contract `tests/parity` replays, so they stay literals here.

use rmcp::model::ToolAnnotations;
use serde::Deserialize;
use serde_json::{Value, json};

use flakeradar_core::{identity::normalize_repo, py::strip_wire_whitespace};
use flakeradar_db::reads::{self, FAILURES_LIMIT};

use crate::mcp::{
    BoxFuture, McpServer, ToolAnswer, ToolFailure, ToolResult, lax_optional_integer, nullable,
    parse, read_only,
};

/// The tool's name on the wire.
pub(super) const NAME: &str = "ci_run_failures";
/// The title a client shows.
pub(super) const TITLE: &str = "Ci Run Failures";
/// The description a client shows.
pub(super) const DESCRIPTION: &str =
    "Tests that failed in one CI run attempt (the latest unless ci_run_attempt
is given), each with its current tier and score, and the CI run's failed
jobs marked explained or unexplained. reports.pending > 0 means some
reports are not processed yet, so the lists can be incomplete.";

/// Whether this tool answers a list, wrapped in `{"result": …}`.
///
/// The definition's output schema, the answer's `_meta` and the shape the handler
/// answers in all come from this one statement, so a tool cannot declare a list
/// and answer an object.
pub(super) const LISTS: bool = false;

/// The arguments `ci_run_failures` takes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CiRunFailuresArgs {
    /// The Repo's `owner/name`.
    repo: String,
    /// The CI provider's run id.
    ci_run_id: String,
    /// Which attempt to read; the newest when absent.
    #[serde(default, deserialize_with = "lax_optional_integer")]
    ci_run_attempt: Option<i64>,
}

/// The tool's argument schema.
pub(super) fn input_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "ci_run_attempt": nullable("integer"),
            "ci_run_id": {"type": "string"},
            "repo": {"type": "string"},
        },
        "required": ["repo", "ci_run_id"],
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
        let args: CiRunFailuresArgs = parse(arguments)?;
        if let Some(attempt) = args.ci_run_attempt
            && attempt < 1
        {
            return Err(ToolFailure::Tool(
                "ci_run_attempt must be 1 or more".to_owned(),
            ));
        }
        if strip_wire_whitespace(&args.ci_run_id).is_empty() {
            return Err(ToolFailure::Tool("ci_run_id must not be empty".to_owned()));
        }
        let name = normalize_repo(&args.repo)?;
        let result = reads::ci_run_failures(
            server.pool(),
            &name,
            &args.ci_run_id,
            args.ci_run_attempt,
            &server.read_settings(),
            FAILURES_LIMIT,
        )
        .await?;
        ToolAnswer::shaped(LISTS, &result)
    })
}
