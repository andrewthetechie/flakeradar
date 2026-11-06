//! Ingest and the Report status routes (ADR 0002, accept-then-process).
//!
//! `POST /api/ingest` validates an uploaded JUnit XML report, stores it as a
//! pending Report and answers 202; the processor turns it into a Run later.
//! `POST /api/ingest/pipeline` does the same for one attempt of a Pipeline run.
//! The rest read the queue, summarise it, retry a failed Report or dismiss one.
//! `backend/app/routers/reports.py` is the reference.

// Rust guideline compliant 2026-07-21

use axum::Json;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRequest, Request, State};
use axum::handler::Handler as _;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Router, middleware};
use chrono::Utc;
use flakeradar_core::{
    AttemptIn, IngestAccepted, ParseError, PipelineReportIn, ReportOut, ReportSummaryOut,
    ValidationIssue, ci_run_attempt_too_large, lax, normalize_project, normalize_repo,
    normalize_root, parse_junit_xml, pipeline_body, py_strip, read_ci_run_attempt,
};
use flakeradar_db::entities::reports;
use flakeradar_db::{
    DbError, NewJunitReport, NewPipelineReport, REPORT_FAILED, REPORT_PENDING, REPORT_STATUSES,
    find_junit_duplicate, get_or_create_project, get_or_create_repo, retry_failed_report,
    stored_attempt,
};
use sea_orm::entity::prelude::DateTimeWithTimeZone;
use sea_orm::{
    ConnectionTrait, DbBackend, DbErr, EntityTrait, FromQueryResult as _, QueryResult, Set,
    Statement, TransactionTrait as _, Value,
};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiPath, ApiQuery, Validate, query_loc};
use crate::middleware::require_token;
use crate::multipart::{MalformedHeaders, REPORT_FIELD, file_field};

/// The largest Report body accepted, in bytes.
///
/// The frozen server's `MAX_REPORT_BYTES`. A larger upload is refused with 413
/// before it is parsed or stored, because a report nobody can process is not
/// worth a row in the queue.
pub const MAX_REPORT_BYTES: usize = 20 * 1024 * 1024;

/// How much framing a multipart upload may add over [`MAX_REPORT_BYTES`].
///
/// The limit applies to the uploaded file, and a multipart body carries a
/// boundary, a header block and a trailer around it. The allowance is far
/// smaller than a second file, so a body that carries one is still refused while
/// it is being read.
pub const MULTIPART_ALLOWANCE: usize = 64 * 1024;

/// The number of body bytes the ingest route buffers at most.
///
/// The route disables the app's own limit and reads under this one, so an
/// oversized upload is cut off instead of buffered whole. A raw body up to this
/// size is then judged against [`MAX_REPORT_BYTES`] itself, which is what makes
/// the answer 413 rather than a body that merely stops.
pub const INGEST_BODY_LIMIT: usize = MAX_REPORT_BYTES + MULTIPART_ALLOWANCE;

/// The `limit` a Report list request defaults to.
const DEFAULT_REPORT_LIMIT: i64 = 50;

/// The most Reports one list request returns.
const MAX_REPORT_LIMIT: i64 = 200;

/// One Report with the names the response spells it by.
///
/// `stored` is the row, and the two names come from the join the frozen server's
/// query did: the Repo is always there (`repo_id` is NOT NULL), and a Pipeline
/// Report has no Project.
const STORED_SQL: &str = "
    SELECT reports.*, repos.name AS repo_name, projects.name AS project_name
      FROM reports
      JOIN repos ON reports.repo_id = repos.id
      LEFT JOIN projects ON reports.project_id = projects.id
     WHERE reports.id = $1
";

/// The Report columns the list serves, without the uploaded body.
const LIST_SQL_HEAD: &str = "
    SELECT reports.id, reports.kind, repos.name AS repo_name,
           projects.name AS project_name, reports.commit_sha, reports.branch,
           reports.ci_run_id, reports.status, reports.error, reports.counts,
           reports.run_id, reports.retry_count, reports.created_at,
           reports.processed_at
      FROM reports
      JOIN repos ON reports.repo_id = repos.id
      LEFT JOIN projects ON reports.project_id = projects.id
";

