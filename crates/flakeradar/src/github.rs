//! GitHub issue automation, filed in each Test's (and Job's) own Repo.
//!
//! When a Test or a Job crosses the filing gate, file an issue in that entity's
//! Repo (`owner/name`) with the evidence. This runs after the processor finishes
//! a Report, never on the upload path, so a slow or unreachable GitHub API never
//! delays CI.
//!
//! The gate needs every configured minimum; `0` disables a signal:
//!
//! - `github_issue_min_score`: the Flakiness score.
//! - `github_issue_min_proven_flakes`: the Proven flake count.
//! - `github_issue_min_failures`: failures in the recent window (for a Job,
//!   unexplained failures only, because explained failures are the Tests').
//! - `github_fixed_streak`, when it is set: a Clean streak *below* the fixed-note
//!   threshold. A row at that streak has no live flake to file.
//!
//! Deduplication works because a filed issue's number is stored on the Test or
//! Job, so an issue is only re-filed after GitHub reports it closed (see
//! [`sync_closed_issues`]) — unless it was closed as `not_planned`, which keeps
//! the number and flags the row, so nothing is filed again until a person
//! unlinks it.
//!
//! **Error policy: this module never raises.** Every failure is logged and the
//! batch moves on, so a GitHub outage costs filings rather than Report processing:
//!
//! - No token configured: a silent no-op.
//! - A rate limit on create: stop this batch and log a warning.
//! - Any other 403, or a 404/410 (issues disabled): skip that Repo for the rest
//!   of the batch.
//! - In the sync, a definitive Repo-wide client error (403, 401, 410) leaves the
//!   link as it is, skips that Repo, and stamps its rows so they rotate; 404
//!   still re-arms.
//! - After an admin delete, any failure but a rate limit costs that one issue.
//! - A row deleted between a filing POST and the filer's commit costs that
//!   filing: the new issue is commented on and closed best effort.
//! - An unreadable body is logged and the row is stamped, so an unusable link
//!   keeps rotating instead of pinning the front of the queue.
//!
//! Nothing here ever logs the token: the credential lives in [`Secret`] and the
//! request carries it in a header only. `reqwest` errors have their URL removed
//! before they reach a log line, for the reason the Webhook sender does.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeSet;
use std::fmt;
use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use flakeradar_core::{JobExecutionOut, JobHistoryOut, MAX_ISSUE_NUMBER, repo_path};
use flakeradar_db::attribution::{int_array, test_window_ids_sql};
use flakeradar_db::issue_links::{self, LinkTarget};
use flakeradar_db::{
    DbError, ProcessOutcome, REPORT_PROCESSED, chunks, get_job, unexplained_failure_counts,
};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, Method};
use sea_orm::sea_query::Value as QueryValue;
use sea_orm::{
    ConnectionTrait, DbBackend, DbErr, QueryResult, Statement, TransactionSession,
    TransactionTrait, TryGetable,
};
use serde_json::{Value, json};

use crate::settings::{Secret, Settings};

/// The GitHub REST base URL production talks to.
pub const API_BASE: &str = "https://api.github.com";

/// The issue API version every request asks for.
pub const API_VERSION: &str = "2022-11-28";

/// The request timeout every call uses.
pub const TIMEOUT_SECONDS: u64 = 15;

/// How many recent Executions an issue body lists.
pub const BODY_EXECUTIONS: usize = 10;

/// How many run commands a Job body lists.
pub const BODY_RUNNERS: usize = 3;

/// How many characters of a sample failure an issue body carries.
pub const SAMPLE_FAILURE_MAX: usize = 1500;

/// How many characters of a response body a log line keeps.
pub const RESPONSE_TEXT_MAX: usize = 300;

/// The comment every issue an admin delete stranded receives.
pub const CLOSE_COMMENT: &str = "FlakeRadar deleted the tracked flaky test or CI job this issue \
was filed for via the admin delete API, so the permalinks in this issue no longer resolve. \
Closing it automatically; if the test or job is uploaded again, FlakeRadar files a fresh issue.";

/// The issue title a filed Test gets.
fn test_title(classname: &str, name: &str) -> String {
    format!("[FlakeRadar] Flaky test: {classname}::{name}")
}

/// The issue title a filed Job gets.
fn job_title(pipeline: &str, name: &str) -> String {
    format!("[FlakeRadar] Flaky CI job: {pipeline} / {name}")
}

/// The best re-run command for a Test, inferred from its file's extension.
///
/// FlakeRadar ingests JUnit XML from any runner, so the framework is never
/// reported directly; a wrong guess would send an agent to run a Python command
/// against a JavaScript, Java or Go test.
#[must_use]
pub fn run_hint(path: Option<&str>, classname: &str, name: &str) -> String {
    let Some(path) = path.filter(|path| !path.is_empty()) else {
        return format!(
            "# Re-run `{classname}::{name}` via your project's test runner (no file path reported)."
        );
    };
    let extension = match path.rsplit_once('.') {
        Some((_, extension)) => extension.to_lowercase(),
        None => String::new(),
    };
    match extension.as_str() {
        "py" => format!("pytest {path}::{name}"),
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" => {
            format!("npx vitest run {path} -t \"{name}\"")
        }
        "go" => format!("go test ./... -run {name}"),
        "java" | "kt" => format!("mvn test -Dtest={classname}#{name}"),
        "rb" => format!("bundle exec rspec {path} -e \"{name}\""),
        _ => format!("# Re-run `{classname}::{name}` via your project's test runner ({path})."),
    }
}

/// The comment that says the fix appears to hold (issue #33).
///
/// `default_branch` is the branch the Clean streak is counted on; when it is
/// unknown the streak spans every branch, so naming one would be a lie and the
/// clause is dropped. With `public_url` set the comment carries a deep link to
/// the entity's own dashboard view.
#[must_use]
pub fn fixed_comment(
    entity: &str,
    id: i64,
    clean_streak: i64,
    score: f64,
    default_branch: Option<&str>,
    public_url: &str,
) -> String {
    let where_clause = default_branch
        .filter(|branch| !branch.is_empty())
        .map(|branch| format!(" on `{branch}`"))
        .unwrap_or_default();
    let body = format!(
        "FlakeRadar: {clean_streak} passing runs{where_clause} since the last failure \
         (score now {score:.2}). The fix appears to hold."
    );
    if public_url.is_empty() {
        return body;
    }
    let base = public_url.trim_end_matches('/');
    let link = if entity == "job" {
        format!("{base}/?view=jobs&job={id}")
    } else {
        format!("{base}/?test={id}")
    };
    format!("{body}\n\n{link}")
}

/// The issue-filing gate, as one value.
#[derive(Debug, Clone, Copy)]
pub struct FilingGate {
    /// The Flakiness score every row must reach.
    pub min_score: f64,
    /// The Proven flake count every row must reach.
    pub min_proven_flakes: i64,
    /// The recent failure count every row must reach; `0` skips the check.
    pub min_failures: i64,
    /// The Clean streak at which a fix holds rather than files; `None` disables it.
    pub fixed_streak: Option<i64>,
}

