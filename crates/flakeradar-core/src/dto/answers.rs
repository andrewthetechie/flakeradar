//! The answer shapes the routes serve: one struct per JSON body this API answers
//! with, mirrored field for field because the frontend, the CI actions and Fabro
//! parse it.
//!
//! The root re-exports every name, so `flakeradar_core::dto::ReportOut` resolves
//! exactly as it did while these shapes lived in the root file. A timestamp field
//! goes through [`timestamp`] or [`optional_timestamp`], which the root re-exports
//! from its wire module, so every one follows the timestamp rule.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::classify::FailureCategory;
use crate::dto::{optional_timestamp, timestamp};
use crate::model::{
    ExecutionStatus, FailedJobOutcome, JobOutcome, ReportKind, ReportStatus, Tier, Trend,
};
use crate::types::{CiRunId, Fingerprint, Score};

/// The response to an accepted upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IngestAccepted {
    /// The new Report's id, or the stored duplicate's.
    pub report_id: i64,
    /// The Report's status: pending for a new one, else the stored status.
    pub status: ReportStatus,
    /// Whether the upload repeated a stored Report.
    pub duplicate: bool,
}

/// The response describing one Report.
#[expect(
    clippy::derive_partial_eq_without_eq,
    reason = "the counts payload is JSON, which holds floats, so the report cannot be Eq"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ReportOut {
    /// The Report's id.
    pub id: i64,
    /// `junit` or `pipeline`.
    pub kind: ReportKind,
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The Project's name, or `None` for a Pipeline report.
    pub project: Option<String>,
    /// The commit the Report covers.
    pub commit_sha: String,
    /// The branch the commit is on.
    pub branch: String,
    /// The CI provider's run id, or the empty id an upload gave no run.
    pub ci_run_id: CiRunId,
    /// `pending`, `processed` or `failed`.
    pub status: ReportStatus,
    /// Why processing failed, when it did.
    pub error: Option<String>,
    /// The Executions the Report produced, when it was processed.
    #[schema(value_type = Option<Object>)]
    pub counts: Option<Value>,
    /// The Run the Report became, when it was processed.
    pub run_id: Option<i64>,
    /// Automatic retries spent, capped by the maximum report retries.
    pub retry_count: i64,
    /// When the Report arrived.
    #[serde(with = "timestamp")]
    pub created_at: DateTime<Utc>,
    /// When processing finished, or failed.
    #[serde(with = "optional_timestamp")]
    pub processed_at: Option<DateTime<Utc>>,
}

/// Pending and failed Report counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReportSummaryOut {
    /// Reports waiting to be processed.
    pub pending: i64,
    /// Reports that failed permanently.
    pub failed: i64,
}

/// The processor's readiness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ReadinessOut {
    /// Whether the database answers.
    pub database: bool,
    /// Whether this process holds the processor lock.
    pub processor_leader: bool,
    /// Reports waiting to be processed.
    pub pending_reports: i64,
    /// How long the oldest pending Report has waited, in seconds.
    pub oldest_pending_age_seconds: f64,
}

/// One Project inside a Repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProjectOut {
    /// The Project's name.
    pub name: String,
    /// The Project root, repo-relative; `""` at the Repo root.
    pub root: String,
}

/// One Repo and its Projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RepoOut {
    /// The Repo's `owner/name`.
    pub name: String,
    /// Every Project in the Repo.
    pub projects: Vec<ProjectOut>,
}

/// How many rows a delete removed, counted before the cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeletedCountsOut {
    /// Tests removed.
    pub tests: i64,
    /// Executions removed.
    pub executions: i64,
    /// Jobs removed.
    pub jobs: i64,
    /// Reports removed.
    pub reports: i64,
}

/// The result of deleting a Repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RepoDeleteOut {
    /// The Repo that was deleted.
    pub repo: String,
    /// How much went with it.
    pub deleted: DeletedCountsOut,
    /// Every GitHub issue number the removed rows stored.
    pub github_issue_numbers: Vec<i64>,
}

/// The result of deleting a Project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProjectDeleteOut {
    /// The Project's Repo.
    pub repo: String,
    /// The Project that was deleted.
    pub project: String,
    /// How much went with it.
    pub deleted: DeletedCountsOut,
    /// Every GitHub issue number the removed Tests stored.
    pub github_issue_numbers: Vec<i64>,
}

