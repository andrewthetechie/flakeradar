//! Commenting once on the linked issue of a Test or Job whose fix appears to hold.
//!
//! Disabled while `github_fixed_streak` is 0. One comment per fix: a comment that
//! posted is stamped before anything else happens, and only a comment that never
//! reached GitHub leaves the row noteable for the next Report. Never raises.

use std::collections::BTreeSet;
use std::ops::ControlFlow;

use chrono::{DateTime, Utc};
use flakeradar_core::model::EntityKind;
use flakeradar_db::clock::SharedClock;
use flakeradar_db::filing::{self, NoteCandidate};
use flakeradar_db::issue_links::{self, NoteRowState};
use sea_orm::{ConnectionTrait, TransactionTrait};
use tokio_util::sync::CancellationToken;

use super::bodies::fixed_comment;
use super::client::{Api, Success};
use super::policy::{Action, BatchReport, BatchStop, Operation, Verdict, action, batch_stop};
use crate::settings::Settings;

/// Comments once on the linked issue of each Test whose fix appears to hold.
///
/// Disabled while `github_fixed_streak` is 0. Never raises.
pub async fn note_fixed_issues_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    test_ids: &[i32],
    settings: &Settings,
    api: &Api,
    clock: &SharedClock,
    stop: &CancellationToken,
) -> BatchReport {
    if !api.configured() || settings.github_fixed_streak <= 0 || test_ids.is_empty() {
        return BatchReport::default();
    }
    match filing::note_test_candidates(db, test_ids, settings.github_fixed_streak).await {
        Ok(candidates) if !candidates.is_empty() => {
            let report =
                note_batch(db, candidates, EntityKind::Test, settings, api, clock, stop).await;
            report.log(Operation::Note);
            report
        }
        Ok(_) => BatchReport::default(),
        Err(error) => {
            tracing::warn!(
                name: "github.notes.candidates_unreadable",
                error = %error,
                "GitHub fixed-note could not read its candidates",
            );
            BatchReport::default()
        }
    }
}

/// Comments once on the linked issue of each Job whose fix appears to hold.
pub async fn note_fixed_job_issues_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    job_ids: &[i32],
    settings: &Settings,
    api: &Api,
    clock: &SharedClock,
    stop: &CancellationToken,
) -> BatchReport {
    if !api.configured() || settings.github_fixed_streak <= 0 || job_ids.is_empty() {
        return BatchReport::default();
    }
    match filing::note_job_candidates(db, job_ids, settings.github_fixed_streak).await {
        Ok(candidates) if !candidates.is_empty() => {
            let report =
                note_batch(db, candidates, EntityKind::Job, settings, api, clock, stop).await;
            report.log(Operation::Note);
            report
        }
        Ok(_) => BatchReport::default(),
        Err(error) => {
            tracing::warn!(
                name: "github.notes.candidates_unreadable",
                error = %error,
                "GitHub fixed-note could not read its candidates",
            );
            BatchReport::default()
        }
    }
}

/// Notes (and maybe closes) one batch of fixed issues, whichever entity they describe.
///
/// A comment and the close it may ask for both run to their answer, because a call
/// dropped mid-flight can have landed at GitHub: a comment that landed unseen would
/// be posted again on the next Report, and a close nobody saw would leave a fixed
/// issue open. The batch stops after the call that was in flight, so a stop costs
/// at most the one request it was waiting on.
async fn note_batch(
    db: &(impl ConnectionTrait + TransactionTrait),
    rows: Vec<NoteCandidate>,
    entity: EntityKind,
    settings: &Settings,
    api: &Api,
    clock: &SharedClock,
    stop: &CancellationToken,
) -> BatchReport {
    let mut report = BatchReport::default();
    for row in rows {
        if report.skipped.contains(&row.repo) {
            continue;
        }
        if stop.is_cancelled() {
            report.stopped = Some(BatchStop::Stopped);
            break;
        }
        let body = fixed_comment(
            entity,
            i64::from(row.entity_id),
            i64::from(row.clean_streak),
            row.score,
            row.default_branch.as_deref(),
            &settings.public_url,
        );
        let answered = api.comment(&row.repo, row.number, &body).await;
        let comment = match answered {
            Ok(comment) => comment,
            Err(error) => {
                tracing::warn!(
                    name: "github.notes.unreachable",
                    error = %error,
                    entity = entity.as_str(),
                    "GitHub unreachable, skipping fixed-note comments",
                );
                report.stopped = Some(batch_stop(Verdict::Unreachable));
                break;
            }
        };
        let verdict = comment.classify(Success::Created);
        match action(Operation::Note, verdict) {
            Action::Use => {
                if !stamp_note(db, &row, entity, settings, clock.now().with_timezone(&Utc)).await
                    || !settings.github_close_fixed
                {
                    continue;
                }
                // A stop that heard about the comment but not the stamp ends here:
                // the comment is recorded, and the issue stays open.
                if stop.is_cancelled() {
                    report.stopped = Some(BatchStop::Stopped);
                    break;
                }
                if let ControlFlow::Break(stop_reason) =
                    close_note(&row, entity, api, &mut report.skipped).await
                {
                    report.stopped = Some(stop_reason);
                    break;
                }
                if stop.is_cancelled() {
                    report.stopped = Some(BatchStop::Stopped);
                    break;
                }
            }
            Action::Stop => {
                tracing::warn!(
                    name: "github.notes.rate_limited",
                    remaining = %comment.remaining_text(),
                    "GitHub rate limit; stopping the fixed-note batch"
                );
                report.stopped = Some(batch_stop(verdict));
                break;
            }
            Action::SkipRepo => {
                tracing::warn!(
                    name: "github.notes.repo_skipped",
                    repo = %row.repo,
                    status = comment.status(),
                    body = %comment.short_text(),
                    "GitHub fixed-note skipped for this Repo"
                );
                report.skipped.insert(row.repo.clone());
            }
            Action::SkipRow | Action::Rearm => {
                tracing::warn!(
                    name: "github.notes.comment_failed",
                    entity = entity.as_str(),
                    entity_id = row.entity_id,
                    repo = %row.repo,
                    status = comment.status(),
                    body = %comment.short_text(),
                    "GitHub fixed-note comment failed"
                );
            }
        }
    }
    if report.stopped == Some(BatchStop::Stopped) {
        tracing::info!(
            name: "github.notes.stopped",
            entity = entity.as_str(),
            "stopping the fixed-note batch: leadership ended"
        );
    }
    report
}