impl FilingGate {
    /// The gate the configuration states.
    #[must_use]
    pub fn from_settings(settings: &Settings) -> Self {
        let fixed_streak =
            (settings.github_fixed_streak > 0).then_some(settings.github_fixed_streak);
        Self {
            min_score: settings.github_issue_min_score,
            min_proven_flakes: settings.github_issue_min_proven_flakes,
            min_failures: settings.github_issue_min_failures,
            fixed_streak,
        }
    }
}

/// One HTTP answer, whatever its status.
#[derive(Debug, Clone)]
pub struct Answer {
    /// The status code.
    pub status: u16,
    /// The response headers.
    pub headers: HeaderMap,
    /// The body as text.
    pub text: String,
}

/// A request that never reached GitHub.
#[derive(Debug, thiserror::Error)]
#[error("GitHub request failed: {message}")]
pub struct TransportError {
    /// The failure text, with the URL removed so a token-bearing base URL cannot leak.
    pub message: String,
}

impl Answer {
    /// The body parsed as a JSON object, or `None` when it is not one.
    #[must_use]
    pub fn json_object(&self) -> Option<Value> {
        let value: Value = serde_json::from_str(&self.text).ok()?;
        value.is_object().then_some(value)
    }

    /// The number of a created issue, or `None` when the body does not carry one.
    #[must_use]
    pub fn issue_number(&self) -> Option<i32> {
        let number = self.json_object()?.get("number")?.as_i64()?;
        (1..=MAX_ISSUE_NUMBER)
            .contains(&number)
            .then(|| i32::try_from(number).ok())
            .flatten()
    }

    /// The state a fetched issue reports.
    #[must_use]
    pub fn issue_state(&self) -> Option<String> {
        let state = self.json_object()?.get("state")?.as_str()?.to_owned();
        Some(state)
    }

    /// Why a fetched issue changed state.
    #[must_use]
    pub fn state_reason(&self) -> Option<String> {
        let reason = self
            .json_object()?
            .get("state_reason")?
            .as_str()?
            .to_owned();
        Some(reason)
    }

    /// Whether GitHub is throttling: 429, or a 403 that is a rate limit.
    ///
    /// A 403 also means *permission*, and treating those as rate limits would
    /// stop the batch for every other Repo, so a 403 counts only when the
    /// remaining count is zero, a `retry-after` header is present, or the body
    /// says "rate limit".
    #[must_use]
    pub fn rate_limited(&self) -> bool {
        if self.status == 429 {
            return true;
        }
        if self.status != 403 {
            return false;
        }
        let remaining = self
            .headers
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok());
        if remaining == Some("0") || self.headers.contains_key("retry-after") {
            return true;
        }
        let message = self
            .json_object()
            .and_then(|body| body.get("message").cloned())
            .map(|message| message.to_string().to_lowercase())
            .unwrap_or_default();
        message.contains("rate limit")
    }

    /// The body text a log line keeps.
    #[must_use]
    pub fn short_text(&self) -> String {
        self.text.chars().take(RESPONSE_TEXT_MAX).collect()
    }

    /// The remaining rate limit, as a log line prints it.
    #[must_use]
    pub fn remaining_text(&self) -> String {
        self.headers
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("unknown")
            .to_owned()
    }
}

/// The GitHub REST client, with its base URL injectable for a test.
///
/// The credential is a [`Secret`], so a `Debug` line cannot print it, and the
/// token only ever reaches a request header.
#[derive(Clone)]
pub struct Api {
    client: Client,
    base: String,
    token: Secret,
}

impl fmt::Debug for Api {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Api")
            .field("base", &self.base)
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

impl Api {
    /// A client over `base`, presenting `token`.
    #[must_use]
    pub fn new(token: Secret, base: &str) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECONDS))
                .user_agent(format!("flakeradar/{}", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            base: base.trim_end_matches('/').to_owned(),
            token,
        }
    }

    /// A client against the production REST API.
    #[must_use]
    pub fn production(token: Secret) -> Self {
        Self::new(token, API_BASE)
    }

    /// Whether a token is configured at all.
    #[must_use]
    pub fn configured(&self) -> bool {
        !self.token.expose().is_empty()
    }

    /// The headers every request carries.
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static(API_VERSION),
        );
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", self.token.expose())) {
            headers.insert(AUTHORIZATION, value);
        }
        headers
    }

    /// Sends one request and reads its answer.
    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Answer, TransportError> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .headers(self.headers());
        if let Some(body) = body {
            request = request.header(CONTENT_TYPE, "application/json").json(&body);
        }
        let response = request.send().await.map_err(|error| TransportError {
            message: error.without_url().to_string(),
        })?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let text = response.text().await.map_err(|error| TransportError {
            message: error.without_url().to_string(),
        })?;
        Ok(Answer {
            status,
            headers,
            text,
        })
    }

    /// Creates one issue in `repo`.
    async fn create_issue(
        &self,
        repo: &str,
        title: &str,
        body: &str,
        label: &str,
    ) -> Result<Answer, TransportError> {
        self.send(
            Method::POST,
            &format!("/repos/{repo}/issues"),
            Some(json!({"title": title, "body": body, "labels": [label]})),
        )
        .await
    }

    /// Fetches one issue.
    async fn get_issue(&self, repo: &str, number: i32) -> Result<Answer, TransportError> {
        self.send(Method::GET, &format!("/repos/{repo}/issues/{number}"), None)
            .await
    }

    /// Posts one comment on an issue.
    async fn comment(&self, repo: &str, number: i32, body: &str) -> Result<Answer, TransportError> {
        self.send(
            Method::POST,
            &format!("/repos/{repo}/issues/{number}/comments"),
            Some(json!({"body": body})),
        )
        .await
    }

    /// Closes one issue, with an optional state reason.
    async fn close_issue(
        &self,
        repo: &str,
        number: i32,
        reason: Option<&str>,
    ) -> Result<Answer, TransportError> {
        let body = match reason {
            Some(reason) => json!({"state": "closed", "state_reason": reason}),
            None => json!({"state": "closed"}),
        };
        self.send(
            Method::PATCH,
            &format!("/repos/{repo}/issues/{number}"),
            Some(body),
        )
        .await
    }
}

/// Whether a stored status counts as a failure.
fn is_failing(status: &str) -> bool {
    matches!(status, "failed" | "error")
}

/// Reads one column of a raw row.
fn get<T: TryGetable>(row: &QueryResult, column: &str) -> Result<T, DbError> {
    row.try_get("", column)
        .map_err(|error: DbErr| DbError::Database(error))
}

/// Runs one raw statement.
async fn rows(
    db: &impl ConnectionTrait,
    sql: String,
    values: Vec<QueryValue>,
) -> Result<Vec<QueryResult>, DbError> {
    Ok(db
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await?)
}