/// One Test as the leaderboard serves it.
#[expect(
    clippy::struct_excessive_bools,
    reason = "a flat wire DTO: broken, quarantined, new and github_issue_closed are \
              independent answers, and the JSON shape is the contract"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TestOut {
    /// The Test's id.
    pub id: i64,
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The Project's name.
    pub project: String,
    /// The Test's stable identity hash.
    pub fingerprint: Fingerprint,
    /// The suite the runner reported.
    pub suite: String,
    /// The classname the runner reported.
    pub classname: String,
    /// The Test's name within its classname.
    pub name: String,
    /// The file the Test is defined in, when reported.
    pub file: Option<String>,
    /// The line the Test is defined on, when reported.
    pub line: Option<i64>,
    /// The Flakiness score.
    pub flakiness_score: Score,
    /// The tier the score falls in.
    pub tier: Tier,
    /// How many commit SHAs recorded both outcomes.
    pub confirmed_flake_count: i64,
    /// The most common Failure category among recent failures.
    pub failure_category: Option<FailureCategory>,
    /// Non-skipped Executions since the newest failure.
    pub clean_streak: i64,
    /// Consecutive failures, newest first.
    pub fail_streak: i64,
    /// Whether the Fail streak reached the broken threshold.
    pub broken: bool,
    /// The score's direction over the trend window.
    #[serde(default)]
    pub trend: Option<Trend>,
    /// The newest Execution's status.
    pub last_status: ExecutionStatus,
    /// When the Test last ran.
    #[serde(with = "timestamp")]
    pub last_seen_at: DateTime<Utc>,
    /// Whether the Test is quarantined.
    pub quarantined: bool,
    /// When the quarantine started.
    #[serde(with = "optional_timestamp")]
    pub quarantined_at: Option<DateTime<Utc>>,
    /// Why the Test was quarantined.
    pub quarantine_reason: Option<String>,
    /// When the quarantine lapses.
    #[serde(with = "optional_timestamp")]
    pub quarantine_expires_at: Option<DateTime<Utc>>,
    /// When the acknowledgement lapses.
    #[serde(with = "optional_timestamp")]
    pub acknowledged_until: Option<DateTime<Utc>>,
    /// Why a person acknowledged the Test.
    pub acknowledge_reason: Option<String>,
    /// The GitHub issue the Test is linked to.
    pub github_issue_number: Option<i64>,
    /// The linked issue's url.
    pub github_issue_url: Option<String>,
    /// Whether the linked issue was closed as not planned.
    pub github_issue_closed: bool,
    /// When the Test was first seen.
    #[serde(with = "timestamp")]
    pub first_seen_at: DateTime<Utc>,
    /// Whether the Test was first seen within the new-test window.
    pub new: bool,
    /// How many CI runs the Test failed in within the impact window.
    pub impacted_runs: i64,
    /// How many non-default branches the Test's failures broke within the impact window.
    pub impacted_branches: i64,
    /// How many seconds of CI time the Test's failures cost within the impact window.
    pub time_lost_seconds: f64,
}

/// One page of Tests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TestPage {
    /// The Tests on this page.
    pub items: Vec<TestOut>,
    /// How many Tests the filter matched.
    pub total: i64,
    /// The page number, numbered from 1.
    pub page: i64,
    /// How many items a page holds.
    pub page_size: i64,
}

/// The dashboard's headline counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SummaryOut {
    /// How many Tests exist.
    pub total_tests: i64,
    /// How many are at or above the Flake threshold.
    pub flaky_tests: i64,
    /// How many are above zero but below the threshold.
    pub suspect_tests: i64,
    /// How many have at least one Proven flake.
    pub confirmed_flaky_tests: i64,
    /// How many became flaky in the last seven days.
    pub new_flaky_7d: i64,
    /// How many Runs exist.
    pub total_runs: i64,
    /// Executions in the last thirty days.
    pub executions_30d: i64,
    /// Failures in the last thirty days.
    pub failures_30d: i64,
    /// The configured Flake threshold.
    pub flake_threshold: f64,
    /// Failing Executions per Failure category.
    pub category_counts: BTreeMap<FailureCategory, i64>,
}

