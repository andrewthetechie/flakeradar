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
//! - `github_issue_min_failures`: failures in the newest `score_window`
//!   Executions of any branch (for a Job, unexplained failures only, because
//!   explained failures are the Tests').
//! - `github_fixed_streak`, when it is set: a Clean streak *below* the fixed-note
//!   threshold. A row at that streak has no live flake to file.
//!
//! Deduplication works because a filed issue's number is stored on the Test or
//! Job, so an issue is only re-filed after GitHub reports it closed (see
//! [`sync_closed_issues`]) — unless it was closed as `not_planned`, which keeps
//! the number and flags the row, so nothing is filed again until a person
//! unlinks it.
//!
//! **Error policy: this module never raises.** Every answer is classified once,
//! next to the seam ([`Answer::classify`]), into one [`Verdict`], and one table
//! ([`action`]) states what each operation does with each verdict. A GitHub outage
//! therefore costs filings rather than Report processing:
//!
//! - No token configured: a silent no-op.
//! - A rate limit ([`Verdict::RateLimited`]) ends every batch.
//! - A call that nothing answers ([`Verdict::Unreachable`]) ends filing, the sync
//!   and the fixed note; the post-delete close costs that one issue and goes on.
//! - A 401, 403, 404 or 410 costs the Repo for the rest of a filing, noting or sync
//!   batch, apart from the three cells the table spells out: a 404 in the sync is
//!   the issue itself being gone, so the link re-arms; a 404 or 410 in the
//!   post-delete close is the issue already being gone; and bad credentials (401)
//!   are a per-item failure on the note paths, not a Repo-wide one.
//! - Any other answer costs that one row, and the batch goes on.
//! - After an admin delete, any failure but a rate limit costs that one issue.
//! - A row deleted between a filing POST and the filer's commit costs that
//!   filing: the new issue is commented on and closed best effort.
//! - A store that fails after a filing POST is not a deleted row: the issue stays
//!   open, the failure is logged with its Repo and number, and the batch stops.
//! - An unreadable body is logged and the row is stamped, so an unusable link
//!   keeps rotating instead of pinning the front of the queue.
//!
//! **No database statement is open across a GitHub call.** The sync reads its
//! rotation list, then calls GitHub for one row, then writes that one row in its
//! own short statement, so a slow or hanging GitHub can never hold a row lock a
//! Report or a route needs. That write applies only while the row still holds the
//! number that was checked, so a link replaced during the `GET` survives it.
//!
//! **Stopping: every batch respects the leader's stop signal.** A sync's `GET`,
//! a fixed note's comment and its close end as soon as the signal fires, so a
//! leader that drops costs at most one request. The `POST` that creates an issue
//! is the exception: it runs on through storing its link, and the batch stops
//! after it, because a POST cancelled between creating the issue and storing its
//! number strands an issue and files a duplicate on the next Report.
//!
//! Nothing here ever logs the token: the credential lives in [`Secret`] and the
//! request carries it in a header only. `reqwest` errors have their URL removed
//! before they reach a log line, for the reason the Webhook sender does.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeSet;
use std::fmt;
use std::ops::ControlFlow;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use flakeradar_core::{JobExecutionOut, JobHistoryOut, MAX_ISSUE_NUMBER, ReportStatus, repo_path};
use flakeradar_db::filing::{self, FilingGate, JobRow, NoteCandidate, RecentExecution, TestRow};
use flakeradar_db::issue_links::{self, LinkTarget, NoteRowState};
use flakeradar_db::{DbError, ProcessOutcome, get_job};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, Method};
use sea_orm::{ConnectionTrait, TransactionSession, TransactionTrait};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::settings::{Secret, Settings};
use crate::worker::until_stopped;

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

/// The issue-filing gate the configuration states.
#[must_use]
pub fn filing_gate(settings: &Settings) -> FilingGate {
    let fixed_streak = (settings.github_fixed_streak > 0).then_some(settings.github_fixed_streak);
    FilingGate::new(
        settings.github_issue_min_score,
        settings.github_issue_min_proven_flakes,
        settings.github_issue_min_failures,
        fixed_streak,
    )
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
    /// Classifies this answer into the one verdict every operation reads.
    ///
    /// `success` is the status this call's success carries: a `201` for a create or
    /// a comment, a `200` for a read or a close. Any other 2xx is
    /// [`Verdict::Other`], which each operation treats as a failure of its own.
    ///
    /// The rate limit is decided first, by [`Answer::rate_limited`], so a batch
    /// stops on exactly the answers it stops on elsewhere.
    #[must_use]
    pub fn classify(&self, success: Success) -> Verdict {
        if self.rate_limited() {
            return Verdict::RateLimited;
        }
        match self.status {
            401 => Verdict::Unauthorized,
            403 => Verdict::RepoDenied,
            404 => Verdict::NotFound,
            410 => Verdict::Gone,
            status if (200..300).contains(&status) => {
                if status == success.status() {
                    Verdict::Accepted
                } else {
                    Verdict::Other
                }
            }
            _ => Verdict::Other,
        }
    }

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

/// The status one call's success carries, and nothing else.
///
/// GitHub answers `201` to a create or a comment and `200` to a read or a close; a
/// 2xx that is not the one the call asks for is a failure the caller reads itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Success {
    /// A create or a comment: `201`.
    Created,
    /// A read or a close: `200`.
    Fetched,
}

