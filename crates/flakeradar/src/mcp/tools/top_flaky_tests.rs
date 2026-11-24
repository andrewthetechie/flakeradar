//! The `top_flaky_tests` tool: the Test leaderboard, optionally one Project.
//!
//! The name, title, description, argument schema and annotations are the wire
//! contract `tests/parity` replays, so they stay literals here.

// Rust guideline compliant 2026-07-21

use rmcp::model::ToolAnnotations;
use serde::Deserialize;
use serde_json::{Value, json};

use flakeradar_core::model::EntityKind;
use flakeradar_db::reads::{self, EntityFilters, ListOptions, TestListFilters};

use crate::mcp::{
    BoxFuture, DEFAULT_LIMIT, McpServer, ToolAnswer, ToolResult, check_limit,
    check_new_within_days, default_limit, default_true, lax_boolean, lax_integer,
    lax_optional_integer, nullable, parse, parse_category, parse_sort, read_only, scope,
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
    /// The sort order, or none for `score`.
    #[serde(default)]
    sort: Option<String>,
}

/// The fields `TopFlakyTestsArgs` accepts, as serde names them.
#[cfg(test)]
pub(super) fn accepted_fields() -> &'static [&'static str] {
    super::record_fields::<TopFlakyTestsArgs>()
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
            "sort": nullable("string"),
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
        let sort = match args.sort.as_deref() {
            Some(text) => Some(parse_sort(text)?),
            None => None,
        };
        if let Some(days) = args.new_within_days {
            check_new_within_days(days)?;
        }
        let scope = scope(&args.repo, args.project.as_deref())?;
        // The tool offers `file` and `category` and nothing else, so the arms it
        // does not offer are the defaults of the Tests listing.
        let filters = EntityFilters::Test(TestListFilters {
            file: args.file,
            category,
            ..Default::default()
        });
        let options =
            ListOptions::top_flaky(args.limit, args.include_suspect, args.new_within_days, sort);
        let listing = reads::Listing {
            scope,
            options,
            filters,
        };
        let page = reads::list(
            server.pool(),
            EntityKind::Test,
            &listing,
            &server.read_settings(),
        )
        .await?;
        ToolAnswer::shaped(LISTS, &page.items())
    })
}
