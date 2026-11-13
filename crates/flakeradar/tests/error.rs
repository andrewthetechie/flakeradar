//! The two bodies a failed request can carry: the JSON `detail` and the plain text.
//!
//! Every route answers a failure through one of these, so a test renders the
//! response and reads the status and the body back.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a rendered response is not the body it names"
)]

use axum::body::to_bytes;
use axum::http::StatusCode;
use axum::response::IntoResponse as _;
use serde_json::{Value, json};

use std::error::Error as _;

use flakeradar::error::{ApiError, RouteError};

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
    let issue = flakeradar_core::dto::ValidationIssue::int_parsing(
        vec![flakeradar_core::dto::LocSegment::Field("limit".to_owned())],
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
    let read = RouteError::from(flakeradar_db::reads::ReadError::Database(
        flakeradar_db::DbError::Database(sea_orm::DbErr::RecordNotFound("a row".to_owned())),
    ));
    assert!(read.source().is_some());
    // A driver error that reaches a route directly, rather than through a crate's
    // own error, is the same 500: only a delete's Deadlock says retry.
    let driver = RouteError::from(sea_orm::DbErr::Query(sea_orm::RuntimeErr::Internal(
        "the driver gave up".to_owned(),
    )));
    assert!(
        matches!(
            driver,
            RouteError::Internal(flakeradar_db::DbError::Database(_))
        ),
        "a driver error is the internal case: {driver}"
    );
    assert_eq!(driver.status(), StatusCode::INTERNAL_SERVER_ERROR);
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
