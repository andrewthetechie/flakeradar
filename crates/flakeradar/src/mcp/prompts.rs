//! The one prompt `prompts/list` answers and the text `prompts/get` builds from it.
//!
//! The prompt walks an agent through investigating one flaky Test with this
//! server's own tools, so every investigation reads the same evidence and links the
//! Issue it files. The text is a template: building it reads no database row and
//! calls no model, so the answer is the same length and the same shape whatever
//! the deployment holds. ADR 0006 keeps the diagnosis with the agent, which is why
//! the template tells it to read the Failure details rather than trust
//! `failure_category`.

// Rust guideline compliant 2026-07-21

use flakeradar_core::lax;
use rmcp::ErrorData as McpError;
use rmcp::model::{GetPromptRequestParams, Prompt, PromptArgument};
use serde_json::Value;

/// The prompt's name on the wire.
pub(super) const NAME: &str = "investigate_flaky_test";

/// The title a client shows.
pub(super) const TITLE: &str = "Investigate Flaky Test";

/// The description a client shows.
pub(super) const DESCRIPTION: &str =
    "Step-by-step investigation of one flaky Test, ending with the Issue link it files.";

/// The text `prompts/get` answers for `request`, or the invalid-params refusal one
/// of its fields earned.
///
/// A name no prompt has is refused rather than answered with the closest one, so a
/// client with a stale list learns which name is stale.
pub(super) fn text(request: &GetPromptRequestParams) -> Result<String, McpError> {
    if request.name != NAME {
        return Err(McpError::invalid_params(
            format!("Unknown prompt: '{}'", request.name),
            None,
        ));
    }
    let fields = request.arguments.as_ref();
    let test_id = fields
        .and_then(|fields| fields.get("test_id"))
        .and_then(Value::as_str)
        .and_then(lax::text_int)
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            refused(
                "test_id",
                "the Test's id, as a positive integer in a string",
            )
        })?;
    let repo = match fields.and_then(|fields| fields.get("repo")) {
        None | Some(Value::Null) => None,
        Some(Value::String(repo)) => Some(repo.as_str()),
        Some(_) => return Err(refused("repo", "the Repo's owner/name, in a string")),
    };
    Ok(template(test_id, repo))
}

/// The refusal an argument that is not what the prompt takes earns.
fn refused(argument: &str, expectation: &str) -> McpError {
    McpError::invalid_params(format!("`{NAME}` needs {argument}: {expectation}"), None)
}

/// The prompt text for Test `test_id`, in `repo` when the client named one.
///
/// The steps are in the order an investigation reads the evidence in, and they
/// name the tools instead of restating what they answer: the agent calls them. The
/// last line keeps Quarantine a human decision, because a client that shows only
/// the prompt would otherwise never say it.
fn template(test_id: i64, repo: Option<&str>) -> String {
    let subject = match repo {
        Some(repo) => format!("the flaky Test {test_id} in the Repo '{repo}'"),
        None => format!("the flaky Test {test_id}"),
    };
    format!(
        "Investigate {subject}.\n\
         \n\
         1. Call get_test with test_id={test_id}.\n\
         2. Read the latest Failure message and the Failure details, and open the \
         Location: the file, the line, and the GitHub permalink at the last failing \
         commit.\n\
         3. Check score_history and clean_streak to see whether a past fix held: a \
         growing clean_streak alongside a falling score is a fix that held.\n\
         4. Treat failure_category as a hint, not a diagnosis; read the failure \
         details before you conclude.\n\
         5. When you file a GitHub issue for it, call link_issue with that issue \
         number and test_id={test_id}, so FlakeRadar shows the link and files no \
         duplicate.\n\
         \n\
         Quarantine stays a human decision: recommend it in your report, and write \
         nothing."
    )
}

/// The definition of this prompt, as `prompts/list` answers it.
pub(super) fn definition() -> Prompt {
    Prompt::new(
        NAME,
        Some(DESCRIPTION),
        Some(vec![
            PromptArgument::new("test_id")
                .with_description("The Test's id, from top_flaky_tests or search_tests")
                .with_required(true),
            PromptArgument::new("repo").with_description("The Repo's owner/name"),
        ]),
    )
    .with_title(TITLE)
}
