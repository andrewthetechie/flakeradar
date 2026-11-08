//! Commenting once on the linked issue of a Test or Job whose fix appears to hold.
//!
//! Disabled while `github_fixed_streak` is 0. One comment per fix: a comment that
//! posted is stamped before anything else happens, and only a comment that never
//! reached GitHub leaves the row noteable for the next Report. Never raises.

// Rust guideline compliant 2026-07-21

use chrono::{DateTime, Utc};
use flakeradar_core::model::EntityKind;
use flakeradar_db::clock::SharedClock;
use flakeradar_db::filing::{self, NoteCandidate};
use flakeradar_db::issue_links::{self, NoteRowState};
use sea_orm::{ConnectionTrait, TransactionTrait};
use tokio_util::sync::CancellationToken;

use super::bodies::fixed_comment;
use super::client::{Api, Success};
use super::policy::{Action, BatchReport, BatchStop, Call, Operation, batch_stop, step};
use crate::settings::Settings;

/// Comments once on the linked issue of each `kind` row whose fix appears to hold.
///
/// Disabled while `github_fixed_streak` is 0. The Test and Job paths are the same
/// batch over different stored rows — `filing::note_candidates` states one
/// selection per entity and maps both the same way — so the batch, its stop rules
/// and its log lines are written once.
///
/// Never raises.
pub async fn note_fixed_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    kind: EntityKind,
    ids: &[i32],
    settings: &Settings,
    api: &Api,
    clock: &SharedClock,
    stop: &CancellationToken,
) -> BatchReport {
    if !api.configured() || settings.github_fixed_streak <= 0 || ids.is_empty() {
        return BatchReport::default();
    }
    let candidates =
        match filing::note_candidates(db, kind, ids, settings.github_fixed_streak).await {
            Ok(candidates) => candidates,
            Err(error) => {
                tracing::warn!(
                    name: "github.notes.candidates_unreadable",
                    error = %error,
                    "GitHub fixed-note could not read its candidates",
                );
                return BatchReport::default();
            }
        };
    if candidates.is_empty() {
        return BatchReport::default();
    }
    let report = note_batch(db, candidates, settings, api, clock, stop).await;
    report.log(Operation::Note);
    report
}

/// What one row of the fixed-note batch came to.
///
/// The batch keeps its own state — which Repos it stopped asking — so a row
/// answers what happened instead of writing into the report behind its back.
enum Noted {
    /// On to the next row.
    Next,
    /// GitHub will not take this Repo's calls for the rest of the batch.
    ///
    /// `after_close` says which call refused it. A refusal from the comment leaves
    /// the row there; one from the close is followed by the stop check every
    /// answered close gets, because the close has already run by then.
    SkipRepo { after_close: bool },
    /// The batch is over, because of this.
    Stop(BatchStop),
}

/// Notes (and maybe closes) one batch of fixed issues, whichever entity they describe.
///
/// The batch holds only the state that spans rows: the Repos it has stopped asking,
/// and why it ended. [`note_row`] does one row, and says which way it went.
async fn note_batch(
    db: &(impl ConnectionTrait + TransactionTrait),
    rows: Vec<NoteCandidate>,
    settings: &Settings,
    api: &Api,
    clock: &SharedClock,
    stop: &CancellationToken,
) -> BatchReport {
    let mut report = BatchReport::default();
    // One batch is one entity: `note_fixed_for` selects the candidates for one kind.
    let entity = rows.first().map_or("", |row| row.target.as_str());
    for row in rows {
        if report.skipped.contains(&row.repo) {
            continue;
        }
        if stop.is_cancelled() {
            report.stopped = Some(BatchStop::Stopped);
            break;
        }
        match note_row(db, &row, settings, api, clock, stop).await {
            Noted::Next => {}
            Noted::SkipRepo { after_close: false } => {
                report.skipped.insert(row.repo.clone());
            }
            Noted::SkipRepo { after_close: true } => {
                report.skipped.insert(row.repo.clone());
                if stop.is_cancelled() {
                    report.stopped = Some(BatchStop::Stopped);
                    break;
                }
            }
            Noted::Stop(stop_reason) => {
                report.stopped = Some(stop_reason);
                break;
            }
        }
    }
    if report.stopped == Some(BatchStop::Stopped) {
        tracing::info!(
            name: "github.notes.stopped",
            entity,
            "stopping the fixed-note batch: leadership ended"
        );
    }
    report
}

