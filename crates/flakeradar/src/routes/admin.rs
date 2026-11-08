//! The admin delete routes: one Repo, or one Project of a Repo.
//!
//! Both are token-protected and have no UI and no MCP tool. The delete itself is
//! `flakeradar_db::deletes`; this module names the Repo and Project, maps the
//! refusals, and closes the stranded issues after the commit — best effort, so a
//! slow or failing GitHub API neither holds the transaction nor changes the
//! answer.

// Rust guideline compliant 2026-07-21

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::delete;
use axum::{Router, middleware};
use flakeradar_core::{
    dto::ProjectDeleteOut, dto::RepoDeleteOut, identity::normalize_project,
    identity::normalize_repo,
};
use flakeradar_db::ProjectDelete;
use serde::Deserialize;

use crate::app::AppState;
use crate::error::{ApiError, RouteResult, identity_error};
use crate::extract::ApiPath;
use crate::middleware::require_token;

/// The admin delete routes.
pub fn router(state: AppState) -> Router<AppState> {
    let token = middleware::from_fn_with_state(state, require_token);
    Router::new()
        .route("/api/repos/{owner}/{name}", delete(delete_repo))
        .route(
            "/api/repos/{owner}/{name}/projects/{project}",
            delete(delete_project),
        )
        .layer(token)
}

/// The path parameters of the Repo delete.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoPath {
    /// The Repo's owner.
    owner: String,
    /// The Repo's name.
    name: String,
}

/// The path parameters of the Project delete.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectPath {
    /// The Repo's owner.
    owner: String,
    /// The Repo's name.
    name: String,
    /// The Project's name.
    project: String,
}

/// Deletes a Repo and, by cascade, everything in it.
///
/// # Errors
///
/// Answers 401 without the token, 404 for an unknown Repo, 422 for a name the
/// identity rules refuse, and 503 when PostgreSQL aborted the delete for a lock
/// cycle with the processor.
#[utoipa::path(
    delete,
    path = "/api/repos/{owner}/{name}",
    tag = "admin",
    params(
        ("x-api-key" = Option<String>, Header, description = "The shared token this instance requires"),
        ("owner" = String, Path, description = "The Repo's owner"),
        ("name" = String, Path, description = "The Repo's name")
    ),
    responses(
        (status = 200, body = RepoDeleteOut),
        (status = 404, description = "Repo not found"),
        (status = 503, description = "The delete raced the report processor; retry")
    )
)]
pub async fn delete_repo(
    State(state): State<AppState>,
    ApiPath(path): ApiPath<RepoPath>,
) -> RouteResult<Json<RepoDeleteOut>> {
    let name = normalize_repo(&format!("{}/{}", path.owner, path.name))
        .map_err(|error| identity_error(&error))?;
    let Some(deleted) = flakeradar_db::delete_repo(state.pool(), &name).await? else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "Repo not found").into());
    };
    let _ = crate::github::close_issues_after_removal(
        &deleted.out.repo,
        &deleted.filed,
        state.github(),
    )
    .await;
    Ok(Json(deleted.out))
}

/// Deletes one Project of a Repo and, by cascade, its Tests, Runs and Reports.
///
/// # Errors
///
/// Answers 401 without the token, 404 for an unknown Repo or Project, 422 for a
/// name the identity rules refuse, and 503 when PostgreSQL aborted the delete
/// for a lock cycle with the processor.
#[utoipa::path(
    delete,
    path = "/api/repos/{owner}/{name}/projects/{project}",
    tag = "admin",
    params(
        ("x-api-key" = Option<String>, Header, description = "The shared token this instance requires"),
        ("owner" = String, Path, description = "The Repo's owner"),
        ("name" = String, Path, description = "The Repo's name"),
        ("project" = String, Path, description = "The Project's name")
    ),
    responses(
        (status = 200, body = ProjectDeleteOut),
        (status = 404, description = "Repo or Project not found"),
        (status = 503, description = "The delete raced the report processor; retry")
    )
)]
pub async fn delete_project(
    State(state): State<AppState>,
    ApiPath(path): ApiPath<ProjectPath>,
) -> RouteResult<Json<ProjectDeleteOut>> {
    let repo = normalize_repo(&format!("{}/{}", path.owner, path.name))
        .map_err(|error| identity_error(&error))?;
    let project = normalize_project(Some(&path.project)).map_err(|error| identity_error(&error))?;
    let deleted = match flakeradar_db::delete_project(state.pool(), &repo, &project).await? {
        ProjectDelete::Deleted(deleted) => deleted,
        ProjectDelete::RepoNotFound => {
            return Err(ApiError::new(StatusCode::NOT_FOUND, "Repo not found").into());
        }
        ProjectDelete::ProjectNotFound => {
            return Err(ApiError::new(StatusCode::NOT_FOUND, "Project not found").into());
        }
    };
    let _ = crate::github::close_issues_after_removal(
        &deleted.out.repo,
        &deleted.filed,
        state.github(),
    )
    .await;
    Ok(Json(deleted.out))
}