impl Success {
    /// The status code this success carries.
    #[must_use]
    fn status(self) -> u16 {
        match self {
            Self::Created => 201,
            Self::Fetched => 200,
        }
    }
}

/// What GitHub said, as one verdict every operation reads the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The 2xx the call asked for.
    Accepted,
    /// GitHub is throttling: a `429`, or a `403` that rides a rate limit.
    RateLimited,
    /// `401`: bad credentials.
    Unauthorized,
    /// A `403` that is not a rate limit: permission, or issues disabled.
    RepoDenied,
    /// `404`.
    NotFound,
    /// `410`.
    Gone,
    /// Any other answer.
    Other,
    /// No answer at all.
    ///
    /// [`Answer::classify`] never returns this: a caller turns a
    /// [`TransportError`] into it, so a batch's policy covers it too.
    Unreachable,
}

/// One of the four GitHub operations a verdict is judged for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Creating an issue for a Test or a Job.
    File,
    /// Reading one linked issue's state in the sync.
    Sync,
    /// Commenting that a fix appears to hold.
    Note,
    /// Closing the issue that comment landed on.
    CloseNote,
    /// Commenting on and closing the issues an admin delete stranded.
    CloseDeleted,
}

/// What one operation does when GitHub answers with one verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Act on the answer: file the issue, or read the issue's state.
    Use,
    /// The issue itself is gone: clear the link and re-arm filing.
    Rearm,
    /// Cost this row only, and go on with the next.
    SkipRow,
    /// Skip this row's Repo for the rest of the batch.
    SkipRepo,
    /// End the batch.
    Stop,
}

/// Why a batch ended before its last row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchStop {
    /// GitHub is throttling, which ends every batch.
    RateLimited,
    /// Nothing answered, so the rest of the batch is not worth trying.
    Unreachable,
    /// The leader lost leadership, so the batch gave up.
    Stopped,
    /// A filed issue's number could not be stored, so the batch ended with it open.
    StoreFailed,
}

/// What one batch did, as a value a caller can assert on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchReport {
    /// The entity ids the batch stored a link for.
    pub linked: BTreeSet<i32>,
    /// The Repos the batch skipped for the rest of its run.
    pub skipped: BTreeSet<String>,
    /// Why the batch ended before its last row, when it did.
    pub stopped: Option<BatchStop>,
}

impl BatchReport {
    /// Logs this batch's summary once, at the edge that owns it.
    fn log(&self, operation: Operation) {
        tracing::info!(
            operation = ?operation,
            linked = self.linked.len(),
            skipped_repos = self.skipped.len(),
            stopped = ?self.stopped,
            "GitHub batch finished"
        );
    }
}

/// What every operation does with every verdict: the module's one status policy.
///
/// A row is an operation and a column a verdict, and every cell is what the
/// reference does with that pair:
///
/// | verdict | `File` | `Sync` | `Note` | `CloseNote` | `CloseDeleted` |
/// |---|---|---|---|---|---|
/// | `Accepted` | use | use | use | use | use |
/// | `RateLimited` | stop | stop | stop | stop | stop |
/// | `Unauthorized` | skip Repo | skip Repo | skip row | skip row | skip row |
/// | `RepoDenied` | skip Repo | skip Repo | skip Repo | skip Repo | skip row |
/// | `NotFound` | skip Repo | re-arm | skip Repo | skip Repo | skip row |
/// | `Gone` | skip Repo | skip Repo | skip Repo | skip Repo | skip row |
/// | `Other` | skip row | skip row | skip row | skip row | skip row |
/// | `Unreachable` | stop | stop | stop | stop | skip row |
///
/// Three cells are the intended differences, and each is the reference's own:
/// a `404` in the sync is the issue itself being gone, so the link re-arms rather
/// than the Repo being skipped; a `401` is a per-item failure on the note paths,
/// because bad credentials are not Repo-wide there; and the post-delete close has
/// no Repo-skip cell at all, because it works through one Repo's numbers and
/// skipping the Repo would end the batch.
#[must_use]
pub fn action(operation: Operation, verdict: Verdict) -> Action {
    use Action::{Rearm, SkipRepo, SkipRow, Stop, Use};
    use Operation::{CloseDeleted, CloseNote, File, Note, Sync};
    use Verdict::{
        Accepted, Gone, NotFound, Other, RateLimited, RepoDenied, Unauthorized, Unreachable,
    };
    match operation {
        File => match verdict {
            Accepted => Use,
            RateLimited | Unreachable => Stop,
            Unauthorized | RepoDenied | NotFound | Gone => SkipRepo,
            Other => SkipRow,
        },
        Sync => match verdict {
            Accepted => Use,
            NotFound => Rearm,
            RateLimited | Unreachable => Stop,
            Unauthorized | RepoDenied | Gone => SkipRepo,
            Other => SkipRow,
        },
        Note | CloseNote => match verdict {
            Accepted => Use,
            RateLimited | Unreachable => Stop,
            RepoDenied | NotFound | Gone => SkipRepo,
            Unauthorized | Other => SkipRow,
        },
        CloseDeleted => match verdict {
            Accepted => Use,
            RateLimited => Stop,
            Unauthorized | RepoDenied | NotFound | Gone | Other | Unreachable => SkipRow,
        },
    }
}

