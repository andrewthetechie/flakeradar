//! The `link_issue` tool: records that a GitHub issue tracks a Test or Job.
//!
//! The name, title, description, argument schema and annotations are the wire
//! contract `tests/parity` replays, so they stay literals here.

use rmcp::model::ToolAnnotations;
use serde::Deserialize;
use serde_json::{Value, json};

use flakeradar_db::issue_links;

use crate::mcp::{
    BoxFuture, McpServer, ToolResult, lax_boolean, lax_integer, lax_optional_integer,
    linked_answer, nullable, one_target, parse,
};

/// The tool's name on the wire.
pub(super) const NAME: &str = "link_issue";
/// The title a client shows.
pub(super) const TITLE: &str = "Link Issue";
/// The description a client shows.
pub(super) const DESCRIPTION: &str =
    "Record that GitHub issue #number tracks a flaky test or job. Pass
exactly one of test_id or job_id. The issue must be in that test's or
job's own repo. FlakeRadar then shows the issue and files none of its
own, and clears the link when the issue closes. Fails when a different
issue is already linked, unless replace=True. Returns the test or job.";

/// Whether this tool answers a list, wrapped in `{"result": …}`.
///
/// The definition's output schema and the answer's `_meta` are both derived from
/// this one statement.
pub(super) const LISTS: bool = false;

/// The arguments `link_issue` takes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkIssueArgs {
    /// The GitHub issue number.
    #[serde(deserialize_with = "lax_integer")]
    number: i64,
    /// The Test to link, when the target is a Test.
    #[serde(default, deserialize_with = "lax_optional_integer")]
    test_id: Option<i64>,
    /// The Job to link, when the target is a Job.
    #[serde(default, deserialize_with = "lax_optional_integer")]
    job_id: Option<i64>,
    /// Whether a different linked issue may be replaced.
    #[serde(default, deserialize_with = "lax_boolean")]
    replace: bool,
}

/// The tool's argument schema.
pub(super) fn input_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "job_id": nullable("integer"),
            "number": {"type": "integer"},
            "replace": {"default": false, "type": "boolean"},
            "test_id": nullable("integer"),
        },
        "required": ["number"],
        "type": "object",
    })
}

/// The annotations `link_issue` carries: additive, repeatable, closed-world.
pub(super) fn annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(false)
        .destructive(false)
        .idempotent(true)
        .open_world(false)
}

/// Runs the tool over its arguments.
pub(super) fn call(server: &McpServer, arguments: Value) -> BoxFuture<'_, ToolResult> {
    Box::pin(async move {
        let args: LinkIssueArgs = parse(arguments)?;
        let (kind, id) = one_target(args.test_id, args.job_id)?;
        let linked = issue_links::link_issue(
            server.pool(),
            kind,
            id,
            args.number,
            None,
            args.replace,
            &server.read_settings(),
        )
        .await?;
        linked_answer(&linked)
    })
}
