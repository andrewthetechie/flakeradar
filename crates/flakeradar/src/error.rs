//! The `detail` body every failed request carries, and the one error a route
//! answers with.
//!
//! A failure answers `{"detail": ...}`: a hand-written message for the errors the
//! routes raise themselves, and a list of issues for a request that failed
//! validation. Both consumers of the API read `detail`, so the shape stays
//! whatever else changes. A query that fails outside those cases is a plain-text
//! 500, written in one place here.

use std::error::Error;
use std::fmt;

use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use flakeradar_core::dto::ValidationIssue;
use flakeradar_db::issue_links::IssueLinkError;
use flakeradar_db::{DbError, DeleteError, issue_links::LinkError, reads::ReadError};
use serde::Serialize;

/// What a failed request says about itself.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Detail {
    /// One hand-written message, as `{"detail": "Report not found"}`.
    Text(String),
    /// The issues a request body or query failed validation with.
    Issues(Vec<ValidationIssue>),
}

/// The body of a failed response.
#[derive(Debug, Serialize)]
struct ErrorBody {
    /// The message or the issues.
    detail: Detail,
}

/// A response that fails with a `detail` body.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    detail: Detail,
}

/// A handler's result.
pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    /// An error with `status` and one hand-written message.
    #[must_use]
    pub fn new(status: StatusCode, detail: impl Into<String>) -> Self {
        Self {
            status,
            detail: Detail::Text(detail.into()),
        }
    }

    /// The 422 a request that failed its own range or length rules gets.
    #[must_use]
    pub fn unprocessable(issues: Vec<ValidationIssue>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            detail: Detail::Issues(issues),
        }
    }

    /// The status code the response carries.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// What the response says about itself.
    #[must_use]
    pub fn detail(&self) -> &Detail {
        &self.detail
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                detail: self.detail,
            }),
        )
            .into_response()
    }
}

/// A route's answer, when it can fail.
pub type RouteResult<T> = Result<T, RouteError>;

/// The ways a route can fail.
///
/// One type for every route: the status and body a failure answers with are
/// decided here, so no route module carries its own copy of the plain-text 500.
#[derive(Debug)]
pub enum RouteError {
    /// A request the route rejected, in the usual `detail` shape.
    Rejected(ApiError),
    /// A resource an id names is unknown; the text is the 404 detail.
    NotFound(String),
    /// The link rules refused the request, with the status they give it.
    Refused(IssueLinkError),
    /// The delete raced the processor and the caller may retry.
    Raced,
    /// A database call the route did not expect to fail.
    Internal(DbError),
}

impl RouteError {
    /// The status code the response carries.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Rejected(error) => error.status(),
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Refused(error) => refused_status(error),
            Self::Raced => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(error) => write!(f, "a rejected request, {}", error.status()),
            Self::NotFound(detail) => f.write_str(detail),
            Self::Refused(error) => f.write_str(error.detail()),
            Self::Raced => f.write_str("the delete raced the report processor"),
            Self::Internal(error) => write!(f, "{error}"),
        }
    }
}

impl Error for RouteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Internal(error) => Some(error),
            Self::Rejected(_) | Self::NotFound(_) | Self::Refused(_) | Self::Raced => None,
        }
    }
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        match self {
            Self::Rejected(error) => error.into_response(),
            Self::NotFound(detail) => ApiError::new(StatusCode::NOT_FOUND, detail).into_response(),
            Self::Refused(error) => {
                ApiError::new(refused_status(&error), error.detail().to_owned()).into_response()
            }
            Self::Raced => ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Delete raced with the report processor; retry.",
            )
            .into_response(),
            Self::Internal(error) => {
                // A database error names a statement, never what a caller asked for.
                tracing::error!(
                    name: "http.route.failed",
                    error = %error,
                    "a route's query failed",
                );
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

/// The 422 an identity rule refuses a Repo, Project or root name with.
///
/// A name the identity rules reject is bad input on every surface that takes one,
/// so both route groups answer the same `{"detail": "<rule>"}` body from this one
/// function rather than each keeping its own copy.
pub(crate) fn identity_error(error: &flakeradar_core::identity::IdentityError) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, error.to_string())
}

/// The status an Issue-link refusal answers with.
///
/// The database layer states the refusal's kind and its text; which HTTP status a
/// kind becomes belongs to the front door, so the mapping lives here.
fn refused_status(error: &IssueLinkError) -> StatusCode {
    match error {
        IssueLinkError::NotFound(_) => StatusCode::NOT_FOUND,
        IssueLinkError::Conflict(_) => StatusCode::CONFLICT,
        IssueLinkError::NotLinkable(_) => StatusCode::UNPROCESSABLE_ENTITY,
    }
}

impl From<ApiError> for RouteError {
    fn from(error: ApiError) -> Self {
        Self::Rejected(error)
    }
}

impl From<DbError> for RouteError {
    fn from(error: DbError) -> Self {
        Self::Internal(error)
    }
}

impl From<sea_orm::DbErr> for RouteError {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::Internal(DbError::Database(error))
    }
}

impl From<ReadError> for RouteError {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::Database(error) => Self::Internal(error),
            ReadError::NotFound(detail) => Self::NotFound(detail),
        }
    }
}

impl From<LinkError> for RouteError {
    fn from(error: LinkError) -> Self {
        match error {
            LinkError::Refused(error) => Self::Refused(error),
            LinkError::Database(error) => Self::Internal(error),
        }
    }
}

impl From<DeleteError> for RouteError {
    fn from(error: DeleteError) -> Self {
        match error {
            DeleteError::Deadlock => Self::Raced,
            DeleteError::Database(error) => Self::Internal(error),
        }
    }
}
