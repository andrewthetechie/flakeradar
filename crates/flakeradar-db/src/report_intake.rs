//! Report intake (ADR 0002, accept-then-process): the pending Report row.
//!
//! Free functions over the entities, in the shape of the frozen server's
//! `report_intake.py`: this module owns how a pending Report row looks per kind
//! (body and its digest) and the JUnit duplicate identity, reads a stored body
//! back per kind — including the rule that a NULL `line_base` reads as 1 — and
//! holds the queue transitions a processor applies to a row. It also serves the
//! queue reads: one Report with its names, the pending and failed counts, and the
//! newest Reports a page lists. Nothing here inserts, flushes or commits: the
//! caller owns the transaction.

// Rust guideline compliant 2026-07-21

use chrono::Utc;
use flakeradar_core::{
    dto::PipelineReportIn, dto::ReportOut, dto::ReportSummaryOut, helpers::body_digest,
    model::ReportKind, model::ReportStatus,
};
use sea_orm::entity::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult as _, JsonValue, Set, Statement, Value};

use crate::entities::reports;
use crate::error::DbError;
use crate::sql::{Binds, get, one, required_one, rows};

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
            kind: Set(ReportKind::Junit.as_str().to_owned()),
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
            status: Set(ReportStatus::Pending.as_str().to_owned()),
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
            kind: Set(ReportKind::Pipeline.as_str().to_owned()),
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
            status: Set(ReportStatus::Pending.as_str().to_owned()),
            created_at: Set(now),
            retry_count: Set(0),
            ..Default::default()
        }
    }
}

