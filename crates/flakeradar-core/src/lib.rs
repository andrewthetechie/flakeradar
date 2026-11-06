//! Pure logic and shared DTOs for FlakeRadar.
//!
//! The crate holds the request and response DTOs that the database layer, the
//! HTTP server and the MCP server share, plus the scoring, parsing and
//! classification logic that needs neither a database nor a network. It depends
//! on no ORM, no web framework and no HTTP client, so every rule in it is
//! testable with plain values. Where each wire rule came from, and which corpus
//! pins it, is `docs/plans/rust-rewrite/wire-notes.md`.

// Rust guideline compliant 2026-07-21

#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        reason = "a unit test fails loudly when the rule under test is wrong"
    )
)]

pub mod classify;
pub mod defaults;
pub mod dto;
pub mod helpers;
pub mod identity;
pub mod lax;
pub mod limits;
pub mod model;
pub mod notify;
pub mod parsing;
pub mod py;
pub mod scoring;
pub mod types;