/// One Execution in a Test's history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ExecutionOut {
    /// The Execution's id.
    pub id: i64,
    /// What the Execution did.
    pub status: ExecutionStatus,
    /// How long it took, in seconds.
    pub duration: f64,
    /// The Failure message.
    pub message: String,
    /// The Failure details.
    pub details: String,
    /// When the Execution was recorded.
    #[serde(with = "timestamp")]
    pub created_at: DateTime<Utc>,
    /// The commit the Execution ran.
    pub commit_sha: String,
    /// The branch the commit is on.
    pub branch: String,
    /// The CI provider's run id.
    pub ci_run_id: CiRunId,
    /// Which retry inside the Run this is, numbered from 0.
    pub attempt: i64,
    /// The Failure category, when the Execution failed.
    pub failure_category: Option<FailureCategory>,
}

/// Where a Test is defined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LocationOut {
    /// The file joined onto the Project root, repo-relative.
    pub path: String,
    /// The line the Test is defined on, when reported.
    pub line: Option<i64>,
    /// A GitHub permalink at the last failing commit, when one exists.
    pub url: Option<String>,
}

/// One Job a Test has run in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TestJobLinkOut {
    /// The Job's id.
    pub job_id: i64,
    /// The Job's Pipeline.
    pub pipeline: String,
    /// The Job's name.
    pub name: String,
}

/// One day of a Test's or Job's Score history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScorePointOut {
    /// The day the snapshot was taken.
    pub day: NaiveDate,
    /// The Flakiness score at the end of that day.
    pub flakiness_score: Score,
    /// The Proven flake count at the end of that day.
    pub confirmed_flake_count: i64,
    /// Executions that day.
    pub executions: i64,
    /// Failures that day.
    pub failures: i64,
}

/// A Test's detail page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HistoryOut {
    /// The Test itself.
    pub test: TestOut,
    /// Where the Test is defined, when the runner reported a file.
    pub location: Option<LocationOut>,
    /// The commit of the newest failing Execution.
    pub last_failing_sha: Option<String>,
    /// The branch of the newest failing Execution.
    pub last_failing_branch: Option<String>,
    /// The newest Executions first.
    pub executions: Vec<ExecutionOut>,
    /// The Jobs whose Executions came from these Runs.
    pub jobs: Vec<TestJobLinkOut>,
    /// The last ninety days, oldest first.
    pub score_history: Vec<ScorePointOut>,
}

/// One Job as the leaderboard serves it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JobOut {
    /// The Job's id.
    pub id: i64,
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The CI provider's label.
    pub provider: String,
    /// The Job's Pipeline.
    pub pipeline: String,
    /// The Job's name.
    pub name: String,
    /// The Flakiness score.
    pub flakiness_score: Score,
    /// The tier the score falls in.
    pub tier: Tier,
    /// How many commit SHAs recorded both outcomes.
    pub confirmed_flake_count: i64,
    /// Non-skipped Executions since the newest failure.
    pub clean_streak: i64,
    /// The score's direction over the trend window.
    #[serde(default)]
    pub trend: Option<Trend>,
    /// The newest Job execution's status, as the column stores it.
    ///
    /// A `JobStatus`, except that the column is a `varchar(16)` with no `CHECK` and
    /// a hand-written row can hold a word the three variants do not name. Serving
    /// the stored word is the behaviour, so the field stays a `String` and only
    /// [`JobOutcome`] is folded — see [`JobStatus::from_stored`](crate::model::JobStatus::from_stored).
    pub last_status: String,
    /// When the Job last ran.
    #[serde(with = "timestamp")]
    pub last_seen_at: DateTime<Utc>,
    /// Whether the Job is a Summary Job.
    pub summary: bool,
    /// When the acknowledgement lapses.
    #[serde(with = "optional_timestamp")]
    pub acknowledged_until: Option<DateTime<Utc>>,
    /// Why a person acknowledged the Job.
    pub acknowledge_reason: Option<String>,
    /// The GitHub issue the Job is linked to.
    pub github_issue_number: Option<i64>,
    /// The linked issue's url, when the Pipeline is on GitHub.
    pub github_issue_url: Option<String>,
    /// Whether the linked issue was closed as not planned.
    pub github_issue_closed: bool,
    /// How many CI runs the Job failed in within the impact window.
    pub impacted_runs: i64,
    /// How many non-default branches the Job's failures broke within the impact window.
    pub impacted_branches: i64,
    /// How many seconds of CI time the Job's failures cost within the impact window.
    pub time_lost_seconds: f64,
}

/// One page of Jobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JobPage {
    /// The Jobs on this page.
    pub items: Vec<JobOut>,
    /// How many Jobs the filter matched.
    pub total: i64,
    /// The page number, numbered from 1.
    pub page: i64,
    /// How many items a page holds.
    pub page_size: i64,
}