/// The Report routes.
pub fn router(state: AppState) -> Router<AppState> {
    let token = middleware::from_fn_with_state(state, require_token);
    Router::new()
        // The body limit is per route: every other route keeps the app's own.
        .route(
            "/api/ingest",
            post(ingest)
                .layer(DefaultBodyLimit::max(INGEST_BODY_LIMIT))
                .layer(token.clone()),
        )
        .route(
            "/api/ingest/pipeline",
            post(ingest_pipeline).layer(token.clone()),
        )
        // Declared before /api/reports/{report_id} so "summary" is not read as an id.
        .route("/api/reports/summary", get(summary))
        .route("/api/reports", get(list_reports))
        .route(
            "/api/reports/{report_id}",
            get(get_report).delete(delete_report.layer(token.clone())),
        )
        .route(
            "/api/reports/{report_id}/retry",
            post(retry_report).layer(token),
        )
}

/// A failed Report request: the JSON `detail` a rejected request gets, or the
/// plain text the frozen server's unhandled error became.
#[derive(Debug)]
pub enum ReportError {
    /// A request the route rejected, in the usual `detail` shape.
    Rejected(ApiError),
    /// A database call the route did not expect to fail.
    Internal(DbError),
}

impl From<ApiError> for ReportError {
    fn from(error: ApiError) -> Self {
        Self::Rejected(error)
    }
}

impl From<DbError> for ReportError {
    fn from(error: DbError) -> Self {
        Self::Internal(error)
    }
}

impl From<DbErr> for ReportError {
    fn from(error: DbErr) -> Self {
        Self::Internal(DbError::Database(error))
    }
}

impl IntoResponse for ReportError {
    fn into_response(self) -> Response {
        match self {
            Self::Rejected(error) => error.into_response(),
            Self::Internal(error) => {
                // Never log the body or a Failure detail: a database error names
                // a statement, not what CI uploaded.
                tracing::error!(error = %error, "a Report route's query failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                    "Internal Server Error",
                )
                    .into_response()
            }
        }
    }
}

/// A Report route's answer.
pub type ReportResult<T> = Result<T, ReportError>;

/// The query parameters of `POST /api/ingest`.
#[derive(Debug, Clone, Deserialize)]
pub struct IngestQuery {
    /// The Repo's `owner/name`, normalized on the way in.
    repo: String,
    /// The Project's name; `default` when the upload names none.
    #[serde(default)]
    project: Option<String>,
    /// The Project root, repo-relative.
    #[serde(default)]
    root: Option<String>,
    /// The base the runner used for line numbers: `1` or `0`.
    #[serde(default = "default_line_base")]
    line_base: i64,
    /// The commit the Report covers.
    commit_sha: String,
    /// The branch the commit is on.
    #[serde(default = "default_branch_name")]
    branch: String,
    /// The CI provider's run id.
    #[serde(default)]
    ci_run_id: String,
    /// The Repo's Default branch, when the upload names it.
    #[serde(default)]
    default_branch: Option<String>,
    /// The CI provider's Job id, when the upload names one.
    #[serde(default)]
    ci_job_id: Option<String>,
    /// Which attempt of the CI run this is, numbered from 1.
    ///
    /// Parsed by [`read_attempt_query`] into the value it spells and the rule it
    /// breaks, so the upload is parsed once and the range is one rule.
    #[serde(default, deserialize_with = "read_attempt_query")]
    ci_run_attempt: Option<AttemptQuery>,
    /// The Pipeline the Job belongs to.
    #[serde(default)]
    pipeline: Option<String>,
}

/// The `line_base` an upload that names none means: the runner's own default.
fn default_line_base() -> i64 {
    1
}

/// The branch an upload that names none is on.
fn default_branch_name() -> String {
    "main".to_owned()
}

/// The `limit` a list request that names none means.
fn default_report_limit() -> i64 {
    DEFAULT_REPORT_LIMIT
}

