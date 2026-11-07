//! The `get_job` tool: one Job's detail page, trimmed to what an agent needs.
//!
//! The name, title, description, argument schema and annotations are the wire
//! contract `tests/parity` replays, so they stay literals here.

use chrono::{DateTime, Utc};
use rmcp::model::ToolAnnotations;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use flakeradar_core::{
    dto::ExplainingJobOut, dto::ExplainingTestOut, dto::JobExecutionOut, dto::JobOut,
    dto::RunnerStatOut, dto::timestamp, identity::normalize_repo, model::JobOutcome,
};
use flakeradar_db::reads;

use crate::mcp::{
    BoxFuture, DEFAULT_LIMIT, EXPLAINED_BY_MAX, McpServer, ScorePointSummary, ToolAnswer,
    ToolFailure, ToolResult, check_limit, default_limit, lax_integer, lax_optional_integer,
    nullable, only_id, parse, read_only, score_points,
};

/// The tool's name on the wire.
pub(super) const NAME: &str = "get_job";
/// The title a client shows.
pub(super) const TITLE: &str = "Get Job";
/// The description a client shows.
pub(super) const DESCRIPTION: &str =
    "One Job: score, tier, unexplained vs explained failures, failures grouped
by runner (label set, plus runner names seen more than once), and recent
executions (outcome, sha, branch, attempt, runner, CI url, explaining tests).";

/// Whether this tool answers a list, wrapped in `{"result": …}`.
///
/// The definition's output schema, the answer's `_meta` and the shape the handler
/// answers in all come from this one statement, so a tool cannot declare a list
/// and answer an object.
pub(super) const LISTS: bool = false;

/// The arguments `get_job` takes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GetJobArgs {
    /// The Job's id, when the caller has one.
    #[serde(default, deserialize_with = "lax_optional_integer")]
    job_id: Option<i64>,
    /// The Repo's `owner/name`, for a lookup by name.
    #[serde(default)]
    repo: Option<String>,
    /// The Job's name, for a lookup by name.
    #[serde(default)]
    name: Option<String>,
    /// The Job's Pipeline, when the name is ambiguous.
    #[serde(default)]
    pipeline: Option<String>,
    /// How many executions to answer.
    #[serde(default = "default_limit", deserialize_with = "lax_integer")]
    executions_limit: i64,
}

/// The tool's argument schema.
pub(super) fn input_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "executions_limit": {"default": DEFAULT_LIMIT, "type": "integer"},
            "job_id": nullable("integer"),
            "name": nullable("string"),
            "pipeline": nullable("string"),
            "repo": nullable("string"),
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
        let args: GetJobArgs = parse(arguments)?;
        check_limit(args.executions_limit)?;
        let read_settings = server.read_settings();
        let job_id = if let Some(job_id) = args.job_id {
            job_id
        } else {
            let (Some(repo), Some(name)) = (args.repo.as_deref(), args.name.as_deref()) else {
                return Err(ToolFailure::Tool(
                    "Pass job_id, or repo + name (+ pipeline when ambiguous).".to_owned(),
                ));
            };
            let repo_name = normalize_repo(repo)?;
            let ids =
                reads::find_job_ids(server.pool(), &repo_name, name, args.pipeline.as_deref())
                    .await?;
            only_id(&ids, "job", name, &repo_name, "pipeline")?
        };
        let Some(history) =
            reads::get_job(server.pool(), job_id, &read_settings, args.executions_limit).await?
        else {
            return Err(ToolFailure::Tool(format!("No job with id {job_id}.")));
        };
        ToolAnswer::shaped(
            LISTS,
            &JobDetail {
                job: history.job,
                unexplained_failures: history.unexplained_failures,
                explained_failures: history.explained_failures,
                runners: history.runners,
                executions: history
                    .executions
                    .iter()
                    .map(JobExecutionSummary::from)
                    .collect(),
                score_history: score_points(&history.score_history),
            },
        )
    })
}

/// One Job execution in `get_job`'s shape.
///
/// The ids and the timings only add tokens, so they are left out; the
/// explaining Tests and Jobs are capped at [`EXPLAINED_BY_MAX`].
#[derive(Debug, Serialize)]
struct JobExecutionSummary {
    /// The stored status.
    status: String,
    /// What the execution counts as once Explained failures are attributed.
    outcome: JobOutcome,
    /// The commit the execution ran.
    commit_sha: String,
    /// The branch the commit is on.
    branch: String,
    /// Which attempt of the CI run this was.
    ci_run_attempt: i64,
    /// The CI provider's Job id.
    ci_job_id: String,
    /// The Job's CI url.
    url: String,
    /// The runner's name.
    runner_name: String,
    /// The runner's labels.
    runner_labels: Vec<String>,
    /// When the execution was recorded.
    #[serde(with = "timestamp")]
    created_at: DateTime<Utc>,
    /// The failing Tests that explain it.
    explained_by: Vec<ExplainingTestOut>,
    /// The failed sibling Jobs that explain a Summary Job.
    explained_by_jobs: Vec<ExplainingJobOut>,
}

impl From<&JobExecutionOut> for JobExecutionSummary {
    fn from(execution: &JobExecutionOut) -> Self {
        Self {
            status: execution.status.clone(),
            outcome: execution.outcome,
            commit_sha: execution.commit_sha.clone(),
            branch: execution.branch.clone(),
            ci_run_attempt: execution.ci_run_attempt,
            ci_job_id: execution.ci_job_id.clone(),
            url: execution.url.clone(),
            runner_name: execution.runner_name.clone(),
            runner_labels: execution.runner_labels.clone(),
            created_at: execution.created_at,
            explained_by: execution
                .explained_by
                .iter()
                .take(EXPLAINED_BY_MAX)
                .cloned()
                .collect(),
            explained_by_jobs: execution
                .explained_by_jobs
                .iter()
                .take(EXPLAINED_BY_MAX)
                .cloned()
                .collect(),
        }
    }
}

/// A Job's `get_job` answer.
#[derive(Debug, Serialize)]
struct JobDetail {
    /// The Job itself.
    job: JobOut,
    /// Unexplained failures among the returned executions.
    unexplained_failures: i64,
    /// Explained failures among the returned executions.
    explained_failures: i64,
    /// Per-runner counts over the newest score window.
    runners: Vec<RunnerStatOut>,
    /// The newest executions first, trimmed.
    executions: Vec<JobExecutionSummary>,
    /// The Score history of the last 30 days, oldest first.
    score_history: Vec<ScorePointSummary>,
}
