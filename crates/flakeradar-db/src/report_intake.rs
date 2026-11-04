//! Report intake (ADR 0002, accept-then-process): the pending Report row.
//!
//! Free functions over the entities, in the shape of the frozen server's
//! `report_intake.py`: this module owns how a pending Report row looks per kind
//! (body and its digest) and the JUnit duplicate identity, reads a stored body
//! back per kind — including the rule that a NULL `line_base` reads as 1 — and
//! holds the queue transitions a processor applies to a row. Nothing here
//! inserts, flushes or commits: the caller owns the transaction.

// Rust guideline compliant 2026-07-21

use flakeradar_core::{PipelineReportIn, body_digest};
use sea_orm::entity::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult as _, JsonValue, Set, Statement, Value};

use crate::entities::reports;
use crate::error::DbError;

/// A Report that has been queued and not processed yet.
pub const REPORT_PENDING: &str = "pending";

/// A Report the processor turned into a Run.
pub const REPORT_PROCESSED: &str = "processed";

/// A Report whose processing failed for good.
pub const REPORT_FAILED: &str = "failed";

/// Every status a Report can hold, in the order the frozen server listed them.
pub const REPORT_STATUSES: [&str; 3] = [REPORT_PENDING, REPORT_PROCESSED, REPORT_FAILED];

/// A Report holding one uploaded JUnit XML file.
pub const REPORT_KIND_JUNIT: &str = "junit";

/// A Report holding one attempt of one Pipeline run.
pub const REPORT_KIND_PIPELINE: &str = "pipeline";

/// The JUnit duplicate identity: a CI retry whose response was lost uploads the
/// same bytes under the same attribution, so it must not queue a second Run.
///
/// A stored row matches when every column here is equal, with NULLs matching
/// NULLs for the attribution the upload did not send, and with a stored NULL
/// `line_base` read as 1. Two parallel identical uploads can both insert: there
/// is no unique constraint behind this, and the check is best effort.
const DUPLICATE_SQL: &str = "
    SELECT * FROM reports
     WHERE kind = $1
       AND project_id = $2
       AND commit_sha = $3
       AND ci_run_id = $4
       AND ci_run_attempt IS NOT DISTINCT FROM $5
       AND ci_job_id IS NOT DISTINCT FROM $6
       AND COALESCE(line_base, 1) = $7
       AND body_sha256 IS NOT DISTINCT FROM $8
     ORDER BY id DESC
     LIMIT 1
";

/// An un-added pending JUnit Report, carrying the raw XML and its digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJunitReport {
    /// The Project the upload named, or the Repo's `default` Project.
    pub project_id: i32,
    /// The Repo the Project belongs to.
    pub repo_id: i32,
    /// The uploaded bytes.
    pub body: Vec<u8>,
    /// The commit the Report covers.
    pub commit_sha: String,
    /// The branch the commit is on; `main` when the upload named none.
    pub branch: String,
    /// The CI provider's run id, `""` when the upload named none.
    pub ci_run_id: String,
    /// The Project root the upload named, when it named one.
    pub root: Option<String>,
    /// The base the runner used for line numbers: `1` or `0`.
    pub line_base: Option<i32>,
    /// The CI provider's Job id, when the upload named one.
    pub ci_job_id: Option<String>,
    /// Which attempt of the CI run this is, when the upload named one.
    pub ci_run_attempt: Option<i32>,
    /// The Pipeline the Job belongs to, when the upload named one.
    pub pipeline: Option<String>,
    /// The Repo's Default branch, when the upload named one.
    pub default_branch: Option<String>,
}

impl NewJunitReport {
    /// The SHA-256 digest of the uploaded bytes.
    ///
    /// The duplicate lookup and the stored row both read it from here, so they
    /// cannot disagree.
    #[must_use]
    pub fn body_sha256(&self) -> String {
        body_digest(&self.body)
    }

    /// The row to insert, pending and created at `now`.
    #[must_use]
    pub fn into_active_model(self, now: DateTimeWithTimeZone) -> reports::ActiveModel {
        let body_sha256 = self.body_sha256();
        reports::ActiveModel {
            kind: Set(REPORT_KIND_JUNIT.to_owned()),
            project_id: Set(Some(self.project_id)),
            repo_id: Set(self.repo_id),
            body: Set(self.body),
            body_sha256: Set(Some(body_sha256)),
            commit_sha: Set(self.commit_sha),
            branch: Set(self.branch),
            ci_run_id: Set(self.ci_run_id),
            root: Set(self.root),
            line_base: Set(self.line_base),
            ci_job_id: Set(self.ci_job_id),
            ci_run_attempt: Set(self.ci_run_attempt),
            pipeline: Set(self.pipeline),
            default_branch: Set(self.default_branch),
            status: Set(REPORT_PENDING.to_owned()),
            created_at: Set(now),
            retry_count: Set(0),
            ..Default::default()
        }
    }
}

