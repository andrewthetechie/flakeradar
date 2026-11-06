//! Every route the server serves, in the order later tasks add them.
//!
//! [`router`] is what [`crate::build_app`] merges: the operations routes, the
//! API documentation, and from task 06 on the Report routes. A route group that
//! needs the shared token attaches
//! [`crate::middleware::require_token`] to its own subtree, so the read APIs stay
//! open.

// Rust guideline compliant 2026-07-21

pub mod admin;
pub mod issues;
pub mod ops;
pub mod reads;
pub mod reports;

use axum::Router;
use axum::routing::get;
use serde::Deserialize;

use crate::app::AppState;
use crate::{docs, mcp};

/// The path parameter of a Test route.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct TestPath {
    /// The Test's id.
    pub test_id: i64,
}

/// The path parameter of a Job route.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct JobPath {
    /// The Job's id.
    pub job_id: i64,
}

/// The app's routes, before the state is attached.
///
/// `state` is what a route group that needs the shared token attaches
/// [`crate::middleware::require_token`] with; the read APIs take none.
pub fn router(state: AppState) -> Router<AppState> {
    let router = Router::new()
        .route("/api/health", get(ops::health))
        .route("/api/health/ready", get(ops::ready))
        .route("/metrics", get(ops::metrics))
        .merge(reports::router(state.clone()))
        .merge(reads::router(state.clone()))
        .merge(issues::router(state.clone()))
        .merge(admin::router(state.clone()))
        .merge(mcp::router(state));
    docs::router(router)
}
