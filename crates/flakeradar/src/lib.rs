//! FlakeRadar server: the Axum routes, the MCP server, the background workers,
//! the GitHub and webhook clients, and the embedded frontend.
//!
//! The library holds everything the binary runs, so the routes and workers are
//! reachable from integration tests without starting a process.
//! [`app::build_app`] assembles the router over a [`flakeradar_db::connect()`]
//! pool and an [`app::AppState`]; [`server::Server`] adds the migration, the
//! socket and the shutdown, and [`settings::Settings`] is the configuration both
//! run with.

// Rust guideline compliant 2026-07-21

// cargo-llvm-cov sets `coverage_nightly` on nightly, where the `coverage`
// attribute is still experimental. `telemetry::init_tracing` is the only item
// that carries one; see `docs/plans/rust-rewrite/coverage-gaps.md`.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        clippy::panic,
        reason = "a unit test fails loudly when the rule under test is wrong"
    )
)]

pub mod app;
pub mod assets;
pub mod cli;
pub mod docs;
pub mod error;
pub mod extract;
pub mod github;
pub mod github_worker;
pub mod mcp;
pub mod middleware;
pub mod multipart;
pub mod queue;
pub mod routes;
pub mod server;
pub mod settings;
mod telemetry;
pub mod webhooks;
pub mod worker;

#[doc(inline)]
pub use telemetry::init_tracing;
