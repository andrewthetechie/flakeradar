//! Filing an issue for every Test and Job that now meets the gate.
//!
//! The candidates are `flakeradar_db::filing`'s and the bodies are `bodies`', and
//! this file decides the batch's order and what a filing answer costs. A filing `POST` is never interrupted, because its issue would be
//! stranded; the batch stops before the next one once the leader's stop signal
//! fires. Never raises: an outage costs filings, not Report processing.

// Rust guideline compliant 2026-07-21

use std::ops::ControlFlow;

use flakeradar_core::model::EntityKind;
use flakeradar_db::filing::{self, FilingGate};
use flakeradar_db::issue_links;
use flakeradar_db::{DbError, clock::SharedClock, reads::get_job};
use sea_orm::{ConnectionTrait, TransactionSession as _, TransactionTrait};
use tokio_util::sync::CancellationToken;

use super::bodies::{BODY_EXECUTIONS, job_issue_body, job_title, test_issue_body, test_title};
use super::client::{Answer, Api, Success};
use super::policy::{Action, BatchReport, BatchStop, Operation, Verdict, action, batch_stop};
use super::sync::close_issues_after_removal;
use crate::settings::Settings;

/// The issue-filing gate the configuration states.
#[must_use]
pub fn filing_gate(settings: &Settings) -> FilingGate {
    let fixed_streak = (settings.github_fixed_streak > 0).then_some(settings.github_fixed_streak);
    FilingGate::new(
        settings.github_issue_min_score,
        settings.github_issue_min_proven_flakes,
        settings.github_issue_min_failures,
        fixed_streak,
    )
}

/// Files issues for Tests that now meet the filing gate. Never raises.
///
/// The report names the Test ids it linked, so the caller can keep the fixed-note
/// step off a link this same Report created.
pub async fn file_issues_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    test_ids: &[i32],
    settings: &Settings,
    api: &Api,
    stop: &CancellationToken,
) -> BatchReport {
    if !api.configured() || test_ids.is_empty() {
        return BatchReport::default();
    }
    let candidates =
        match filing::test_candidates(db, test_ids, &filing_gate(settings), settings.score_window)
            .await
        {
            Ok(candidates) => candidates,
            Err(error) => {
                tracing::warn!(
                    name: "github.filing.test_candidates_unreadable",
                    error = %error,
                    "GitHub issue filing could not read its Test candidates",
                );
                return BatchReport::default();
            }
        };
    if candidates.is_empty() {
        return BatchReport::default();
    }
    let mut batch = Vec::new();
    for candidate in candidates {
        let recent = match filing::recent_executions(db, candidate.test.id, BODY_EXECUTIONS).await {
            Ok(recent) => recent,
            Err(error) => {
                tracing::warn!(
                    name: "github.filing.test_executions_unreadable",
                    error = %error,
                    "GitHub issue filing could not read a Test's Executions",
                );
                Vec::new()
            }
        };
        let title = test_title(&candidate.test.classname, &candidate.test.name);
        let body = test_issue_body(
            &candidate.test,
            &recent,
            &candidate.repo,
            &candidate.project,
            &candidate.root,
            settings,
        );
        batch.push(FileRequest {
            target: EntityKind::Test,
            entity_id: candidate.test.id,
            repo: candidate.repo,
            title,
            body,
        });
    }
    let report = file_batch(db, batch, EntityKind::Test, settings, api, stop).await;
    report.log(Operation::File);
    report
}

