//! The `detail` body every failed request carries.
//!
//! FastAPI renders a failure as `{"detail": ...}`: a hand-written message for
//! the errors the routes raise themselves, and a list of issues for a request
//! that failed validation. Both consumers of the API read `detail`, so the shape
//! stays whatever else changes.

// Rust guideline compliant 2026-07-21

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use flakeradar_core::ValidationIssue;
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
