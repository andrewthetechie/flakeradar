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

#[cfg(test)]
mod tests {
    use super::{ExecutionStatus, FailedJobOutcome, JobOutcome, JobStatus, Tier, Trend};

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
