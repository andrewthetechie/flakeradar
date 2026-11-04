//! FlakeRadar server: the Axum routes, the MCP server, the background workers,
//! the GitHub and webhook clients, and the embedded frontend.
//!
//! The library holds everything the binary runs, so the routes and workers are
//! reachable from integration tests without starting a process. [`build_app`]
//! assembles the router over a [`flakeradar_db::connect`] pool and an
//! [`AppState`]; [`Settings`] is the configuration it runs with.

// Rust guideline compliant 2026-07-21

#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        clippy::panic,
        reason = "a unit test fails loudly when the rule under test is wrong"
    )
)]

pub mod settings;
mod telemetry;

#[doc(inline)]
pub use settings::{Secret, Settings, SettingsError, check_api_token, is_default_token};
#[doc(inline)]
pub use telemetry::init_tracing;
