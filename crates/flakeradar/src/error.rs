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

#[cfg(test)]
mod tests {
    //! The two bodies a failed request can carry.

    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::IntoResponse as _;
    use serde_json::{Value, json};

    use super::ApiError;

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
}