/// Lands the fixed-note stamp for one row, answering whether it landed.
async fn stamp_note(
    db: &impl ConnectionTrait,
    row: &NoteCandidate,
    entity: EntityKind,
    settings: &Settings,
    now: DateTime<Utc>,
) -> bool {
    let threshold = i32::try_from(settings.github_fixed_streak).unwrap_or(i32::MAX);
    let stamp = match issue_links::stamp_fixed_note(
        db,
        row.target,
        i64::from(row.entity_id),
        row.number,
        threshold,
        now,
    )
    .await
    {
        Ok(stamp) => stamp,
        Err(error) => {
            tracing::warn!(
                name: "github.notes.stamp_failed",
                error = %error,
                "GitHub fixed-note could not stamp its row",
            );
            return false;
        }
    };
    if !stamp.stamped {
        log_note_stamp_failure(stamp.state, entity, row);
        return false;
    }
    tracing::info!(
        name: "github.notes.noted",
        repo = %row.repo,
        number = row.number,
        entity = entity.as_str(),
        entity_id = row.entity_id,
        "noted that the fix appears to hold"
    );
    true
}

/// Logs why a note's stamp did not land: the row vanished or stopped being noteable.
fn log_note_stamp_failure(state: Option<NoteRowState>, entity: EntityKind, row: &NoteCandidate) {
    match state {
        None => tracing::warn!(
            name: "github.notes.row_gone",
            entity = entity.as_str(),
            entity_id = row.entity_id,
            repo = %row.repo,
            number = row.number,
            "row deleted (admin delete) while noting its issue"
        ),
        Some(state) => tracing::warn!(
            name: "github.notes.no_longer_noteable",
            entity = entity.as_str(),
            entity_id = row.entity_id,
            streak = state.clean_streak,
            number = ?state.issue_number,
            "row stopped being noteable while its comment was posted"
        ),
    }
}

/// Closes the issue a fix was noted on, when the configuration asks for it.
///
/// A close that was already sent runs to its answer, so a stop cannot leave an
/// issue open after its fix was noted. A rate limit ends the whole batch rather
/// than only this issue.
async fn close_note(
    row: &NoteCandidate,
    entity: EntityKind,
    api: &Api,
    skipped: &mut BTreeSet<String>,
) -> ControlFlow<BatchStop> {
    let answered = api
        .close_issue(&row.repo, row.number, Some("completed"))
        .await;
    let close = match answered {
        Ok(close) => close,
        Err(error) => {
            tracing::warn!(
                name: "github.notes.close_unreachable",
                error = %error,
                entity = entity.as_str(),
                "GitHub unreachable, skipping the close",
            );
            return ControlFlow::Break(batch_stop(Verdict::Unreachable));
        }
    };
    let verdict = close.classify(Success::Fetched);
    match action(Operation::CloseNote, verdict) {
        Action::Use => {
            tracing::info!(
                name: "github.notes.closed",
                repo = %row.repo,
                number = row.number,
                "closed an issue after noting the fix"
            );
            ControlFlow::Continue(())
        }
        Action::Stop => {
            tracing::warn!(
                name: "github.notes.close_rate_limited",
                remaining = %close.remaining_text(),
                repo = %row.repo,
                number = row.number,
                "GitHub rate limit; stopping the fixed-close batch"
            );
            ControlFlow::Break(batch_stop(verdict))
        }
        Action::SkipRepo => {
            tracing::warn!(
                name: "github.notes.close_repo_skipped",
                repo = %row.repo,
                status = close.status(),
                body = %close.short_text(),
                "GitHub issue close skipped for this Repo"
            );
            skipped.insert(row.repo.clone());
            ControlFlow::Continue(())
        }
        Action::SkipRow | Action::Rearm => {
            tracing::warn!(
                name: "github.notes.close_failed",
                repo = %row.repo,
                number = row.number,
                status = close.status(),
                body = %close.short_text(),
                "GitHub close after noting the fix failed"
            );
            ControlFlow::Continue(())
        }
    }
}
