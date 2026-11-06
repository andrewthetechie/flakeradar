//! What this module promises, asserted from outside it.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::unwrap_used,
    reason = "a test fails loudly when the rule under test is wrong"
)]

use flakeradar_core::{
    model::EntityKind, model::ExecutionStatus, model::FailedJobOutcome, model::JobOutcome,
    model::JobStatus, model::ReportKind, model::ReportStatus, model::Tier, model::Trend,
};

#[test]
fn statuses_keep_their_wire_spelling() {
    // The spellings are the frozen server's wire vocabulary, so each expected value
    // is the contract rather than a copy of what the enum says about itself.
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
