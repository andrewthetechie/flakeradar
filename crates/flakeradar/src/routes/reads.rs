//! The dashboard read APIs: Repos, the two leaderboards, summaries, detail and
//! quarantine, plus the CI run failure listing.
//!
//! Every handler validates the way FastAPI did, calls one function in
//! `flakeradar_db::reads`, and serializes the DTO from `flakeradar-core`. The
//! frozen server's `routers/tests.py`, `routers/jobs.py` and
//! `routers/ci_runs.py` are the reference, including their 422 shapes.

// Rust guideline compliant 2026-07-21

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Router, middleware};
use flakeradar_core::limits::{LIST_LIMIT_MAX, NEW_WITHIN_DAYS_MAX};
use flakeradar_core::{
    CiRunFailuresOut, FailureCategory, HistoryOut, JobHistoryOut, JobPage, JobSummaryOut,
    QuarantineIn, QuarantineItem, RepoOut, SummaryOut, TestOut, TestPage, ValidationIssue, lax,
    normalize_project, normalize_repo,
};
use flakeradar_db::{
    DEFAULT_EXECUTIONS_LIMIT, FAILURES_LIMIT, JobListFilters, ListOptions, MAX_EXECUTIONS_LIMIT,
    ReadSettings, Scope, SortKey, TestListFilters, ci_run_failures, get_job, get_test, job_summary,
    list_jobs, list_repos, list_tests, quarantine_list, set_quarantine, summary,
};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::app::AppState;
use crate::error::{ApiError, ApiResult, RouteError, RouteResult};
use crate::extract::{ApiJson, ApiPath, ApiQuery, Validate, query_loc};
use crate::middleware::require_token;
use crate::routes::{JobPath, TestPath};

/// The dashboard's read routes.
///
/// `GO /api/quarantine` is the only one behind the shared token: a runner reads
/// the skip list with the same token it uploads with.
pub fn router(state: AppState) -> Router<AppState> {
    let token = middleware::from_fn_with_state(state, require_token);
    Router::new()
        .route("/api/repos", get(repos))
        .route("/api/tests", get(tests))
        .route("/api/summary", get(test_summary))
        .route("/api/tests/{test_id}/history", get(test_history))
        .route("/api/tests/{test_id}/quarantine", post(quarantine_test))
        .route("/api/quarantine", get(quarantine).layer(token))
        // Declared before /api/jobs/{job_id}/... so "summary" is not read as an id.
        .route("/api/jobs/summary", get(jobs_summary))
        .route("/api/jobs", get(jobs))
        .route("/api/jobs/{job_id}/history", get(job_history))
        .route("/api/ci-runs/{ci_run_id}/failures", get(ci_run))
}

/// The read settings the configuration holds.
fn read_settings(state: &AppState) -> ReadSettings {
    state.settings().reads()
}

/// A 422 from an identity rule, as `{"detail": "<message>"}`.
fn identity_error(error: &flakeradar_core::IdentityError) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, error.to_string())
}

/// A 422 message the route writes itself.
fn detail_error(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, message)
}

/// Records a string with a NUL in it, and one longer than its limit.
///
/// The length counts characters as Pydantic counted them. A NUL is refused
/// because a PostgreSQL `text` column cannot store one, where the reference
/// answered 500 and logged at error level on an unauthenticated route.
fn check_max(value: Option<&String>, max: usize, field: &str, issues: &mut Vec<ValidationIssue>) {
    let Some(value) = value else {
        return;
    };
    lax::refuse_nul(value, query_loc(field), issues);
    if value.chars().count() > max {
        issues.push(ValidationIssue::string_too_long(
            query_loc(field),
            JsonValue::from(value.clone()),
            max,
        ));
    }
}

/// Records a number below its minimum.
fn check_ge(value: Option<i64>, min: i64, field: &str, issues: &mut Vec<ValidationIssue>) {
    if let Some(value) = value
        && value < min
    {
        issues.push(ValidationIssue::greater_than_equal(
            query_loc(field),
            JsonValue::from(value),
            min,
        ));
    }
}

/// Records a number above its maximum.
fn check_le(value: Option<i64>, max: i64, field: &str, issues: &mut Vec<ValidationIssue>) {
    if let Some(value) = value
        && value > max
    {
        issues.push(ValidationIssue::less_than_equal(
            query_loc(field),
            JsonValue::from(value),
            max,
        ));
    }
}

