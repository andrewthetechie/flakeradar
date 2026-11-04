//! Every route the server serves, in the order later tasks add them.
//!
//! [`router`] is what [`crate::build_app`] merges: the operations routes, the
//! API documentation, and from task 06 on the Report routes. A route group that
//! needs the shared token attaches
//! [`crate::middleware::require_token`] to its own subtree, so the read APIs stay
//! open.

// Rust guideline compliant 2026-07-21

pub mod ops;

use axum::Router;
use axum::routing::get;

use crate::app::AppState;
use crate::docs;

/// The app's routes, before the state is attached.
pub fn router() -> Router<AppState> {
    let router = Router::new()
        .route("/api/health", get(ops::health))
        .route("/api/health/ready", get(ops::ready))
        .route("/metrics", get(ops::metrics));
    docs::router(router)
}
