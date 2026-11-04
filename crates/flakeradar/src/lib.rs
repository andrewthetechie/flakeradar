//! FlakeRadar server: the Axum routes, the MCP server, the background workers,
//! the GitHub and webhook clients, and the embedded frontend.
//!
//! The library holds everything the binary runs, so the routes and workers are
//! reachable from integration tests without starting a process.

// Rust guideline compliant 2026-07-21