/// An un-added pending Pipeline Report: no Project (a Pipeline run is not tied
/// to one) and no body digest (only JUnit rows are duplicate-checked).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPipelineReport {
    /// The Repo the Pipeline run belongs to.
    pub repo_id: i32,
    /// The stored body: the validated payload, canonically serialized.
    pub body: Vec<u8>,
    /// The commit the attempt ran.
    pub commit_sha: String,
    /// The branch the commit is on.
    pub branch: String,
    /// The CI provider's run id, `""` when the reporter sent none.
    pub ci_run_id: String,
    /// Which attempt of the CI run this is, numbered from 1.
    pub ci_run_attempt: i32,
    /// The Pipeline's name.
    pub pipeline: Option<String>,
    /// The Repo's Default branch, when the reporter knew it.
    pub default_branch: Option<String>,
}

impl NewPipelineReport {
    /// The row to insert, pending and created at `now`.
    #[must_use]
    pub fn into_active_model(self, now: DateTimeWithTimeZone) -> reports::ActiveModel {
        reports::ActiveModel {
            kind: Set(REPORT_KIND_PIPELINE.to_owned()),
            project_id: Set(None),
            repo_id: Set(self.repo_id),
            body: Set(self.body),
            body_sha256: Set(None),
            commit_sha: Set(self.commit_sha),
            branch: Set(self.branch),
            ci_run_id: Set(self.ci_run_id),
            ci_run_attempt: Set(Some(self.ci_run_attempt)),
            pipeline: Set(self.pipeline),
            default_branch: Set(self.default_branch),
            status: Set(REPORT_PENDING.to_owned()),
            created_at: Set(now),
            retry_count: Set(0),
            ..Default::default()
        }
    }
}

/// A CI attempt number as the column holds it.
///
/// A validated attempt is at least 1 and realistically small. A value past
/// `i32::MAX` saturates rather than failing the upload: the column is what
/// limits it, and losing a whole report over an absurd attempt number helps
/// nobody.
#[must_use]
pub fn stored_attempt(ci_run_attempt: i64) -> i32 {
    i32::try_from(ci_run_attempt).unwrap_or(i32::MAX)
}

/// The stored JUnit Report with the same identity as `candidate`, newest first.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the lookup fails or a row cannot be read.
pub async fn find_junit_duplicate(
    db: &impl ConnectionTrait,
    candidate: &NewJunitReport,
) -> Result<Option<reports::Model>, DbError> {
    let optional_int = |value: Option<i32>| value.map_or(Value::Int(None), Value::from);
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        DUPLICATE_SQL,
        [
            Value::from(REPORT_KIND_JUNIT),
            Value::from(candidate.project_id),
            Value::from(candidate.commit_sha.clone()),
            Value::from(candidate.ci_run_id.clone()),
            optional_int(candidate.ci_run_attempt),
            candidate
                .ci_job_id
                .clone()
                .map_or(Value::String(None), Value::from),
            optional_int(candidate.line_base),
            Value::from(candidate.body_sha256()),
        ],
    );
    match db.query_one_raw(statement).await? {
        Some(row) => Ok(Some(reports::Model::from_query_result(&row, "")?)),
        None => Ok(None),
    }
}

/// The stored body of a JUnit Report and the `line_base` it was parsed with.
///
/// A stored NULL `line_base` predates the parameter and reads as 1.
#[must_use]
pub fn stored_junit_body(report: &reports::Model) -> (&[u8], i32) {
    (&report.body, report.line_base.unwrap_or(1))
}

/// The stored body of a Pipeline Report read back as its payload: the inverse of
/// [`flakeradar_core::pipeline_body`].
///
/// # Errors
///
/// Returns [`DbError::StoredPayload`] when the stored body is not a valid
/// Pipeline payload, which means the row was not written by this server.
pub fn stored_pipeline_payload(report: &reports::Model) -> Result<PipelineReportIn, DbError> {
    serde_json::from_slice(&report.body).map_err(|error| DbError::StoredPayload {
        detail: error.to_string(),
    })
}

/// The complete transition: processed, with its counts, its Run link and no
/// error. Field-only; the caller owns the transaction.
pub fn complete_report(
    report: &mut reports::Model,
    now: DateTimeWithTimeZone,
    run_id: Option<i32>,
    counts: JsonValue,
) {
    REPORT_PROCESSED.clone_into(&mut report.status);
    report.counts = Some(counts);
    report.run_id = run_id;
    report.error = None;
    report.processed_at = Some(now);
}

/// The permanent-failure transition. A permanent failure spends no retry budget,
/// so `retry_count` is untouched. Field-only; the caller owns the transaction.
pub fn fail_report(report: &mut reports::Model, now: DateTimeWithTimeZone, error: &str) {
    REPORT_FAILED.clone_into(&mut report.status);
    report.error = Some(error.to_owned());
    report.processed_at = Some(now);
}

/// The requeue transition: spend one automatic retry and put the Report back in
/// the queue. Only the transient path uses it. Field-only; the caller owns the
/// transaction.
pub fn requeue_report(report: &mut reports::Model, error: Option<&str>) {
    report.retry_count += 1;
    REPORT_PENDING.clone_into(&mut report.status);
    report.error = error.map(str::to_owned);
    report.processed_at = None;
}

/// The manual-retry transition: a human puts a failed Report back in the queue
/// with a fresh automatic-retry budget. Field-only; the caller owns the
/// transaction.
pub fn retry_failed_report(report: &mut reports::Model) {
    report.retry_count = 0;
    REPORT_PENDING.clone_into(&mut report.status);
    report.error = None;
    report.processed_at = None;
}
