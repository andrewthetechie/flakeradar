//! The `top_flaky_tests` tool: the Test leaderboard, optionally one Project.
//!
//! The name, title, description, argument schema and annotations are the wire
//! contract `tests/parity` replays, so they stay literals here.

// Rust guideline compliant 2026-07-21

use rmcp::model::ToolAnnotations;
use serde::Deserialize;
use serde_json::{Value, json};

use flakeradar_db::reads::{self, ListOptions, TestListFilters};

use crate::mcp::{
    BoxFuture, DEFAULT_LIMIT, McpServer, ToolAnswer, ToolResult, check_limit,
    check_new_within_days, default_limit, default_true, lax_boolean, lax_integer,
    lax_optional_integer, nullable, parse, parse_category, read_only, scope,
};

/// The tool's name on the wire.
pub(super) const NAME: &str = "top_flaky_tests";
/// The title a client shows.
pub(super) const TITLE: &str = "Top Flaky Tests";
/// The description a client shows.
pub(super) const DESCRIPTION: &str = "Worst tests first in a Repo (optionally one Project).

include_suspect=False returns only 'flaky' tier tests. `file` keeps
tests whose reported file contains that text (e.g. 'tests/test_cron.py').
`category` keeps tests whose likely cause (Failure category) is one of
network, environment, timing, assertion, other. new_within_days=N keeps
only tests that became flaky in the last N days: score at/above the
threshold now, below it N days ago or with no history that old.";

/// Whether this tool answers a list, wrapped in `{"result": …}`.
///
/// The definition's output schema, the answer's `_meta` and the shape the handler
/// answers in all come from this one statement, so a tool cannot declare a list
/// and answer an object.
pub(super) const LISTS: bool = true;

/// The arguments `top_flaky_tests` takes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopFlakyTestsArgs {
    /// The Repo's `owner/name`.
    repo: String,
    /// The Project's name, or none for the whole Repo.
    #[serde(default)]
    project: Option<String>,
    /// How many Tests to answer.
    #[serde(default = "default_limit", deserialize_with = "lax_integer")]
    limit: i64,
    /// Whether `suspect` tier Tests are listed.
    #[serde(default = "default_true", deserialize_with = "lax_boolean")]
    include_suspect: bool,
    /// A literal substring of the Test's file.
    #[serde(default)]
    file: Option<String>,
    /// The Failure category to list.
    #[serde(default)]
    category: Option<String>,
    /// The "became flaky recently" window, in days.
    #[serde(default, deserialize_with = "lax_optional_integer")]
    new_within_days: Option<i64>,
}

/// The tool's argument schema.
pub(super) fn input_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "category": nullable("string"),
            "file": nullable("string"),
            "include_suspect": {"default": true, "type": "boolean"},
            "limit": {"default": DEFAULT_LIMIT, "type": "integer"},
            "new_within_days": nullable("integer"),
            "project": nullable("string"),
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
        let args: TopFlakyTestsArgs = parse(arguments)?;
        check_limit(args.limit)?;
        let category = match args.category.as_deref() {
            Some(text) => Some(parse_category(text)?),
            None => None,
        };
        if let Some(days) = args.new_within_days {
            check_new_within_days(days)?;
        }
        let scope = scope(&args.repo, args.project.as_deref())?;
        let filters = TestListFilters {
            file: args.file,
            category,
            quarantined: None,
            broken: false,
        };
        let options =
            ListOptions::top_flaky(args.limit, args.include_suspect, args.new_within_days);
        let page = reads::list_tests(
            server.pool(),
            &scope,
            &filters,
            &options,
            &server.read_settings(),
        )
        .await?;
        ToolAnswer::shaped(LISTS, &page.items)
    })
}
