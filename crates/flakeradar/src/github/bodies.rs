//! The issue bodies FlakeRadar files, and the comment it leaves on a fixed one.
//!
//! A body is a heading, a bullet list, a paragraph, the execution table and, for a
//! Test, a sample failure, and both bodies put those blocks in the same order. Every
//! literal here is text a person reads, byte for byte: the bodies are what an
//! engineer or an agent acts on, so none of them is paraphrased, and
//! `tests/github/filing.rs` names the lines each body must carry.

// Rust guideline compliant 2026-07-21

use flakeradar_core::{
    dto::JobExecutionOut, dto::JobHistoryOut, helpers::repo_path, model::EntityKind,
};
use flakeradar_db::filing::{JobRow, RecentExecution, TestRow};

use crate::settings::Settings;

/// How many recent Executions an issue body lists.
///
/// The body's table is what a person reads to see the flake they were assigned,
/// so it shows the newest runs and stops: ten covers the flips that made the Test
/// flaky without turning the issue into a log. Each row carries a status, a
/// commit and a message, and GitHub caps an issue body at 65536 characters, which
/// ten rows cannot reach. Raising it lengthens every filed issue; lowering it can
/// hide the flip the body exists to show.
pub const BODY_EXECUTIONS: usize = 10;

/// How many run commands a Job body lists.
///
/// A Job's body names the runners whose outcomes differed, which is how a person
/// tells "only Windows" from "everywhere"; three is the usual spread of a matrix
/// dimension's values, so a longer list is a matrix with more legs than a reader
/// compares at a glance. Raising it grows every filed Job issue; lowering it can
/// hide the leg that is failing.
pub const BODY_RUNNERS: usize = 3;

/// How many characters of a sample failure an issue body carries.
///
/// One traceback's worth, as a code block, is what makes an issue actionable
/// without opening the dashboard. 1500 code points holds an assertion error with
/// its own frames, and the GitHub body limit leaves room for the rest of the
/// body. Raising it spends the body on one traceback; lowering it cuts the frames
/// that name the cause.
pub const SAMPLE_FAILURE_MAX: usize = 1500;

/// The issue title a filed Test gets.
pub(super) fn test_title(classname: &str, name: &str) -> String {
    format!("[FlakeRadar] Flaky test: {classname}::{name}")
}

/// The issue title a filed Job gets.
pub(super) fn job_title(pipeline: &str, name: &str) -> String {
    format!("[FlakeRadar] Flaky CI job: {pipeline} / {name}")
}

/// The best re-run command for a Test, inferred from its file's extension.
///
/// FlakeRadar ingests JUnit XML from any runner, so the framework is never
/// reported directly; a wrong guess would send an agent to run a `pytest` command
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
    entity: EntityKind,
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
    let link = if entity == EntityKind::Job {
        format!("{base}/?view=jobs&job={id}")
    } else {
        format!("{base}/?test={id}")
    };
    format!("{body}\n\n{link}")
}

/// How much of a commit SHA a body's table prints.
///
/// Ten hex characters is GitHub's own short form, so the value in the table is
/// the one `git log --oneline` and GitHub's UI show and a reader can paste
/// straight into a search. A shorter prefix can be ambiguous in a busy
/// repository; a longer one wraps the column.
const SHORT_SHA_CHARS: usize = 10;