/// Files issues for github-provider Jobs that now meet the filing gate. Never raises.
pub async fn file_job_issues_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    job_ids: &[i32],
    settings: &Settings,
    api: &Api,
    clock: &SharedClock,
    stop: &CancellationToken,
) -> BatchReport {
    if !api.configured() || job_ids.is_empty() {
        return BatchReport::default();
    }
    let candidates =
        match filing::job_candidates(db, job_ids, &filing_gate(settings), settings.score_window)
            .await
        {
            Ok(candidates) => candidates,
            Err(error) => {
                tracing::warn!(
                    name: "github.filing.job_candidates_unreadable",
                    error = %error,
                    "GitHub issue filing could not read its Job candidates",
                );
                return BatchReport::default();
            }
        };
    if candidates.is_empty() {
        return BatchReport::default();
    }
    let mut batch = Vec::new();
    for candidate in candidates {
        let history = match get_job(
            db,
            i64::from(candidate.job.id),
            &settings.reads(std::sync::Arc::clone(clock)),
            i64::try_from(BODY_EXECUTIONS).unwrap_or(10),
        )
        .await
        {
            Ok(history) => history,
            Err(error) => {
                tracing::warn!(
                    name: "github.filing.job_executions_unreadable",
                    error = %error,
                    repo = %candidate.repo,
                    entity_id = candidate.job.id,
                    "GitHub issue filing could not read a Job's Executions"
                );
                None
            }
        };
        let title = job_title(&candidate.pipeline, &candidate.job.name);
        let body = job_issue_body(
            &candidate.job,
            &candidate.pipeline,
            &candidate.repo,
            history.as_ref(),
            settings,
        );
        batch.push(FileRequest {
            target: EntityKind::Job,
            entity_id: candidate.job.id,
            repo: candidate.repo,
            title,
            body,
        });
    }
    let report = file_batch(db, batch, EntityKind::Job, settings, api, stop).await;
    report.log(Operation::File);
    report
}

/// One rendered issue to file.
#[derive(Debug, Clone)]
struct FileRequest {
    target: EntityKind,
    entity_id: i32,
    repo: String,
    title: String,
    body: String,
}

/// Files one batch of rendered issues, whichever entity they describe.
///
/// A filing `POST` is never interrupted, because its issue would be stranded; the
/// batch stops before the next one once `stop` fires.
async fn file_batch(
    db: &(impl ConnectionTrait + TransactionTrait),
    batch: Vec<FileRequest>,
    entity: EntityKind,
    settings: &Settings,
    api: &Api,
    stop: &CancellationToken,
) -> BatchReport {
    let mut report = BatchReport::default();
    for request in batch {
        if stop.is_cancelled() {
            tracing::info!(
                name: "github.filing.stopped",
                entity = entity.as_str(),
                "stopping the issue-filing batch: leadership ended"
            );
            report.stopped = Some(BatchStop::Stopped);
            break;
        }
        if report.skipped.contains(&request.repo) {
            continue;
        }
        let answered = api
            .create_issue(
                &request.repo,
                &request.title,
                &request.body,
                &settings.github_issue_label,
            )
            .await;
        let answer = match answered {
            Ok(answer) => answer,
            Err(error) => {
                // Filing never carries on without an answer, which is the table's
                // `File`/`Unreachable` cell.
                tracing::warn!(
                    name: "github.filing.unreachable",
                    error = %error,
                    entity = entity.as_str(),
                    "GitHub unreachable, skipping issue filing",
                );
                report.stopped = Some(batch_stop(Verdict::Unreachable));
                break;
            }
        };
        let verdict = answer.classify(Success::Created);
        match action(Operation::File, verdict) {
            Action::Use => {
                if let ControlFlow::Break(stop) =
                    record_filed(db, &request, &answer, entity, api, &mut report).await
                {
                    report.stopped = Some(stop);
                    break;
                }
            }
            Action::Stop => {
                tracing::warn!(
                    name: "github.filing.rate_limited",
                    remaining = %answer.remaining_text(),
                    "GitHub rate limit; stopping the filing batch"
                );
                report.stopped = Some(batch_stop(verdict));
                break;
            }
            Action::SkipRepo => {
                tracing::warn!(
                    name: "github.filing.repo_skipped",
                    repo = %request.repo,
                    status = answer.status(),
                    body = %answer.short_text(),
                    "GitHub issue creation skipped for this Repo"
                );
                report.skipped.insert(request.repo.clone());
            }
            Action::SkipRow | Action::Rearm => {
                tracing::warn!(
                    name: "github.filing.create_failed",
                    entity = entity.as_str(),
                    entity_id = request.entity_id,
                    repo = %request.repo,
                    status = answer.status(),
                    body = %answer.short_text(),
                    "GitHub issue creation failed"
                );
            }
        }
    }
    report
}