/// The largest `page` whose `OFFSET` fits an `i64` at [`LIST_LIMIT_MAX`].
///
/// The reference bounded only the bottom of the range, so a page past this one
/// wrapped in release builds and panicked in development ones. A page real
/// clients ask for is a handful of digits.
const PAGE_MAX: i64 = i64::MAX / LIST_LIMIT_MAX;

/// The paging range both leaderboards share.
fn validate_paging(page: Option<i64>, page_size: Option<i64>, issues: &mut Vec<ValidationIssue>) {
    check_ge(page, 1, "page", issues);
    check_le(page, PAGE_MAX, "page", issues);
    check_ge(page_size, 1, "page_size", issues);
    check_le(page_size, LIST_LIMIT_MAX, "page_size", issues);
}

/// The `new_within_days` range both leaderboards share.
fn validate_new_within_days(value: Option<i64>, issues: &mut Vec<ValidationIssue>) {
    check_ge(value, 1, "new_within_days", issues);
    check_le(value, NEW_WITHIN_DAYS_MAX, "new_within_days", issues);
}

/// Records a `sort` value that is not one of the three orders.
fn validate_sort(sort: Option<&String>, issues: &mut Vec<ValidationIssue>) {
    if let Some(sort) = sort
        && SortKey::parse(sort).is_none()
    {
        issues.push(ValidationIssue::literal_error(
            query_loc("sort"),
            JsonValue::from(sort.clone()),
            "'score', 'last_seen' or 'proven'",
        ));
    }
}

/// The expected list of Failure categories, as Pydantic printed it.
fn category_expected() -> String {
    let names: Vec<String> = FailureCategory::ALL
        .iter()
        .map(|category| format!("'{}'", category.as_str()))
        .collect();
    match names.split_last() {
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// Records a `category` value that is not one of the five.
fn validate_category(category: Option<&String>, issues: &mut Vec<ValidationIssue>) {
    if let Some(category) = category
        && !FailureCategory::ALL
            .iter()
            .any(|known| known.as_str() == category)
    {
        issues.push(ValidationIssue::literal_error(
            query_loc("category"),
            JsonValue::from(category.clone()),
            &category_expected(),
        ));
    }
}

/// Records a `limit` outside the detail routes' range.
fn validate_limit(limit: Option<i64>, issues: &mut Vec<ValidationIssue>) {
    check_ge(limit, 1, "limit", issues);
    check_le(limit, MAX_EXECUTIONS_LIMIT, "limit", issues);
}

/// The optional `?repo=&project=` pair several routes take.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ScopeQuery {
    /// The Repo's `owner/name`.
    #[serde(default)]
    repo: Option<String>,
    /// The Project's name, which needs a Repo.
    #[serde(default)]
    project: Option<String>,
}

impl Validate for ScopeQuery {
    fn validate(&self) -> ApiResult<()> {
        let mut issues = Vec::new();
        check_max(self.repo.as_ref(), 255, "repo", &mut issues);
        check_max(self.project.as_ref(), 100, "project", &mut issues);
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ApiError::unprocessable(issues))
        }
    }
}

/// Builds the Scope a query names, normalizing both names.
fn scope_of(repo: Option<String>, project: Option<String>) -> RouteResult<Scope> {
    let repo = match repo {
        Some(raw) => Some(normalize_repo(&raw).map_err(|error| identity_error(&error))?),
        None => None,
    };
    let project = match project {
        Some(raw) => Some(normalize_project(Some(&raw)).map_err(|error| identity_error(&error))?),
        None => None,
    };
    Scope::new(repo, project).map_err(|error| detail_error(error.to_string()).into())
}

/// The query parameters of `GET /api/tests`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TestsQuery {
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default, deserialize_with = "flakeradar_core::dto::lax_bool::optional")]
    include_stable: Option<bool>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default, deserialize_with = "flakeradar_core::dto::lax_int::optional")]
    page: Option<i64>,
    #[serde(default, deserialize_with = "flakeradar_core::dto::lax_int::optional")]
    page_size: Option<i64>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default, deserialize_with = "flakeradar_core::dto::lax_bool::optional")]
    quarantined: Option<bool>,
    #[serde(default, deserialize_with = "flakeradar_core::dto::lax_bool::optional")]
    has_issue: Option<bool>,
    #[serde(default, deserialize_with = "flakeradar_core::dto::lax_int::optional")]
    new_within_days: Option<i64>,
    #[serde(default, deserialize_with = "flakeradar_core::dto::lax_bool::optional")]
    broken: Option<bool>,
}