/// A CI attempt number as the column holds it.
///
/// Both ingest routes refuse an attempt past `i32::MAX` with a 400 before one is
/// stored, so this saturates only for a payload a `2.0` build wrote before that
/// rule existed: the column is what limits it, and failing that stored Report
/// would lose a Run over an attempt number nobody reads.
#[must_use]
pub fn stored_ci_run_attempt(ci_run_attempt: i64) -> i32 {
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
            Value::from(ReportKind::Junit.as_str()),
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
/// [`flakeradar_core::dto::pipeline_body`].
///
/// # Errors
///
/// Returns [`DbError::StoredReportBody`] when the stored body is not a valid
/// Pipeline payload, which means the row was not written by this server.
pub fn stored_pipeline_report(report: &reports::Model) -> Result<PipelineReportIn, DbError> {
    serde_json::from_slice(&report.body).map_err(|error| DbError::StoredReportBody {
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
    ReportStatus::Processed
        .as_str()
        .clone_into(&mut report.status);
    report.counts = Some(counts);
    report.run_id = run_id;
    report.error = None;
    report.processed_at = Some(now);
}

/// The permanent-failure transition. A permanent failure spends no retry budget,
/// so `retry_count` is untouched. Field-only; the caller owns the transaction.
pub fn fail_report(report: &mut reports::Model, now: DateTimeWithTimeZone, error: &str) {
    ReportStatus::Failed.as_str().clone_into(&mut report.status);
    report.error = Some(error.to_owned());
    report.processed_at = Some(now);
}

/// The requeue transition: spend one automatic retry and put the Report back in
/// the queue. Only the transient path uses it. Field-only; the caller owns the
/// transaction.
pub fn requeue_report(report: &mut reports::Model, error: Option<&str>) {
    report.retry_count += 1;
    ReportStatus::Pending
        .as_str()
        .clone_into(&mut report.status);
    report.error = error.map(str::to_owned);
    report.processed_at = None;
}

/// The manual-retry transition: a human puts a failed Report back in the queue
/// with a fresh automatic-retry budget. Field-only; the caller owns the
/// transaction.
pub fn reset_for_retry(report: &mut reports::Model) {
    report.retry_count = 0;
    ReportStatus::Pending
        .as_str()
        .clone_into(&mut report.status);
    report.error = None;
    report.processed_at = None;
}

/// The columns one queue row serves, with the names the response spells them by.
///
/// `repo_name` is always there, because `repo_id` is NOT NULL; `project_name` is
/// `NULL` for a Pipeline Report.
const QUEUE_SELECT: &str = "
    SELECT reports.id, reports.kind, repos.name AS repo_name,
           projects.name AS project_name, reports.commit_sha, reports.branch,
           reports.ci_run_id, reports.status, reports.error, reports.counts,
           reports.run_id, reports.retry_count, reports.created_at,
           reports.processed_at
      FROM reports
      JOIN repos ON reports.repo_id = repos.id
      LEFT JOIN projects ON reports.project_id = projects.id
";

/// One stored Report with the names the response spells it by.
#[derive(Debug)]
pub struct StoredReport {
    /// The row itself.
    pub report: reports::Model,
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The Project's name, or `None` for a Pipeline Report.
    pub project: Option<String>,
}

impl StoredReport {
    /// The Report as the API serves it.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::UnknownReportStatus`] when the row holds a status this
    /// version does not write, which means the row was not written by this server.
    pub fn into_out(self) -> Result<ReportOut, DbError> {
        Ok(ReportOut {
            id: i64::from(self.report.id),
            kind: report_kind(&self.report.kind)?,
            repo: self.repo,
            project: self.project,
            commit_sha: self.report.commit_sha,
            branch: self.report.branch,
            ci_run_id: self.report.ci_run_id,
            status: report_status(&self.report.status)?,
            error: self.report.error,
            counts: self.report.counts,
            run_id: self.report.run_id.map(i64::from),
            retry_count: i64::from(self.report.retry_count),
            created_at: self.report.created_at.with_timezone(&Utc),
            processed_at: self.report.processed_at.map(|at| at.with_timezone(&Utc)),
        })
    }
}

/// The stored Report `report_id` names, with its Repo and Project names.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails or a row cannot be read.
pub async fn stored_report(
    db: &impl ConnectionTrait,
    report_id: i64,
) -> Result<Option<StoredReport>, DbError> {
    let sql = "SELECT reports.*, repos.name AS repo_name, projects.name AS project_name \
           FROM reports \
           JOIN repos ON reports.repo_id = repos.id \
           LEFT JOIN projects ON reports.project_id = projects.id \
          WHERE reports.id = $1";
    let Some(row) = one(db, sql, vec![Value::from(report_id)]).await? else {
        return Ok(None);
    };
    Ok(Some(StoredReport {
        report: reports::Model::from_query_result(&row, "")?,
        repo: get(&row, "repo_name")?,
        project: get(&row, "project_name")?,
    }))
}

/// How many Reports are pending and how many failed.
///
/// A database that holds no Report of a status counts zero rather than leaving
/// the status out, which is what one filtered `count(*)` per status gives.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn report_summary(db: &impl ConnectionTrait) -> Result<ReportSummaryOut, DbError> {
    let sql = "SELECT count(*) FILTER (WHERE status = $1) AS pending, \
                      count(*) FILTER (WHERE status = $2) AS failed \
                 FROM reports";
    let row = required_one(
        db,
        sql,
        vec![
            Value::from(ReportStatus::Pending.as_str()),
            Value::from(ReportStatus::Failed.as_str()),
        ],
        "report summary",
    )
    .await?;
    Ok(ReportSummaryOut {
        pending: get(&row, "pending")?,
        failed: get(&row, "failed")?,
    })
}

/// The queue, newest first, optionally only one status.
///
/// `limit` is the route's own bound; the caller validates it.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails or a row cannot be read.
pub async fn list_reports(
    db: &impl ConnectionTrait,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<ReportOut>, DbError> {
    let mut sql = QUEUE_SELECT.to_owned();
    let mut binds = Binds::new();
    if let Some(status) = status {
        binds.eq("reports.status", Value::from(status));
    }
    let limit = binds.bind(Value::from(limit));
    sql.push_str(&binds.suffix());
    sql.push_str(" ORDER BY reports.id DESC LIMIT ");
    sql.push_str(&limit);
    let mut out = Vec::new();
    for row in rows(db, sql, binds.into_values()).await? {
        out.push(report_out(&row)?);
    }
    Ok(out)
}

/// The kind one stored spelling names, refusing what this version never writes.
///
/// # Errors
///
/// Returns [`DbError::UnknownReportKind`] for a spelling the two [`ReportKind`]
/// variants do not cover.
pub fn report_kind(text: &str) -> Result<ReportKind, DbError> {
    ReportKind::from_wire(text).ok_or_else(|| DbError::UnknownReportKind {
        value: text.to_owned(),
    })
}

/// The status one stored spelling names, refusing what this version never writes.
///
/// # Errors
///
/// Returns [`DbError::UnknownReportStatus`] for a spelling the three
/// [`ReportStatus`] variants do not cover.
pub fn report_status(text: &str) -> Result<ReportStatus, DbError> {
    ReportStatus::from_wire(text).ok_or_else(|| DbError::UnknownReportStatus {
        value: text.to_owned(),
    })
}

/// One Report of the list, from its row.
fn report_out(row: &sea_orm::QueryResult) -> Result<ReportOut, DbError> {
    let created_at: DateTimeWithTimeZone = get(row, "created_at")?;
    let processed_at: Option<DateTimeWithTimeZone> = get(row, "processed_at")?;
    Ok(ReportOut {
        id: i64::from(get::<i32>(row, "id")?),
        kind: report_kind(&get::<String>(row, "kind")?)?,
        repo: get(row, "repo_name")?,
        project: get(row, "project_name")?,
        commit_sha: get(row, "commit_sha")?,
        branch: get(row, "branch")?,
        ci_run_id: get(row, "ci_run_id")?,
        status: report_status(&get::<String>(row, "status")?)?,
        error: get(row, "error")?,
        counts: get(row, "counts")?,
        run_id: get::<Option<i32>>(row, "run_id")?.map(i64::from),
        retry_count: i64::from(get::<i32>(row, "retry_count")?),
        created_at: created_at.with_timezone(&Utc),
        processed_at: processed_at.map(|at| at.with_timezone(&Utc)),
    })
}
