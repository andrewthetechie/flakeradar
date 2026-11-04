//! Pure logic and shared DTOs for FlakeRadar.
//!
//! The crate holds the request and response DTOs that the database layer, the
//! HTTP server and the MCP server share, plus the scoring, parsing and
//! classification logic that needs neither a database nor a network. It depends
//! on no ORM, no web framework and no HTTP client, so every rule in it is
//! testable with plain values.

// Rust guideline compliant 2026-07-21