/// What storing one filed issue's number found.
#[derive(Debug)]
enum FiledLink {
    /// The number is stored on the row.
    Stored,
    /// The row was deleted between the filing `POST` and this store.
    RowGone,
    /// The store failed, so the row may still be there and the issue is live.
    StoreFailed(DbError),
}

/// Records one filed issue's outcome, answering where the batch goes next.
///
/// A store that fails is not a deleted row: the issue stays open, and the batch
/// ends. Only a vanished row closes the issue just created.
async fn record_filed(
    db: &(impl ConnectionTrait + TransactionTrait),
    request: &FileRequest,
    answer: &Answer,
    entity: EntityKind,
    api: &Api,
    report: &mut BatchReport,
) -> ControlFlow<BatchStop> {
    let Some(number) = answer.issue_number() else {
        tracing::warn!(
            name: "github.filing.create_without_number",
            repo = %request.repo,
            body = %answer.short_text(),
            "GitHub issue creation returned 201 with no issue number"
        );
        return ControlFlow::Continue(());
    };
    match commit_filed_link(db, request, number).await {
        FiledLink::Stored => {
            report.linked.insert(request.entity_id);
            tracing::info!(
                name: "github.filing.filed",
                repo = %request.repo,
                number,
                entity = entity.as_str(),
                entity_id = request.entity_id,
                "filed a GitHub issue"
            );
        }
        FiledLink::RowGone => {
            // The row vanished mid-filing: the number is referenced nowhere, so
            // close the issue just created, best effort.
            let _ = close_issues_after_removal(&request.repo, &[number], api).await;
            tracing::warn!(
                name: "github.filing.row_gone",
                entity = entity.as_str(),
                entity_id = request.entity_id,
                repo = %request.repo,
                number,
                "row deleted (admin delete) while filing; closed the new issue"
            );
        }
        FiledLink::StoreFailed(error) => {
            // The reference lets this error propagate: the issue just created stays
            // open, and the batch ends here.
            tracing::error!(
                name: "github.filing.store_failed",
                error = %error,
                entity = entity.as_str(),
                entity_id = request.entity_id,
                repo = %request.repo,
                number,
                "GitHub filing could not store its link; the issue stays open and the \
                 batch stops"
            );
            return ControlFlow::Break(BatchStop::StoreFailed);
        }
    }
    ControlFlow::Continue(())
}

/// Stores the number GitHub just created, reporting what became of the row.
///
/// A savepoint, so a row deleted under us by an admin delete costs only this
/// filing rather than the rest of the batch; a vanished row is an `UPDATE` with
/// no affected row. A failed store is neither: the row may still exist, so the
/// caller must not treat it as gone.
async fn commit_filed_link(
    db: &(impl ConnectionTrait + TransactionTrait),
    request: &FileRequest,
    number: i32,
) -> FiledLink {
    let savepoint = match db.begin().await {
        Ok(savepoint) => savepoint,
        Err(error) => return FiledLink::StoreFailed(DbError::Database(error)),
    };
    let stored = issue_links::store_filed_link(
        &savepoint,
        request.target,
        i64::from(request.entity_id),
        number,
    )
    .await;
    // A vanished row is an UPDATE that affected no row: the admin delete won the
    // race, so the number is referenced nowhere and the new issue is stranded.
    let affected = match stored {
        Ok(affected) => affected,
        Err(error) => {
            let _ = savepoint.rollback().await;
            return FiledLink::StoreFailed(error);
        }
    };
    if affected == 0 {
        let _ = savepoint.rollback().await;
        return FiledLink::RowGone;
    }
    if let Err(error) = savepoint.commit().await {
        return FiledLink::StoreFailed(DbError::Database(error));
    }
    FiledLink::Stored
}