impl Validate for TestsQuery {
    fn validate(&self) -> ApiResult<()> {
        let mut issues = Vec::new();
        check_max(self.repo.as_ref(), 255, "repo", &mut issues);
        check_max(self.project.as_ref(), 100, "project", &mut issues);
        validate_sort(self.sort.as_ref(), &mut issues);
        validate_paging(self.page, self.page_size, &mut issues);
        check_max(self.file.as_ref(), 1024, "file", &mut issues);
        validate_category(self.category.as_ref(), &mut issues);
        check_max(self.q.as_ref(), 200, "q", &mut issues);
        validate_new_within_days(self.new_within_days, &mut issues);
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ApiError::unprocessable(issues))
        }
    }
}

impl TestsQuery {
    /// The filters and options the query names.
    fn parts(&self) -> (TestListFilters, ListOptions) {
        let filters = TestListFilters {
            file: self.file.clone(),
            category: self
                .category
                .as_deref()
                .and_then(FailureCategory::from_wire),
            quarantined: self.quarantined,
            broken: self.broken.unwrap_or(false),
        };
        let options = ListOptions {
            include_stable: self.include_stable.unwrap_or(false),
            flaky_only: false,
            sort: self
                .sort
                .as_deref()
                .and_then(SortKey::parse)
                .unwrap_or(SortKey::Score),
            page: self.page.unwrap_or(1),
            page_size: self.page_size.unwrap_or(50),
            q: self.q.clone(),
            has_issue: self.has_issue,
            new_within_days: self.new_within_days,
        };
        (filters, options)
    }
}

/// The query parameters of `GET /api/jobs`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct JobsQuery {
    #[serde(default)]
    repo: Option<String>,
    #[serde(default, deserialize_with = "flakeradar_core::dto::lax_bool::optional")]
    include_stable: Option<bool>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default, deserialize_with = "flakeradar_core::dto::lax_int::optional")]
    page: Option<i64>,
    #[serde(default, deserialize_with = "flakeradar_core::dto::lax_int::optional")]
    page_size: Option<i64>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    pipeline: Option<String>,
    #[serde(default, deserialize_with = "flakeradar_core::dto::lax_bool::optional")]
    has_issue: Option<bool>,
    #[serde(default, deserialize_with = "flakeradar_core::dto::lax_int::optional")]
    new_within_days: Option<i64>,
}

impl Validate for JobsQuery {
    fn validate(&self) -> ApiResult<()> {
        let mut issues = Vec::new();
        check_max(self.repo.as_ref(), 255, "repo", &mut issues);
        validate_sort(self.sort.as_ref(), &mut issues);
        validate_paging(self.page, self.page_size, &mut issues);
        check_max(self.q.as_ref(), 200, "q", &mut issues);
        check_max(self.pipeline.as_ref(), 512, "pipeline", &mut issues);
        validate_new_within_days(self.new_within_days, &mut issues);
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ApiError::unprocessable(issues))
        }
    }
}

/// The query parameters of the two detail routes.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HistoryQuery {
    /// How many Executions to return.
    #[serde(default, deserialize_with = "flakeradar_core::dto::lax_int::optional")]
    limit: Option<i64>,
}

impl Validate for HistoryQuery {
    fn validate(&self) -> ApiResult<()> {
        let mut issues = Vec::new();
        validate_limit(self.limit, &mut issues);
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ApiError::unprocessable(issues))
        }
    }
}

impl HistoryQuery {
    /// The limit the route lists Executions with.
    fn executions_limit(&self) -> i64 {
        self.limit.unwrap_or(DEFAULT_EXECUTIONS_LIMIT)
    }
}

/// The path parameter of `GET /api/ci-runs/{ci_run_id}/failures`.
#[derive(Debug, Clone, Deserialize)]
pub struct CiRunPath {
    /// The CI provider's run id.
    pub ci_run_id: String,
}

/// The query parameters of the CI run failure listing.
#[derive(Debug, Clone, Deserialize)]
pub struct CiRunFailuresQuery {
    /// The Repo's `owner/name`; required.
    repo: String,
    /// Which attempt to read; the newest when absent.
    #[serde(default, deserialize_with = "flakeradar_core::dto::lax_int::optional")]
    ci_run_attempt: Option<i64>,
}