impl Validate for IngestQuery {
    /// Checks the lengths and ranges `reports.py` put on the query parameters.
    ///
    /// FastAPI checked them before the handler ran, so a query that breaks one
    /// answers 422 before the body is read, and the issues come in the order the
    /// parameters are declared.
    fn validate(&self) -> ApiResult<()> {
        let mut issues = Vec::new();
        check_max(&self.repo, 255, "repo", &mut issues);
        if let Some(project) = &self.project {
            check_max(project, 100, "project", &mut issues);
        }
        if let Some(root) = &self.root {
            check_max(root, 1024, "root", &mut issues);
        }
        if self.line_base < 0 {
            issues.push(ValidationIssue::greater_than_equal(
                query_loc("line_base"),
                JsonValue::from(self.line_base),
                0,
            ));
        } else if self.line_base > 1 {
            issues.push(ValidationIssue::less_than_equal(
                query_loc("line_base"),
                JsonValue::from(self.line_base),
                1,
            ));
        }
        check_min(&self.commit_sha, 1, "commit_sha", &mut issues);
        check_max(&self.commit_sha, 64, "commit_sha", &mut issues);
        check_max(&self.branch, 255, "branch", &mut issues);
        check_max(&self.ci_run_id, 255, "ci_run_id", &mut issues);
        if let Some(default_branch) = &self.default_branch {
            check_max(default_branch, 255, "default_branch", &mut issues);
        }
        if let Some(ci_job_id) = &self.ci_job_id {
            check_max(ci_job_id, 255, "ci_job_id", &mut issues);
        }
        let attempt_over_range = match &self.ci_run_attempt {
            Some(AttemptQuery(reading)) => {
                if let Some(issue) = reading.issue(query_loc("ci_run_attempt")) {
                    issues.push(issue);
                }
                *reading == AttemptIn::PastColumn
            }
            None => false,
        };
        if let Some(pipeline) = &self.pipeline {
            check_max(pipeline, 512, "pipeline", &mut issues);
        }
        if !issues.is_empty() {
            return Err(ApiError::unprocessable(issues));
        }
        // An attempt past the column's range is not a body this server can
        // store, and Python crashed on it after creating rows this Request
        // would have to undo. It is refused with 400, the same way an
        // unparsable `time` is, rather than rewritten into a value CI never
        // sent.
        if attempt_over_range {
            return Err(too_large_attempt());
        }
        Ok(())
    }
}

/// The `ci_run_attempt` an upload named, parsed once.
#[derive(Debug, Clone)]
struct AttemptQuery(AttemptIn);

/// Reads a query `ci_run_attempt` as the value it spells.
///
/// The reading is never a parse error: a value Pydantic could not read is a 422
/// with Pydantic's kind, and one past the `int4` column is the 400 both ingest
/// routes answer, so the rule runs in `Validate` where the route can say which.
fn read_attempt_query<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<AttemptQuery>, D::Error> {
    let text = Option::<String>::deserialize(deserializer)?;
    Ok(text.map(|text| AttemptQuery(read_ci_run_attempt(&JsonValue::from(text)))))
}

/// The 400 for a `ci_run_attempt` the `int4` column cannot hold.
fn too_large_attempt() -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, ci_run_attempt_too_large())
}

/// The attempt to store, once `Validate` has accepted the upload.
fn stored_ci_run_attempt(attempt: Option<&AttemptQuery>) -> Option<i32> {
    match attempt {
        Some(AttemptQuery(AttemptIn::Number(number))) => Some(*number),
        _ => None,
    }
}

/// Records a NUL or a maximum-length failure for `field`.
///
/// The length counts characters as Python counted them. A NUL is refused because
/// a PostgreSQL `text` column cannot store one, where the reference answered 500
/// after creating the Repo and Project rows.
fn check_max(value: &str, max: usize, field: &str, issues: &mut Vec<ValidationIssue>) {
    lax::refuse_nul(value, query_loc(field), issues);
    if value.chars().count() > max {
        issues.push(ValidationIssue::string_too_long(
            query_loc(field),
            JsonValue::from(value.to_owned()),
            max,
        ));
    }
}

/// Records a minimum-length failure for `field`, in characters as Python counted.
fn check_min(value: &str, min: usize, field: &str, issues: &mut Vec<ValidationIssue>) {
    if value.chars().count() < min {
        issues.push(ValidationIssue::string_too_short(
            query_loc(field),
            JsonValue::from(value.to_owned()),
            min,
        ));
    }
}