/// Comments on one row's issue, stamps the note, and closes it when asked to.
///
/// A comment and the close it may ask for both run to their answer, because a call
/// dropped mid-flight can have landed at GitHub: a comment that landed unseen would
/// be posted again on the next Report, and a close nobody saw would leave a fixed
/// issue open. The batch stops after the call that was in flight, so a stop costs
/// at most the one request it was waiting on.
async fn note_row(
    db: &(impl ConnectionTrait + TransactionTrait),
    row: &NoteCandidate,
    settings: &Settings,
    api: &Api,
    clock: &SharedClock,
    stop: &CancellationToken,
) -> Noted {
    let body = fixed_comment(
        row.target,
        i64::from(row.entity_id),
        i64::from(row.clean_streak),
        row.score,
        row.default_branch.as_deref(),
        &settings.public_url,
    );
    let call = step(
        Operation::Note,
        Success::Created,
        Box::pin(api.comment(&row.repo, row.number, &body)),
        |error| {
            tracing::warn!(
                name: "github.notes.unreachable",
                error = %error,
                entity = row.target.as_str(),
                "GitHub unreachable, skipping fixed-note comments",
            );
        },
    )
    .await;
    let (comment, note, verdict) = match call {
        Call::Answered {
            answer,
            verdict,
            action,
        } => (answer, action, verdict),
        Call::Unreachable { action } => {
            // The table's `Note` row answers an outage by ending the batch.
            return match action {
                Action::Stop => Noted::Stop(call.stop()),
                Action::SkipRepo => Noted::SkipRepo { after_close: false },
                _ => Noted::Next,
            };
        }
    };
    match note {
        Action::Use => {
            if !stamp_note(db, row, settings, clock.now().with_timezone(&Utc)).await
                || !settings.github_close_fixed
            {
                return Noted::Next;
            }
            // A stop that heard about the comment but not the stamp ends here: the
            // comment is recorded, and the issue stays open.
            if stop.is_cancelled() {
                return Noted::Stop(BatchStop::Stopped);
            }
            match close_note(row, api).await {
                Closed::Stop(stop_reason) => Noted::Stop(stop_reason),
                Closed::SkipRepo => Noted::SkipRepo { after_close: true },
                Closed::Done if stop.is_cancelled() => Noted::Stop(BatchStop::Stopped),
                Closed::Done => Noted::Next,
            }
        }
        Action::Stop => {
            tracing::warn!(
                name: "github.notes.rate_limited",
                remaining = %comment.remaining_text(),
                "GitHub rate limit; stopping the fixed-note batch"
            );
            Noted::Stop(batch_stop(verdict))
        }
        Action::SkipRepo => {
            tracing::warn!(
                name: "github.notes.repo_skipped",
                repo = %row.repo,
                status = comment.status(),
                body = %comment.short_text(),
                "GitHub fixed-note skipped for this Repo"
            );
            Noted::SkipRepo { after_close: false }
        }
        Action::SkipRow | Action::Rearm => {
            tracing::warn!(
                name: "github.notes.comment_failed",
                entity = row.target.as_str(),
                entity_id = row.entity_id,
                repo = %row.repo,
                status = comment.status(),
                body = %comment.short_text(),
                "GitHub fixed-note comment failed"
            );
            Noted::Next
        }
    }
}

/// Lands the fixed-note stamp for one row, answering whether it landed.
async fn stamp_note(
    db: &impl ConnectionTrait,
    row: &NoteCandidate,
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
        log_note_stamp_failure(stamp.state, row);
        return false;
    }
    tracing::info!(
        name: "github.notes.noted",
        repo = %row.repo,
        number = row.number,
        entity = row.target.as_str(),
        entity_id = row.entity_id,
        "noted that the fix appears to hold"
    );
    true
}

/// Logs why a note's stamp did not land: the row vanished or stopped being noteable.
fn log_note_stamp_failure(state: Option<NoteRowState>, row: &NoteCandidate) {
    match state {
        None => tracing::warn!(
            name: "github.notes.row_gone",
            entity = row.target.as_str(),
            entity_id = row.entity_id,
            repo = %row.repo,
            number = row.number,
            "row deleted (admin delete) while noting its issue"
        ),
        Some(state) => tracing::warn!(
            name: "github.notes.no_longer_noteable",
            entity = row.target.as_str(),
            entity_id = row.entity_id,
            streak = state.clean_streak,
            number = ?state.issue_number,
            "row stopped being noteable while its comment was posted"
        ),
    }
}

/// What one fixed-close answered, in the terms its batch needs.
///
/// `SkipRepo` is the answer the caller folds into its own skipped set, which keeps
/// the batch's state inside the batch instead of an output parameter.
enum Closed {
    /// The close landed, or cost this row only.
    Done,
    /// GitHub will not take this Repo's calls for the rest of the batch.
    SkipRepo,
    /// The batch is over, because of this.
    Stop(BatchStop),
}

/// Closes the issue a fix was noted on, when the configuration asks for it.
///
/// A close that was already sent runs to its answer, so a stop cannot leave an
/// issue open after its fix was noted. A rate limit ends the whole batch rather
/// than only this issue.
async fn close_note(row: &NoteCandidate, api: &Api) -> Closed {
    let call = step(
        Operation::CloseNote,
        Success::Fetched,
        Box::pin(api.close_issue(&row.repo, row.number, Some("completed"))),
        |error| {
            tracing::warn!(
                name: "github.notes.close_unreachable",
                error = %error,
                entity = row.target.as_str(),
                "GitHub unreachable, skipping the close",
            );
        },
    )
    .await;
    let (close, note, verdict) = match call {
        Call::Answered {
            answer,
            verdict,
            action,
        } => (answer, action, verdict),
        Call::Unreachable { action } => {
            // The table's `CloseNote` row answers an outage by ending the batch.
            return match action {
                Action::Stop => Closed::Stop(call.stop()),
                Action::SkipRepo => Closed::SkipRepo,
                _ => Closed::Done,
            };
        }
    };
    match note {
        Action::Use => {
            tracing::info!(
                name: "github.notes.closed",
                repo = %row.repo,
                number = row.number,
                "closed an issue after noting the fix"
            );
            Closed::Done
        }
        Action::Stop => {
            tracing::warn!(
                name: "github.notes.close_rate_limited",
                remaining = %close.remaining_text(),
                repo = %row.repo,
                number = row.number,
                "GitHub rate limit; stopping the fixed-close batch"
            );
            Closed::Stop(batch_stop(verdict))
        }
        Action::SkipRepo => {
            tracing::warn!(
                name: "github.notes.close_repo_skipped",
                repo = %row.repo,
                status = close.status(),
                body = %close.short_text(),
                "GitHub issue close skipped for this Repo"
            );
            Closed::SkipRepo
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
            Closed::Done
        }
    }
}