impl Validate for CiRunFailuresQuery {
    fn validate(&self) -> ApiResult<()> {
        let mut issues = Vec::new();
        check_max(Some(&self.repo), 255, "repo", &mut issues);
        check_ge(self.ci_run_attempt, 1, "ci_run_attempt", &mut issues);
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ApiError::unprocessable(issues))
        }
    }
}

/// The query parameters of `GET /api/quarantine`.
#[derive(Debug, Clone, Deserialize)]
pub struct QuarantineQuery {
    /// The Repo's `owner/name`; required.
    repo: String,
    /// The Project's name; `default` when absent.
    #[serde(default)]
    project: Option<String>,
}

impl Validate for QuarantineQuery {
    fn validate(&self) -> ApiResult<()> {
        let mut issues = Vec::new();
        check_max(Some(&self.repo), 255, "repo", &mut issues);
        check_max(self.project.as_ref(), 100, "project", &mut issues);
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ApiError::unprocessable(issues))
        }
    }
}

/// Every Repo with its Projects.
///
/// # Errors
///
/// Answers 500 when the query fails.
#[utoipa::path(
    get,
    path = "/api/repos",
    tag = "reads",
    responses((status = 200, body = Vec<RepoOut>))
)]
pub async fn repos(State(state): State<AppState>) -> RouteResult<Json<Vec<RepoOut>>> {
    Ok(Json(list_repos(state.pool()).await?))
}

/// The Tests leaderboard.
///
/// # Errors
///
/// Answers 422 for a bad filter or range, and 500 when a query fails.
#[utoipa::path(
    get,
    path = "/api/tests",
    tag = "reads",
    params(
        ("repo" = Option<String>, Query, description = "The Repo's owner/name"),
        ("project" = Option<String>, Query, description = "The Project's name, which needs a repo"),
        ("include_stable" = Option<bool>, Query, description = "Include Tests with a zero score"),
        ("sort" = Option<String>, Query, description = "score, last_seen or proven"),
        ("page" = Option<i64>, Query, description = "The page number, from 1"),
        ("page_size" = Option<i64>, Query, description = "How many Tests a page holds, 1-100"),
        ("file" = Option<String>, Query, description = "A literal substring of the Test's file"),
        ("category" = Option<String>, Query, description = "A Failure category"),
        ("q" = Option<String>, Query, description = "A literal substring of name, classname or file"),
        ("quarantined" = Option<bool>, Query, description = "The quarantine state to list"),
        ("has_issue" = Option<bool>, Query, description = "Whether a GitHub issue is linked"),
        ("new_within_days" = Option<i64>, Query, description = "Only Tests that became flaky in this many days, 1-90"),
        ("broken" = Option<bool>, Query, description = "Only Tests whose Fail streak reached the broken threshold")
    ),
    responses((status = 200, body = TestPage))
)]
pub async fn tests(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<TestsQuery>,
) -> RouteResult<Json<TestPage>> {
    let scope = scope_of(query.repo.clone(), query.project.clone())?;
    let (filters, options) = query.parts();
    Ok(Json(
        list_tests(
            state.pool(),
            &scope,
            &filters,
            &options,
            &read_settings(&state),
        )
        .await?,
    ))
}

/// The dashboard's headline counts.
///
/// # Errors
///
/// Answers 422 for a bad scope and 500 when a query fails.
#[utoipa::path(
    get,
    path = "/api/summary",
    tag = "reads",
    params(
        ("repo" = Option<String>, Query, description = "The Repo's owner/name"),
        ("project" = Option<String>, Query, description = "The Project's name, which needs a repo")
    ),
    responses((status = 200, body = SummaryOut))
)]
pub async fn test_summary(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<ScopeQuery>,
) -> RouteResult<Json<SummaryOut>> {
    let scope = scope_of(query.repo, query.project)?;
    Ok(Json(
        summary(state.pool(), &scope, &read_settings(&state)).await?,
    ))
}

