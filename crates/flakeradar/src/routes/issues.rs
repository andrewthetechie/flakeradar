//! The Issue link routes: link and unlink a GitHub issue on a Test or a Job.
//!
//! Both write routes are token-protected. The rules and the response shape live
//! in `flakeradar_db::issue_links`, so the MCP tools of task 11 reuse the same
//! text, and the route only maps a refusal to its status.

// Rust guideline compliant 2026-07-21

use axum::Json;
use axum::extract::State;
use axum::routing::put;
use axum::{Router, middleware};
use flakeradar_core::dto::lax_bool;
use flakeradar_core::{IssueLinkIn, JobOut, TestOut};
use flakeradar_db::issue_links::{self, IssueLinkError, LinkTarget, LinkedOut};

use serde::Deserialize;

use crate::app::AppState;
use crate::error::{ApiResult, RouteError, RouteResult};
use crate::extract::{ApiJson, ApiPath, ApiQuery, Validate};
use crate::middleware::require_token;
use crate::routes::{JobPath, TestPath};

/// The two Issue link routes of a Test or a Job.
pub fn router(state: AppState) -> Router<AppState> {
    let token = middleware::from_fn_with_state(state, require_token);
    Router::new()
        .route(
            "/api/tests/{test_id}/issue",
            put(link_test).delete(unlink_test),
        )
        .route("/api/jobs/{job_id}/issue", put(link_job).delete(unlink_job))
        .layer(token)
}

/// The query parameter the link routes take.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct LinkQuery {
    /// Whether to replace a different issue the entity already links.
    #[serde(default, deserialize_with = "lax_bool::optional")]
    replace: Option<bool>,
}

impl Validate for LinkQuery {
    fn validate(&self) -> ApiResult<()> {
        Ok(())
    }
}

/// Links a GitHub issue to a Test.
///
/// # Errors
///
/// Answers 401 without the token, 404 for an unknown Test, 409 when another
/// issue is linked, 422 when a link rule fails, and 500 when a query fails.
#[utoipa::path(
    put,
    path = "/api/tests/{test_id}/issue",
    tag = "issues",
    params(
        ("x-api-key" = Option<String>, Header, description = "The shared token this instance requires"),
        ("test_id" = i64, Path, description = "The Test's id"),
        ("replace" = Option<bool>, Query, description = "Replace a different linked issue")
    ),
    request_body = IssueLinkIn,
    responses(
        (status = 200, body = TestOut),
        (status = 404, description = "Test not found"),
        (status = 409, description = "Test already links another issue"),
        (status = 422, description = "The number or url breaks a link rule")
    )
)]
pub async fn link_test(
    State(state): State<AppState>,
    ApiPath(path): ApiPath<TestPath>,
    ApiQuery(query): ApiQuery<LinkQuery>,
    ApiJson(body): ApiJson<IssueLinkIn>,
) -> RouteResult<Json<TestOut>> {
    let out = issue_links::link_issue(
        state.pool(),
        LinkTarget::Test,
        path.test_id,
        body.number,
        body.url.as_deref(),
        query.replace.unwrap_or(false),
        &state.settings().reads(),
    )
    .await?;
    match out {
        LinkedOut::Test(test) => Ok(Json(test)),
        LinkedOut::Job(_) => Err(RouteError::Refused(IssueLinkError::NotFound(
            "Test not found".to_owned(),
        ))),
    }
}

/// Removes a Test's Issue link.
///
/// # Errors
///
/// Answers 401 without the token, 404 for an unknown Test, and 500 when a query
/// fails.
#[utoipa::path(
    delete,
    path = "/api/tests/{test_id}/issue",
    tag = "issues",
    params((
        "x-api-key" = Option<String>, Header, description = "The shared token this instance requires"
    ), ("test_id" = i64, Path, description = "The Test's id")),
    responses(
        (status = 200, body = TestOut),
        (status = 404, description = "Test not found")
    )
)]
pub async fn unlink_test(
    State(state): State<AppState>,
    ApiPath(path): ApiPath<TestPath>,
) -> RouteResult<Json<TestOut>> {
    let out = issue_links::unlink_issue(
        state.pool(),
        LinkTarget::Test,
        path.test_id,
        &state.settings().reads(),
    )
    .await?;
    match out {
        LinkedOut::Test(test) => Ok(Json(test)),
        LinkedOut::Job(_) => Err(RouteError::Refused(IssueLinkError::NotFound(
            "Test not found".to_owned(),
        ))),
    }
}

/// Links a GitHub issue to a Job.
///
/// # Errors
///
/// Answers 401 without the token, 404 for an unknown Job, 409 when another
/// issue is linked, 422 when the Pipeline is not a GitHub one or another rule
/// fails, and 500 when a query fails.
#[utoipa::path(
    put,
    path = "/api/jobs/{job_id}/issue",
    tag = "issues",
    params(
        ("x-api-key" = Option<String>, Header, description = "The shared token this instance requires"),
        ("job_id" = i64, Path, description = "The Job's id"),
        ("replace" = Option<bool>, Query, description = "Replace a different linked issue")
    ),
    request_body = IssueLinkIn,
    responses(
        (status = 200, body = JobOut),
        (status = 404, description = "Job not found"),
        (status = 409, description = "Job already links another issue"),
        (status = 422, description = "Only a GitHub Pipeline's Job can link an issue")
    )
)]
pub async fn link_job(
    State(state): State<AppState>,
    ApiPath(path): ApiPath<JobPath>,
    ApiQuery(query): ApiQuery<LinkQuery>,
    ApiJson(body): ApiJson<IssueLinkIn>,
) -> RouteResult<Json<JobOut>> {
    let out = issue_links::link_issue(
        state.pool(),
        LinkTarget::Job,
        path.job_id,
        body.number,
        body.url.as_deref(),
        query.replace.unwrap_or(false),
        &state.settings().reads(),
    )
    .await?;
    match out {
        LinkedOut::Job(job) => Ok(Json(job)),
        LinkedOut::Test(_) => Err(RouteError::Refused(IssueLinkError::NotFound(
            "Job not found".to_owned(),
        ))),
    }
}

/// Removes a Job's Issue link.
///
/// # Errors
///
/// Answers 401 without the token, 404 for an unknown Job, and 500 when a query
/// fails.
#[utoipa::path(
    delete,
    path = "/api/jobs/{job_id}/issue",
    tag = "issues",
    params((
        "x-api-key" = Option<String>, Header, description = "The shared token this instance requires"
    ), ("job_id" = i64, Path, description = "The Job's id")),
    responses(
        (status = 200, body = JobOut),
        (status = 404, description = "Job not found")
    )
)]
pub async fn unlink_job(
    State(state): State<AppState>,
    ApiPath(path): ApiPath<JobPath>,
) -> RouteResult<Json<JobOut>> {
    let out = issue_links::unlink_issue(
        state.pool(),
        LinkTarget::Job,
        path.job_id,
        &state.settings().reads(),
    )
    .await?;
    match out {
        LinkedOut::Job(job) => Ok(Json(job)),
        LinkedOut::Test(_) => Err(RouteError::Refused(IssueLinkError::NotFound(
            "Job not found".to_owned(),
        ))),
    }
}