/// The path parameters of a Report route.
///
/// The field is named, so a path that is not an integer reports `loc`
/// `["path", "report_id"]` as FastAPI's named path parameter did.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ReportPath {
    /// The Report's id.
    pub report_id: i64,
}

/// The query parameters of `GET /api/reports`.
#[derive(Debug, Clone, Deserialize)]
pub struct ListQuery {
    /// The status to filter by, out of [`REPORT_STATUSES`].
    #[serde(default)]
    status: Option<String>,
    /// How many Reports to return.
    #[serde(
        default = "default_report_limit",
        deserialize_with = "flakeradar_core::dto::lax_int::value"
    )]
    limit: i64,
}

impl Validate for ListQuery {
    /// Checks the range `reports.py` put on `limit`.
    ///
    /// The `status` membership check is the handler's own, and answers a
    /// hand-written detail rather than a list of issues.
    fn validate(&self) -> ApiResult<()> {
        let mut issues = Vec::new();
        if self.limit < 1 {
            issues.push(ValidationIssue::greater_than_equal(
                query_loc("limit"),
                JsonValue::from(self.limit),
                1,
            ));
        } else if self.limit > MAX_REPORT_LIMIT {
            issues.push(ValidationIssue::less_than_equal(
                query_loc("limit"),
                JsonValue::from(self.limit),
                MAX_REPORT_LIMIT,
            ));
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ApiError::unprocessable(issues))
        }
    }
}

/// A Pipeline Report body that passed the rules `schemas.py` put on it.
#[derive(Debug)]
pub struct PipelineBody(pub PipelineReportIn);

impl<S> FromRequest<S> for PipelineBody
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let content_type = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|rejection| crate::extract::body_error(&rejection))?;
        let object = crate::extract::parse_json_object(&bytes, content_type.as_deref())?;
        let payload = PipelineReportIn::from_value(&object)?;
        Ok(Self(payload))
    }
}

/// Accepts a JUnit XML report as a multipart upload or a raw body.
///
/// Only validation happens here: the Report is queued, and the processor turns
/// it into a Run later. A duplicate of a stored upload answers the stored
/// Report's id and status instead of queueing a second Run.
///
/// # Errors
///
/// Answers 400 for an empty body, for a `<testcase time>` that is not a number
/// and for a `ci_run_attempt` past the column's range, 413 for an upload over
/// [`MAX_REPORT_BYTES`], 422 for an unusable Repo, Project or root name and for
/// a body that is not a JUnit XML report, 401 without the token, and 500 when a
/// query fails.
#[utoipa::path(
    post,
    path = "/api/ingest",
    tag = "reports",
    params(
        ("x-api-key" = Option<String>, Header, description = "The shared token this instance requires"),
        ("repo" = String, Query, description = "The Repo's owner/name"),
        ("project" = Option<String>, Query, description = "The Project's name; default when omitted"),
        ("root" = Option<String>, Query, description = "The Project root, repo-relative"),
        ("line_base" = Option<i64>, Query, description = "The runner's line numbering: 1 or 0"),
        ("commit_sha" = String, Query, description = "The commit the Report covers"),
        ("branch" = Option<String>, Query, description = "The branch the commit is on"),
        ("ci_run_id" = Option<String>, Query, description = "The CI provider's run id"),
        ("default_branch" = Option<String>, Query, description = "The Repo's Default branch"),
        ("ci_job_id" = Option<String>, Query, description = "The CI provider's Job id"),
        ("ci_run_attempt" = Option<i64>, Query, description = "Which attempt of the CI run this is, from 1 to 2147483647"),
        ("pipeline" = Option<String>, Query, description = "The Pipeline the Job belongs to")
    ),
    responses(
        (status = 202, body = IngestAccepted),
        (status = 400, description = "Empty body, a testcase time that is not a number, or a ci_run_attempt past the column's range"),
        (status = 413, description = "The upload exceeds 20 MB"),
        (status = 422, description = "An unusable name, or a body that is not JUnit XML")
    )
)]
pub async fn ingest(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<IngestQuery>,
    request: Request,
) -> ReportResult<Response> {
    let content = report_body(request).await?;
    if content.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Empty report body").into());
    }
    if content.len() > MAX_REPORT_BYTES {
        return Err(
            ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "Report exceeds 20 MB limit").into(),
        );
    }
    let repo_name = normalize_repo(&query.repo).map_err(|error| named_error(&error))?;
    let project_name =
        normalize_project(query.project.as_deref()).map_err(|error| named_error(&error))?;
    let root = normalize_root(query.root.as_deref()).map_err(|error| named_error(&error))?;
    let line_base = query.line_base;
    parse_upload(content.clone(), line_base).await?;
    // Checked here, where Python's insert failed on the same value, so every
    // refusal before this one answers exactly as it did.
    let ci_run_attempt = stored_ci_run_attempt(query.ci_run_attempt.as_ref());

    let now = Utc::now().fixed_offset();
    // The Repo, Project and Report are one unit of work: Python committed them
    // once, at the end, so a duplicate answer or a failed insert left none of
    // them behind. A transaction keeps that promise when the insert fails.
    let txn = state.pool().begin().await?;
    let project = get_or_create_project(&txn, &repo_name, &project_name, now).await?;
    let candidate = NewJunitReport {
        project_id: project.id,
        repo_id: project.repo_id,
        body: content,
        commit_sha: query.commit_sha,
        branch: query.branch,
        ci_run_id: query.ci_run_id,
        root,
        line_base: Some(stored_attempt(line_base)),
        ci_job_id: blank_to_none(query.ci_job_id),
        ci_run_attempt,
        pipeline: blank_to_none(query.pipeline),
        default_branch: blank_to_none(query.default_branch),
    };
    if let Some(existing) = find_junit_duplicate(&txn, &candidate).await? {
        // A duplicate queues nothing, and the Repo and Project this request may
        // have created are not committed either, as Python's single `commit`
        // site never reached them.
        txn.rollback().await?;
        return Ok(accepted(IngestAccepted {
            report_id: i64::from(existing.id),
            status: existing.status,
            duplicate: true,
        }));
    }
    let inserted = reports::Entity::insert(candidate.into_active_model(now))
        .exec_with_returning(&txn)
        .await?;
    txn.commit().await?;
    Ok(accepted(IngestAccepted {
        report_id: i64::from(inserted.id),
        status: REPORT_PENDING.to_owned(),
        duplicate: false,
    }))
}