/// One Test's detail page.
///
/// # Errors
///
/// Answers 404 for an id that names no Test, 422 for a bad limit or path, and 500
/// when a query fails.
#[utoipa::path(
    get,
    path = "/api/tests/{test_id}/history",
    tag = "reads",
    params(
        ("test_id" = i64, Path, description = "The Test's id"),
        ("limit" = Option<i64>, Query, description = "How many Executions to return, 1-500")
    ),
    responses(
        (status = 200, body = HistoryOut),
        (status = 404, description = "Test not found")
    )
)]
pub async fn test_history(
    State(state): State<AppState>,
    ApiPath(path): ApiPath<TestPath>,
    ApiQuery(query): ApiQuery<HistoryQuery>,
) -> RouteResult<Json<HistoryOut>> {
    let history = get_test(
        state.pool(),
        path.test_id,
        &read_settings(&state),
        query.executions_limit(),
    )
    .await?;
    match history {
        Some(history) => Ok(Json(history)),
        None => Err(RouteError::NotFound("Test not found".to_owned())),
    }
}

/// Stores a human's quarantine decision for one Test.
///
/// # Errors
///
/// Answers 404 for an id that names no Test, 422 for a body the schema refuses,
/// and 500 when a query fails.
#[utoipa::path(
    post,
    path = "/api/tests/{test_id}/quarantine",
    tag = "reads",
    params(("test_id" = i64, Path, description = "The Test's id")),
    request_body = QuarantineIn,
    responses(
        (status = 200, body = TestOut),
        (status = 404, description = "Test not found"),
        (status = 422, description = "A reason over 500 characters or an expiry outside 1-365 days")
    )
)]
pub async fn quarantine_test(
    State(state): State<AppState>,
    ApiPath(path): ApiPath<TestPath>,
    ApiJson(body): ApiJson<QuarantineIn>,
) -> RouteResult<Json<TestOut>> {
    let stored = set_quarantine(
        state.pool(),
        path.test_id,
        body.quarantined,
        body.reason.as_deref(),
        body.expires_in_days,
        &read_settings(&state),
    )
    .await?;
    match stored {
        Some(test) => Ok(Json(test)),
        None => Err(RouteError::NotFound("Test not found".to_owned())),
    }
}

/// For test runners: the Tests to skip in one Repo and Project.
///
/// # Errors
///
/// Answers 401 without the token, 422 for an unusable Repo or Project name, and
/// 500 when the query fails.
#[utoipa::path(
    get,
    path = "/api/quarantine",
    tag = "reads",
    params(
        ("x-api-key" = Option<String>, Header, description = "The shared token this instance requires"),
        ("repo" = String, Query, description = "The Repo's owner/name"),
        ("project" = Option<String>, Query, description = "The Project's name; default when omitted")
    ),
    responses((status = 200, body = Vec<QuarantineItem>))
)]
pub async fn quarantine(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<QuarantineQuery>,
) -> RouteResult<Json<Vec<QuarantineItem>>> {
    let repo = normalize_repo(&query.repo).map_err(|error| identity_error(&error))?;
    let project =
        normalize_project(query.project.as_deref()).map_err(|error| identity_error(&error))?;
    Ok(Json(quarantine_list(state.pool(), &repo, &project).await?))
}

/// The Jobs leaderboard.
///
/// # Errors
///
/// Answers 422 for a bad filter or range and 500 when a query fails.
#[utoipa::path(
    get,
    path = "/api/jobs",
    tag = "reads",
    params(
        ("repo" = Option<String>, Query, description = "The Repo's owner/name"),
        ("include_stable" = Option<bool>, Query, description = "Include Jobs with a zero score"),
        ("sort" = Option<String>, Query, description = "score, last_seen or proven"),
        ("page" = Option<i64>, Query, description = "The page number, from 1"),
        ("page_size" = Option<i64>, Query, description = "How many Jobs a page holds, 1-100"),
        ("q" = Option<String>, Query, description = "A literal substring of the Job or Pipeline name"),
        ("pipeline" = Option<String>, Query, description = "An exact Pipeline name"),
        ("has_issue" = Option<bool>, Query, description = "Whether a GitHub issue is linked"),
        ("new_within_days" = Option<i64>, Query, description = "Only Jobs that became flaky in this many days, 1-90")
    ),
    responses((status = 200, body = JobPage))
)]
pub async fn jobs(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<JobsQuery>,
) -> RouteResult<Json<JobPage>> {
    let repo = match &query.repo {
        Some(raw) => Some(normalize_repo(raw).map_err(|error| identity_error(&error))?),
        None => None,
    };
    let options = ListOptions {
        include_stable: query.include_stable.unwrap_or(false),
        flaky_only: false,
        sort: query
            .sort
            .as_deref()
            .and_then(SortKey::parse)
            .unwrap_or(SortKey::Score),
        page: query.page.unwrap_or(1),
        page_size: query.page_size.unwrap_or(50),
        q: query.q.clone(),
        has_issue: query.has_issue,
        new_within_days: query.new_within_days,
    };
    let filters = JobListFilters {
        pipeline: query.pipeline.clone(),
    };
    Ok(Json(
        list_jobs(
            state.pool(),
            repo.as_deref(),
            &filters,
            &options,
            &read_settings(&state),
        )
        .await?,
    ))
}