/// Why GitHub's verdict ends a batch.
fn batch_stop(verdict: Verdict) -> BatchStop {
    if verdict == Verdict::Unreachable {
        BatchStop::Unreachable
    } else {
        BatchStop::RateLimited
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

/// The HTTP client every GitHub request goes through.
///
/// The timeout is what keeps a slow or unreachable GitHub API from holding a
/// batch, and the user agent is what every endpoint expects to be told.
fn github_client() -> anyhow::Result<Client> {
    Ok(Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECONDS))
        .user_agent(format!("flakeradar/{}", env!("CARGO_PKG_VERSION")))
        .build()?)
}

impl Api {
    /// A client over `base`, presenting `token`.
    ///
    /// # Errors
    ///
    /// Returns an error when the client cannot be built, which is the TLS backend
    /// failing to initialize. A client without the timeout would break this
    /// module's error policy, so none is used.
    pub fn new(token: Secret, base: &str) -> anyhow::Result<Self> {
        Self::with_client_build(token, base, github_client)
    }

    /// The same client, with `build` making the HTTP client instead.
    ///
    /// The seam is for the one failure a test cannot stage: only a TLS backend
    /// that cannot initialize fails a build of a healthy [`Client`], so a test
    /// asks `build` for the error and asserts the constructor carries it rather
    /// than answering a client without the timeout.
    ///
    /// # Errors
    ///
    /// Returns `build`'s error, contexted with what could not be built.
    pub fn with_client_build(
        token: Secret,
        base: &str,
        build: impl FnOnce() -> anyhow::Result<Client>,
    ) -> anyhow::Result<Self> {
        let client = build().context("building the GitHub HTTP client")?;
        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_owned(),
            token,
        })
    }

    /// A client against the production REST API.
    ///
    /// # Errors
    ///
    /// Returns an error when the client cannot be built; see [`Api::new`].
    pub fn production(token: Secret) -> anyhow::Result<Self> {
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
    /// The first non-empty Failure details or message among the failing Executions.
    sample: String,
}

