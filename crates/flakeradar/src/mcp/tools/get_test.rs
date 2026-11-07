//! The `get_test` tool: one Test's detail page plus its newest failure.
//!
//! The name, title, description, argument schema and annotations are the wire
//! contract `tests/parity` replays, so they stay literals here.

use chrono::{DateTime, Utc};
use rmcp::model::ToolAnnotations;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use flakeradar_core::{
    classify::FailureCategory, dto::ExecutionOut, dto::LocationOut, dto::TestJobLinkOut,
    dto::TestOut, dto::timestamp, identity::DEFAULT_PROJECT, model::ExecutionStatus,
};
use flakeradar_db::reads;

use crate::mcp::{
    BoxFuture, DEFAULT_LIMIT, McpServer, ScorePointSummary, ToolAnswer, ToolFailure, ToolResult,
    cap_details, check_limit, default_limit, default_project, lax_integer, lax_optional_integer,
    nullable, only_id, parse, read_only, scope, score_points,
};

/// The tool's name on the wire.
pub(super) const NAME: &str = "get_test";
/// The title a client shows.
pub(super) const TITLE: &str = "Get Test";
/// The description a client shows.
pub(super) const DESCRIPTION: &str =
    "Everything about one test: pass test_id, or repo + project + name
(+ classname when the name is ambiguous). Returns Location with a
GitHub permalink at the last failing commit, the latest failure's
message and details (traceback, capped at 4 KB), and recent executions.";

/// Whether this tool answers a list, wrapped in `{"result": …}`.
///
/// The definition's output schema and the answer's `_meta` are both derived from
/// this one statement.
pub(super) const LISTS: bool = false;

/// The arguments `get_test` takes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GetTestArgs {
    /// The Test's id, when the caller has one.
    #[serde(default, deserialize_with = "lax_optional_integer")]
    test_id: Option<i64>,
    /// The Repo's `owner/name`, for a lookup by name.
    #[serde(default)]
    repo: Option<String>,
    /// The Project's name, for a lookup by name.
    #[serde(default = "default_project")]
    project: String,
    /// The Test's name, for a lookup by name.
    #[serde(default)]
    name: Option<String>,
    /// The Test's classname, when the name is ambiguous.
    #[serde(default)]
    classname: Option<String>,
    /// How many Executions to answer.
    #[serde(default = "default_limit", deserialize_with = "lax_integer")]
    executions_limit: i64,
}

/// The tool's argument schema.
pub(super) fn input_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "classname": nullable("string"),
            "executions_limit": {"default": DEFAULT_LIMIT, "type": "integer"},
            "name": nullable("string"),
            "project": {"default": DEFAULT_PROJECT, "type": "string"},
            "repo": nullable("string"),
            "test_id": nullable("integer"),
        },
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
        let args: GetTestArgs = parse(arguments)?;
        check_limit(args.executions_limit)?;
        let read_settings = server.read_settings();
        let test_id = if let Some(test_id) = args.test_id {
            test_id
        } else {
            let (Some(repo), Some(name)) = (args.repo.as_deref(), args.name.as_deref()) else {
                return Err(ToolFailure::Tool(
                    "Pass test_id, or repo + name (+ project, classname).".to_owned(),
                ));
            };
            let scope = scope(repo, Some(args.project.as_str()))?;
            let ids = reads::find_test_ids(
                server.pool(),
                scope.repo().unwrap_or_default(),
                scope.project().unwrap_or(DEFAULT_PROJECT),
                name,
                args.classname.as_deref(),
            )
            .await?;
            only_id(
                &ids,
                "test",
                name,
                &format!(
                    "{} / {}",
                    scope.repo().unwrap_or_default(),
                    scope.project().unwrap_or(DEFAULT_PROJECT)
                ),
                "classname",
            )?
        };
        let Some(history) = reads::get_test(
            server.pool(),
            test_id,
            &read_settings,
            args.executions_limit,
        )
        .await?
        else {
            return Err(ToolFailure::Tool(format!("No test with id {test_id}.")));
        };
        let failure = reads::latest_failure(server.pool(), test_id).await?;
        ToolAnswer::object(&TestDetail {
            test: history.test,
            location: history.location,
            last_failing_sha: history.last_failing_sha,
            last_failing_branch: history.last_failing_branch,
            latest_failure: failure.as_ref().map(LatestFailure::from),
            executions: history
                .executions
                .iter()
                .map(ExecutionSummary::from)
                .collect(),
            jobs: history.jobs,
            score_history: score_points(&history.score_history),
        })
    })
}

/// One Test's newest failing Execution, in `get_test`'s shape.
#[derive(Debug, Serialize)]
struct LatestFailure {
    /// What the Execution did.
    status: ExecutionStatus,
    /// The Failure message.
    message: String,
    /// The Failure details, capped at the tool's details limit.
    details: String,
    /// The commit the Execution ran.
    commit_sha: String,
    /// The branch the commit is on.
    branch: String,
    /// The Failure category.
    failure_category: Option<FailureCategory>,
    /// When the Execution was recorded.
    #[serde(with = "timestamp")]
    created_at: DateTime<Utc>,
}

impl From<&ExecutionOut> for LatestFailure {
    fn from(failure: &ExecutionOut) -> Self {
        Self {
            status: failure.status,
            message: failure.message.clone(),
            details: cap_details(&failure.details),
            commit_sha: failure.commit_sha.clone(),
            branch: failure.branch.clone(),
            failure_category: failure.failure_category,
            created_at: failure.created_at,
        }
    }
}

/// One Execution in `get_test`'s shape: the nine fields an agent reads, without
/// the traceback it already has from `latest_failure`.
#[derive(Debug, Serialize)]
struct ExecutionSummary {
    /// What the Execution did.
    status: ExecutionStatus,
    /// The commit the Execution ran.
    commit_sha: String,
    /// The branch the commit is on.
    branch: String,
    /// The CI provider's run id.
    ci_run_id: String,
    /// When the Execution was recorded.
    #[serde(with = "timestamp")]
    created_at: DateTime<Utc>,
    /// How long it took, in seconds.
    duration: f64,
    /// The Failure message.
    message: String,
    /// Which retry inside the Run this is, numbered from 0.
    attempt: i64,
    /// The Failure category, when the Execution failed.
    failure_category: Option<FailureCategory>,
}

impl From<&ExecutionOut> for ExecutionSummary {
    fn from(execution: &ExecutionOut) -> Self {
        Self {
            status: execution.status,
            commit_sha: execution.commit_sha.clone(),
            branch: execution.branch.clone(),
            ci_run_id: execution.ci_run_id.clone(),
            created_at: execution.created_at,
            duration: execution.duration,
            message: execution.message.clone(),
            attempt: execution.attempt,
            failure_category: execution.failure_category,
        }
    }
}

/// A Test's `get_test` answer.
#[derive(Debug, Serialize)]
struct TestDetail {
    /// The Test itself.
    test: TestOut,
    /// Where the Test is defined, when the runner reported a file.
    location: Option<LocationOut>,
    /// The commit of the newest failing Execution.
    last_failing_sha: Option<String>,
    /// The branch of the newest failing Execution.
    last_failing_branch: Option<String>,
    /// The newest failing Execution, capped.
    latest_failure: Option<LatestFailure>,
    /// The newest Executions first.
    executions: Vec<ExecutionSummary>,
    /// The Jobs whose Executions came from these Runs.
    jobs: Vec<TestJobLinkOut>,
    /// The Score history of the last 30 days, oldest first.
    score_history: Vec<ScorePointSummary>,
}
