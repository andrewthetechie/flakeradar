//! The issue bodies FlakeRadar files, and the comment it leaves on a fixed one.
//!
//! A body is a heading, a bullet list, a paragraph, the execution table and, for a
//! Test, a sample failure, and both bodies put those blocks in the same order. Every
//! literal here is the sentence the reference writes, byte for byte: the bodies are
//! what an engineer or an agent acts on, so none of them is paraphrased.

// Rust guideline compliant 2026-07-21

use flakeradar_core::{EntityKind, JobExecutionOut, JobHistoryOut, repo_path};
use flakeradar_db::filing::{JobRow, RecentExecution, TestRow};

use crate::settings::Settings;

/// How many recent Executions an issue body lists.
pub const BODY_EXECUTIONS: usize = 10;

/// How many run commands a Job body lists.
pub const BODY_RUNNERS: usize = 3;

/// How many characters of a sample failure an issue body carries.
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