/// One Test as the issue body reads it.
#[derive(Debug, Clone)]
pub struct TestRow {
    /// The Test's id.
    pub id: i32,
    /// The classname the runner reported.
    pub classname: String,
    /// The Test's name.
    pub name: String,
    /// The suite the runner reported.
    pub suite: String,
    /// The file the Test is defined in.
    pub file: Option<String>,
    /// The line the Test is defined on.
    pub line: Option<i32>,
    /// The Test's stable identity hash.
    pub fingerprint: String,
    /// The Flakiness score.
    pub flakiness_score: f64,
    /// The Proven flake count.
    pub confirmed_flake_count: i32,
}

/// One Job as the issue body reads it.
#[derive(Debug, Clone)]
pub struct JobRow {
    /// The Job's id.
    pub id: i32,
    /// The Job's name within its Pipeline.
    pub name: String,
    /// The Flakiness score.
    pub flakiness_score: f64,
    /// The Proven flake count.
    pub confirmed_flake_count: i32,
}

/// Reads the columns an issue body needs from one Test row.
fn test_row(row: &QueryResult) -> Result<TestRow, DbError> {
    Ok(TestRow {
        id: get(row, "id")?,
        classname: get(row, "classname")?,
        name: get(row, "name")?,
        suite: get(row, "suite")?,
        file: get(row, "file")?,
        line: get(row, "line")?,
        fingerprint: get(row, "fingerprint")?,
        flakiness_score: get(row, "flakiness_score")?,
        confirmed_flake_count: get(row, "confirmed_flake_count")?,
    })
}

/// Reads the columns a Job issue body needs.
fn job_row(row: &QueryResult) -> Result<JobRow, DbError> {
    Ok(JobRow {
        id: get(row, "id")?,
        name: get(row, "name")?,
        flakiness_score: get(row, "flakiness_score")?,
        confirmed_flake_count: get(row, "confirmed_flake_count")?,
    })
}

/// One recent Execution of a Test, with the Run it is in.
#[derive(Debug, Clone)]
pub struct RecentExecution {
    /// The Execution's outcome.
    pub status: String,
    /// The Failure message.
    pub message: String,
    /// The Failure details.
    pub details: Option<String>,
    /// The Run's commit.
    pub commit_sha: String,
    /// The Run's branch.
    pub branch: String,
    /// When the Execution ran.
    pub created_at: DateTime<Utc>,
}

/// The newest Executions of one Test, newest first.
async fn recent_executions(
    db: &impl ConnectionTrait,
    test_id: i32,
) -> Result<Vec<RecentExecution>, DbError> {
    let sql = format!(
        "SELECT e.status, e.message, e.details, e.created_at, r.commit_sha, r.branch \
         FROM test_executions e JOIN test_runs r ON r.id = e.test_run_id \
         WHERE e.test_case_id = $1 ORDER BY e.id DESC LIMIT {BODY_EXECUTIONS}"
    );
    let mut recent = Vec::new();
    for row in rows(db, sql, vec![QueryValue::from(test_id)]).await? {
        let created: DateTime<chrono::FixedOffset> = get(&row, "created_at")?;
        recent.push(RecentExecution {
            status: get(&row, "status")?,
            message: get(&row, "message")?,
            details: get(&row, "details")?,
            commit_sha: get(&row, "commit_sha")?,
            branch: get(&row, "branch")?,
            created_at: created.with_timezone(&Utc),
        });
    }
    Ok(recent)
}

/// The first ten characters of a commit SHA, as Python's slice did.
fn short_sha(sha: &str) -> String {
    sha.chars().take(10).collect()
}

/// The newest failing Execution of a Test, as an issue body reports it.
#[derive(Debug, Clone)]
struct NewestFailure {
    /// The commit the failing Run ran.
    commit_sha: String,
    /// The branch that commit is on.
    branch: String,
    /// The Failure details when there are any, otherwise the Failure message.
    sample: String,
}

/// The newest failing Execution among `recent`, or `None` when none failed.
fn newest_failure(recent: &[RecentExecution]) -> Option<NewestFailure> {
    let execution = recent
        .iter()
        .find(|execution| is_failing(&execution.status))?;
    Some(NewestFailure {
        commit_sha: execution.commit_sha.clone(),
        branch: execution.branch.clone(),
        sample: execution
            .details
            .clone()
            .filter(|details| !details.is_empty())
            .unwrap_or_else(|| execution.message.clone()),
    })
}

/// Renders a self-contained issue body an engineer or agent can act on.
#[must_use]
pub fn test_issue_body(
    test: &TestRow,
    recent: &[RecentExecution],
    repo: &str,
    project: &str,
    root: &str,
    settings: &Settings,
) -> String {
    let mut lines = vec![
        format!(
            "FlakeRadar detected a flaky test: `{}::{}`",
            test.classname, test.name
        ),
        String::new(),
        format!("- **Repo / project:** `{repo}` / `{project}`"),
    ];
    let path = test.file.as_ref().map(|file| repo_path(root, file));
    let location = match (&path, test.line) {
        (Some(path), Some(line)) => format!("`{path}` line {line}"),
        (Some(path), None) => format!("`{path}`"),
        (None, _) => "(not reported by the test runner)".to_owned(),
    };
    lines.push(format!("- **Location:** {location}"));
    let failing = newest_failure(recent);
    let last_fail_sha = failing.as_ref().map(|failure| failure.commit_sha.clone());
    let last_fail_branch = failing.as_ref().map(|failure| failure.branch.clone());
    let sample_failure = failing.map(|failure| failure.sample).unwrap_or_default();
    let permalink = match (&path, &last_fail_sha) {
        (Some(path), Some(sha)) => {
            let mut permalink = format!("https://github.com/{repo}/blob/{sha}/{path}");
            if let Some(line) = test.line
                && line >= 1
            {
                let _ = write!(permalink, "#L{line}");
            }
            Some(permalink)
        }
        _ => None,
    };
    if let Some(permalink) = permalink {
        lines.push(format!(
            "- **Permalink (last failing commit):** {permalink}"
        ));
    }
    let hint = run_hint(path.as_deref(), &test.classname, &test.name);
    lines.push(format!(
        "- **Flakiness score:** {:.2} (filed at ≥ {:.2})",
        test.flakiness_score, settings.github_issue_min_score
    ));
    lines.push(format!(
        "- **Proven flakes (same commit failed *and* passed):** {}",
        test.confirmed_flake_count
    ));
    let suite = if test.suite.is_empty() {
        "(none)"
    } else {
        test.suite.as_str()
    };
    lines.push(format!(
        "- **Suite / classname:** `{suite}` / `{}`",
        test.classname
    ));
    if let Some(sha) = &last_fail_sha {
        let branch = match last_fail_branch.as_deref() {
            Some(branch) if !branch.is_empty() => format!(" on branch `{branch}`"),
            _ => String::new(),
        };
        lines.push(format!("- **Last failing commit:** `{sha}`{branch}"));
    }
    lines.extend([
        String::new(),
        "FlakeRadar flags this test because it reports nondeterministic results (a failure gives way to a"
            .to_owned(),
        "pass on identical code). To reproduce it locally:".to_owned(),
        String::new(),
        "```".to_owned(),
        hint,
        "```".to_owned(),
        String::new(),
        "Then look in the traceback below for a shared resource the test depends on: timing, execution"
            .to_owned(),
        "order, shared/global state, or the network. Re-run the test several times to confirm.".to_owned(),
        String::new(),
        "### Last 10 executions".to_owned(),
        String::new(),
        "| Status | Commit | Branch | When (UTC) |".to_owned(),
        "|---|---|---|---|".to_owned(),
    ]);
    lines.extend(execution_table(recent));
    if !sample_failure.is_empty() {
        let sample: String = sample_failure.chars().take(SAMPLE_FAILURE_MAX).collect();
        lines.extend([
            String::new(),
            "### Sample failure".to_owned(),
            String::new(),
            "```".to_owned(),
            sample,
            "```".to_owned(),
        ]);
    }
    lines.extend([
        String::new(),
        format!("_Fingerprint: `{}`_", test.fingerprint),
    ]);
    lines.join("\n")
}

