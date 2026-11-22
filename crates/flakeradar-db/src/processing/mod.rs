//! The Report processor: one pending Report becomes a Run or Job executions,
//! and Tests and Jobs are rescored.
//!
//! Scoring depends on the order Executions ran in, so one Report is processed at
//! a time, and every bulk statement keeps a shape whose row order is the report's
//! own: a rewritten statement that reordered rows would move served scores.
//! `tests/parity` replays the recorded answers.

// Rust guideline compliant 2026-07-21

mod events;
mod history;
mod intake;
mod rescore;

pub use history::DayCounts;
#[expect(
    unused_imports,
    reason = "ProcessError is `pub(crate)`, so only this module's own users could name it; the re-export keeps the path it had before the split"
)]
pub(crate) use intake::ProcessError;
pub use intake::{ERROR_MAX, ProcessOutcome, ProcessingSettings, claim_next_report, process_next};
pub use rescore::{JobHistory, job_history, rescore};
