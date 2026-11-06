//! Commenting once on the linked issue of a Test or Job whose fix appears to hold.
//!
//! Disabled while `github_fixed_streak` is 0. A comment that never posted stamps
//! nothing, so the row is noteable on the next Report instead. Never raises.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeSet;
use std::ops::ControlFlow;

use chrono::Utc;
use flakeradar_core::EntityKind;
use flakeradar_db::filing::{self, NoteCandidate};
use flakeradar_db::issue_links::{self, NoteRowState};
use sea_orm::{ConnectionTrait, TransactionTrait};
use tokio_util::sync::CancellationToken;

use super::bodies::fixed_comment;
use super::client::{Api, Success};
use super::policy::{Action, BatchReport, BatchStop, Operation, Verdict, action, batch_stop};
use crate::settings::Settings;
use crate::worker::until_stopped;

/// Comments once on the linked issue of each Test whose fix appears to hold.
///
/// Disabled while `github_fixed_streak` is 0. Never raises.
pub async fn note_fixed_issues_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    test_ids: &[i32],
    settings: &Settings,
    api: &Api,
    stop: &CancellationToken,
) -> BatchReport {
    if !api.configured() || settings.github_fixed_streak <= 0 || test_ids.is_empty() {
        return BatchReport::default();
    }
    match filing::note_test_candidates(db, test_ids, settings.github_fixed_streak).await {
        Ok(candidates) if !candidates.is_empty() => {
            let report = note_batch(db, candidates, EntityKind::Test, settings, api, stop).await;
            report.log(Operation::Note);
            report
        }
        Ok(_) => BatchReport::default(),
        Err(error) => {
            tracing::warn!(error = %error, "GitHub fixed-note could not read its candidates");
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
    stop: &CancellationToken,
) -> BatchReport {
    if !api.configured() || settings.github_fixed_streak <= 0 || job_ids.is_empty() {
        return BatchReport::default();
    }
    match filing::note_job_candidates(db, job_ids, settings.github_fixed_streak).await {
        Ok(candidates) if !candidates.is_empty() => {
            let report = note_batch(db, candidates, EntityKind::Job, settings, api, stop).await;
            report.log(Operation::Note);
            report
        }
        Ok(_) => BatchReport::default(),
        Err(error) => {
            tracing::warn!(error = %error, "GitHub fixed-note could not read its candidates");
            BatchReport::default()
        }
    }
}

/// Notes (and maybe closes) one batch of fixed issues, whichever entity they describe.
///
/// The comment ends when `stop` fires: a comment that never posted stamps
/// nothing, so the row is still noteable on the next Report.
async fn note_batch(
    db: &(impl ConnectionTrait + TransactionTrait),
    rows: Vec<NoteCandidate>,
    entity: EntityKind,
    settings: &Settings,
    api: &Api,
    stop: &CancellationToken,
) -> BatchReport {
    let mut report = BatchReport::default();
    for row in rows {
        if report.skipped.contains(&row.repo) {
            continue;
        }
        let body = fixed_comment(
            entity,
            i64::from(row.entity_id),
            i64::from(row.clean_streak),
            row.score,
            row.default_branch.as_deref(),
            &settings.public_url,
        );
        let Some(answered) = until_stopped(stop, api.comment(&row.repo, row.number, &body)).await
        else {
            tracing::info!(
                entity = entity.as_str(),
                "stopping the fixed-note batch: leadership ended"
            );
            report.stopped = Some(BatchStop::Stopped);
            break;
        };
        let comment = match answered {
            Ok(comment) => comment,
            Err(error) => {
                tracing::warn!(error = %error, entity = entity.as_str(), "GitHub unreachable, skipping fixed-note comments");
                report.stopped = Some(batch_stop(Verdict::Unreachable));
                break;
            }
        };
        let verdict = comment.classify(Success::Created);
        match action(Operation::Note, verdict) {
            Action::Use => {
                if !stamp_note(db, &row, entity, settings).await || !settings.github_close_fixed {
                    continue;
                }
                if let ControlFlow::Break(stop_reason) =
                    close_note(&row, entity, api, &mut report.skipped, stop).await
                {
                    report.stopped = Some(stop_reason);
                    break;
                }
            }
            Action::Stop => {
                tracing::warn!(
                    remaining = %comment.remaining_text(),
                    "GitHub rate limit; stopping the fixed-note batch"
                );
                report.stopped = Some(batch_stop(verdict));
                break;
            }
            Action::SkipRepo => {
                tracing::warn!(
                    repo = %row.repo,
                    status = comment.status(),
                    body = %comment.short_text(),
                    "GitHub fixed-note skipped for this Repo"
                );
                report.skipped.insert(row.repo.clone());
            }
            Action::SkipRow | Action::Rearm => {
                tracing::warn!(
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
    report
}

/// Lands the fixed-note stamp for one row, answering whether it landed.
async fn stamp_note(
    db: &impl ConnectionTrait,
    row: &NoteCandidate,
    entity: EntityKind,
    settings: &Settings,
) -> bool {
    let threshold = i32::try_from(settings.github_fixed_streak).unwrap_or(i32::MAX);
    let stamp = match issue_links::stamp_fixed_note(
        db,
        row.target,
        i64::from(row.entity_id),
        row.number,
        threshold,
        Utc::now(),
    )
    .await
    {
        Ok(stamp) => stamp,
        Err(error) => {
            tracing::warn!(error = %error, "GitHub fixed-note could not stamp its row");
            return false;
        }
    };
    if !stamp.stamped {
        log_note_stamp_failure(stamp.state, entity, row);
        return false;
    }
    tracing::info!(
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
            entity = entity.as_str(),
            entity_id = row.entity_id,
            repo = %row.repo,
            number = row.number,
            "row deleted (admin delete) while noting its issue"
        ),
        Some(state) => tracing::warn!(
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
/// A stop signal or a rate limit ends the whole batch rather than only this issue.
async fn close_note(
    row: &NoteCandidate,
    entity: EntityKind,
    api: &Api,
    skipped: &mut BTreeSet<String>,
    stop: &CancellationToken,
) -> ControlFlow<BatchStop> {
    let Some(answered) = until_stopped(
        stop,
        api.close_issue(&row.repo, row.number, Some("completed")),
    )
    .await
    else {
        tracing::info!(
            entity = entity.as_str(),
            "stopping the fixed-close batch: leadership ended"
        );
        return ControlFlow::Break(BatchStop::Stopped);
    };
    let close = match answered {
        Ok(close) => close,
        Err(error) => {
            tracing::warn!(error = %error, entity = entity.as_str(), "GitHub unreachable, skipping the close");
            return ControlFlow::Break(batch_stop(Verdict::Unreachable));
        }
    };
    let verdict = close.classify(Success::Fetched);
    match action(Operation::CloseNote, verdict) {
        Action::Use => {
            tracing::info!(
                repo = %row.repo,
                number = row.number,
                "closed an issue after noting the fix"
            );
            ControlFlow::Continue(())
        }
        Action::Stop => {
            tracing::warn!(
                remaining = %close.remaining_text(),
                repo = %row.repo,
                number = row.number,
                "GitHub rate limit; stopping the fixed-close batch"
            );
            ControlFlow::Break(batch_stop(verdict))
        }
        Action::SkipRepo => {
            tracing::warn!(
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
