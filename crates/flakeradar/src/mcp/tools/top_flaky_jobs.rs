//! The `top_flaky_jobs` tool: the Job leaderboard of one Repo.
//!
//! The name, title, description, argument schema and annotations are the wire
//! contract the frozen server recorded, so they stay literals here.

// Rust guideline compliant 2026-07-21

use rmcp::model::ToolAnnotations;
use serde::Deserialize;
use serde_json::{Value, json};

use flakeradar_core::normalize_repo;
use flakeradar_db::reads::{self, JobListFilters, ListOptions};

use crate::mcp::{
    BoxFuture, DEFAULT_LIMIT, McpServer, ToolAnswer, ToolResult, check_limit,
    check_new_within_days, default_limit, default_true, lax_boolean, lax_integer,
    lax_optional_integer, nullable, parse, read_only,
};

/// The tool's name on the wire.
pub(super) const NAME: &str = "top_flaky_jobs";
/// The title a client shows.
pub(super) const TITLE: &str = "Top Flaky Jobs";
/// The description a client shows.
pub(super) const DESCRIPTION: &str =
    "Worst CI jobs first in a Repo. Job scores count only UNEXPLAINED failures
(failures with no failing test in the same job). new_within_days=N keeps
only jobs that became flaky in the last N days: score at/above the
threshold now, below it N days ago or with no history that old.";

/// Whether this tool answers a list, wrapped in `{"result": …}`.
///
/// The definition's output schema and the answer's `_meta` are both derived from
/// this one statement.
pub(super) const LISTS: bool = true;

/// The arguments `top_flaky_jobs` takes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopFlakyJobsArgs {
    /// The Repo's `owner/name`.
    repo: String,
    /// How many Jobs to answer.
    #[serde(default = "default_limit", deserialize_with = "lax_integer")]
    limit: i64,
    /// Whether `suspect` tier Jobs are listed.
    #[serde(default = "default_true", deserialize_with = "lax_boolean")]
    include_suspect: bool,
    /// The "became flaky recently" window, in days.
    #[serde(default, deserialize_with = "lax_optional_integer")]
    new_within_days: Option<i64>,
}

/// The tool's argument schema.
pub(super) fn input_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "include_suspect": {"default": true, "type": "boolean"},
            "limit": {"default": DEFAULT_LIMIT, "type": "integer"},
            "new_within_days": nullable("integer"),
            "repo": {"type": "string"},
        },
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
        let args: TopFlakyJobsArgs = parse(arguments)?;
        check_limit(args.limit)?;
        if let Some(days) = args.new_within_days {
            check_new_within_days(days)?;
        }
        let name = normalize_repo(&args.repo)?;
        let filters = JobListFilters { pipeline: None };
        let options =
            ListOptions::top_flaky(args.limit, args.include_suspect, args.new_within_days);
        let page = reads::list_jobs(
            server.pool(),
            Some(&name),
            &filters,
            &options,
            &server.read_settings(),
        )
        .await?;
        ToolAnswer::list(&page.items)
    })
}