/// The 202 every accepted upload answers with, and its body.
fn accepted(body: IngestAccepted) -> Response {
    (StatusCode::ACCEPTED, Json(body)).into_response()
}

/// The uploaded bytes: the `report` file of a multipart body, else the raw body.
///
/// The `Content-Type` decides, and nothing else: `application/x-www-form-urlencoded`
/// carries raw XML too, because that is curl's default for `--data-binary`.
async fn report_body(request: Request) -> Result<Vec<u8>, ApiError> {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_default();
    let bytes = Bytes::from_request(request, &())
        .await
        .map_err(|rejection| upload_error(&rejection))?;
    if !content_type.starts_with("multipart/form-data") {
        return Ok(bytes.to_vec());
    }
    // Finding the field reads the whole body, which can be 20 MiB, so it runs
    // on a blocking thread rather than on the one serving every other request.
    let found = tokio::task::spawn_blocking(move || {
        file_field(&bytes, &content_type, REPORT_FIELD).map(|found| found.map(<[u8]>::to_vec))
    })
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("The multipart scan did not finish: {error}"),
        )
    })?;
    match found {
        Ok(found) => Ok(found.unwrap_or_default()),
        // The reference handed the body to Starlette's form parser, which
        // refused a part whose headers are no headers with this detail.
        Err(MalformedHeaders) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Invalid multipart data.",
        )),
    }
}

/// The 413 or 500 for an upload that could not be read.
///
/// The route's own limit is what a body over [`INGEST_BODY_LIMIT`] hits, so the
/// frozen server's "Report exceeds 20 MB limit" is the answer rather than Axum's
/// generic one.
fn upload_error(rejection: &axum::extract::rejection::BytesRejection) -> ApiError {
    use axum::extract::rejection::{BytesRejection, FailedToBufferBody};

    match rejection {
        BytesRejection::FailedToBufferBody(FailedToBufferBody::LengthLimitError(_)) => {
            ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "Report exceeds 20 MB limit")
        }
        _ => ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Reading the report body failed: {rejection}"),
        ),
    }
}

