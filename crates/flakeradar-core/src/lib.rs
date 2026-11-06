//! Pure logic and shared DTOs for FlakeRadar.
//!
//! The crate holds the request and response DTOs that the database layer, the
//! HTTP server and the MCP server share, plus the scoring, parsing and
//! classification logic that needs neither a database nor a network. It depends
//! on no ORM, no web framework and no HTTP client, so every rule in it is
//! testable with plain values.

// Rust guideline compliant 2026-07-21

#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "a unit test fails loudly when the rule under test is wrong"
    )
)]

pub mod classify;
pub mod defaults;
pub mod dto;
pub mod helpers;
pub mod identity;
pub mod lax;
pub mod limits;
pub mod model;
pub mod notify;
pub mod parsing;
pub mod py;
pub mod scoring;
pub mod types;

#[doc(inline)]
pub use classify::{FailureCategory, classify_failure, dominant_category};
#[doc(inline)]
pub use dto::{
    ATTEMPT_MAX, AttemptIn, BodyRefusal, CiRunFailedJobOut, CiRunFailuresOut, CiRunTestFailureOut,
    DeletedCountsOut, ExecutionOut, ExplainingJobOut, ExplainingTestOut, HistoryOut,
    IngestAccepted, IssueLinkIn, JobExecutionOut, JobHistoryOut, JobOut, JobPage, JobResultIn,
    JobSummaryOut, LocSegment, LocationOut, MAX_ISSUE_NUMBER, PipelineReportIn, ProjectDeleteOut,
    ProjectOut, QuarantineIn, QuarantineItem, ReadinessOut, RepoDeleteOut, RepoOut,
    ReportCountsOut, ReportOut, ReportSummaryOut, RunnerStatOut, ScorePointOut, SummaryOut,
    TestJobLinkOut, TestOut, TestPage, ValidationIssue, ci_run_attempt_too_large, pipeline_body,
    read_ci_run_attempt,
};
#[doc(inline)]
pub use helpers::{
    LocationRef, TREND_DAYS, TREND_DELTA, body_digest, escape_like, is_broken, issue_url,
    permalink, repo_path, tier_for, trend_for,
};
#[doc(inline)]
pub use identity::{
    DEFAULT_PROJECT, IdentityError, normalize_project, normalize_provider, normalize_repo,
    normalize_root,
};
#[doc(inline)]
pub use model::{ExecutionStatus, FailedJobOutcome, JobOutcome, JobStatus, Tier, Trend};
#[doc(inline)]
pub use notify::{Kind, TierChange, WebhookFormat, parse_urls, render, render_json, render_slack};
#[doc(inline)]
pub use parsing::{
    DETAILS_MAX, MESSAGE_MAX, PARSE_BYTES_MAX, ParseError, ParsedCase, SUITE_DEPTH_MAX,
    fingerprint, parse_junit_xml,
};
#[doc(inline)]
pub use py::{TRUNCATION_MARKER, cap, py_splitlines, round4, strip_wire_whitespace};
#[doc(inline)]
pub use scoring::{
    FULL_CONFIDENCE_SAMPLES, PROVEN_FLAKE_FLOOR, ScoredExecution, branch_scoped_fail_streak,
    branch_scoped_score, branch_scoped_streak, clean_streak, combined_score, count_proven_flakes,
    fail_streak, flip_score,
};
#[doc(inline)]
pub use types::{CiRunId, CiRunIdError, Fingerprint, FingerprintError, Score, ScoreError};
