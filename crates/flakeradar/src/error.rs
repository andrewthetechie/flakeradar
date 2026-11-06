//! The `detail` body every failed request carries, and the one error a route
//! answers with.
//!
//! FastAPI renders a failure as `{"detail": ...}`: a hand-written message for
//! the errors the routes raise themselves, and a list of issues for a request
//! that failed validation. Both consumers of the API read `detail`, so the shape
//! stays whatever else changes. A query that fails outside those cases is the
//! plain-text 500 Starlette answered with, written in one place here.

// Rust guideline compliant 2026-07-21

use std::error::Error;
use std::fmt;

use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use flakeradar_core::ValidationIssue;
use flakeradar_db::issue_links::IssueLinkError;
use flakeradar_db::{DbError, DeleteError, LinkError, ReadError};
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
                tracing::error!(error = %error, "a route's query failed");
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

#[cfg(test)]
mod tests {
    //! The two bodies a failed request can carry.

    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::IntoResponse as _;
    use serde_json::{Value, json};

    use std::error::Error as _;

    use super::{ApiError, RouteError};

    /// The status and the body of a rendered error.
    async fn rendered(error: ApiError) -> (StatusCode, Value) {
        let response = error.into_response();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body reads to the end");
        let body = serde_json::from_slice(&bytes).expect("the body is JSON");
        (status, body)
    }

    #[tokio::test]
    async fn a_hand_written_error_carries_its_status_and_its_message() {
        let error = ApiError::new(StatusCode::NOT_FOUND, "Report not found");
        assert_eq!(error.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            serde_json::to_value(error.detail()).expect("the detail serializes"),
            json!("Report not found")
        );
        let (status, body) = rendered(error).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, json!({"detail": "Report not found"}));
    }

    #[tokio::test]
    async fn a_validation_error_is_a_422_with_a_list_of_issues() {
        let issue = flakeradar_core::ValidationIssue::int_parsing(
            vec![flakeradar_core::LocSegment::Field("limit".to_owned())],
            json!("abc"),
        );
        let error = ApiError::unprocessable(vec![issue]);
        assert_eq!(error.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let (status, body) = rendered(error).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["detail"][0]["type"], json!("int_parsing"));
        assert_eq!(body["detail"][0]["loc"], json!(["limit"]));
    }

    #[test]
    fn a_route_error_is_an_error_that_names_its_case() {
        let database =
            flakeradar_db::DbError::Database(sea_orm::DbErr::RecordNotFound("a row".to_owned()));
        let internal = RouteError::Internal(database);
        assert!(internal.to_string().contains("a row"));
        let boxed: Box<dyn std::error::Error> = Box::new(internal);
        assert!(
            boxed.source().is_some(),
            "the database failure is the source"
        );
        assert_eq!(
            RouteError::NotFound("Test not found".to_owned()).to_string(),
            "Test not found"
        );
        assert_eq!(RouteError::Raced.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            RouteError::NotFound("Test not found".to_owned()).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            RouteError::Internal(flakeradar_db::DbError::Database(
                sea_orm::DbErr::RecordNotFound("a row".to_owned())
            ))
            .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            RouteError::Raced.to_string(),
            "the delete raced the report processor"
        );
        let rejected = RouteError::Rejected(ApiError::new(StatusCode::BAD_REQUEST, "bad body"));
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        assert_eq!(rejected.to_string(), "a rejected request, 400 Bad Request");
        assert!(rejected.source().is_none(), "a rejection wraps no error");
        assert_eq!(
            RouteError::Refused(flakeradar_db::issue_links::IssueLinkError::NotLinkable(
                "not linkable".to_owned()
            ))
            .to_string(),
            "not linkable"
        );
        let read = RouteError::from(flakeradar_db::ReadError::Database(
            flakeradar_db::DbError::Database(sea_orm::DbErr::RecordNotFound("a row".to_owned())),
        ));
        assert!(read.source().is_some());
        assert_eq!(
            RouteError::Refused(flakeradar_db::issue_links::IssueLinkError::Conflict(
                "already linked".to_owned()
            ))
            .status(),
            StatusCode::CONFLICT
        );
    }

    #[tokio::test]
    async fn an_internal_route_error_is_the_plain_text_500() {
        let error = RouteError::Internal(flakeradar_db::DbError::Database(
            sea_orm::DbErr::RecordNotFound("a row".to_owned()),
        ));
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .expect("a content type"),
            "text/plain; charset=utf-8"
        );
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body reads to the end");
        assert_eq!(bytes.as_ref(), b"Internal Server Error");
    }
}