/// The Jobs dashboard's headline counts.
///
/// # Errors
///
/// Answers 422 for an unusable Repo name and 500 when a query fails.
#[utoipa::path(
    get,
    path = "/api/jobs/summary",
    tag = "reads",
    params(("repo" = Option<String>, Query, description = "The Repo's owner/name")),
    responses((status = 200, body = JobSummaryOut))
)]
pub async fn jobs_summary(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<ScopeQuery>,
) -> RouteResult<Json<JobSummaryOut>> {
    let repo = match query.repo {
        Some(raw) => Some(normalize_repo(&raw).map_err(|error| identity_error(&error))?),
        None => None,
    };
    Ok(Json(
        job_summary(state.pool(), repo.as_deref(), &read_settings(&state)).await?,
    ))
}

/// One Job's detail page.
///
/// # Errors
///
/// Answers 404 for an id that names no Job, 422 for a bad limit or path, and 500
/// when a query fails.
#[utoipa::path(
    get,
    path = "/api/jobs/{job_id}/history",
    tag = "reads",
    params(
        ("job_id" = i64, Path, description = "The Job's id"),
        ("limit" = Option<i64>, Query, description = "How many executions to return, 1-500")
    ),
    responses(
        (status = 200, body = JobHistoryOut),
        (status = 404, description = "Job not found")
    )
)]
pub async fn job_history(
    State(state): State<AppState>,
    ApiPath(path): ApiPath<JobPath>,
    ApiQuery(query): ApiQuery<HistoryQuery>,
) -> RouteResult<Json<JobHistoryOut>> {
    let history = get_job(
        state.pool(),
        path.job_id,
        &read_settings(&state),
        query.executions_limit(),
    )
    .await?;
    match history {
        Some(history) => Ok(Json(history)),
        None => Err(RouteError::NotFound("Job not found".to_owned())),
    }
}

/// The failing Tests and failed Jobs of one CI run attempt.
///
/// # Errors
///
/// Answers 404 for an unknown Repo, CI run or attempt, 422 for an unusable Repo
/// name or a path or attempt outside its rules, and 500 when a query fails.
#[utoipa::path(
    get,
    path = "/api/ci-runs/{ci_run_id}/failures",
    tag = "reads",
    params(
        ("ci_run_id" = String, Path, description = "The CI provider's run id"),
        ("repo" = String, Query, description = "The Repo's owner/name"),
        ("ci_run_attempt" = Option<i64>, Query, description = "Which attempt to read; the newest when absent")
    ),
    responses(
        (status = 200, body = CiRunFailuresOut),
        (status = 404, description = "Repo, CI run or CI run attempt not found")
    )
)]
pub async fn ci_run(
    State(state): State<AppState>,
    ApiPath(path): ApiPath<CiRunPath>,
    ApiQuery(query): ApiQuery<CiRunFailuresQuery>,
) -> RouteResult<Json<CiRunFailuresOut>> {
    // `/api/ci-runs//failures` never matched the frozen server's route, so it
    // answered the 404 body rather than FastAPI's `min_length=1` 422.
    if path.ci_run_id.is_empty() {
        return Err(RouteError::NotFound("Not Found".to_owned()));
    }
    let mut issues = Vec::new();
    let length = path.ci_run_id.chars().count();
    if length > 255 {
        issues.push(ValidationIssue::string_too_long(
            vec![
                flakeradar_core::LocSegment::Field("path".to_owned()),
                flakeradar_core::LocSegment::Field("ci_run_id".to_owned()),
            ],
            JsonValue::from(path.ci_run_id.clone()),
            255,
        ));
    }
    if !issues.is_empty() {
        return Err(ApiError::unprocessable(issues).into());
    }
    let repo = normalize_repo(&query.repo).map_err(|error| identity_error(&error))?;
    Ok(Json(
        ci_run_failures(
            state.pool(),
            &repo,
            &path.ci_run_id,
            query.ci_run_attempt,
            &read_settings(&state),
            FAILURES_LIMIT,
        )
        .await?,
    ))
}