/// Parses the upload off the request's thread, so a large report never blocks it.
///
/// Parsing is CPU-bound, and the frozen server ran it in a worker thread for the
/// same reason. A `<testcase time>` that is not a number answers 400, and every
/// other parse failure answers 422.
async fn parse_upload(content: Vec<u8>, line_base: i64) -> ApiResult<()> {
    let parsed = tokio::task::spawn_blocking(move || parse_junit_xml(&content, line_base))
        .await
        .map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("The report parser did not finish: {error}"),
            )
        })?;
    match parsed {
        Ok(_) => Ok(()),
        Err(error @ ParseError::InvalidTime { .. }) => {
            Err(ApiError::new(StatusCode::BAD_REQUEST, error.to_string()))
        }
        Err(error) => Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            error.to_string(),
        )),
    }
}

/// The 422 for a Repo, Project or root name the server cannot accept.
fn named_error(error: &flakeradar_core::IdentityError) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, error.to_string())
}

/// A query parameter's value with the surrounding whitespace trimmed, or `None`
/// when nothing is left.
///
/// The frozen server wrote `(value or "").strip() or None`, which is Python's
/// strip, so a value that is only whitespace becomes NULL in the database.
fn blank_to_none(value: Option<String>) -> Option<String> {
    value
        .map(|value| py_strip(&value).to_owned())
        .filter(|value| !value.is_empty())
}

/// Accepts one attempt of one Pipeline run as a JSON report.
///
/// The body already validated and normalized, so the stored body is canonical
/// and the processor can trust it. Pipeline Reports are never compared for
/// duplicates: a retried attempt is a second attempt, not the same one.
///
/// # Errors
///
/// Answers 422 for a body that breaks the payload's rules, 401 without the
/// token, and 500 when a query fails.
#[utoipa::path(
    post,
    path = "/api/ingest/pipeline",
    tag = "reports",
    params(("x-api-key" = Option<String>, Header, description = "The shared token this instance requires")),
    request_body = PipelineReportIn,
    responses(
        (status = 202, body = IngestAccepted),
        (status = 422, description = "The payload breaks one of its rules")
    )
)]
pub async fn ingest_pipeline(
    State(state): State<AppState>,
    PipelineBody(payload): PipelineBody,
) -> ReportResult<Response> {
    let now = Utc::now().fixed_offset();
    // The Repo and the Report are one unit of work, as they were for Python's
    // single `commit`: a failed insert leaves no Repo behind.
    let txn = state.pool().begin().await?;
    let repo = get_or_create_repo(&txn, &payload.repo, now).await?;
    let report = NewPipelineReport {
        repo_id: repo.id,
        body: pipeline_body(&payload),
        commit_sha: payload.commit_sha.clone(),
        branch: payload.branch.clone(),
        ci_run_id: payload.ci_run_id.clone(),
        ci_run_attempt: stored_attempt(payload.ci_run_attempt),
        pipeline: Some(payload.pipeline.clone()),
        default_branch: payload.default_branch.clone(),
    };
    let inserted = reports::Entity::insert(report.into_active_model(now))
        .exec_with_returning(&txn)
        .await?;
    txn.commit().await?;
    Ok(accepted(IngestAccepted {
        report_id: i64::from(inserted.id),
        status: REPORT_PENDING.to_owned(),
        duplicate: false,
    }))
}

/// How many Reports are pending and how many failed.
///
/// # Errors
///
/// Answers 500 when the query fails.
#[utoipa::path(
    get,
    path = "/api/reports/summary",
    tag = "reports",
    responses((status = 200, body = ReportSummaryOut))
)]
pub async fn summary(State(state): State<AppState>) -> ReportResult<Json<ReportSummaryOut>> {
    // One row, one column per status the queue reports: a database that holds
    // no Report of a status counts zero rather than leaving it out.
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*) FILTER (WHERE status = $1) AS pending, \
                count(*) FILTER (WHERE status = $2) AS failed \
           FROM reports",
        [Value::from(REPORT_PENDING), Value::from(REPORT_FAILED)],
    );
    let row = state
        .pool()
        .query_one_raw(statement)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("the summary query returned no row".to_owned()))?;
    Ok(Json(ReportSummaryOut {
        pending: row.try_get("", "pending")?,
        failed: row.try_get("", "failed")?,
    }))
}