/// The Test body's table: one row per recent Execution, newest first.
fn execution_table(recent: &[RecentExecution]) -> Vec<String> {
    recent
        .iter()
        .map(|execution| {
            format!(
                "| {} | `{}` | {} | {} |",
                execution.status,
                short_sha(&execution.commit_sha),
                execution.branch,
                execution.created_at.format("%Y-%m-%d %H:%M")
            )
        })
        .collect()
}

/// Renders a self-contained issue body for a flaky CI Job.
#[must_use]
pub fn job_issue_body(
    job: &JobRow,
    pipeline: &str,
    repo: &str,
    history: Option<&JobHistoryOut>,
    settings: &Settings,
) -> String {
    let mut lines = vec![
        format!(
            "FlakeRadar detected a flaky CI job: `{pipeline}` / `{}`",
            job.name
        ),
        String::new(),
        format!("- **Repo:** `{repo}`"),
        format!("- **Pipeline:** `{pipeline}`"),
        format!("- **Job:** `{}`", job.name),
        format!(
            "- **Flakiness score:** {:.2} (filed at ≥ {:.2})",
            job.flakiness_score, settings.github_issue_min_score
        ),
        format!(
            "- **Proven flakes (same commit failed *and* passed):** {}",
            job.confirmed_flake_count
        ),
    ];
    if let Some(history) = history {
        lines.push(format!(
            "- **Unexplained failures:** {} (failures with no failing test in the job)",
            history.unexplained_failures
        ));
        lines.push(format!(
            "- **Explained by tests:** {}",
            history.explained_failures
        ));
        let top_runners: Vec<_> = history
            .runners
            .iter()
            .filter(|runner| runner.unexplained_failures > 0)
            .take(BODY_RUNNERS)
            .collect();
        if !top_runners.is_empty() {
            lines.push("- **Failures by runner:**".to_owned());
            for runner in top_runners {
                let label = runner
                    .runner_name
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| {
                        if runner.labels.iter().any(|label| !label.is_empty()) {
                            runner.labels.join(", ")
                        } else {
                            "(no labels)".to_owned()
                        }
                    });
                lines.push(format!(
                    "  - `{label}`: {} unexplained of {} executions",
                    runner.unexplained_failures, runner.executions
                ));
            }
        }
    }
    lines.extend([
        String::new(),
        "Job scores count only **unexplained** failures — a failure that a failing Test in the same job"
            .to_owned(),
        "explains is not counted. An unexplained failure points at setup, network, runner capacity,"
            .to_owned(),
        "timeouts, or a crash before the tests run. If the failures cluster on one runner, that runner"
            .to_owned(),
        "may be the cause. Open the job and fix the failing step, then re-run.".to_owned(),
        String::new(),
        "### Last 10 executions".to_owned(),
        String::new(),
        "| Outcome | Commit | Branch | Attempt | Runner | Job |".to_owned(),
        "|---|---|---|---|---|---|".to_owned(),
    ]);
    if let Some(history) = history {
        for execution in &history.executions {
            lines.push(execution_line(execution));
        }
    }
    lines.join("\n")
}

/// One row of the Job body's execution table.
fn execution_line(execution: &JobExecutionOut) -> String {
    let link = if execution.url.is_empty() {
        "—".to_owned()
    } else {
        format!("[job]({})", execution.url)
    };
    let runner = if execution.runner_name.is_empty() {
        "—".to_owned()
    } else {
        execution.runner_name.clone()
    };
    format!(
        "| {} | `{}` | {} | {} | {} | {} |",
        outcome_word(execution),
        short_sha(&execution.commit_sha),
        execution.branch,
        execution.ci_run_attempt,
        runner,
        link
    )
}

/// The word the Job body prints for one execution's outcome.
fn outcome_word(execution: &JobExecutionOut) -> &'static str {
    match execution.outcome {
        flakeradar_core::JobOutcome::Explained => "explained",
        flakeradar_core::JobOutcome::Passed => "passed",
        flakeradar_core::JobOutcome::Skipped => "skipped",
        flakeradar_core::JobOutcome::Failed => "failed",
    }
}

/// The failed/error Executions per Test over its newest `window` Executions.
async fn window_failures(
    db: &impl ConnectionTrait,
    test_ids: &[i32],
    window: usize,
) -> Result<std::collections::BTreeMap<i32, i64>, DbError> {
    let mut counts = std::collections::BTreeMap::new();
    for chunk in chunks(test_ids) {
        let sql = format!(
            "SELECT e.test_case_id, e.status FROM test_executions e \
             WHERE e.id IN (SELECT w.id FROM ({}) AS w)",
            test_window_ids_sql()
        );
        let window = i64::try_from(window).unwrap_or(i64::MAX);
        let values = vec![
            int_array(chunk),
            QueryValue::from(window),
            int_array(chunk),
            QueryValue::from(window),
        ];
        for row in rows(db, sql, values).await? {
            let status: String = get(&row, "status")?;
            if is_failing(&status) {
                *counts.entry(get::<i32>(&row, "test_case_id")?).or_insert(0) += 1;
            }
        }
    }
    Ok(counts)
}

/// One Test that meets the filing gate and still has no open issue.
#[derive(Debug, Clone)]
struct TestCandidate {
    test: TestRow,
    repo: String,
    project: String,
    root: String,
}

/// One Job that meets the filing gate and still has no open issue.
#[derive(Debug, Clone)]
struct JobCandidate {
    job: JobRow,
    pipeline: String,
    repo: String,
}

