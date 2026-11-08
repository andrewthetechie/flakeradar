//! The eleven tools, one file each, in the order `tools/list` answers.
//!
//! Each tool file states its own name, title, description, argument schema,
//! argument struct, annotations and handler, so adding a tool is one file plus
//! one row in [`TOOLS`]. [`definition`] turns a row into the `Tool` the wire
//! carries, and the handler is what `tools/call` runs.

// Rust guideline compliant 2026-07-21

mod ci_run_failures;
mod get_job;
mod get_test;
mod link_issue;
mod list_projects;
mod list_repos;
mod search_jobs;
mod search_tests;
mod top_flaky_jobs;
mod top_flaky_tests;
mod unlink_issue;

use std::sync::Arc;

use rmcp::model::{Tool, ToolAnnotations};
use serde_json::Value;

use crate::mcp::{
    BoxFuture, McpServer, ToolResult, object, object_output_schema, tool_meta,
    wrapped_output_schema,
};

/// What one tool's handler is, as the table stores it.
pub(crate) type Handler = for<'a> fn(&'a McpServer, Value) -> BoxFuture<'a, ToolResult>;

/// One tool, as `tools/list` and `tools/call` both read it.
pub(crate) struct ToolSpec {
    /// The name a client calls.
    pub name: &'static str,
    /// The title a client shows.
    pub title: &'static str,
    /// The description a client shows.
    pub description: &'static str,
    /// The argument schema, as the wire declares it.
    pub input_schema: fn() -> Value,
    /// Whether the answer is the `{"result": …}` wrap.
    pub lists: bool,
    /// The annotations a client reads.
    pub annotations: fn() -> ToolAnnotations,
    /// The handler `tools/call` runs.
    pub call: Handler,
}

/// One table row, naming what a tool file declares.
macro_rules! row {
    ($module:ident) => {
        ToolSpec {
            name: $module::NAME,
            title: $module::TITLE,
            description: $module::DESCRIPTION,
            input_schema: $module::input_schema,
            lists: $module::LISTS,
            annotations: $module::annotations,
            call: $module::call,
        }
    };
}

/// Every tool, in the order `tools/list` answers.
pub(crate) const TOOLS: [ToolSpec; 11] = [
    row!(list_repos),
    row!(list_projects),
    row!(top_flaky_tests),
    row!(search_tests),
    row!(get_test),
    row!(top_flaky_jobs),
    row!(search_jobs),
    row!(get_job),
    row!(ci_run_failures),
    row!(link_issue),
    row!(unlink_issue),
];

/// The row a client's tool name names, or `None` when no tool has it.
pub(crate) fn named(name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|spec| spec.name == name)
}

/// The definition `tools/list` answers for one row.
///
/// The output schema follows the row's own statement of whether the answer is a
/// list, so the wrap is declared in one place per tool.
pub(crate) fn definition(spec: &ToolSpec) -> Tool {
    Tool::new(spec.name, spec.description, object((spec.input_schema)()))
        .with_title(spec.title)
        .with_raw_output_schema(Arc::new(object(output_schema(spec.lists))))
        .with_annotations((spec.annotations)())
        .with_meta(tool_meta())
}

/// The output schema a tool's declared shape carries.
fn output_schema(lists: bool) -> Value {
    if lists {
        wrapped_output_schema()
    } else {
        object_output_schema()
    }
}