/// The Report queue, newest first.
///
/// # Errors
///
/// Answers 422 for a `status` that is not one of [`REPORT_STATUSES`] and for a
/// `limit` outside 1–200, and 500 when the query fails.
#[utoipa::path(
    get,
    path = "/api/reports",
    tag = "reports",
    params(
        ("status" = Option<String>, Query, description = "pending, processed or failed"),
        ("limit" = Option<i64>, Query, description = "How many Reports to return, 1-200")
    ),
    responses((status = 200, body = Vec<ReportOut>))
)]
pub async fn list_reports(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> ReportResult<Json<Vec<ReportOut>>> {
    if let Some(status) = &query.status
        && !REPORT_STATUSES.contains(&status.as_str())
    {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("status must be one of {}", REPORT_STATUSES.join(", ")),
        )
        .into());
    }
    let mut sql = LIST_SQL_HEAD.to_owned();
    let mut values = Vec::new();
    if let Some(status) = query.status {
        sql.push_str(" WHERE reports.status = $1");
        values.push(Value::from(status));
    }
    sql.push_str(if values.is_empty() {
        " ORDER BY reports.id DESC LIMIT $1"
    } else {
        " ORDER BY reports.id DESC LIMIT $2"
    });
    values.push(Value::from(query.limit));
    let statement = Statement::from_sql_and_values(DbBackend::Postgres, sql, values);
    let mut out = Vec::new();
    for row in state.pool().query_all_raw(statement).await? {
        out.push(report_out(&row)?);
    }
    Ok(Json(out))
}

/// One Report, by its id.
///
/// # Errors
///
/// Answers 404 for an id that names no Report, 422 for a path that is not an
/// integer, and 500 when the query fails.
#[utoipa::path(
    get,
    path = "/api/reports/{report_id}",
    tag = "reports",
    params(("report_id" = i64, Path, description = "The Report's id")),
    responses((status = 200, body = ReportOut), (status = 404, description = "Report not found"))
)]
pub async fn get_report(
    State(state): State<AppState>,
    ApiPath(path): ApiPath<ReportPath>,
) -> ReportResult<Json<ReportOut>> {
    let stored = stored_report(state.pool(), path.report_id).await?;
    match stored {
        Some(stored) => Ok(Json(stored.into_out())),
        None => Err(not_found()),
    }
}

/// Puts a failed Report back in the queue with a fresh automatic-retry budget.
///
/// A manual retry is a human decision, so it works even when the automatic
/// retries are spent: `retry_count` goes back to 0.
///
/// # Errors
///
/// Answers 404 for an id that names no Report, 409 for a Report that has not
/// failed, 401 without the token, and 500 when a query fails.
#[utoipa::path(
    post,
    path = "/api/reports/{report_id}/retry",
    tag = "reports",
    params(
        ("x-api-key" = Option<String>, Header, description = "The shared token this instance requires"),
        ("report_id" = i64, Path, description = "The Report's id")
    ),
    responses(
        (status = 200, body = ReportOut),
        (status = 404, description = "Report not found"),
        (status = 409, description = "Only failed reports can be retried")
    )
)]
pub async fn retry_report(
    State(state): State<AppState>,
    ApiPath(path): ApiPath<ReportPath>,
) -> ReportResult<Json<ReportOut>> {
    let Some(mut stored) = stored_report(state.pool(), path.report_id).await? else {
        return Err(not_found());
    };
    if stored.report.status != REPORT_FAILED {
        return Err(
            ApiError::new(StatusCode::CONFLICT, "Only failed reports can be retried").into(),
        );
    }
    retry_failed_report(&mut stored.report);
    reports::Entity::update(reports::ActiveModel {
        id: Set(stored.report.id),
        retry_count: Set(stored.report.retry_count),
        status: Set(stored.report.status.clone()),
        error: Set(stored.report.error.clone()),
        processed_at: Set(stored.report.processed_at),
        ..Default::default()
    })
    .exec(state.pool())
    .await?;
    Ok(Json(stored.into_out()))
}

