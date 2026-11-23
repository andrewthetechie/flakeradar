//! The request and response DTOs, and the lax readers that validate a body.
//!
//! Each struct is a field-for-field mirror of the JSON this API answers and
//! accepts, because the frontend, the CI actions and Fabro parse it. A field that
//! has no value serializes as `null` rather than being skipped, except where a
//! request may leave it out. Timestamps go through [`timestamp`] so every one
//! follows the timestamp rule.
//!
//! The rules live in four private submodules, each re-exported here so every
//! caller's path resolves unchanged: `request` holds the untrusted request
//! bodies and the lax readers that validate them, `answers` the thirty answer
//! shapes, `wire` the timestamp, boolean and integer wire readers, and
//! `refusal` the refusal vocabulary a 422 answer carries.

// Rust guideline compliant 2026-07-21

mod answers;
mod refusal;
mod request;
mod wire;

#[doc(inline)]
pub use answers::{
    CiRunFailedJobOut, CiRunFailuresOut, CiRunTestFailureOut, DeletedCountsOut, ExecutionOut,
    ExplainingJobOut, ExplainingTestOut, HistoryOut, IngestAccepted, JobExecutionOut,
    JobHistoryOut, JobOut, JobPage, JobSummaryOut, LocationOut, ProjectDeleteOut, ProjectOut,
    QuarantineItem, ReadinessOut, RepoDeleteOut, RepoOut, ReportCountsOut, ReportOut,
    ReportSummaryOut, RunnerStatOut, ScorePointOut, SummaryOut, TestJobLinkOut, TestOut, TestPage,
};
#[doc(inline)]
pub use refusal::{
    AttemptIn, BodyRefusal, FinishIssues, Issues, LocSegment, ValidationIssue,
    ci_run_attempt_too_large, read_ci_run_attempt,
};
#[doc(inline)]
pub use request::{
    ATTEMPT_MAX, AcknowledgeIn, COMMIT_SHA_MAX, IssueLinkIn, JOB_NAME_MAX, JobResultIn,
    MAX_ISSUE_NUMBER, NAME_MAX, PATH_MAX, PIPELINE_MAX, PROJECT_MAX, PROVIDER_MAX,
    PipelineReportIn, QuarantineIn, URL_MAX, pipeline_body,
};
#[doc(inline)]
pub use wire::{lax_bool, lax_int, optional_timestamp, timestamp};
