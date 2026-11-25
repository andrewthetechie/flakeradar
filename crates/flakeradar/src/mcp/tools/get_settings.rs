//! The `get_settings` tool: the thresholds and windows this instance runs with.
//!
//! The name, title, description, argument schema and annotations are the wire
//! contract `tests/parity` replays, so they stay literals here.

// Rust guideline compliant 2026-07-21

use rmcp::model::ToolAnnotations;
use serde::Serialize;
use serde_json::{Value, json};

use crate::mcp::{BoxFuture, McpServer, ToolAnswer, ToolResult, no_arguments, read_only};

/// The tool's name on the wire.
pub(super) const NAME: &str = "get_settings";
/// The title a client shows.
pub(super) const TITLE: &str = "Get Settings";
/// The description a client shows.
pub(super) const DESCRIPTION: &str =
    "This instance's scoring and filing settings: thresholds, windows and the
weekly digest schedule, as text. Contains no credentials.";

/// Whether this tool answers a list, wrapped in `{"result": …}`.
///
/// The definition's output schema, the answer's `_meta` and the shape the handler
/// answers in all come from this one statement, so a tool cannot declare a list
/// and answer an object.
pub(super) const LISTS: bool = false;

/// The fields `get_settings` accepts: none, because its handler refuses every
/// argument through `no_arguments` before it reads any.
#[cfg(test)]
pub(super) fn accepted_fields() -> &'static [&'static str] {
    &[]
}

/// The tool's argument schema.
pub(super) fn input_schema() -> Value {
    json!({"additionalProperties": false, "properties": {}, "type": "object"})
}

/// The annotations a read-only tool carries.
pub(super) fn annotations() -> ToolAnnotations {
    read_only()
}

/// The GitHub filing gates, as this instance has them.
#[derive(Debug, Serialize)]
struct GithubOut {
    /// The Flakiness score a Test must reach before an issue is filed.
    min_score: f64,
    /// The Proven flake count a Test must reach before an issue is filed.
    min_proven_flakes: i64,
    /// The recent failure count a Test must reach before an issue is filed.
    min_failures: i64,
    /// The Clean streak at which FlakeRadar comments that a fix appears to hold.
    fixed_streak: i64,
    /// Whether to also close the issue whose fix appears to hold.
    close_fixed: bool,
    /// How many days a closed issue stays eligible to be reopened.
    reopen_days: i64,
}

/// This instance's settings, as the tool answers them.
///
/// The fields are the non-secret configuration a reader needs to explain a tier, a
/// broken flag, a `new` flag or a filing decision; the token, the database URL and
/// the Webhook URLs are credentials and are never read.
#[derive(Debug, Serialize)]
struct SettingsOut {
    /// The Flakiness score at or above which a Test or Job counts as flaky.
    flake_threshold: f64,
    /// The Flakiness score below which a flaky Test or Job recovers.
    recover_threshold: f64,
    /// How many recent Executions the Flakiness score weighs.
    score_window: usize,
    /// The per-step decay of that weight.
    score_decay: f64,
    /// Consecutive default-branch failures at which a Test counts as broken.
    broken_streak: i64,
    /// How many days a Test stays new.
    new_test_days: i64,
    /// The share of a Run's Tests that must fail for the Run to be infrastructure.
    infra_fail_share: f64,
    /// The fewest distinct Tests a Run must hold to be judged for infrastructure.
    infra_min_tests: i64,
    /// The filing gates, under their own keys.
    github: GithubOut,
    /// The weekly digest schedule as text, or `None` when it is off.
    digest: Option<String>,
}

/// Runs the tool over its arguments.
pub(super) fn call(server: &McpServer, arguments: Value) -> BoxFuture<'_, ToolResult> {
    Box::pin(async move {
        no_arguments(&arguments)?;
        let settings = server.settings();
        ToolAnswer::shaped(
            LISTS,
            &SettingsOut {
                flake_threshold: settings.flake_threshold,
                recover_threshold: settings.recover_threshold,
                score_window: settings.score_window,
                score_decay: settings.score_decay,
                broken_streak: settings.broken_streak,
                new_test_days: settings.new_test_days,
                infra_fail_share: settings.infra_fail_share,
                infra_min_tests: settings.infra_min_tests,
                github: GithubOut {
                    min_score: settings.github_issue_min_score,
                    min_proven_flakes: settings.github_issue_min_proven_flakes,
                    min_failures: settings.github_issue_min_failures,
                    fixed_streak: settings.github_fixed_streak,
                    close_fixed: settings.github_close_fixed,
                    reopen_days: settings.github_reopen_days,
                },
                digest: settings.digest.map(|schedule| schedule.to_string()),
            },
        )
    })
}