/// The first [`SHORT_SHA_CHARS`] of a commit SHA.
fn short_sha(sha: &str) -> String {
    sha.chars().take(SHORT_SHA_CHARS).collect()
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
/// Execution that has any text: a bare `<failure/>` on the newest commit must not
/// hide the traceback of an older one.
fn newest_failure(recent: &[RecentExecution]) -> Option<NewestFailure> {
    let mut newest: Option<&RecentExecution> = None;
    let mut sample: Option<&str> = None;
    for execution in recent
        .iter()
        .filter(|execution| execution.status.is_failing())
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

/// Builds an issue body one block at a time.
///
/// Both bodies are a heading, a bullet list, a paragraph and the execution
/// table, and a Test adds a sample failure. The block order, and the blank line
/// between two blocks, are stated here rather than at each call site.
#[derive(Debug)]
struct Body {
    lines: Vec<String>,
}

impl Body {
    /// A body whose first line is `heading`.
    fn new(heading: String) -> Self {
        Self {
            lines: vec![heading],
        }
    }

    /// One line, as given.
    fn line(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    /// The blank line that separates two blocks.
    fn blank(&mut self) {
        self.line(String::new());
    }

    /// One bullet, with `text` after the marker.
    fn bullet(&mut self, text: impl Into<String>) {
        self.line(format!("- {}", text.into()));
    }

    /// One bullet indented under the bullet above it.
    fn nested_bullet(&mut self, text: impl Into<String>) {
        self.line(format!("  - {}", text.into()));
    }

    /// The score and Proven flake bullets, which both bodies carry.
    fn filing_gate(&mut self, score: f64, proven_flakes: i32, filed_at: f64) {
        self.bullet(format!(
            "**Flakiness score:** {score:.2} (filed at ≥ {filed_at:.2})"
        ));
        self.bullet(format!(
            "**Proven flakes (same commit failed *and* passed):** {proven_flakes}"
        ));
    }

    /// Prose, one line of `lines` per entry.
    fn prose(&mut self, lines: &[&str]) {
        self.lines
            .extend(lines.iter().map(|line| (*line).to_owned()));
    }

    /// A `###` section: the blank line before it, the heading, and one after.
    fn section(&mut self, heading: &str) {
        self.blank();
        self.line(format!("### {heading}"));
        self.blank();
    }

    /// A fenced code block holding `text`.
    fn code(&mut self, text: &str) {
        self.line("```");
        self.line(text);
        self.line("```");
    }

    /// The executions section: its heading, its columns and its rows.
    fn executions(&mut self, columns: &str, rule: &str, rows: impl IntoIterator<Item = String>) {
        self.section(&format!("Last {BODY_EXECUTIONS} executions"));
        self.line(columns);
        self.line(rule);
        self.lines.extend(rows);
    }

    /// The body as GitHub receives it.
    fn render(self) -> String {
        self.lines.join("\n")
    }
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
    let mut body = Body::new(format!(
        "FlakeRadar detected a flaky test: `{}::{}`",
        test.classname, test.name
    ));
    body.blank();
    body.bullet(format!("**Repo / project:** `{repo}` / `{project}`"));
    let path = test.file.as_ref().map(|file| repo_path(root, file));
    body.bullet(format!(
        "**Location:** {}",
        location(path.as_deref(), test.line)
    ));
    let failing = newest_failure(recent);
    if let Some(permalink) = permalink(test, repo, root, failing.as_ref()) {
        body.bullet(format!("**Permalink (last failing commit):** {permalink}"));
    }
    body.filing_gate(
        test.flakiness_score,
        test.confirmed_flake_count,
        settings.github_issue_min_score,
    );
    let suite = if test.suite.is_empty() {
        "(none)"
    } else {
        test.suite.as_str()
    };
    body.bullet(format!(
        "**Suite / classname:** `{suite}` / `{}`",
        test.classname
    ));
    if let Some(failure) = &failing {
        body.bullet(last_failing_commit(failure));
    }
    body.blank();
    body.prose(&[
        "FlakeRadar flags this test because it reports nondeterministic results (a failure gives way to a",
        "pass on identical code). To reproduce it locally:",
    ]);
    body.blank();
    body.code(&run_hint(path.as_deref(), &test.classname, &test.name));
    body.blank();
    body.prose(&[
        "Then look in the traceback below for a shared resource the test depends on: timing, execution",
        "order, shared/global state, or the network. Re-run the test several times to confirm.",
    ]);
    body.executions(
        "| Status | Commit | Branch | When (UTC) |",
        "|---|---|---|---|",
        execution_table(recent),
    );
    if let Some(sample) = failing
        .as_ref()
        .map(|failure| failure.sample.as_str())
        .filter(|sample| !sample.is_empty())
    {
        body.section("Sample failure");
        body.code(&sample.chars().take(SAMPLE_FAILURE_MAX).collect::<String>());
    }
    body.blank();
    body.line(format!("_Fingerprint: `{}`_", test.fingerprint));
    body.render()
}

/// How a body names a Test's Location, or says the runner reported none.
fn location(path: Option<&str>, line: Option<i32>) -> String {
    match (path, line) {
        (Some(path), Some(line)) => format!("`{path}` line {line}"),
        (Some(path), None) => format!("`{path}`"),
        (None, _) => "(not reported by the test runner)".to_owned(),
    }
}

/// The bullet naming the newest failing commit, and its branch when one was
/// reported.
fn last_failing_commit(failure: &NewestFailure) -> String {
    if failure.branch.is_empty() {
        format!("**Last failing commit:** `{}`", failure.commit_sha)
    } else {
        format!(
            "**Last failing commit:** `{}` on branch `{}`",
            failure.commit_sha, failure.branch
        )
    }
}

/// The permalink to the newest failing commit, when the Test reports a file.
fn permalink(
    test: &TestRow,
    repo: &str,
    root: &str,
    failing: Option<&NewestFailure>,
) -> Option<String> {
    let file = test.file.as_ref()?;
    let commit_sha = &failing?.commit_sha;
    Some(flakeradar_core::helpers::permalink(
        &flakeradar_core::helpers::LocationRef {
            repo,
            commit_sha,
            project_root: root,
            file,
            line: test.line.map(i64::from),
        },
    ))
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
    let mut body = Body::new(format!(
        "FlakeRadar detected a flaky CI job: `{pipeline}` / `{}`",
        job.name
    ));
    body.blank();
    body.bullet(format!("**Repo:** `{repo}`"));
    body.bullet(format!("**Pipeline:** `{pipeline}`"));
    body.bullet(format!("**Job:** `{}`", job.name));
    body.filing_gate(
        job.flakiness_score,
        job.confirmed_flake_count,
        settings.github_issue_min_score,
    );
    if let Some(history) = history {
        failure_counts(&mut body, history);
    }
    body.blank();
    body.prose(&[
        "Job scores count only **unexplained** failures — a failure that a failing Test in the same job",
        "explains is not counted. An unexplained failure points at setup, network, runner capacity,",
        "timeouts, or a crash before the tests run. If the failures cluster on one runner, that runner",
        "may be the cause. Open the job and fix the failing step, then re-run.",
    ]);
    let rows: Vec<String> = history
        .map(|history| history.executions.iter().map(execution_line).collect())
        .unwrap_or_default();
    body.executions(
        "| Outcome | Commit | Branch | Attempt | Runner | Job |",
        "|---|---|---|---|---|---|",
        rows,
    );
    body.render()
}

/// The bullets that say what a Job's failures were and which runners they
/// clustered on.
fn failure_counts(body: &mut Body, history: &JobHistoryOut) {
    body.bullet(format!(
        "**Unexplained failures:** {} (failures with no failing test in the job)",
        history.unexplained_failures
    ));
    body.bullet(format!(
        "**Explained by tests:** {}",
        history.explained_failures
    ));
    let top_runners: Vec<_> = history
        .runners
        .iter()
        .filter(|runner| runner.unexplained_failures > 0)
        .take(BODY_RUNNERS)
        .collect();
    if top_runners.is_empty() {
        return;
    }
    body.bullet("**Failures by runner:**");
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
        body.nested_bullet(format!(
            "`{label}`: {} unexplained of {} executions",
            runner.unexplained_failures, runner.executions
        ));
    }
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
        flakeradar_core::model::JobOutcome::Explained => "explained",
        flakeradar_core::model::JobOutcome::Passed => "passed",
        flakeradar_core::model::JobOutcome::Skipped => "skipped",
        flakeradar_core::model::JobOutcome::Failed => "failed",
    }
}
