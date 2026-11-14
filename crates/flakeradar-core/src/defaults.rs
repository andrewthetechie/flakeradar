//! The shipped value of every setting the server starts with.
//!
//! A default that is written in four places is a default that silently
//! disagrees with itself in three: `Settings`, the processor's settings, the read
//! layer's settings and retention each restated `0.30`, `50` and `5`. Each value
//! below is written once and read by all of them, and each doc comment says why
//! that number, what a change costs and what reads it.
//!
//! Every value is the shipped default, so an install that sets nothing gets these
//! scores, tiers and retention windows. `docs/configuration.md` and `.env.example` state the
//! same numbers, and `every_setting_defaults_to_the_documented_value` pins them.

// Rust guideline compliant 2026-07-21

use std::time::Duration;

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

/// The Flakiness score below which a Test or Job that counts as flaky recovers.
///
/// It sits below [`FLAKE_THRESHOLD`] so a score near the boundary stops crossing
/// back and forth with each Report: between the two thresholds a Test keeps the
/// state it already has, which is what keeps a Tier change — and the Notification
/// it announces — from firing on every flip. Raising it towards the Flake threshold
/// makes the band narrow and the announcements frequent again; at or above it, the
/// state can never be held. `FLAKERADAR_RECOVER_THRESHOLD` sets it.
pub const RECOVER_THRESHOLD: f64 = 0.20;

/// The share of a Run's Tests that must fail for the Run to count as suspected
/// infrastructure.
///
/// Most of a large Run failing at once has one cause — the runner, the network, a
/// shared service — rather than one cause per Test, and four fifths is where that
/// reading beats the literal one: a regression rarely reaches that far, and requiring
/// every Test to fail would miss a runner that broke halfway through. The processor's
/// marking rule compares a Run's failing-Test share to it and leaves a Run at or
/// above it out of scoring. Lowering it writes a real regression off as a bad runner;
/// raising it lets one bad runner move every Test it touched.
/// `FLAKERADAR_INFRA_FAIL_SHARE` sets it.
pub const INFRA_FAIL_SHARE: f64 = 0.8;

/// The fewest distinct Tests a Run must hold to be judged for suspected
/// infrastructure at all.
///
/// The share is meaningless on a small Run: a 3-Test Report that fails entirely is a
/// broken Test, and a floor of 1 would call it infrastructure. Twenty distinct Tests
/// is large enough that a shared cause is the simpler explanation when most of them
/// fail together. The processor's marking rule asks this of a Run before it asks the
/// share. Lowering the floor writes a small suite's real outage off as infrastructure;
/// raising it leaves a mass failure scored, so the Tests it touched look flaky or
/// broken. `FLAKERADAR_INFRA_MIN_TESTS` sets it.
pub const INFRA_MIN_TESTS: i64 = 20;

/// The Flakiness score a Test or Job must reach before FlakeRadar files an Issue.
///
/// It ships at [`FLAKE_THRESHOLD`]'s value on purpose: an Issue is the record of a
/// flake the leaderboard already calls flaky, so the two start as one question. It
/// is a separate setting because an operator who wants fewer Issues can raise the
/// filing gate without moving the tier boundary; the shipped `github` parity
/// scenario files at 0.5 for that reason. Lowering it files Issues for Suspect
/// rows; raising it leaves Flaky tests untracked. The filing gate reads it, and
/// `FLAKERADAR_GITHUB_ISSUE_MIN_SCORE` sets it.
pub const GITHUB_ISSUE_MIN_SCORE: f64 = 0.30;

/// How many linked Tests, and separately linked Jobs, one maintenance sync asks
/// GitHub about.
///
/// The sync rotates: the rows checked longest ago go first, so 2000 is how much of
/// that queue one run drains, and a deployment with thousands of linked Tests
/// still reaches every row within a few runs. A smaller batch leaves rows
/// unanswered for longer, so a closed Issue is kept longer; a larger one is more
/// requests per run against GitHub's secondary rate limit, which stops the batch.
/// `github::sync` reads it and `FLAKERADAR_GITHUB_SYNC_BATCH` sets it.
pub const GITHUB_SYNC_BATCH: i64 = 2000;

/// How long the Report processor waits after a claim that found nothing.
///
/// One second is the longest an upload should wait behind a queue it believes is
/// empty, and the claim is one indexed `SELECT ... FOR UPDATE SKIP LOCKED`, so an
/// idle instance costs one statement a second. Shortening it multiplies that read
/// by every running instance; lengthening it is latency a person sees as a Report
/// stuck `pending`. The worker loop sleeps on it, and
/// `FLAKERADAR_WORKER_POLL_SECONDS` sets it as a number of seconds (`1.0`).
pub const WORKER_POLL: Duration = Duration::from_secs(1);

/// How often the processor runs the retention prune.
///
/// An hour is the coarsest thing retention has to be: every window it enforces is
/// measured in days, so a run up to an hour late deletes nothing that should
/// already have gone, while a run every few minutes holds rows in `DELETE` that
/// the processor is still writing. Shortening it buys nothing a reader sees;
/// lengthening it lets the tables grow past one batch's reach. The worker loop
/// compares it against the clock, and `FLAKERADAR_PRUNE_INTERVAL_SECONDS` sets it
/// as a number of seconds (`3600.0`).
pub const PRUNE_INTERVAL: Duration = Duration::from_secs(3600);

/// The `page_size` either leaderboard serves when a request names none, in rows.
///
/// One page is what the dashboard loads and what a person scans, so 50 rows is the
/// worst flakiness with room to see a pattern; `frontend`'s pager sends the same
/// number, so a request that names no `page_size` and one the dashboard makes
/// agree. It is a shipped default, not a bound: `limits::LIST_LIMIT_MAX` is the
/// largest a request may ask for, and the MCP listing tools have their own smaller
/// `DEFAULT_LIMIT` because a model reads those answers. Raising it grows every
/// answer that names no page; lowering it buries the rows past the fold. Both route
/// groups read it, and `tests/parity` replays the paging fields it decides.
pub const DEFAULT_PAGE_SIZE: i64 = 50;

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