/// The Tests that meet the configured filing gate and still have no open issue.
async fn test_candidates(
    db: &impl ConnectionTrait,
    test_ids: &[i32],
    settings: &Settings,
) -> Result<Vec<TestCandidate>, DbError> {
    let gate = FilingGate::from_settings(settings);
    let mut candidates = Vec::new();
    for chunk in chunks(test_ids) {
        let fixed = if gate.fixed_streak.is_some() {
            " AND t.clean_streak < $4"
        } else {
            ""
        };
        let sql = format!(
            "SELECT t.*, p.name AS project_name, p.root AS root, r.name AS repo_name \
             FROM test_cases t JOIN projects p ON p.id = t.project_id \
             JOIN repos r ON r.id = p.repo_id \
             WHERE t.id = ANY($1::INTEGER[]) AND t.flakiness_score >= $2 \
               AND t.confirmed_flake_count >= $3 AND t.github_issue_number IS NULL{fixed} \
             ORDER BY t.id"
        );
        let mut values = vec![
            int_array(chunk),
            QueryValue::from(gate.min_score),
            QueryValue::from(i32::try_from(gate.min_proven_flakes).unwrap_or(i32::MAX)),
        ];
        if let Some(streak) = gate.fixed_streak {
            values.push(QueryValue::from(i32::try_from(streak).unwrap_or(i32::MAX)));
        }
        for row in rows(db, sql, values).await? {
            candidates.push(TestCandidate {
                test: test_row(&row)?,
                repo: get(&row, "repo_name")?,
                project: get(&row, "project_name")?,
                root: get(&row, "root")?,
            });
        }
    }
    if gate.min_failures > 0 {
        let ids: Vec<i32> = candidates
            .iter()
            .map(|candidate| candidate.test.id)
            .collect();
        let failures = window_failures(
            db,
            &ids,
            usize::try_from(settings.score_window.max(1)).unwrap_or(usize::MAX),
        )
        .await?;
        candidates.retain(|candidate| {
            failures
                .get(&candidate.test.id)
                .is_some_and(|count| *count >= gate.min_failures)
        });
    }
    Ok(candidates)
}

/// The github-provider Jobs that meet the gate and still have no open issue.
async fn job_candidates(
    db: &impl ConnectionTrait,
    job_ids: &[i32],
    settings: &Settings,
) -> Result<Vec<JobCandidate>, DbError> {
    let gate = FilingGate::from_settings(settings);
    let mut candidates = Vec::new();
    for chunk in chunks(job_ids) {
        let fixed = if gate.fixed_streak.is_some() {
            " AND j.clean_streak < $4"
        } else {
            ""
        };
        let sql = format!(
            "SELECT j.*, p.name AS pipeline_name, r.name AS repo_name \
             FROM jobs j JOIN pipelines p ON p.id = j.pipeline_id \
             JOIN repos r ON r.id = p.repo_id \
             WHERE j.id = ANY($1::INTEGER[]) AND p.provider = 'github' \
               AND j.flakiness_score >= $2 AND j.confirmed_flake_count >= $3 \
               AND j.github_issue_number IS NULL{fixed} \
             ORDER BY j.id"
        );
        let mut values = vec![
            int_array(chunk),
            QueryValue::from(gate.min_score),
            QueryValue::from(i32::try_from(gate.min_proven_flakes).unwrap_or(i32::MAX)),
        ];
        if let Some(streak) = gate.fixed_streak {
            values.push(QueryValue::from(i32::try_from(streak).unwrap_or(i32::MAX)));
        }
        for row in rows(db, sql, values).await? {
            candidates.push(JobCandidate {
                job: job_row(&row)?,
                pipeline: get(&row, "pipeline_name")?,
                repo: get(&row, "repo_name")?,
            });
        }
    }
    if gate.min_failures > 0 {
        let ids: Vec<i32> = candidates
            .iter()
            .map(|candidate| candidate.job.id)
            .collect();
        let failures = unexplained_failure_counts(
            db,
            &ids,
            usize::try_from(settings.score_window.max(1)).unwrap_or(usize::MAX),
        )
        .await?;
        candidates.retain(|candidate| {
            failures
                .get(&candidate.job.id)
                .is_some_and(|count| i64::from(*count) >= gate.min_failures)
        });
    }
    Ok(candidates)
}

/// Files issues for Tests that now meet the filing gate. Never raises.
///
/// Returns the Test ids it linked, so the caller can keep the fixed-note step
/// off a link this same Report created.
pub async fn file_issues_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    test_ids: &[i32],
    settings: &Settings,
    api: &Api,
) -> BTreeSet<i32> {
    if !api.configured() || test_ids.is_empty() {
        return BTreeSet::new();
    }
    let candidates = match test_candidates(db, test_ids, settings).await {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(error = %error, "GitHub issue filing could not read its Test candidates");
            return BTreeSet::new();
        }
    };
    if candidates.is_empty() {
        return BTreeSet::new();
    }
    let mut batch = Vec::new();
    for candidate in candidates {
        let recent = match recent_executions(db, candidate.test.id).await {
            Ok(recent) => recent,
            Err(error) => {
                tracing::warn!(error = %error, "GitHub issue filing could not read a Test's Executions");
                Vec::new()
            }
        };
        let title = test_title(&candidate.test.classname, &candidate.test.name);
        let body = test_issue_body(
            &candidate.test,
            &recent,
            &candidate.repo,
            &candidate.project,
            &candidate.root,
            settings,
        );
        batch.push(FileRequest {
            target: LinkTarget::Test,
            entity_id: candidate.test.id,
            repo: candidate.repo,
            title,
            body,
        });
    }
    file_batch(db, batch, "test", settings, api).await
}

/// Files issues for github-provider Jobs that now meet the filing gate. Never raises.
pub async fn file_job_issues_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    job_ids: &[i32],
    settings: &Settings,
    api: &Api,
) -> BTreeSet<i32> {
    if !api.configured() || job_ids.is_empty() {
        return BTreeSet::new();
    }
    let candidates = match job_candidates(db, job_ids, settings).await {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(error = %error, "GitHub issue filing could not read its Job candidates");
            return BTreeSet::new();
        }
    };
    if candidates.is_empty() {
        return BTreeSet::new();
    }
    let mut batch = Vec::new();
    for candidate in candidates {
        let history = get_job(
            db,
            i64::from(candidate.job.id),
            &settings.reads(),
            i64::try_from(BODY_EXECUTIONS).unwrap_or(10),
        )
        .await
        .ok()
        .flatten();
        let title = job_title(&candidate.pipeline, &candidate.job.name);
        let body = job_issue_body(
            &candidate.job,
            &candidate.pipeline,
            &candidate.repo,
            history.as_ref(),
            settings,
        );
        batch.push(FileRequest {
            target: LinkTarget::Job,
            entity_id: candidate.job.id,
            repo: candidate.repo,
            title,
            body,
        });
    }
    file_batch(db, batch, "job", settings, api).await
}

/// One rendered issue to file.
#[derive(Debug, Clone)]
struct FileRequest {
    target: LinkTarget,
    entity_id: i32,
    repo: String,
    title: String,
    body: String,
}

