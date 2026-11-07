//! GitHub issue automation, filed in each Test's (and Job's) own Repo.
//!
//! When a Test or a Job crosses the filing gate, file an issue in that entity's
//! Repo (`owner/name`) with the evidence. This runs after the processor finishes
//! a Report, never on the upload path, so a slow or unreachable GitHub API never
//! delays CI.
//!
//! The gate needs every configured minimum; `0` disables a signal:
//!
//! - `github_issue_min_score`: the Flakiness score.
//! - `github_issue_min_proven_flakes`: the Proven flake count.
//! - `github_issue_min_failures`: failures in the newest `score_window`
//!   Executions of any branch (for a Job, unexplained failures only, because
//!   explained failures are the Tests').
//! - `github_fixed_streak`, when it is set: a Clean streak *below* the fixed-note
//!   threshold. A row at that streak has no live flake to file.
//!
//! Deduplication works because a filed issue's number is stored on the Test or
//! Job, so an issue is only re-filed after GitHub reports it closed (see
//! [`sync_closed_issues`]) — unless it was closed as `not_planned`, which keeps
//! the number and flags the row, so nothing is filed again until a person
//! unlinks it.
//!
//! **Error policy: this module never raises.** Every answer is classified once,
//! next to the seam ([`Answer::classify`]), into one [`Verdict`], and one table
//! ([`action`]) states what each operation does with each verdict. A GitHub outage
//! therefore costs filings rather than Report processing:
//!
//! - No token configured: a silent no-op.
//! - A rate limit ([`Verdict::RateLimited`]) ends every batch.
//! - A call that nothing answers ([`Verdict::Unreachable`]) ends filing, the sync
//!   and the fixed note; the post-delete close costs that one issue and goes on.
//! - A 401, 403, 404 or 410 costs the Repo for the rest of a filing, noting or sync
//!   batch, apart from the three cells the table spells out: a 404 in the sync is
//!   the issue itself being gone, so the link re-arms; a 404 or 410 in the
//!   post-delete close is the issue already being gone; and bad credentials (401)
//!   are a per-item failure on the note paths, not a Repo-wide one.
//! - Any other answer costs that one row, and the batch goes on.
//! - After an admin delete, any failure but a rate limit costs that one issue.
//! - A row deleted between a filing POST and the filer's commit costs that
//!   filing: the new issue is commented on and closed best effort.
//! - A store that fails after a filing POST is not a deleted row: the issue stays
//!   open, the failure is logged with its Repo and number, and the filing ends
//!   the outcome — no Job filing and no note batch follows.
//! - An unreadable body is logged and the row is stamped, so an unusable link
//!   keeps rotating instead of pinning the front of the queue.
//!
//! **No database statement is open across a GitHub call.** The sync reads its
//! rotation list, then calls GitHub for one row, then writes that one row in its
//! own short statement, so a slow or hanging GitHub can never hold a row lock a
//! Report or a route needs. That write applies only while the row still holds the
//! number that was checked, so a link replaced during the `GET` survives it.
//!
//! **Stopping: every batch respects the leader's stop signal.** A sync's `GET`
//! ends as soon as the signal fires, so a leader that drops costs nothing it has
//! to retry. Every call that changes something at GitHub runs to its own answer
//! and the batch stops after it: the `POST` that creates an issue through storing
//! its link, and a fixed note's comment through its stamp and the close that may
//! follow. Cancelling one of those could strand an issue, post the same comment
//! twice, or leave a fixed issue open, so a stop waits for at most the one request
//! that was in flight.
//!
//! **One file per responsibility**, which is why this module is a directory:
//!
//! - `client` — the REST client and its one classifier.
//! - `policy` — what every operation does with every verdict.
//! - `bodies` — the issue bodies and the fixed-fix comment.
//! - `filing` — the filing batch.
//! - `sync` — the issue-state sync and the post-delete close.
//! - `notes` — the fixed-note batch and the close it may ask for.
//!
//! Nothing here ever logs the token: the credential lives in a `Secret` and the
//! request carries it in a header only. `reqwest` errors have their URL removed
//! before they reach a log line, for the reason the Webhook sender does.

// Rust guideline compliant 2026-07-21

mod bodies;
mod client;
mod filing;
mod notes;
mod policy;
mod sync;

use flakeradar_core::model::ReportStatus;
use flakeradar_db::{clock::SharedClock, processing::ProcessOutcome};
use sea_orm::{ConnectionTrait, TransactionTrait};
use tokio_util::sync::CancellationToken;

#[doc(inline)]
pub use bodies::{
    BODY_EXECUTIONS, BODY_RUNNERS, SAMPLE_FAILURE_MAX, fixed_comment, job_issue_body, run_hint,
    test_issue_body,
};
#[doc(inline)]
pub use client::{
    API_BASE, API_VERSION, Answer, Api, RESPONSE_TEXT_MAX, Success, TIMEOUT_SECONDS, TransportError,
};
#[doc(inline)]
pub use filing::{file_issues_for, file_job_issues_for, filing_gate};
#[doc(inline)]
pub use notes::{note_fixed_issues_for, note_fixed_job_issues_for};
#[doc(inline)]
pub use policy::{Action, BatchReport, BatchStop, Operation, RemovalReport, Verdict, action};
#[doc(inline)]
pub use sync::{CLOSE_COMMENT, close_issues_after_removal, sync_closed_issues};

use crate::settings::Settings;

/// The `ReportWorker` hook: file issues for the Tests and Jobs a Report touched.
///
/// Also notes the rows whose Clean streak now proves the fix held. A newly filed
/// Issue is never the one commented on: the gate refuses a row already at the
/// note streak, and the ids this pass linked are dropped from the note step too,
/// in case a concurrent Report lifted a fresh link's streak between the two
/// selects.
///
/// A store that failed ends the whole outcome: the issue it created stays open,
/// and neither the Job filing nor a note batch runs after it, which is what the
/// reference's propagating exception does.
pub async fn on_report_processed(
    db: &(impl ConnectionTrait + TransactionTrait),
    outcome: &ProcessOutcome,
    settings: &Settings,
    api: &Api,
    clock: &SharedClock,
    stop: &CancellationToken,
) {
    if outcome.status != ReportStatus::Processed || !api.configured() {
        return;
    }
    let filed = file_issues_for(db, &outcome.touched_test_ids, settings, api, stop).await;
    if filed.stopped == Some(BatchStop::StoreFailed) {
        return;
    }
    let filed_jobs =
        file_job_issues_for(db, &outcome.touched_job_ids, settings, api, clock, stop).await;
    if filed_jobs.stopped == Some(BatchStop::StoreFailed) {
        return;
    }
    let note_ids: Vec<i32> = outcome
        .touched_test_ids
        .iter()
        .copied()
        .filter(|id| !filed.linked.contains(id))
        .collect();
    let note_job_ids: Vec<i32> = outcome
        .touched_job_ids
        .iter()
        .copied()
        .filter(|id| !filed_jobs.linked.contains(id))
        .collect();
    note_fixed_issues_for(db, &note_ids, settings, api, clock, stop).await;
    note_fixed_job_issues_for(db, &note_job_ids, settings, api, clock, stop).await;
}
