//! The GitHub issue automation: filing, the filing gate, the sync and the notes.
//!
//! Every GitHub call goes to a stub server, so a test asserts the request it made
//! — method, path, body, headers — and never that a function was called.
//!
//! One module per file of `flakeradar::github`: `client`, `policy`, `bodies`,
//! `filing`, `sync` and `notes`, plus `hook` for the `ReportWorker` hook that
//! composes filing and noting, and `support` for the fixtures they share.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a stub or an answer does not behave"
)]

mod bodies;
mod client;
mod filing;
mod hook;
mod notes;
mod policy;
mod support;
mod sync;
