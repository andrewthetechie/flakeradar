//! The small enums the DTOs and the scoring rules share.
//!
//! Each variant's wire spelling is the string the frozen Python server stored
//! and served, so a value never changes form between the database, the JSON
//! responses and the MCP tools.

// Rust guideline compliant 2026-07-21

use std::fmt;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// One Execution's outcome.
///
/// `Error` is a failure the runner reported separately from an assertion
/// failure; scoring treats both as failing.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "lowercase")]
#[schema(rename_all = "lowercase")]
pub enum ExecutionStatus {
    /// The Execution passed.
    Passed,
    /// The Execution failed an assertion or an expectation.
    Failed,
    /// The Execution raised an error rather than failing an assertion.
    Error,
    /// The Execution was skipped and says nothing about flakiness.
    Skipped,
}

impl ExecutionStatus {
    /// Whether this status counts as failing for flakiness scoring.
    #[must_use]
    pub fn is_failing(self) -> bool {
        matches!(self, Self::Failed | Self::Error)
    }

    /// The status as the Python server spelled it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Error => "error",
            Self::Skipped => "skipped",
        }
    }

    /// Parses the stored spelling, or `None` when it is not one of the four.
    #[must_use]
    pub fn from_wire(text: &str) -> Option<Self> {
        match text {
            "passed" => Some(Self::Passed),
            "failed" => Some(Self::Failed),
            "error" => Some(Self::Error),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }
}

impl fmt::Display for ExecutionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A Test's or Job's flakiness tier, from its score against the Flake threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(rename_all = "lowercase")]
pub enum Tier {
    /// At or above the Flake threshold.
    Flaky,
    /// Above zero but below the threshold.
    Suspect,
    /// Exactly zero.
    Stable,
}

impl Tier {
    /// The tier as the Python server spelled it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flaky => "flaky",
            Self::Suspect => "suspect",
            Self::Stable => "stable",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The direction a score moved over the last [`TREND_DAYS`](crate::TREND_DAYS) days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(rename_all = "lowercase")]
pub enum Trend {
    /// The score rose by at least the trend delta.
    Worsening,
    /// The score fell by at least the trend delta.
    Improving,
    /// The score stayed within the trend delta.
    Steady,
}

impl Trend {
    /// The trend as the Python server spelled it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Worsening => "worsening",
            Self::Improving => "improving",
            Self::Steady => "steady",
        }
    }
}

impl fmt::Display for Trend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A Job execution's reported status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(rename_all = "lowercase")]
pub enum JobStatus {
    /// The Job passed.
    Passed,
    /// The Job failed.
    Failed,
    /// The Job was skipped or cancelled.
    Skipped,
}

impl JobStatus {
    /// Every spelling a Pipeline report may use, in Pydantic's order.
    pub const ALL: [Self; 3] = [Self::Passed, Self::Failed, Self::Skipped];

    /// The status as the Python server spelled it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    /// The status one of those spellings names.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.as_str() == text)
    }
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a Job execution counts as once Explained failures are attributed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(rename_all = "lowercase")]
pub enum JobOutcome {
    /// The Job passed.
    Passed,
    /// The Job failed with nothing explaining it: it counts against the Job.
    Failed,
    /// The Job failed and a failing Test or sibling Job explains it.
    Explained,
    /// The Job was skipped or cancelled.
    Skipped,
}

/// Whether a failed Job execution counts against the Job's Flakiness score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(rename_all = "lowercase")]
pub enum FailedJobOutcome {
    /// A failing Test execution from the same Job attempt explains it.
    Explained,
    /// Nothing explains it, so it counts against the Job.
    Unexplained,
}

/// Which of the two scored entities something is about.
///
/// A Test is scored per Project; a Job belongs to a Repo through its Pipeline.
/// The wire spells both lowercase, which is also the key a Notification's JSON
/// payload uses for the entity object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntityKind {
    /// A Test.
    Test,
    /// A Job.
    Job,
}

impl EntityKind {
    /// Both entities, in the order the sync and the leaderboard list them.
    pub const ALL: [Self; 2] = [Self::Test, Self::Job];

    /// The entity as the frozen server spelled it: `test` or `job`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Job => "job",
        }
    }

    /// The entity's name in a sentence: `Test` or `Job`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Test => "Test",
            Self::Job => "Job",
        }
    }
}

impl fmt::Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which of the two uploads a Report holds.
///
/// A JUnit Report carries one JUnit XML file and becomes a Run; a Pipeline
/// Report carries one attempt of a Pipeline run and becomes Job executions.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "lowercase")]
#[schema(rename_all = "lowercase")]
pub enum ReportKind {
    /// One uploaded JUnit XML file.
    Junit,
    /// One attempt of one Pipeline run.
    Pipeline,
}

impl ReportKind {
    /// Both kinds, in the order the frozen server listed them.
    pub const ALL: [Self; 2] = [Self::Junit, Self::Pipeline];

    /// The kind as the frozen server stored it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Junit => "junit",
            Self::Pipeline => "pipeline",
        }
    }

    /// The kind one stored spelling names, or `None` when nothing does.
    #[must_use]
    pub fn from_wire(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

impl fmt::Display for ReportKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A Report's place in the ingest queue.
///
/// A Report is queued [`Pending`](Self::Pending) and leaves the queue as
/// [`Processed`](Self::Processed) or [`Failed`](Self::Failed). A transient
/// failure requeues it, so a Report that failed once is `Pending` again while a
/// retry is still allowed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "lowercase")]
#[schema(rename_all = "lowercase")]
pub enum ReportStatus {
    /// Queued and not processed yet.
    Pending,
    /// The processor turned it into a Run or Job executions.
    Processed,
    /// Processing failed for good.
    Failed,
}

impl ReportStatus {
    /// Every status a stored Report may hold, in the order the queue filter lists them.
    pub const ALL: [Self; 3] = [Self::Pending, Self::Processed, Self::Failed];

    /// The status as the frozen server stored it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Processed => "processed",
            Self::Failed => "failed",
        }
    }

    /// The status one stored spelling names, or `None` when nothing does.
    #[must_use]
    pub fn from_wire(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.as_str() == text)
    }
}