/// Files one batch of rendered issues, whichever entity they describe.
async fn file_batch(
    db: &(impl ConnectionTrait + TransactionTrait),
    batch: Vec<FileRequest>,
    entity: &str,
    settings: &Settings,
    api: &Api,
) -> BTreeSet<i32> {
    let mut linked = BTreeSet::new();
    let mut skipped: BTreeSet<String> = BTreeSet::new();
    for request in batch {
        if skipped.contains(&request.repo) {
            continue;
        }
        let answer = match api
            .create_issue(
                &request.repo,
                &request.title,
                &request.body,
                &settings.github_issue_label,
            )
            .await
        {
            Ok(answer) => answer,
            Err(error) => {
                tracing::warn!(error = %error, entity, "GitHub unreachable, skipping issue filing");
                return linked;
            }
        };
        match answer.status {
            201 => {
                let Some(number) = answer.issue_number() else {
                    tracing::warn!(
                        repo = %request.repo,
                        body = %answer.short_text(),
                        "GitHub issue creation returned 201 with no issue number"
                    );
                    continue;
                };
                if !commit_filed_link(db, &request, number).await {
                    // The row vanished mid-filing: the number is referenced
                    // nowhere, so close the issue just created, best effort.
                    close_issues_for_deleted_repo(&request.repo, &[number], settings, api).await;
                    tracing::warn!(
                        entity,
                        entity_id = request.entity_id,
                        repo = %request.repo,
                        number,
                        "row deleted (admin delete) while filing; closed the new issue"
                    );
                    continue;
                }
                linked.insert(request.entity_id);
                tracing::info!(
                    repo = %request.repo,
                    number,
                    entity,
                    entity_id = request.entity_id,
                    "filed a GitHub issue"
                );
            }
            _ if answer.rate_limited() => {
                tracing::warn!(
                    remaining = %answer.remaining_text(),
                    "GitHub rate limit; stopping the filing batch"
                );
                return linked;
            }
            _ if matches!(answer.status, 403 | 404 | 410) => {
                tracing::warn!(
                    repo = %request.repo,
                    status = answer.status,
                    body = %answer.short_text(),
                    "GitHub issue creation skipped for this Repo"
                );
                skipped.insert(request.repo.clone());
            }
            _ => {
                tracing::warn!(
                    entity,
                    entity_id = request.entity_id,
                    repo = %request.repo,
                    status = answer.status,
                    body = %answer.short_text(),
                    "GitHub issue creation failed"
                );
            }
        }
    }
    linked
}

/// Stores the number GitHub just created, answering whether the row was still there.
///
/// A savepoint, so a row deleted under us by an admin delete costs only this
/// filing rather than the rest of the batch; a vanished row is an `UPDATE` with
/// no affected row.
async fn commit_filed_link(
    db: &(impl ConnectionTrait + TransactionTrait),
    request: &FileRequest,
    number: i32,
) -> bool {
    let savepoint = match db.begin().await {
        Ok(savepoint) => savepoint,
        Err(error) => {
            tracing::warn!(error = %error, "GitHub filing could not open a savepoint");
            return false;
        }
    };
    let stored = issue_links::store_filed_link(
        &savepoint,
        request.target,
        i64::from(request.entity_id),
        number,
    )
    .await;
    // A vanished row is an UPDATE that affected no row: the admin delete won the
    // race, so the number is referenced nowhere and the new issue is stranded.
    let exists = match stored {
        Ok(affected) => affected > 0,
        Err(error) => {
            tracing::warn!(error = %error, "GitHub filing could not store its link");
            false
        }
    };
    if !exists {
        let _ = savepoint.rollback().await;
        return false;
    }
    if let Err(error) = savepoint.commit().await {
        tracing::warn!(error = %error, "GitHub filing could not commit its link");
        return false;
    }
    true
}

/// Re-arms deduplication for Tests and Jobs whose GitHub issue has been closed.
///
/// Leader-only maintenance: each stored link is checked with its own GET, capped
/// at the configured batch for each target, newest-never-checked first. Never
/// raises.
pub async fn sync_closed_issues(
    db: &(impl ConnectionTrait + TransactionTrait),
    settings: &Settings,
    api: &Api,
) {
    if !api.configured() {
        return;
    }
    let batch = settings.github_sync_batch;
    let transaction = match db.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(error = %error, "GitHub issue-state sync could not open a transaction");
            return;
        }
    };
    let mut rows: Vec<(LinkTarget, issue_links::LinkedIssue)> = Vec::new();
    for target in LinkTarget::ALL {
        match issue_links::linked_for_check(&transaction, target, batch).await {
            Ok(linked) => rows.extend(linked.into_iter().map(|row| (target, row))),
            Err(error) => {
                tracing::warn!(error = %error, "GitHub issue-state sync could not read its rotation list");
                return;
            }
        }
    }
    if rows.is_empty() {
        let _ = transaction.rollback().await;
        return;
    }
    let now = Utc::now();
    let mut skipped: BTreeSet<String> = BTreeSet::new();
    for (target, row) in rows {
        if skipped.contains(&row.repo) {
            // An unreadable Repo this run: rotate it like a checked row.
            if let Err(error) =
                issue_links::stamp_checked(&transaction, target, i64::from(row.entity_id), now)
                    .await
            {
                tracing::warn!(error = %error, "GitHub issue-state sync could not stamp a row");
            }
            continue;
        }
        if !clear_if_closed(&transaction, target, &row, settings, api, &mut skipped, now).await {
            break;
        }
    }
    if let Err(error) = transaction.commit().await {
        tracing::warn!(error = %error, "GitHub issue-state sync could not commit");
    }
}

/// Checks one issue and clears its number when it is closed or gone.
///
/// Returns `false` on a rate limit, when the batch must stop. Every row that got
/// an answer is stamped, so an unusable link keeps rotating instead of pinning
/// the front of the queue.
async fn clear_if_closed(
    db: &impl ConnectionTrait,
    target: LinkTarget,
    row: &issue_links::LinkedIssue,
    settings: &Settings,
    api: &Api,
    skipped: &mut BTreeSet<String>,
    now: DateTime<Utc>,
) -> bool {
    let answer = match api.get_issue(&row.repo, row.number).await {
        Ok(answer) => answer,
        Err(error) => {
            tracing::warn!(error = %error, "GitHub unreachable, skipping issue-state sync");
            return true;
        }
    };
    if answer.rate_limited() {
        tracing::warn!("GitHub rate limit in sync; stopping the batch");
        return false;
    }
    if let Err(error) = issue_links::stamp_checked(db, target, i64::from(row.entity_id), now).await
    {
        tracing::warn!(error = %error, "GitHub issue-state sync could not stamp a row");
    }
    if matches!(answer.status, 401 | 403 | 410) {
        tracing::warn!(
            repo = %row.repo,
            status = answer.status,
            body = %answer.short_text(),
            "GitHub issue-state sync skipped for this Repo"
        );
        skipped.insert(row.repo.clone());
        return true;
    }
    if answer.status == 200 {
        let Some(state) = answer.issue_state() else {
            tracing::warn!(
                repo = %row.repo,
                number = row.number,
                body = %answer.short_text(),
                "GitHub issue-state sync got an unusable 200 body"
            );
            return true;
        };
        if state == "closed" {
            let reason = answer.state_reason();
            if reason.as_deref() == Some("not_planned") {
                tracing::info!(
                    repo = %row.repo,
                    number = row.number,
                    target = target.entity(),
                    entity_id = row.entity_id,
                    "issue closed as not planned; keeping the link"
                );
                if let Err(error) =
                    issue_links::set_closed(db, target, i64::from(row.entity_id), true).await
                {
                    tracing::warn!(error = %error, "GitHub issue-state sync could not keep the link");
                }
            } else {
                tracing::info!(
                    repo = %row.repo,
                    number = row.number,
                    target = target.entity(),
                    entity_id = row.entity_id,
                    reason = %reason.as_deref().unwrap_or("without a reason"),
                    "issue closed; re-arming the link"
                );
                if let Err(error) =
                    issue_links::rearm_link(db, target, i64::from(row.entity_id)).await
                {
                    tracing::warn!(error = %error, "GitHub issue-state sync could not re-arm the link");
                }
            }
        } else if state == "open" {
            // Possibly reopened after a close as not planned: filing is live again.
            if let Err(error) =
                issue_links::set_closed(db, target, i64::from(row.entity_id), false).await
            {
                tracing::warn!(error = %error, "GitHub issue-state sync could not clear the flag");
            }
        }
        return true;
    }
    if answer.status == 404 {
        tracing::info!(
            repo = %row.repo,
            number = row.number,
            target = target.entity(),
            entity_id = row.entity_id,
            "issue gone; re-arming the link"
        );
        if let Err(error) = issue_links::rearm_link(db, target, i64::from(row.entity_id)).await {
            tracing::warn!(error = %error, "GitHub issue-state sync could not re-arm the link");
        }
    }
    let _ = settings;
    true
}

