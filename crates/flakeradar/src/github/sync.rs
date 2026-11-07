//! The issue-state sync, and the close an admin delete strands.
//!
//! Each stored link is checked with its own `GET`, capped at the configured batch
//! per target, never-checked first. No statement is open across a `GET`: the
//! rotation list is read first, and each answered row is then written by its own
//! short statement. Never raises.

// Rust guideline compliant 2026-07-21

use std::ops::ControlFlow;

use chrono::{DateTime, Utc};
use flakeradar_core::model::EntityKind;
use flakeradar_db::{clock::SharedClock, issue_links};
use sea_orm::ConnectionTrait;
use tokio_util::sync::CancellationToken;

use super::client::{Answer, Api, Success};
use super::policy::{
    Action, BatchReport, BatchStop, Operation, RemovalReport, Verdict, action, batch_stop,
};
use crate::settings::Settings;
use crate::worker::until_stopped;

/// The comment every issue an admin delete stranded receives.
pub const CLOSE_COMMENT: &str = "FlakeRadar deleted the tracked flaky test or CI job this issue \
was filed for via the admin delete API, so the permalinks in this issue no longer resolve. \
Closing it automatically; if the test or job is uploaded again, FlakeRadar files a fresh issue.";

/// Re-arms deduplication for Tests and Jobs whose GitHub issue has been closed.
///
/// Leader-only maintenance: each stored link is checked with its own `GET`, capped
/// at the configured batch for each target, never-checked first. Never raises.
///
/// No statement is open across a `GET`: the rotation list is read first, and each
/// answered row is then written by its own short statement, which is guarded on the
/// issue number that was checked (see [`issue_links::record_check`]).
pub async fn sync_closed_issues(
    db: &impl ConnectionTrait,
    settings: &Settings,
    api: &Api,
    clock: &SharedClock,
    stop: &CancellationToken,
) -> BatchReport {
    let mut report = BatchReport::default();
    if !api.configured() {
        return report;
    }
    let batch = settings.github_sync_batch;
    let mut rows: Vec<(EntityKind, issue_links::LinkedIssue)> = Vec::new();
    for target in EntityKind::ALL {
        match issue_links::linked_for_check(db, target, batch).await {
            Ok(linked) => rows.extend(linked.into_iter().map(|row| (target, row))),
            Err(error) => {
                tracing::warn!(
                    name: "github.sync.rotation_unreadable",
                    error = %error,
                    "GitHub issue-state sync could not read its rotation list",
                );
                return report;
            }
        }
    }
    let now = clock.now().with_timezone(&Utc);
    for (target, row) in rows {
        if report.skipped.contains(&row.repo) {
            // An unreadable Repo this run: rotate it like a checked row.
            record_check(db, target, &row, issue_links::Check::Kept, now).await;
            continue;
        }
        if let ControlFlow::Break(stop_reason) =
            clear_if_closed(db, target, &row, api, &mut report, now, stop).await
        {
            report.stopped = Some(stop_reason);
            break;
        }
    }
    report.log(Operation::Sync);
    report
}

/// Checks one issue and records what the answer meant for its link.
///
/// Every row that got an answer is stamped, whatever the answer said, so a closed
/// issue is not asked about again; a rate-limit stop or a transport error leaves
/// the row unstamped, so it is retried first.
async fn clear_if_closed(
    db: &impl ConnectionTrait,
    target: EntityKind,
    row: &issue_links::LinkedIssue,
    api: &Api,
    report: &mut BatchReport,
    now: DateTime<Utc>,
    stop: &CancellationToken,
) -> ControlFlow<BatchStop> {
    let Some(answered) = until_stopped(stop, api.get_issue(&row.repo, row.number)).await else {
        tracing::info!(
            name: "github.sync.stopped",
            "stopping the GitHub issue-state sync: leadership ended",
        );
        return ControlFlow::Break(BatchStop::Stopped);
    };
    let answer = match answered {
        Ok(answer) => answer,
        Err(error) => {
            // No answer at all, so this row goes unstamped and the run stops here.
            tracing::warn!(
                name: "github.sync.unreachable",
                error = %error,
                "GitHub unreachable, skipping issue-state sync",
            );
            return ControlFlow::Break(batch_stop(Verdict::Unreachable));
        }
    };
    let verdict = answer.classify(Success::Fetched);
    let checked = match action(Operation::Sync, verdict) {
        Action::Use => sync_decision(&answer, row, target),
        Action::Rearm => {
            tracing::info!(
                name: "github.sync.issue_gone",
                repo = %row.repo,
                number = row.number,
                target = target.as_str(),
                entity_id = row.entity_id,
                "issue gone; re-arming the link"
            );
            issue_links::Check::Rearmed
        }
        Action::SkipRepo => {
            tracing::warn!(
                name: "github.sync.repo_skipped",
                repo = %row.repo,
                status = answer.status(),
                body = %answer.short_text(),
                "GitHub issue-state sync skipped for this Repo"
            );
            report.skipped.insert(row.repo.clone());
            issue_links::Check::Kept
        }
        Action::SkipRow => issue_links::Check::Kept,
        Action::Stop => {
            tracing::warn!(
                name: "github.sync.rate_limited",
                "GitHub rate limit in sync; stopping the batch",
            );
            return ControlFlow::Break(batch_stop(verdict));
        }
    };
    record_check(db, target, row, checked, now).await;
    ControlFlow::Continue(())
}