/// One Test execution that explains a failed Job execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ExplainingTestOut {
    /// The Test's id.
    pub test_id: i64,
    /// The Test's Project.
    pub project: String,
    /// The Test's classname.
    pub classname: String,
    /// The Test's name.
    pub name: String,
    /// The newest Execution's status in this Run.
    pub status: ExecutionStatus,
}

/// One sibling Job that explains a Summary Job's failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ExplainingJobOut {
    /// The sibling Job's id.
    pub job_id: i64,
    /// The sibling Job's Pipeline.
    pub pipeline: String,
    /// The sibling Job's name.
    pub name: String,
    /// The sibling Execution's CI url.
    pub url: String,
}

/// One Job execution in a Job's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct JobExecutionOut {
    /// The Job execution's id.
    pub id: i64,
    /// The stored status, as the column stores it; see [`JobOut::last_status`] for
    /// why this is not a `JobStatus`.
    pub status: String,
    /// What the execution counts as once Explained failures are attributed.
    pub outcome: JobOutcome,
    /// The commit the execution ran.
    pub commit_sha: String,
    /// The branch the commit is on.
    pub branch: String,
    /// The CI provider's run id.
    pub ci_run_id: CiRunId,
    /// Which attempt of the CI run this was.
    pub ci_run_attempt: i64,
    /// The CI provider's Job id.
    pub ci_job_id: String,
    /// The Job's CI url.
    pub url: String,
    /// The runner's name.
    pub runner_name: String,
    /// The runner's labels.
    pub runner_labels: Vec<String>,
    /// When the Job started.
    #[serde(with = "optional_timestamp")]
    pub started_at: Option<DateTime<Utc>>,
    /// When the Job finished.
    #[serde(with = "optional_timestamp")]
    pub completed_at: Option<DateTime<Utc>>,
    /// When the execution was recorded.
    #[serde(with = "timestamp")]
    pub created_at: DateTime<Utc>,
    /// The failing Tests that explain it; empty otherwise.
    pub explained_by: Vec<ExplainingTestOut>,
    /// The failed sibling Jobs that explain a Summary Job; empty otherwise.
    pub explained_by_jobs: Vec<ExplainingJobOut>,
}

/// One runner (or label set) a Job ran on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RunnerStatOut {
    /// The sorted label set.
    pub labels: Vec<String>,
    /// The runner's name, when it ran more than once.
    pub runner_name: Option<String>,
    /// How many executions.
    pub executions: i64,
    /// How many failures nothing explained.
    pub unexplained_failures: i64,
}

/// A Job's detail page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JobHistoryOut {
    /// The Job itself.
    pub job: JobOut,
    /// Unexplained failures among the returned executions.
    pub unexplained_failures: i64,
    /// Explained failures among the returned executions.
    pub explained_failures: i64,
    /// The newest executions first.
    pub executions: Vec<JobExecutionOut>,
    /// Per-runner counts over the newest score window, worst first.
    pub runners: Vec<RunnerStatOut>,
    /// The last ninety days, oldest first.
    pub score_history: Vec<ScorePointOut>,
}

/// The Jobs dashboard's headline counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JobSummaryOut {
    /// How many Jobs exist.
    pub total_jobs: i64,
    /// How many are at or above the Flake threshold.
    pub flaky_jobs: i64,
    /// How many are above zero but below the threshold.
    pub suspect_jobs: i64,
    /// How many have at least one Proven flake.
    pub confirmed_flaky_jobs: i64,
    /// How many became flaky in the last seven days.
    pub new_flaky_7d: i64,
    /// Job executions in the last thirty days.
    pub job_executions_30d: i64,
    /// Job failures in the last thirty days.
    pub job_failures_30d: i64,
    /// The configured Flake threshold.
    pub flake_threshold: f64,
    /// Every Pipeline that has a Job.
    pub pipelines: Vec<String>,
}

/// One quarantined Test in the quarantine list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct QuarantineItem {
    /// The suite the runner reported.
    pub suite: String,
    /// The classname the runner reported.
    pub classname: String,
    /// The Test's name.
    pub name: String,
    /// The Test's identity hash.
    pub fingerprint: Fingerprint,
    /// The file the Test is defined in.
    pub file: Option<String>,
    /// The line the Test is defined on.
    pub line: Option<i64>,
    /// When the quarantine started.
    #[serde(with = "optional_timestamp")]
    pub quarantined_at: Option<DateTime<Utc>>,
    /// Why the Test was quarantined.
    pub reason: Option<String>,
    /// When the quarantine lapses.
    #[serde(with = "optional_timestamp")]
    pub expires_at: Option<DateTime<Utc>>,
}