/// Closes the issues whose Test or Job rows an admin delete removed, best effort.
///
/// An admin delete takes the tracked row with it, so no later Report re-files or
/// syncs the issue: without this it would stay open forever, pointing at
/// permalinks that now 404. Never raises; a rate limit stops the batch and any
/// other failure costs that one issue.
pub async fn close_issues_for_deleted_repo(
    repo: &str,
    numbers: &[i32],
    settings: &Settings,
    api: &Api,
) {
    if !api.configured() || numbers.is_empty() {
        return;
    }
    let _ = settings;
    for number in numbers {
        let answer = match api.comment(repo, *number, CLOSE_COMMENT).await {
            Ok(answer) => answer,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    repo = %repo,
                    number,
                    "GitHub post-delete close skipped for one issue"
                );
                continue;
            }
        };
        if answer.rate_limited() {
            tracing::warn!(
                remaining = %answer.remaining_text(),
                "GitHub rate limit; stopping the post-delete close batch"
            );
            return;
        }
        if matches!(answer.status, 404 | 410) {
            tracing::info!(repo = %repo, number, "issue already gone after an admin delete");
            continue;
        }
        if answer.status != 201 {
            tracing::warn!(
                repo = %repo,
                number,
                status = answer.status,
                body = %answer.short_text(),
                "GitHub comment after an admin delete failed"
            );
            continue;
        }
        let close = match api.close_issue(repo, *number, None).await {
            Ok(answer) => answer,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    repo = %repo,
                    number,
                    "GitHub post-delete close skipped for one issue"
                );
                continue;
            }
        };
        if close.rate_limited() {
            tracing::warn!(
                remaining = %close.remaining_text(),
                repo = %repo,
                number,
                "GitHub rate limit; stopping the post-delete close batch"
            );
            return;
        }
        if matches!(close.status, 404 | 410) {
            tracing::info!(repo = %repo, number, "issue already gone after an admin delete");
        } else if close.status == 200 {
            tracing::info!(repo = %repo, number, "closed an issue after an admin delete");
        } else {
            tracing::warn!(
                repo = %repo,
                number,
                status = close.status,
                body = %close.short_text(),
                "GitHub close after an admin delete failed"
            );
        }
    }
}

/// One linked row the fixed-note step selected.
#[derive(Debug, Clone)]
struct NoteCandidate {
    target: LinkTarget,
    entity_id: i32,
    repo: String,
    default_branch: Option<String>,
    clean_streak: i32,
    score: f64,
    number: i32,
}

/// The linked Tests at the note threshold whose fix has not been noted yet.
async fn note_test_candidates(
    db: &impl ConnectionTrait,
    test_ids: &[i32],
    settings: &Settings,
) -> Result<Vec<NoteCandidate>, DbError> {
    let mut candidates = Vec::new();
    for chunk in chunks(test_ids) {
        let sql = "SELECT t.id AS id, t.github_issue_number AS number, t.clean_streak, \
                   t.flakiness_score, r.name AS repo_name, r.default_branch \
                   FROM test_cases t JOIN projects p ON p.id = t.project_id \
                   JOIN repos r ON r.id = p.repo_id \
                   WHERE t.id = ANY($1::INTEGER[]) AND t.github_issue_number IS NOT NULL \
                     AND t.github_issue_fixed_noted_at IS NULL AND t.github_issue_closed IS FALSE \
                     AND t.clean_streak >= $2 ORDER BY t.id";
        let threshold = i32::try_from(settings.github_fixed_streak).unwrap_or(i32::MAX);
        for row in rows(
            db,
            sql.to_owned(),
            vec![int_array(chunk), QueryValue::from(threshold)],
        )
        .await?
        {
            candidates.push(NoteCandidate {
                target: LinkTarget::Test,
                entity_id: get(&row, "id")?,
                repo: get(&row, "repo_name")?,
                default_branch: get(&row, "default_branch")?,
                clean_streak: get(&row, "clean_streak")?,
                score: get(&row, "flakiness_score")?,
                number: get(&row, "number")?,
            });
        }
    }
    Ok(candidates)
}

/// The linked github-provider Jobs at the note threshold.
async fn note_job_candidates(
    db: &impl ConnectionTrait,
    job_ids: &[i32],
    settings: &Settings,
) -> Result<Vec<NoteCandidate>, DbError> {
    let mut candidates = Vec::new();
    for chunk in chunks(job_ids) {
        let sql = "SELECT j.id AS id, j.github_issue_number AS number, j.clean_streak, \
                   j.flakiness_score, r.name AS repo_name, r.default_branch \
                   FROM jobs j JOIN pipelines p ON p.id = j.pipeline_id \
                   JOIN repos r ON r.id = p.repo_id \
                   WHERE j.id = ANY($1::INTEGER[]) AND p.provider = 'github' \
                     AND j.github_issue_number IS NOT NULL \
                     AND j.github_issue_fixed_noted_at IS NULL AND j.github_issue_closed IS FALSE \
                     AND j.clean_streak >= $2 ORDER BY j.id";
        let threshold = i32::try_from(settings.github_fixed_streak).unwrap_or(i32::MAX);
        for row in rows(
            db,
            sql.to_owned(),
            vec![int_array(chunk), QueryValue::from(threshold)],
        )
        .await?
        {
            candidates.push(NoteCandidate {
                target: LinkTarget::Job,
                entity_id: get(&row, "id")?,
                repo: get(&row, "repo_name")?,
                default_branch: get(&row, "default_branch")?,
                clean_streak: get(&row, "clean_streak")?,
                score: get(&row, "flakiness_score")?,
                number: get(&row, "number")?,
            });
        }
    }
    Ok(candidates)
}

