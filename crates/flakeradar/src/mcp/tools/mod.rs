//! The eleven tools, one file each, in the order `tools/list` answers.
//!
//! Each tool file states its own name, title, description, argument schema,
//! argument struct, annotations and handler, so adding a tool is one file plus
//! one row in [`TOOLS`]. [`definition`] turns a row into the `Tool` the wire
//! carries, and the handler is what `tools/call` runs. The schema and the
//! argument struct are two statements of the same arguments, and the test case
//! here asserts they name the same fields.

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

/// The field names the derived `Deserialize` of one argument struct accepts.
///
/// The list is serde's own: the derived implementation hands
/// [`serde::de::Deserializer::deserialize_struct`] the field names it was derived
/// from, so [`Recorder`] stores that slice and builds nothing. Asking serde is
/// what keeps a third statement of the arguments from ever appearing.
#[cfg(test)]
pub(super) fn record_fields<T: serde::de::DeserializeOwned>() -> &'static [&'static str] {
    let recorder = Recorder::default();
    // The recorder stops at the struct, so nothing is ever built and the answer
    // is always its error. The field slice it stored is the whole point.
    let _ = T::deserialize(&recorder);
    recorder
        .fields
        .get()
        .expect("serde named the struct's fields")
}

/// The field names a [`Recorder`] was handed, until it was.
#[cfg(test)]
#[derive(Default)]
struct Recorder {
    fields: std::cell::Cell<Option<&'static [&'static str]>>,
}

/// Why a [`Recorder`] never answers with a value.
#[cfg(test)]
#[derive(Debug)]
struct Stopped;

/// What a refusal says: a recorder has read a struct's field names and nothing
/// else, so every path that wants a value reaches the same sentence.
#[cfg(test)]
const REFUSAL: &str = "the recorder stopped at the field list";

#[cfg(test)]
impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REFUSAL)
    }
}

#[cfg(test)]
impl std::error::Error for Stopped {}

#[cfg(test)]
impl serde::de::Error for Stopped {
    fn custom<T: std::fmt::Display>(_message: T) -> Self {
        // Every message a recorder can be refused with is [`REFUSAL`], so which
        // one serde carried makes no difference to the answer.
        Self
    }
}

#[cfg(test)]
impl<'de> serde::de::Deserializer<'de> for &Recorder {
    type Error = Stopped;

    fn deserialize_any<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        // Every type but a struct arrives here, through the forwarding macro
        // below, and none of them can be built from a field list.
        Err(serde::de::Error::custom(REFUSAL))
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.fields.set(Some(fields));
        // The names it came for are already stored, so this refusal carries no
        // message of its own.
        Err(Stopped)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        option unit unit_struct newtype_struct seq tuple tuple_struct map identifier ignored_any
        enum
    }
}

#[cfg(test)]
mod tests {
    //! A tool states its arguments twice, in the schema it publishes and in the
    //! struct its handler parses, and the two are checked against each other
    //! here: [`TOOLS`] is `pub(crate)` and every argument struct is private to
    //! its tool file, so no integration test can reach either half of a pair.

    use std::collections::BTreeSet;

    use serde::Deserialize as _;
    use serde_json::Value;

    use super::{
        REFUSAL, Recorder, TOOLS, ci_run_failures, get_job, get_test, link_issue, list_projects,
        list_repos, search_jobs, search_tests, top_flaky_jobs, top_flaky_tests, unlink_issue,
    };

    /// The fields one tool's argument struct accepts, as serde names them.
    type Fields = fn() -> &'static [&'static str];

    /// Every tool's accepted fields, in the order [`TOOLS`] answers `tools/list`.
    ///
    /// The list names tools and holds one function pointer each; the argument
    /// names themselves come from serde, so nothing here restates a schema.
    const ACCEPTED: [(&str, Fields); 11] = [
        (list_repos::NAME, list_repos::accepted_fields),
        (list_projects::NAME, list_projects::accepted_fields),
        (top_flaky_tests::NAME, top_flaky_tests::accepted_fields),
        (search_tests::NAME, search_tests::accepted_fields),
        (get_test::NAME, get_test::accepted_fields),
        (top_flaky_jobs::NAME, top_flaky_jobs::accepted_fields),
        (search_jobs::NAME, search_jobs::accepted_fields),
        (get_job::NAME, get_job::accepted_fields),
        (ci_run_failures::NAME, ci_run_failures::accepted_fields),
        (link_issue::NAME, link_issue::accepted_fields),
        (unlink_issue::NAME, unlink_issue::accepted_fields),
    ];

    #[test]
    fn every_tools_schema_names_exactly_the_fields_it_accepts() {
        let listed: Vec<&str> = TOOLS.iter().map(|spec| spec.name).collect();
        let declared: Vec<&str> = ACCEPTED.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            declared, listed,
            "the table and the field statement name different tools"
        );

        for (spec, (_, fields)) in TOOLS.iter().zip(ACCEPTED) {
            let schema = (spec.input_schema)();
            let properties = schema
                .get("properties")
                .and_then(Value::as_object)
                .expect("a published schema declares a properties object");
            let published: BTreeSet<&str> = properties.keys().map(String::as_str).collect();
            let accepted: BTreeSet<&str> = fields().iter().copied().collect();
            assert_eq!(
                published, accepted,
                "{}'s schema and its argument struct name different arguments",
                spec.name
            );
            for required in schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let declared = required.as_str();
                assert!(
                    declared.is_some_and(|name| properties.contains_key(name)),
                    "{} requires an argument it does not declare: {required}",
                    spec.name
                );
            }
            assert_eq!(
                schema.get("additionalProperties"),
                Some(&Value::Bool(false)),
                "{} leaves its arguments open",
                spec.name
            );
        }
    }

    /// Why the field list [`super::record_fields`] returns is serde's and not the
    /// recorder's invention: a recorder answers a struct with its field names
    /// and refuses every other type, so it can never supply an argument name.
    #[test]
    fn a_recorder_names_a_struct_and_refuses_every_other_type() {
        let recorder = Recorder::default();
        let refusal = bool::deserialize(&recorder).expect_err("a recorder builds no value");

        assert_eq!(refusal.to_string(), REFUSAL);
        assert!(
            recorder.fields.get().is_none(),
            "a type without fields named none"
        );
    }
}