impl fmt::Display for ReportStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EntityKind, ExecutionStatus, FailedJobOutcome, JobOutcome, JobStatus, ReportKind,
        ReportStatus, Tier, Trend,
    };

    #[test]
    fn statuses_keep_their_wire_spelling() {
        assert_eq!(ExecutionStatus::Passed.as_str(), "passed");
        assert_eq!(ExecutionStatus::Failed.as_str(), "failed");
        assert_eq!(ExecutionStatus::Error.as_str(), "error");
        assert_eq!(ExecutionStatus::Skipped.as_str(), "skipped");
        assert_eq!(ExecutionStatus::Error.to_string(), "error");
        assert_eq!(
            ExecutionStatus::from_wire("failed"),
            Some(ExecutionStatus::Failed)
        );
        assert_eq!(ExecutionStatus::from_wire("done"), None);
    }

    #[test]
    fn only_failed_and_error_count_as_failing() {
        assert!(ExecutionStatus::Failed.is_failing());
        assert!(ExecutionStatus::Error.is_failing());
        assert!(!ExecutionStatus::Passed.is_failing());
        assert!(!ExecutionStatus::Skipped.is_failing());
    }

    #[test]
    fn every_status_and_tier_prints_its_wire_spelling() {
        assert_eq!(
            ExecutionStatus::from_wire("passed"),
            Some(ExecutionStatus::Passed)
        );
        assert_eq!(
            ExecutionStatus::from_wire("error"),
            Some(ExecutionStatus::Error)
        );
        assert_eq!(
            ExecutionStatus::from_wire("skipped"),
            Some(ExecutionStatus::Skipped)
        );
        assert_eq!(Tier::Flaky.to_string(), "flaky");
        assert_eq!(Tier::Suspect.to_string(), "suspect");
        assert_eq!(Trend::Worsening.to_string(), "worsening");
        assert_eq!(Trend::Steady.to_string(), "steady");
        assert_eq!(JobStatus::Passed.to_string(), "passed");
        assert_eq!(JobStatus::Skipped.to_string(), "skipped");
    }

    #[test]
    fn an_entity_kind_names_the_entity_two_ways() {
        assert_eq!(EntityKind::ALL.map(EntityKind::as_str), ["test", "job"]);
        assert_eq!(EntityKind::ALL.map(EntityKind::name), ["Test", "Job"]);
        assert_eq!(EntityKind::Job.to_string(), "job");
    }

    #[test]
    fn a_report_kind_is_one_of_the_two_stored_spellings() {
        assert_eq!(
            ReportKind::ALL.map(ReportKind::as_str),
            ["junit", "pipeline"]
        );
        assert_eq!(ReportKind::Pipeline.to_string(), "pipeline");
        assert_eq!(ReportKind::from_wire("junit"), Some(ReportKind::Junit));
        assert_eq!(
            ReportKind::from_wire("pipeline"),
            Some(ReportKind::Pipeline)
        );
        assert_eq!(ReportKind::from_wire("xml"), None);
        assert_eq!(
            serde_json::to_string(&ReportKind::Junit).unwrap(),
            "\"junit\""
        );
    }

    #[test]
    fn a_report_status_is_one_of_the_three_stored_spellings() {
        assert_eq!(
            ReportStatus::ALL.map(ReportStatus::as_str),
            ["pending", "processed", "failed"]
        );
        assert_eq!(ReportStatus::Failed.to_string(), "failed");
        assert_eq!(
            ReportStatus::from_wire("pending"),
            Some(ReportStatus::Pending)
        );
        assert_eq!(
            ReportStatus::from_wire("processed"),
            Some(ReportStatus::Processed)
        );
        assert_eq!(
            ReportStatus::from_wire("failed"),
            Some(ReportStatus::Failed)
        );
        assert_eq!(ReportStatus::from_wire("retrying"), None);
        assert_eq!(
            serde_json::to_string(&ReportStatus::Processed).unwrap(),
            "\"processed\""
        );
    }

    #[test]
    fn enums_serialize_to_the_python_strings() {
        assert_eq!(serde_json::to_string(&Tier::Flaky).unwrap(), "\"flaky\"");
        assert_eq!(
            serde_json::to_string(&Tier::Suspect).unwrap(),
            "\"suspect\""
        );
        assert_eq!(serde_json::to_string(&Tier::Stable).unwrap(), "\"stable\"");
        assert_eq!(Tier::Stable.to_string(), "stable");
        assert_eq!(
            serde_json::to_string(&Trend::Worsening).unwrap(),
            "\"worsening\""
        );
        assert_eq!(Trend::Improving.to_string(), "improving");
        assert_eq!(serde_json::to_string(&Trend::Steady).unwrap(), "\"steady\"");
        assert_eq!(
            serde_json::to_string(&JobStatus::Skipped).unwrap(),
            "\"skipped\""
        );
        assert_eq!(JobStatus::Failed.to_string(), "failed");
        assert_eq!(
            serde_json::to_string(&JobOutcome::Explained).unwrap(),
            "\"explained\""
        );
        assert_eq!(
            serde_json::to_string(&FailedJobOutcome::Unexplained).unwrap(),
            "\"unexplained\""
        );
    }
}
