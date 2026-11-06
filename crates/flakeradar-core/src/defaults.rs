//! The shipped value of every setting more than one layer reads.
//!
//! A default that is written in four places is a default that silently
//! disagrees with itself in three: `Settings`, the processor's settings, the read
//! layer's settings and retention each restated `0.30`, `50` and `5`. Each value
//! below is written once and read by all of them, and each doc comment says why
//! that number and what a change costs.
//!
//! Every value is the frozen Python server's `config.py` default, so an install
//! that sets nothing keeps the scores, tiers and retention it had.

// Rust guideline compliant 2026-07-21

/// How many recent Executions the Flakiness score weighs.
///
/// Fifty Executions is a few weeks of a busy Test and a few months of a quiet
/// one: long enough that one flip does not make a Test flaky, short enough that a
/// fix shows up in the window. Every served score, and so every tier, depends on
/// it.
pub const SCORE_WINDOW: usize = 50;

/// The per-step weight decay, so the newest flip counts most.
///
/// `0.85` halves a flip's weight about four steps into the past. Larger values
/// weigh old flips almost as much as new ones, so a Test that has stopped
/// flipping keeps its score; smaller ones forget a Test's history within a few
/// runs. Every served score depends on it.
pub const SCORE_DECAY: f64 = 0.85;

/// Consecutive Default-branch failures at which a Test counts as broken.
///
/// A Test that fails five times in a row on the Default branch has not fixed
/// itself, so it is broken rather than flaky (see `CONTEXT.md`); `0` disables the
/// flag. Reading it anywhere but on the Default branch is not the same question,
/// so the fail streak it compares against is branch-scoped.
pub const BROKEN_STREAK: i64 = 5;

/// The Flakiness score at or above which a Test or Job counts as flaky.
///
/// The leaderboard's tier filter, the `flaky` field of every read DTO, the GitHub
/// filing gate and the Tier changes that produce Notifications all read it, so a
/// change here moves Tests between tiers and announces the move.
pub const FLAKE_THRESHOLD: f64 = 0.30;

/// How many times a Report may be re-processed after a transient failure.
///
/// Three attempts ride out a short database outage; a larger number keeps a
/// permanently bad Report re-entering the queue, and a smaller one marks a Report
/// failed during a blip.
pub const MAX_REPORT_RETRIES: i32 = 3;

/// Seconds a statement may run before PostgreSQL cancels it.
///
/// A hung statement holds a pooled connection until the client gives up, so the
/// server cancels it instead. Thirty seconds is far longer than any statement
/// this server issues and far shorter than a request timeout.
pub const DB_COMMAND_TIMEOUT_SECONDS: f64 = 30.0;

/// How many days a processed Report is kept.
///
/// A processed Report is the audit trail of one upload: a week answers "what did
/// CI send yesterday" and keeps the table small enough to prune in one batch.
pub const REPORT_RETENTION_DAYS: i64 = 7;

/// How many days a failed Report is kept; `0` keeps it until a human acts.
///
/// A failed Report is work: the queue indicator shows it until someone retries or
/// dismisses it, so the default never deletes one.
pub const FAILED_REPORT_RETENTION_DAYS: i64 = 0;

/// How many days Executions, and Runs left empty, are kept.
///
/// Ninety days is the history the leaderboard, the Score history and the trend
/// arrows are computed from; pruning sooner empties them, pruning later grows the
/// largest tables in the schema.
pub const EXECUTION_RETENTION_DAYS: i64 = 90;

/// How many days a Score history row is kept.
///
/// A year of daily rows is what makes "was this Test flaky a year ago" answerable
/// (ADR 0007); the table grows by one row per Test or Job per day.
pub const SCORE_HISTORY_RETENTION_DAYS: i64 = 365;

/// How many rows one prune round deletes per table.
///
/// Bounded batches keep every `DELETE` well under the statement timeout even when
/// a large backlog has built up; a batch larger than this is one statement that
/// can be cancelled, and a batch of 0 deletes nothing and spins the run.
pub const PRUNE_BATCH: i64 = 5000;

/// The wall-clock budget of one prune run, in seconds.
///
/// A run stops after this and the next interval resumes where the committed
/// batches left off, so a large backlog drains over several hours instead of
/// holding one transaction open.
pub const PRUNE_TIME_BUDGET_SECONDS: f64 = 60.0;