/// Dismisses a failed Report by deleting it.
///
/// # Errors
///
/// Answers 404 for an id that names no Report, 409 for a Report that has not
/// failed, 401 without the token, and 500 when a query fails.
#[utoipa::path(
    delete,
    path = "/api/reports/{report_id}",
    tag = "reports",
    params(
        ("x-api-key" = Option<String>, Header, description = "The shared token this instance requires"),
        ("report_id" = i64, Path, description = "The Report's id")
    ),
    responses(
        (status = 204, description = "The Report is gone"),
        (status = 404, description = "Report not found"),
        (status = 409, description = "Only failed reports can be deleted")
    )
)]
pub async fn delete_report(
    State(state): State<AppState>,
    ApiPath(path): ApiPath<ReportPath>,
) -> ReportResult<StatusCode> {
    let Some(stored) = stored_report(state.pool(), path.report_id).await? else {
        return Err(not_found());
    };
    if stored.report.status != REPORT_FAILED {
        return Err(
            ApiError::new(StatusCode::CONFLICT, "Only failed reports can be deleted").into(),
        );
    }
    reports::Entity::delete_by_id(stored.report.id)
        .exec(state.pool())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The 404 a Report route answers for an id that names no Report.
fn not_found() -> ReportError {
    ApiError::new(StatusCode::NOT_FOUND, "Report not found").into()
}

/// One stored Report with the names the response spells it by.
#[derive(Debug)]
struct StoredReport {
    /// The row itself.
    report: reports::Model,
    /// The Repo's `owner/name`.
    repo: String,
    /// The Project's name, or `None` for a Pipeline Report.
    project: Option<String>,
}

impl StoredReport {
    /// The Report as the API serves it.
    fn into_out(self) -> ReportOut {
        ReportOut {
            id: i64::from(self.report.id),
            kind: self.report.kind,
            repo: self.repo,
            project: self.project,
            commit_sha: self.report.commit_sha,
            branch: self.report.branch,
            ci_run_id: self.report.ci_run_id,
            status: self.report.status,
            error: self.report.error,
            counts: self.report.counts,
            run_id: self.report.run_id.map(i64::from),
            retry_count: i64::from(self.report.retry_count),
            created_at: self.report.created_at.with_timezone(&Utc),
            processed_at: self.report.processed_at.map(|at| at.with_timezone(&Utc)),
        }
    }
}

/// The stored Report `report_id` names, with its Repo and Project names.
async fn stored_report(
    pool: &impl ConnectionTrait,
    report_id: i64,
) -> Result<Option<StoredReport>, DbError> {
    let statement =
        Statement::from_sql_and_values(DbBackend::Postgres, STORED_SQL, [Value::from(report_id)]);
    let Some(row) = pool.query_one_raw(statement).await? else {
        return Ok(None);
    };
    Ok(Some(StoredReport {
        report: reports::Model::from_query_result(&row, "")?,
        repo: row.try_get("", "repo_name")?,
        project: row.try_get("", "project_name")?,
    }))
}

/// One Report of the list, from its row.
fn report_out(row: &QueryResult) -> Result<ReportOut, DbErr> {
    let created_at: DateTimeWithTimeZone = row.try_get("", "created_at")?;
    let processed_at: Option<DateTimeWithTimeZone> = row.try_get("", "processed_at")?;
    Ok(ReportOut {
        id: i64::from(row.try_get::<i32>("", "id")?),
        kind: row.try_get("", "kind")?,
        repo: row.try_get("", "repo_name")?,
        project: row.try_get("", "project_name")?,
        commit_sha: row.try_get("", "commit_sha")?,
        branch: row.try_get("", "branch")?,
        ci_run_id: row.try_get("", "ci_run_id")?,
        status: row.try_get("", "status")?,
        error: row.try_get("", "error")?,
        counts: row.try_get("", "counts")?,
        run_id: row.try_get::<Option<i32>>("", "run_id")?.map(i64::from),
        retry_count: i64::from(row.try_get::<i32>("", "retry_count")?),
        created_at: created_at.with_timezone(&Utc),
        processed_at: processed_at.map(|at| at.with_timezone(&Utc)),
    })
}
