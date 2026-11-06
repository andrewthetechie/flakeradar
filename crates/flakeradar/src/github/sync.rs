//! The issue-state sync, and the close an admin delete strands.
//!
//! Each stored link is checked with its own `GET`, capped at the configured batch
//! per target, never-checked first. No statement is open across a `GET`: the
//! rotation list is read first, and each answered row is then written by its own
//! short statement. Never raises.

// Rust guideline compliant 2026-07-21

use std::ops::ControlFlow;

use chrono::{DateTime, Utc};
use flakeradar_core::EntityKind;
use flakeradar_db::issue_links;
use sea_orm::ConnectionTrait;
use tokio_util::sync::CancellationToken;

use super::client::{Answer, Api, Success};
use super::policy::{Action, BatchReport, BatchStop, Operation, Verdict, action, batch_stop};
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
                tracing::warn!(error = %error, "GitHub issue-state sync could not read its rotation list");
                return report;
            }
        }
    }
    let now = Utc::now();
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
/// Stamping stays as the reference's `_clear_if_closed` docstring states: every row
/// that got an answer is stamped, whatever the answer said, while a rate-limit stop
/// or a transport error leaves the row unstamped so it is retried first.
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
        tracing::info!("stopping the GitHub issue-state sync: leadership ended");
        return ControlFlow::Break(BatchStop::Stopped);
    };
    let answer = match answered {
        Ok(answer) => answer,
        Err(error) => {
            // No answer at all, so this row goes unstamped and the run stops here.
            tracing::warn!(error = %error, "GitHub unreachable, skipping issue-state sync");
            return ControlFlow::Break(batch_stop(Verdict::Unreachable));
        }
    };
    let verdict = answer.classify(Success::Fetched);
    let checked = match action(Operation::Sync, verdict) {
        Action::Use => sync_decision(&answer, row, target),
        Action::Rearm => {
            tracing::info!(
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
            tracing::warn!("GitHub rate limit in sync; stopping the batch");
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
                    repo = %row.repo,
                    number = row.number,
                    target = target.as_str(),
                    entity_id = row.entity_id,
                    "issue closed as not planned; keeping the link"
                );
                issue_links::Check::Declined
            } else {
                tracing::info!(
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
        tracing::warn!(error = %error, "GitHub issue-state sync could not write its check");
    }
}

/// Closes the issues whose Test or Job rows an admin delete removed, best effort.
///
/// An admin delete takes the tracked row with it, so no later Report re-files or
/// syncs the issue: without this it would stay open forever, pointing at
/// permalinks that now 404. Never raises; a rate limit stops the batch and any
/// other failure costs that one issue.
pub async fn close_issues_after_removal(repo: &str, numbers: &[i32], api: &Api) -> BatchReport {
    let mut report = BatchReport::default();
    if !api.configured() || numbers.is_empty() {
        return report;
    }
    for number in numbers {
        let comment = match api.comment(repo, *number, CLOSE_COMMENT).await {
            Ok(comment) => comment,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    repo = %repo,
                    number,
                    "GitHub post-delete close skipped for one issue"
                );
                continue;
            }
        };
        let verdict = comment.classify(Success::Created);
        match action(Operation::CloseDeleted, verdict) {
            Action::Use => {}
            Action::Stop => {
                tracing::warn!(
                    remaining = %comment.remaining_text(),
                    "GitHub rate limit; stopping the post-delete close batch"
                );
                report.stopped = Some(batch_stop(verdict));
                break;
            }
            Action::SkipRow | Action::SkipRepo | Action::Rearm => {
                if already_gone(&comment) {
                    tracing::info!(repo = %repo, number, "issue already gone after an admin delete");
                } else {
                    tracing::warn!(
                        repo = %repo,
                        number,
                        status = comment.status(),
                        body = %comment.short_text(),
                        "GitHub comment after an admin delete failed"
                    );
                }
                continue;
            }
        }
        let close = match api.close_issue(repo, *number, None).await {
            Ok(close) => close,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    repo = %repo,
                    number,
                    "GitHub post-delete close skipped for one issue"
                );
                continue;
            }
        };
        let verdict = close.classify(Success::Fetched);
        match action(Operation::CloseDeleted, verdict) {
            Action::Use => {
                tracing::info!(repo = %repo, number, "closed an issue after an admin delete");
            }
            Action::Stop => {
                tracing::warn!(
                    remaining = %close.remaining_text(),
                    repo = %repo,
                    number,
                    "GitHub rate limit; stopping the post-delete close batch"
                );
                report.stopped = Some(batch_stop(verdict));
                break;
            }
            Action::SkipRow | Action::SkipRepo | Action::Rearm => {
                if already_gone(&close) {
                    tracing::info!(repo = %repo, number, "issue already gone after an admin delete");
                } else {
                    tracing::warn!(
                        repo = %repo,
                        number,
                        status = close.status(),
                        body = %close.short_text(),
                        "GitHub close after an admin delete failed"
                    );
                }
            }
        }
    }
    report.log(Operation::CloseDeleted);
    report
}

/// Whether a post-delete answer is the issue already being gone.
///
/// That is the outcome this batch wanted: it costs that one number and the next is
/// still tried, exactly as a failure other than a rate limit does.
fn already_gone(answer: &Answer) -> bool {
    matches!(answer.status(), 404 | 410)
}