/// What one answered check means for its link, with the line that says why.
fn sync_decision(
    answer: &Answer,
    row: &issue_links::LinkedIssue,
    target: EntityKind,
) -> issue_links::Check {
    match answer.issue_state().as_deref() {
        Some("closed") => {
            let reason = answer.state_reason();
            if reason.as_deref() == Some("not_planned") {
                tracing::info!(
                    name: "github.sync.issue_declined",
                    repo = %row.repo,
                    number = row.number,
                    target = target.as_str(),
                    entity_id = row.entity_id,
                    "issue closed as not planned; keeping the link"
                );
                issue_links::Check::Declined
            } else {
                tracing::info!(
                    name: "github.sync.issue_closed",
                    repo = %row.repo,
                    number = row.number,
                    target = target.as_str(),
                    entity_id = row.entity_id,
                    reason = %reason.as_deref().unwrap_or("without a reason"),
                    "issue closed; re-arming the link"
                );
                issue_links::Check::Rearmed
            }
        }
        Some("open") => {
            // Possibly reopened after a close as not planned: filing is live again.
            issue_links::Check::Reopened
        }
        Some(_) => issue_links::Check::Kept,
        None => {
            tracing::warn!(
                name: "github.sync.unusable_body",
                repo = %row.repo,
                number = row.number,
                body = %answer.short_text(),
                "GitHub issue-state sync got an unusable 200 body"
            );
            issue_links::Check::Kept
        }
    }
}

/// Writes one row's check, warning when the write cannot land.
async fn record_check(
    db: &impl ConnectionTrait,
    target: EntityKind,
    row: &issue_links::LinkedIssue,
    check: issue_links::Check,
    at: DateTime<Utc>,
) {
    if let Err(error) =
        issue_links::record_check(db, target, i64::from(row.entity_id), row.number, check, at).await
    {
        tracing::warn!(
            name: "github.sync.check_unwritable",
            error = %error,
            "GitHub issue-state sync could not write its check",
        );
    }
}

/// Closes the issues whose Test or Job rows an admin delete removed, best effort.
///
/// An admin delete takes the tracked row with it, so no later Report re-files or
/// syncs the issue: without this it would stay open forever, pointing at
/// permalinks that now 404. Never raises; a rate limit stops the batch and any
/// other failure costs that one issue.
///
/// Answers what became of each number — the issues it closed, and the numbers
/// nothing was closed for with GitHub's answer — so a caller reads the outcome
/// rather than the batch's log.
pub async fn close_issues_after_removal(repo: &str, numbers: &[i32], api: &Api) -> RemovalReport {
    let mut report = RemovalReport::default();
    if !api.configured() || numbers.is_empty() {
        return report;
    }
    for number in numbers {
        let comment = match api.comment(repo, *number, CLOSE_COMMENT).await {
            Ok(comment) => comment,
            Err(error) => {
                tracing::warn!(
                    name: "github.sync.close_skipped",
                    error = %error,
                    repo = %repo,
                    number,
                    "GitHub post-delete close skipped for one issue"
                );
                report.refused.insert(*number, Verdict::Unreachable);
                continue;
            }
        };
        let verdict = comment.classify(Success::Created);
        match action(Operation::CloseDeleted, verdict) {
            Action::Use => {}
            Action::Stop => {
                tracing::warn!(
                    name: "github.sync.close_rate_limited",
                    remaining = %comment.remaining_text(),
                    "GitHub rate limit; stopping the post-delete close batch"
                );
                report.stopped = Some(batch_stop(verdict));
                break;
            }
            Action::SkipRow | Action::SkipRepo | Action::Rearm => {
                note_refused(&mut report, repo, *number, verdict, &comment);
                continue;
            }
        }
        let close = match api.close_issue(repo, *number, None).await {
            Ok(close) => close,
            Err(error) => {
                tracing::warn!(
                    name: "github.sync.close_skipped",
                    error = %error,
                    repo = %repo,
                    number,
                    "GitHub post-delete close skipped for one issue"
                );
                report.refused.insert(*number, Verdict::Unreachable);
                continue;
            }
        };
        let verdict = close.classify(Success::Fetched);
        match action(Operation::CloseDeleted, verdict) {
            Action::Use => {
                report.closed.insert(*number);
                tracing::info!(
                    name: "github.sync.closed",
                    repo = %repo,
                    number = *number,
                    "closed an issue after an admin delete",
                );
            }
            Action::Stop => {
                tracing::warn!(
                    name: "github.sync.close_rate_limited",
                    remaining = %close.remaining_text(),
                    repo = %repo,
                    number,
                    "GitHub rate limit; stopping the post-delete close batch"
                );
                report.stopped = Some(batch_stop(verdict));
                break;
            }
            Action::SkipRow | Action::SkipRepo | Action::Rearm => {
                note_refused(&mut report, repo, *number, verdict, &close);
            }
        }
    }
    report.log();
    report
}

/// Records an issue this batch refused, and says which way it went.
///
/// The two outcomes read differently to an operator: the issue was already gone,
/// which is what the batch wanted, or the request failed for a reason other than
/// a rate limit, which names the status and the body that explain it.
fn note_refused(
    report: &mut RemovalReport,
    repo: &str,
    number: i32,
    verdict: Verdict,
    answer: &Answer,
) {
    if already_gone(answer) {
        tracing::info!(
            name: "github.sync.close_already_gone",
            repo = %repo,
            number,
            "issue already gone after an admin delete",
        );
    } else {
        tracing::warn!(
            name: "github.sync.close_failed",
            repo = %repo,
            number,
            status = answer.status(),
            body = %answer.short_text(),
            "GitHub close after an admin delete failed"
        );
    }
    report.refused.insert(number, verdict);
}

/// Whether a post-delete answer is the issue already being gone.
///
/// That is the outcome this batch wanted: it costs that one number and the next is
/// still tried, exactly as a failure other than a rate limit does.
fn already_gone(answer: &Answer) -> bool {
    matches!(answer.status(), 404 | 410)
}