/// The newest failing Execution among `recent`, or `None` when none failed.
///
/// The commit is the newest failing one, while the sample is the first failing
/// Execution that has any text, which is what the reference's loop over every
/// failing row leaves behind: a bare `<failure/>` on the newest commit must not
/// hide the traceback of an older one.
fn newest_failure(recent: &[RecentExecution]) -> Option<NewestFailure> {
    let mut newest: Option<&RecentExecution> = None;
    let mut sample: Option<&str> = None;
    for execution in recent
        .iter()
        .filter(|execution| is_failing(&execution.status))
    {
        newest.get_or_insert(execution);
        if sample.is_none() {
            let text = execution
                .details
                .as_deref()
                .filter(|details| !details.is_empty())
                .unwrap_or(&execution.message);
            if !text.is_empty() {
                sample = Some(text);
            }
        }
    }
    let newest = newest?;
    Some(NewestFailure {
        commit_sha: newest.commit_sha.clone(),
        branch: newest.branch.clone(),
        sample: sample.unwrap_or_default().to_owned(),
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
    let permalink = match (&test.file, &last_fail_sha) {
        (Some(file), Some(sha)) => {
            Some(flakeradar_core::permalink(&flakeradar_core::LocationRef {
                repo,
                commit_sha: sha,
                project_root: root,
                file,
                line: test.line.map(i64::from),
            }))
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

/// Files issues for Tests that now meet the filing gate. Never raises.
///
/// The report names the Test ids it linked, so the caller can keep the fixed-note
/// step off a link this same Report created.
pub async fn file_issues_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    test_ids: &[i32],
    settings: &Settings,
    api: &Api,
    stop: &CancellationToken,
) -> BatchReport {
    if !api.configured() || test_ids.is_empty() {
        return BatchReport::default();
    }
    let candidates = match filing::test_candidates(
        db,
        test_ids,
        &filing_gate(settings),
        settings.score_window,
    )
    .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(error = %error, "GitHub issue filing could not read its Test candidates");
            return BatchReport::default();
        }
    };
    if candidates.is_empty() {
        return BatchReport::default();
    }
    let mut batch = Vec::new();
    for candidate in candidates {
        let recent = match filing::recent_executions(db, candidate.test.id, BODY_EXECUTIONS).await {
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
    let report = file_batch(db, batch, "test", settings, api, stop).await;
    report.log(Operation::File);
    report
}

/// Files issues for github-provider Jobs that now meet the filing gate. Never raises.
pub async fn file_job_issues_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    job_ids: &[i32],
    settings: &Settings,
    api: &Api,
    stop: &CancellationToken,
) -> BatchReport {
    if !api.configured() || job_ids.is_empty() {
        return BatchReport::default();
    }
    let candidates = match filing::job_candidates(
        db,
        job_ids,
        &filing_gate(settings),
        settings.score_window,
    )
    .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(error = %error, "GitHub issue filing could not read its Job candidates");
            return BatchReport::default();
        }
    };
    if candidates.is_empty() {
        return BatchReport::default();
    }
    let mut batch = Vec::new();
    for candidate in candidates {
        let history = match get_job(
            db,
            i64::from(candidate.job.id),
            &settings.reads(),
            i64::try_from(BODY_EXECUTIONS).unwrap_or(10),
        )
        .await
        {
            Ok(history) => history,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    repo = %candidate.repo,
                    entity_id = candidate.job.id,
                    "GitHub issue filing could not read a Job's Executions"
                );
                None
            }
        };
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
    let report = file_batch(db, batch, "job", settings, api, stop).await;
    report.log(Operation::File);
    report
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
///
/// A filing `POST` is never interrupted, because its issue would be stranded; the
/// batch stops before the next one once `stop` fires.
async fn file_batch(
    db: &(impl ConnectionTrait + TransactionTrait),
    batch: Vec<FileRequest>,
    entity: &str,
    settings: &Settings,
    api: &Api,
    stop: &CancellationToken,
) -> BatchReport {
    let mut report = BatchReport::default();
    for request in batch {
        if stop.is_cancelled() {
            tracing::info!(entity, "stopping the issue-filing batch: leadership ended");
            report.stopped = Some(BatchStop::Stopped);
            break;
        }
        if report.skipped.contains(&request.repo) {
            continue;
        }
        let answered = api
            .create_issue(
                &request.repo,
                &request.title,
                &request.body,
                &settings.github_issue_label,
            )
            .await;
        let answer = match answered {
            Ok(answer) => answer,
            Err(error) => {
                // Filing never carries on without an answer, which is the table's
                // `File`/`Unreachable` cell.
                tracing::warn!(error = %error, entity, "GitHub unreachable, skipping issue filing");
                report.stopped = Some(batch_stop(Verdict::Unreachable));
                break;
            }
        };
        let verdict = answer.classify(Success::Created);
        match action(Operation::File, verdict) {
            Action::Use => {
                if let ControlFlow::Break(stop) =
                    record_filed(db, &request, &answer, entity, api, &mut report).await
                {
                    report.stopped = Some(stop);
                    break;
                }
            }
            Action::Stop => {
                tracing::warn!(
                    remaining = %answer.remaining_text(),
                    "GitHub rate limit; stopping the filing batch"
                );
                report.stopped = Some(batch_stop(verdict));
                break;
            }
            Action::SkipRepo => {
                tracing::warn!(
                    repo = %request.repo,
                    status = answer.status,
                    body = %answer.short_text(),
                    "GitHub issue creation skipped for this Repo"
                );
                report.skipped.insert(request.repo.clone());
            }
            Action::SkipRow | Action::Rearm => {
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
    report
}

/// What storing one filed issue's number found.
#[derive(Debug)]
enum FiledLink {
    /// The number is stored on the row.
    Stored,
    /// The row was deleted between the filing `POST` and this store.
    RowGone,
    /// The store failed, so the row may still be there and the issue is live.
    StoreFailed(DbError),
}

/// Records one filed issue's outcome, answering where the batch goes next.
///
/// A store that fails is not a deleted row: the issue stays open, and the batch
/// ends. Only a vanished row closes the issue just created.
async fn record_filed(
    db: &(impl ConnectionTrait + TransactionTrait),
    request: &FileRequest,
    answer: &Answer,
    entity: &str,
    api: &Api,
    report: &mut BatchReport,
) -> ControlFlow<BatchStop> {
    let Some(number) = answer.issue_number() else {
        tracing::warn!(
            repo = %request.repo,
            body = %answer.short_text(),
            "GitHub issue creation returned 201 with no issue number"
        );
        return ControlFlow::Continue(());
    };
    match commit_filed_link(db, request, number).await {
        FiledLink::Stored => {
            report.linked.insert(request.entity_id);
            tracing::info!(
                repo = %request.repo,
                number,
                entity,
                entity_id = request.entity_id,
                "filed a GitHub issue"
            );
        }
        FiledLink::RowGone => {
            // The row vanished mid-filing: the number is referenced nowhere, so
            // close the issue just created, best effort.
            let _ = close_issues_after_removal(&request.repo, &[number], api).await;
            tracing::warn!(
                entity,
                entity_id = request.entity_id,
                repo = %request.repo,
                number,
                "row deleted (admin delete) while filing; closed the new issue"
            );
        }
        FiledLink::StoreFailed(error) => {
            // The reference lets this error propagate: the issue just created stays
            // open, and the batch ends here.
            tracing::error!(
                error = %error,
                entity,
                entity_id = request.entity_id,
                repo = %request.repo,
                number,
                "GitHub filing could not store its link; the issue stays open and the \
                 batch stops"
            );
            return ControlFlow::Break(BatchStop::StoreFailed);
        }
    }
    ControlFlow::Continue(())
}

/// Stores the number GitHub just created, reporting what became of the row.
///
/// A savepoint, so a row deleted under us by an admin delete costs only this
/// filing rather than the rest of the batch; a vanished row is an `UPDATE` with
/// no affected row. A failed store is neither: the row may still exist, so the
/// caller must not treat it as gone.
async fn commit_filed_link(
    db: &(impl ConnectionTrait + TransactionTrait),
    request: &FileRequest,
    number: i32,
) -> FiledLink {
    let savepoint = match db.begin().await {
        Ok(savepoint) => savepoint,
        Err(error) => return FiledLink::StoreFailed(DbError::Database(error)),
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
    let affected = match stored {
        Ok(affected) => affected,
        Err(error) => {
            let _ = savepoint.rollback().await;
            return FiledLink::StoreFailed(error);
        }
    };
    if affected == 0 {
        let _ = savepoint.rollback().await;
        return FiledLink::RowGone;
    }
    if let Err(error) = savepoint.commit().await {
        return FiledLink::StoreFailed(DbError::Database(error));
    }
    FiledLink::Stored
}

/// Re-arms deduplication for Tests and Jobs whose GitHub issue has been closed.
///
/// Leader-only maintenance: each stored link is checked with its own `GET`, capped
/// at the configured batch for each target, never-checked first. Never raises.
///
/// No statement is open across a `GET`: the rotation list is read first, and each
/// answered row is then written by its own short statement, which is guarded on the
/// issue number that was checked (see [`issue_links::record_check`]).
pub async fn sync_closed_issues(
    db: &impl ConnectionTrait,
    settings: &Settings,
    api: &Api,
    stop: &CancellationToken,
) -> BatchReport {
    let mut report = BatchReport::default();
    if !api.configured() {
        return report;
    }
    let batch = settings.github_sync_batch;
    let mut rows: Vec<(LinkTarget, issue_links::LinkedIssue)> = Vec::new();
    for target in LinkTarget::ALL {
        match issue_links::linked_for_check(db, target, batch).await {
            Ok(linked) => rows.extend(linked.into_iter().map(|row| (target, row))),
            Err(error) => {
                tracing::warn!(error = %error, "GitHub issue-state sync could not read its rotation list");
                return report;
            }
        }
    }
    let now = Utc::now();
    for (target, row) in rows {
        if report.skipped.contains(&row.repo) {
            // An unreadable Repo this run: rotate it like a checked row.
            record_check(db, target, &row, issue_links::Check::Kept, now).await;
            continue;
        }
        if let ControlFlow::Break(stop_reason) =
            clear_if_closed(db, target, &row, api, &mut report, now, stop).await
        {
            report.stopped = Some(stop_reason);
            break;
        }
    }
    report.log(Operation::Sync);
    report
}

/// Checks one issue and records what the answer meant for its link.
///
/// Stamping stays as the reference's `_clear_if_closed` docstring states: every row
/// that got an answer is stamped, whatever the answer said, while a rate-limit stop
/// or a transport error leaves the row unstamped so it is retried first.
async fn clear_if_closed(
    db: &impl ConnectionTrait,
    target: LinkTarget,
    row: &issue_links::LinkedIssue,
    api: &Api,
    report: &mut BatchReport,
    now: DateTime<Utc>,
    stop: &CancellationToken,
) -> ControlFlow<BatchStop> {
    let Some(answered) = until_stopped(stop, api.get_issue(&row.repo, row.number)).await else {
        tracing::info!("stopping the GitHub issue-state sync: leadership ended");
        return ControlFlow::Break(BatchStop::Stopped);
    };
    let answer = match answered {
        Ok(answer) => answer,
        Err(error) => {
            // No answer at all, so this row goes unstamped and the run stops here.
            tracing::warn!(error = %error, "GitHub unreachable, skipping issue-state sync");
            return ControlFlow::Break(batch_stop(Verdict::Unreachable));
        }
    };
    let verdict = answer.classify(Success::Fetched);
    let checked = match action(Operation::Sync, verdict) {
        Action::Use => sync_decision(&answer, row, target),
        Action::Rearm => {
            tracing::info!(
                repo = %row.repo,
                number = row.number,
                target = target.entity(),
                entity_id = row.entity_id,
                "issue gone; re-arming the link"
            );
            issue_links::Check::Rearmed
        }
        Action::SkipRepo => {
            tracing::warn!(
                repo = %row.repo,
                status = answer.status,
                body = %answer.short_text(),
                "GitHub issue-state sync skipped for this Repo"
            );
            report.skipped.insert(row.repo.clone());
            issue_links::Check::Kept
        }
        Action::SkipRow => issue_links::Check::Kept,
        Action::Stop => {
            tracing::warn!("GitHub rate limit in sync; stopping the batch");
            return ControlFlow::Break(batch_stop(verdict));
        }
    };
    record_check(db, target, row, checked, now).await;
    ControlFlow::Continue(())
}

/// What one answered check means for its link, with the line that says why.
fn sync_decision(
    answer: &Answer,
    row: &issue_links::LinkedIssue,
    target: LinkTarget,
) -> issue_links::Check {
    match answer.issue_state().as_deref() {
        Some("closed") => {
            let reason = answer.state_reason();
            if reason.as_deref() == Some("not_planned") {
                tracing::info!(
                    repo = %row.repo,
                    number = row.number,
                    target = target.entity(),
                    entity_id = row.entity_id,
                    "issue closed as not planned; keeping the link"
                );
                issue_links::Check::Declined
            } else {
                tracing::info!(
                    repo = %row.repo,
                    number = row.number,
                    target = target.entity(),
                    entity_id = row.entity_id,
                    reason = %reason.as_deref().unwrap_or("without a reason"),
                    "issue closed; re-arming the link"
                );
                issue_links::Check::Rearmed
            }
        }
        Some("open") => {
            // Possibly reopened after a close as not planned: filing is live again.
            issue_links::Check::Reopened
        }
        Some(_) => issue_links::Check::Kept,
        None => {
            tracing::warn!(
                repo = %row.repo,
                number = row.number,
                body = %answer.short_text(),
                "GitHub issue-state sync got an unusable 200 body"
            );
            issue_links::Check::Kept
        }
    }
}

/// Writes one row's check, warning when the write cannot land.
async fn record_check(
    db: &impl ConnectionTrait,
    target: LinkTarget,
    row: &issue_links::LinkedIssue,
    check: issue_links::Check,
    at: DateTime<Utc>,
) {
    if let Err(error) =
        issue_links::record_check(db, target, i64::from(row.entity_id), row.number, check, at).await
    {
        tracing::warn!(error = %error, "GitHub issue-state sync could not write its check");
    }
}

/// Closes the issues whose Test or Job rows an admin delete removed, best effort.
///
/// An admin delete takes the tracked row with it, so no later Report re-files or
/// syncs the issue: without this it would stay open forever, pointing at
/// permalinks that now 404. Never raises; a rate limit stops the batch and any
/// other failure costs that one issue.
pub async fn close_issues_after_removal(repo: &str, numbers: &[i32], api: &Api) -> BatchReport {
    let mut report = BatchReport::default();
    if !api.configured() || numbers.is_empty() {
        return report;
    }
    for number in numbers {
        let comment = match api.comment(repo, *number, CLOSE_COMMENT).await {
            Ok(comment) => comment,
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
        let verdict = comment.classify(Success::Created);
        match action(Operation::CloseDeleted, verdict) {
            Action::Use => {}
            Action::Stop => {
                tracing::warn!(
                    remaining = %comment.remaining_text(),
                    "GitHub rate limit; stopping the post-delete close batch"
                );
                report.stopped = Some(batch_stop(verdict));
                break;
            }
            Action::SkipRow | Action::SkipRepo | Action::Rearm => {
                if already_gone(&comment) {
                    tracing::info!(repo = %repo, number, "issue already gone after an admin delete");
                } else {
                    tracing::warn!(
                        repo = %repo,
                        number,
                        status = comment.status,
                        body = %comment.short_text(),
                        "GitHub comment after an admin delete failed"
                    );
                }
                continue;
            }
        }
        let close = match api.close_issue(repo, *number, None).await {
            Ok(close) => close,
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
        let verdict = close.classify(Success::Fetched);
        match action(Operation::CloseDeleted, verdict) {
            Action::Use => {
                tracing::info!(repo = %repo, number, "closed an issue after an admin delete");
            }
            Action::Stop => {
                tracing::warn!(
                    remaining = %close.remaining_text(),
                    repo = %repo,
                    number,
                    "GitHub rate limit; stopping the post-delete close batch"
                );
                report.stopped = Some(batch_stop(verdict));
                break;
            }
            Action::SkipRow | Action::SkipRepo | Action::Rearm => {
                if already_gone(&close) {
                    tracing::info!(repo = %repo, number, "issue already gone after an admin delete");
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
    }
    report.log(Operation::CloseDeleted);
    report
}

/// Whether a post-delete answer is the issue already being gone.
///
/// That is the outcome this batch wanted: it costs that one number and the next is
/// still tried, exactly as a failure other than a rate limit does.
fn already_gone(answer: &Answer) -> bool {
    matches!(answer.status, 404 | 410)
}

/// Comments once on the linked issue of each Test whose fix appears to hold.
///
/// Disabled while `github_fixed_streak` is 0. Never raises.
pub async fn note_fixed_issues_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    test_ids: &[i32],
    settings: &Settings,
    api: &Api,
    stop: &CancellationToken,
) -> BatchReport {
    if !api.configured() || settings.github_fixed_streak <= 0 || test_ids.is_empty() {
        return BatchReport::default();
    }
    match filing::note_test_candidates(db, test_ids, settings.github_fixed_streak).await {
        Ok(candidates) if !candidates.is_empty() => {
            let report = note_batch(db, candidates, "test", settings, api, stop).await;
            report.log(Operation::Note);
            report
        }
        Ok(_) => BatchReport::default(),
        Err(error) => {
            tracing::warn!(error = %error, "GitHub fixed-note could not read its candidates");
            BatchReport::default()
        }
    }
}

/// Comments once on the linked issue of each Job whose fix appears to hold.
pub async fn note_fixed_job_issues_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    job_ids: &[i32],
    settings: &Settings,
    api: &Api,
    stop: &CancellationToken,
) -> BatchReport {
    if !api.configured() || settings.github_fixed_streak <= 0 || job_ids.is_empty() {
        return BatchReport::default();
    }
    match filing::note_job_candidates(db, job_ids, settings.github_fixed_streak).await {
        Ok(candidates) if !candidates.is_empty() => {
            let report = note_batch(db, candidates, "job", settings, api, stop).await;
            report.log(Operation::Note);
            report
        }
        Ok(_) => BatchReport::default(),
        Err(error) => {
            tracing::warn!(error = %error, "GitHub fixed-note could not read its candidates");
            BatchReport::default()
        }
    }
}

/// Notes (and maybe closes) one batch of fixed issues, whichever entity they describe.
///
/// The comment ends when `stop` fires: a comment that never posted stamps
/// nothing, so the row is still noteable on the next Report.
async fn note_batch(
    db: &(impl ConnectionTrait + TransactionTrait),
    rows: Vec<NoteCandidate>,
    entity: &str,
    settings: &Settings,
    api: &Api,
    stop: &CancellationToken,
) -> BatchReport {
    let mut report = BatchReport::default();
    for row in rows {
        if report.skipped.contains(&row.repo) {
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
        let Some(answered) = until_stopped(stop, api.comment(&row.repo, row.number, &body)).await
        else {
            tracing::info!(entity, "stopping the fixed-note batch: leadership ended");
            report.stopped = Some(BatchStop::Stopped);
            break;
        };
        let comment = match answered {
            Ok(comment) => comment,
            Err(error) => {
                tracing::warn!(error = %error, entity, "GitHub unreachable, skipping fixed-note comments");
                report.stopped = Some(batch_stop(Verdict::Unreachable));
                break;
            }
        };
        let verdict = comment.classify(Success::Created);
        match action(Operation::Note, verdict) {
            Action::Use => {
                if !stamp_note(db, &row, entity, settings).await || !settings.github_close_fixed {
                    continue;
                }
                if let ControlFlow::Break(stop_reason) =
                    close_note(&row, entity, api, &mut report.skipped, stop).await
                {
                    report.stopped = Some(stop_reason);
                    break;
                }
            }
            Action::Stop => {
                tracing::warn!(
                    remaining = %comment.remaining_text(),
                    "GitHub rate limit; stopping the fixed-note batch"
                );
                report.stopped = Some(batch_stop(verdict));
                break;
            }
            Action::SkipRepo => {
                tracing::warn!(
                    repo = %row.repo,
                    status = comment.status,
                    body = %comment.short_text(),
                    "GitHub fixed-note skipped for this Repo"
                );
                report.skipped.insert(row.repo.clone());
            }
            Action::SkipRow | Action::Rearm => {
                tracing::warn!(
                    entity,
                    entity_id = row.entity_id,
                    repo = %row.repo,
                    status = comment.status,
                    body = %comment.short_text(),
                    "GitHub fixed-note comment failed"
                );
            }
        }
    }
    report
}

/// Lands the fixed-note stamp for one row, answering whether it landed.
async fn stamp_note(
    db: &impl ConnectionTrait,
    row: &NoteCandidate,
    entity: &str,
    settings: &Settings,
) -> bool {
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
            return false;
        }
    };
    if !stamp.stamped {
        log_note_stamp_failure(stamp.state, entity, row);
        return false;
    }
    tracing::info!(
        repo = %row.repo,
        number = row.number,
        entity,
        entity_id = row.entity_id,
        "noted that the fix appears to hold"
    );
    true
}

/// Logs why a note's stamp did not land: the row vanished or stopped being noteable.
fn log_note_stamp_failure(state: Option<NoteRowState>, entity: &str, row: &NoteCandidate) {
    match state {
        None => tracing::warn!(
            entity,
            entity_id = row.entity_id,
            repo = %row.repo,
            number = row.number,
            "row deleted (admin delete) while noting its issue"
        ),
        Some(state) => tracing::warn!(
            entity,
            entity_id = row.entity_id,
            streak = state.clean_streak,
            number = ?state.issue_number,
            "row stopped being noteable while its comment was posted"
        ),
    }
}

/// Closes the issue a fix was noted on, when the configuration asks for it.
///
/// A stop signal or a rate limit ends the whole batch rather than only this issue.
async fn close_note(
    row: &NoteCandidate,
    entity: &str,
    api: &Api,
    skipped: &mut BTreeSet<String>,
    stop: &CancellationToken,
) -> ControlFlow<BatchStop> {
    let Some(answered) = until_stopped(
        stop,
        api.close_issue(&row.repo, row.number, Some("completed")),
    )
    .await
    else {
        tracing::info!(entity, "stopping the fixed-close batch: leadership ended");
        return ControlFlow::Break(BatchStop::Stopped);
    };
    let close = match answered {
        Ok(close) => close,
        Err(error) => {
            tracing::warn!(error = %error, entity, "GitHub unreachable, skipping the close");
            return ControlFlow::Break(batch_stop(Verdict::Unreachable));
        }
    };
    let verdict = close.classify(Success::Fetched);
    match action(Operation::CloseNote, verdict) {
        Action::Use => {
            tracing::info!(
                repo = %row.repo,
                number = row.number,
                "closed an issue after noting the fix"
            );
            ControlFlow::Continue(())
        }
        Action::Stop => {
            tracing::warn!(
                remaining = %close.remaining_text(),
                repo = %row.repo,
                number = row.number,
                "GitHub rate limit; stopping the fixed-close batch"
            );
            ControlFlow::Break(batch_stop(verdict))
        }
        Action::SkipRepo => {
            tracing::warn!(
                repo = %row.repo,
                status = close.status,
                body = %close.short_text(),
                "GitHub issue close skipped for this Repo"
            );
            skipped.insert(row.repo.clone());
            ControlFlow::Continue(())
        }
        Action::SkipRow | Action::Rearm => {
            tracing::warn!(
                repo = %row.repo,
                number = row.number,
                status = close.status,
                body = %close.short_text(),
                "GitHub close after noting the fix failed"
            );
            ControlFlow::Continue(())
        }
    }
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
    stop: &CancellationToken,
) {
    if outcome.status != ReportStatus::Processed || !api.configured() {
        return;
    }
    let filed = file_issues_for(db, &outcome.touched_test_ids, settings, api, stop).await;
    let filed_jobs = file_job_issues_for(db, &outcome.touched_job_ids, settings, api, stop).await;
    let note_ids: Vec<i32> = outcome
        .touched_test_ids
        .iter()
        .copied()
        .filter(|id| !filed.linked.contains(id))
        .collect();
    let note_job_ids: Vec<i32> = outcome
        .touched_job_ids
        .iter()
        .copied()
        .filter(|id| !filed_jobs.linked.contains(id))
        .collect();
    note_fixed_issues_for(db, &note_ids, settings, api, stop).await;
    note_fixed_job_issues_for(db, &note_job_ids, settings, api, stop).await;
}