/// Report counts for one CI run attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReportCountsOut {
    /// Reports still waiting.
    pub pending: i64,
    /// Reports processed.
    pub processed: i64,
    /// Reports that failed.
    pub failed: i64,
}

/// One failing Test in a CI run attempt.
#[expect(
    clippy::struct_excessive_bools,
    reason = "a flat wire DTO: broken, quarantined, infra_suspect, new and \
              github_issue_closed are independent answers, and the JSON shape is the contract"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CiRunTestFailureOut {
    /// The Test's id.
    pub test_id: i64,
    /// The Test's Project.
    pub project: String,
    /// The suite the runner reported.
    pub suite: String,
    /// The classname the runner reported.
    pub classname: String,
    /// The Test's name.
    pub name: String,
    /// The Run the failure came from.
    pub run_id: i64,
    /// The CI provider's Job id.
    pub ci_job_id: Option<String>,
    /// Whether the Run this failure came from is suspected infrastructure, and so left out of scoring (ADR 0013).
    pub infra_suspect: bool,
    /// The Test's newest status in this Run.
    pub status: ExecutionStatus,
    /// How many failing Executions this Run had.
    pub failed_attempts: i64,
    /// The last failing Execution's category.
    pub failure_category: Option<FailureCategory>,
    /// The last failing Execution's message, first 1024 characters.
    pub message: String,
    /// The tier the score falls in.
    pub tier: Tier,
    /// The Flakiness score.
    pub flakiness_score: Score,
    /// How many commit SHAs recorded both outcomes.
    pub confirmed_flake_count: i64,
    /// Non-skipped Executions since the newest failure.
    pub clean_streak: i64,
    /// Consecutive failures, newest first.
    pub fail_streak: i64,
    /// Whether the Fail streak reached the broken threshold.
    pub broken: bool,
    /// Whether the Test is quarantined.
    pub quarantined: bool,
    /// The GitHub issue the Test is linked to.
    pub github_issue_number: Option<i64>,
    /// The linked issue's url.
    pub github_issue_url: Option<String>,
    /// Whether the linked issue was closed as not planned.
    pub github_issue_closed: bool,
    /// Whether the Test was first seen within the new-test window.
    pub new: bool,
}

/// One failed Job in a CI run attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CiRunFailedJobOut {
    /// The Job's id.
    pub job_id: i64,
    /// The Job's Pipeline.
    pub pipeline: String,
    /// The Job's name.
    pub name: String,
    /// The CI provider's Job id.
    pub ci_job_id: String,
    /// The Job's CI url.
    pub url: String,
    /// Whether something explains the failure.
    pub outcome: FailedJobOutcome,
    /// Whether the Job is a Summary Job.
    pub summary: bool,
    /// The tier the score falls in.
    pub tier: Tier,
    /// The Flakiness score.
    pub flakiness_score: Score,
    /// How many commit SHAs recorded both outcomes.
    pub confirmed_flake_count: i64,
    /// Non-skipped Executions since the newest failure.
    pub clean_streak: i64,
    /// The GitHub issue the Job is linked to.
    pub github_issue_number: Option<i64>,
    /// The linked issue's url, when the Pipeline is on GitHub.
    pub github_issue_url: Option<String>,
    /// Whether the linked issue was closed as not planned.
    pub github_issue_closed: bool,
}

/// Every failure in one CI run attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CiRunFailuresOut {
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The CI provider's run id.
    pub ci_run_id: CiRunId,
    /// Which attempt this is.
    pub ci_run_attempt: i64,
    /// The configured Flake threshold.
    pub flake_threshold: f64,
    /// Report counts for both kinds, this attempt only.
    pub reports: ReportCountsOut,
    /// How many (Test, Run) pairs there were before the cap.
    pub total_failures: i64,
    /// Whether the failure list was capped.
    pub truncated: bool,
    /// The failing Tests.
    pub failures: Vec<CiRunTestFailureOut>,
    /// The failed Jobs.
    pub failed_jobs: Vec<CiRunFailedJobOut>,
}