/// Comments once on the linked issue of each Test whose fix appears to hold.
///
/// Disabled while `github_fixed_streak` is 0. Never raises.
pub async fn note_fixed_issues_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    test_ids: &[i32],
    settings: &Settings,
    api: &Api,
) {
    if !api.configured() || settings.github_fixed_streak <= 0 || test_ids.is_empty() {
        return;
    }
    match note_test_candidates(db, test_ids, settings).await {
        Ok(candidates) if !candidates.is_empty() => {
            note_batch(db, candidates, "test", settings, api).await;
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(error = %error, "GitHub fixed-note could not read its candidates");
        }
    }
}

/// Comments once on the linked issue of each Job whose fix appears to hold.
pub async fn note_fixed_job_issues_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    job_ids: &[i32],
    settings: &Settings,
    api: &Api,
) {
    if !api.configured() || settings.github_fixed_streak <= 0 || job_ids.is_empty() {
        return;
    }
    match note_job_candidates(db, job_ids, settings).await {
        Ok(candidates) if !candidates.is_empty() => {
            note_batch(db, candidates, "job", settings, api).await;
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(error = %error, "GitHub fixed-note could not read its candidates");
        }
    }
}

/// Notes (and maybe closes) one batch of fixed issues, whichever entity they describe.
async fn note_batch(
    db: &(impl ConnectionTrait + TransactionTrait),
    rows: Vec<NoteCandidate>,
    entity: &str,
    settings: &Settings,
    api: &Api,
) {
    let mut skipped: BTreeSet<String> = BTreeSet::new();
    for row in rows {
        if skipped.contains(&row.repo) {
            continue;
        }
        let body = fixed_comment(
            entity,
            i64::from(row.entity_id),
            i64::from(row.clean_streak),
            row.score,
            row.default_branch.as_deref(),
            &settings.public_url,
        );
        let answer = match api.comment(&row.repo, row.number, &body).await {
            Ok(answer) => answer,
            Err(error) => {
                tracing::warn!(error = %error, entity, "GitHub unreachable, skipping fixed-note comments");
                return;
            }
        };
        match answer.status {
            201 => {
                let threshold = i32::try_from(settings.github_fixed_streak).unwrap_or(i32::MAX);
                let stamp = match issue_links::stamp_fixed_note(
                    db,
                    row.target,
                    i64::from(row.entity_id),
                    row.number,
                    threshold,
                    Utc::now(),
                )
                .await
                {
                    Ok(stamp) => stamp,
                    Err(error) => {
                        tracing::warn!(error = %error, "GitHub fixed-note could not stamp its row");
                        continue;
                    }
                };
                if !stamp.stamped {
                    log_note_stamp_failure(stamp.state, entity, &row);
                    continue;
                }
                tracing::info!(
                    repo = %row.repo,
                    number = row.number,
                    entity,
                    entity_id = row.entity_id,
                    "noted that the fix appears to hold"
                );
                if settings.github_close_fixed
                    && close_note(db, &row, entity, settings, api, &mut skipped).await
                {
                    return;
                }
            }
            _ if answer.rate_limited() => {
                tracing::warn!(
                    remaining = %answer.remaining_text(),
                    "GitHub rate limit; stopping the fixed-note batch"
                );
                return;
            }
            _ if matches!(answer.status, 403 | 404 | 410) => {
                tracing::warn!(
                    repo = %row.repo,
                    status = answer.status,
                    body = %answer.short_text(),
                    "GitHub fixed-note skipped for this Repo"
                );
                skipped.insert(row.repo.clone());
            }
            _ => {
                tracing::warn!(
                    entity,
                    entity_id = row.entity_id,
                    repo = %row.repo,
                    status = answer.status,
                    body = %answer.short_text(),
                    "GitHub fixed-note comment failed"
                );
            }
        }
    }
}

/// Logs why a note's stamp did not land: the row vanished or stopped being noteable.
fn log_note_stamp_failure(state: Option<(i32, Option<i32>)>, entity: &str, row: &NoteCandidate) {
    match state {
        None => tracing::warn!(
            entity,
            entity_id = row.entity_id,
            repo = %row.repo,
            number = row.number,
            "row deleted (admin delete) while noting its issue"
        ),
        Some((streak, linked)) => tracing::warn!(
            entity,
            entity_id = row.entity_id,
            streak,
            number = ?linked,
            "row stopped being noteable while its comment was posted"
        ),
    }
}

/// Closes the issue a fix was noted on, when the configuration asks for it.
///
/// Returns `true` when GitHub is throttling, which stops the whole batch rather
/// than only this issue, the same way the close of a deleted Repo's issues does.
async fn close_note(
    db: &impl ConnectionTrait,
    row: &NoteCandidate,
    entity: &str,
    settings: &Settings,
    api: &Api,
    skipped: &mut BTreeSet<String>,
) -> bool {
    let _ = (db, settings);
    let answer = match api
        .close_issue(&row.repo, row.number, Some("completed"))
        .await
    {
        Ok(answer) => answer,
        Err(error) => {
            tracing::warn!(error = %error, entity, "GitHub unreachable, skipping the close");
            return false;
        }
    };
    if answer.rate_limited() {
        tracing::warn!(
            remaining = %answer.remaining_text(),
            repo = %row.repo,
            number = row.number,
            "GitHub rate limit; stopping the fixed-close batch"
        );
        return true;
    }
    if matches!(answer.status, 403 | 404 | 410) {
        tracing::warn!(
            repo = %row.repo,
            status = answer.status,
            body = %answer.short_text(),
            "GitHub issue close skipped for this Repo"
        );
        skipped.insert(row.repo.clone());
    } else if answer.status == 200 {
        tracing::info!(
            repo = %row.repo,
            number = row.number,
            "closed an issue after noting the fix"
        );
    } else {
        tracing::warn!(
            repo = %row.repo,
            number = row.number,
            status = answer.status,
            body = %answer.short_text(),
            "GitHub close after noting the fix failed"
        );
    }
    false
}

/// The `ReportWorker` hook: file issues for the Tests and Jobs a Report touched.
///
/// Also notes the rows whose Clean streak now proves the fix held. A newly filed
/// Issue is never the one commented on: the gate refuses a row already at the
/// note streak, and the ids this pass linked are dropped from the note step too,
/// in case a concurrent Report lifted a fresh link's streak between the two
/// selects.
pub async fn on_report_processed(
    db: &(impl ConnectionTrait + TransactionTrait),
    outcome: &ProcessOutcome,
    settings: &Settings,
    api: &Api,
) {
    if outcome.status != REPORT_PROCESSED || !api.configured() {
        return;
    }
    let filed = file_issues_for(db, &outcome.touched_test_ids, settings, api).await;
    let filed_jobs = file_job_issues_for(db, &outcome.touched_job_ids, settings, api).await;
    let note_ids: Vec<i32> = outcome
        .touched_test_ids
        .iter()
        .copied()
        .filter(|id| !filed.contains(id))
        .collect();
    let note_job_ids: Vec<i32> = outcome
        .touched_job_ids
        .iter()
        .copied()
        .filter(|id| !filed_jobs.contains(id))
        .collect();
    note_fixed_issues_for(db, &note_ids, settings, api).await;
    note_fixed_job_issues_for(db, &note_job_ids, settings, api).await;
}
