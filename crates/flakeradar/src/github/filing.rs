//! Filing an issue for every Test and Job that now meets the gate.
//!
//! The candidates are `flakeradar_db::filing`'s and the bodies are `bodies`', and
//! this file decides the batch's order and what a filing answer costs. A filing `POST` is never interrupted, because its issue would be
//! stranded; the batch stops before the next one once the leader's stop signal
//! fires. Never raises: an outage costs filings, not Report processing.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;

use chrono::TimeDelta;
use flakeradar_core::model::EntityKind;
use flakeradar_db::filing::{self, FilingGate};
use flakeradar_db::issue_links;
use flakeradar_db::{DbError, clock::SharedClock, reads::get_job};
use sea_orm::{ConnectionTrait, TransactionSession as _, TransactionTrait};
use tokio_util::sync::CancellationToken;

use super::bodies::{BODY_EXECUTIONS, job_issue_body, job_title, test_issue_body, test_title};
use super::client::{Answer, Api, Success};
use super::policy::{Action, BatchReport, BatchStop, Call, Operation, Verdict, batch_stop, step};
use super::sync::close_issues_after_removal;
use crate::settings::Settings;

/// The comment a reopen posts on the issue it reopens.
pub const REOPEN_COMMENT: &str =
    "FlakeRadar reopened this issue: the flaky test or CI job it tracks is flaky again.";

/// The issue-filing gate the configuration states.
#[must_use]
pub fn filing_gate(settings: &Settings) -> FilingGate {
    let github = settings.github();
    let fixed_streak = (github.fixed_streak > 0).then_some(github.fixed_streak);
    FilingGate::new(
        github.issue_min_score,
        github.issue_min_proven_flakes,
        github.issue_min_failures,
        fixed_streak,
    )
}

/// Files issues for the `kind` rows that now meet the filing gate. Never raises.
///
/// The two entities share the batch and differ in what an issue body reads: a Test
/// quotes its own Executions, a Job quotes its history, which the read settings
/// build from `clock`. Both gates read `clock` for the acknowledgement instant, so
/// an acknowledged row files nothing. One entry point states the gate, the emptiness
/// and the finished log once; `test_requests` and `job_requests` keep the two
/// candidate selections.
///
/// The report names the entity ids it linked, so the caller can keep the fixed-note
/// step off a link this same Report created.
pub async fn file_for(
    db: &(impl ConnectionTrait + TransactionTrait),
    kind: EntityKind,
    ids: &[i32],
    settings: &Settings,
    api: &Api,
    clock: &SharedClock,
    stop: &CancellationToken,
) -> BatchReport {
    if !api.configured() || ids.is_empty() {
        return BatchReport::default();
    }
    let batch = match kind {
        EntityKind::Test => Box::pin(test_requests(db, ids, settings, clock)).await,
        EntityKind::Job => Box::pin(job_requests(db, ids, settings, clock)).await,
    };
    if batch.is_empty() {
        return BatchReport::default();
    }
    let report = file_batch(db, batch, settings, api, stop).await;
    report.log(Operation::File);
    report
}

/// The Tests that pass the gate, each with the body its Executions make.
///
/// An Execution read that fails costs the body its table, not the filing: the
/// gate already decided this Test is worth an issue.
async fn test_requests(
    db: &(impl ConnectionTrait + TransactionTrait),
    ids: &[i32],
    settings: &Settings,
    clock: &SharedClock,
) -> Vec<FileRequest> {
    let candidates = match filing::test_candidates(
        db,
        ids,
        &filing_gate(settings),
        settings.score_window,
        clock.now(),
    )
    .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(
                name: "github.filing.test_candidates_unreadable",
                error = %error,
                "GitHub issue filing could not read its Test candidates",
            );
            return Vec::new();
        }
    };
    let ids: Vec<i32> = candidates
        .iter()
        .map(|candidate| candidate.test.id)
        .collect();
    let reopen = reopen_numbers(db, EntityKind::Test, &ids, settings, clock).await;
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
            reopen: reopen.get(&candidate.test.id).copied(),
            ..FileRequest::new(
                EntityKind::Test,
                candidate.test.id,
                candidate.repo,
                title,
                body,
            )
        });
    }
    batch
}

/// The github-provider Jobs that pass the gate, each with the body its history makes.
async fn job_requests(
    db: &(impl ConnectionTrait + TransactionTrait),
    ids: &[i32],
    settings: &Settings,
    clock: &SharedClock,
) -> Vec<FileRequest> {
    let candidates = match filing::job_candidates(
        db,
        ids,
        &filing_gate(settings),
        settings.score_window,
        clock.now(),
    )
    .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(
                name: "github.filing.job_candidates_unreadable",
                error = %error,
                "GitHub issue filing could not read its Job candidates",
            );
            return Vec::new();
        }
    };
    let ids: Vec<i32> = candidates
        .iter()
        .map(|candidate| candidate.job.id)
        .collect();
    let reopen = reopen_numbers(db, EntityKind::Job, &ids, settings, clock).await;
    let mut batch = Vec::new();
    for candidate in candidates {
        let history = match get_job(
            db,
            i64::from(candidate.job.id),
            &settings.reads(std::sync::Arc::clone(clock)),
            BODY_EXECUTIONS,
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
            reopen: reopen.get(&candidate.job.id).copied(),
            ..FileRequest::new(
                EntityKind::Job,
                candidate.job.id,
                candidate.repo,
                title,
                body,
            )
        });
    }
    batch
}

/// The largest reopen window `TimeDelta` can represent, in days.
///
/// The window is `clock now - last_closed <= days`; `chrono` cannot represent a
/// duration wider than `i64::MAX` milliseconds, so a larger setting is clamped
/// rather than panicking. A window this wide keeps every remembered close,
/// which is what a setting that large meant anyway.
const MAX_REOPEN_DAYS: i64 = 100_000;

/// The issue numbers a reopen may revive, keyed by the entity id that remembers them.
///
/// A row whose completed close the issue-state sync stored within
/// `github_reopen_days` before `clock` now reopens that issue instead of filing
/// a fresh one; a close older than the window, or the setting at 0, files fresh.
/// A read that fails costs the reopen, not the filing: the row still files a new
/// issue.
async fn reopen_numbers(
    db: &(impl ConnectionTrait + TransactionTrait),
    kind: EntityKind,
    ids: &[i32],
    settings: &Settings,
    clock: &SharedClock,
) -> BTreeMap<i32, i32> {
    let github = settings.github();
    if github.reopen_days == 0 || ids.is_empty() {
        return BTreeMap::new();
    }
    let closed = match issue_links::last_closed(db, kind, ids).await {
        Ok(closed) => closed,
        Err(error) => {
            tracing::warn!(
                name: "github.filing.last_closed_unreadable",
                error = %error,
                entity = kind.as_str(),
                "GitHub issue filing could not read the closes a re-arm remembered",
            );
            return BTreeMap::new();
        }
    };
    let window = TimeDelta::days(github.reopen_days.clamp(0, MAX_REOPEN_DAYS));
    closed
        .into_iter()
        .filter(|(_, (_, at))| clock.now().signed_duration_since(*at) <= window)
        .map(|(id, (number, _))| (id, number))
        .collect()
}

/// One rendered issue to file.
#[derive(Debug, Clone)]
struct FileRequest {
    target: EntityKind,
    entity_id: i32,
    repo: String,
    title: String,
    body: String,
    /// The issue a recent completed close cleared, when this filing reopens it.
    reopen: Option<i32>,
}

impl FileRequest {
    /// One rendered issue, from the entity it was rendered for.
    #[must_use]
    fn new(target: EntityKind, entity_id: i32, repo: String, title: String, body: String) -> Self {
        Self {
            target,
            entity_id,
            repo,
            title,
            body,
            reopen: None,
        }
    }
}

/// What one row of the filing batch came to.
///
/// The batch holds the state that spans rows — which Repos it stopped asking, and
/// why it ended — so a row answers what happened rather than writing into the
/// report behind its back.
enum Filed {
    /// The link to the issue GitHub just created is stored on this entity.
    Linked(i32),
    /// Nothing was filed for this row, and the batch carries on.
    Next,
    /// GitHub will not take this Repo's calls for the rest of the batch.
    SkipRepo,
    /// The batch is over, because of this.
    Stop(BatchStop),
}

/// Files one batch of rendered issues, whichever entity they describe.
///
/// A filing `POST` is never interrupted, because its issue would be stranded; the
/// batch stops before the next one once `stop` fires.
async fn file_batch(
    db: &(impl ConnectionTrait + TransactionTrait),
    batch: Vec<FileRequest>,
    settings: &Settings,
    api: &Api,
    stop: &CancellationToken,
) -> BatchReport {
    let mut report = BatchReport::default();
    for request in batch {
        if stop.is_cancelled() {
            tracing::info!(
                name: "github.filing.stopped",
                entity = request.target.as_str(),
                "stopping the issue-filing batch: leadership ended"
            );
            report.stopped = Some(BatchStop::Stopped);
            break;
        }
        if report.skipped.contains(&request.repo) {
            continue;
        }
        match file_one(db, &request, settings, api).await {
            Filed::Linked(entity_id) => {
                report.linked.insert(entity_id);
            }
            Filed::Next => {}
            Filed::SkipRepo => {
                report.skipped.insert(request.repo.clone());
            }
            Filed::Stop(stop_reason) => {
                report.stopped = Some(stop_reason);
                break;
            }
        }
    }
    report
}

/// Files one issue and stores its number, answering which way the batch goes.
async fn file_one(
    db: &(impl ConnectionTrait + TransactionTrait),
    request: &FileRequest,
    settings: &Settings,
    api: &Api,
) -> Filed {
    if let Some(number) = request.reopen {
        match reopen_one(db, request, number, api).await {
            Reopened::Decided(filed) => return filed,
            // The issue is gone; the create below files a fresh one.
            Reopened::Rearm => {}
        }
    }
    let call = step(
        Operation::File,
        Success::Created,
        // Boxed so the batch's frame holds a pointer rather than the request's own
        // future while `step` runs.
        Box::pin(api.create_issue(
            &request.repo,
            &request.title,
            &request.body,
            &settings.github().issue_label,
        )),
        |error| {
            tracing::warn!(
                name: "github.filing.unreachable",
                error = %error,
                entity = request.target.as_str(),
                "GitHub unreachable, skipping issue filing",
            );
        },
    )
    .await;
    let (answer, filing) = match call {
        Call::Answered { answer, action, .. } => (answer, action),
        // Filing never carries on without an answer, which is the table's own
        // `File`/`Unreachable` cell.
        Call::Unreachable { .. } => return Filed::Stop(call.stop()),
    };
    match filing {
        Action::Use => record_filed(db, request, &answer, api).await,
        Action::Stop => {
            tracing::warn!(
                name: "github.filing.rate_limited",
                remaining = %answer.remaining_text(),
                "GitHub rate limit; stopping the filing batch"
            );
            Filed::Stop(batch_stop(answer.classify(Success::Created)))
        }
        Action::SkipRepo => {
            tracing::warn!(
                name: "github.filing.repo_skipped",
                repo = %request.repo,
                status = answer.status(),
                body = %answer.short_text(),
                "GitHub issue creation skipped for this Repo"
            );
            Filed::SkipRepo
        }
        Action::SkipRow | Action::Rearm => {
            tracing::warn!(
                name: "github.filing.create_failed",
                entity = request.target.as_str(),
                entity_id = request.entity_id,
                repo = %request.repo,
                status = answer.status(),
                body = %answer.short_text(),
                "GitHub issue creation failed"
            );
            Filed::Next
        }
    }
}

/// What one reopen attempt came to.
enum Reopened {
    /// The batch's direction for this row is decided.
    Decided(Filed),
    /// The issue is gone, so the create path files a fresh one.
    Rearm,
}

/// Reopens one issue whose completed close the sync remembered, and stores its link.
///
/// The comment is best effort: a comment that fails leaves the reopen and the
/// link standing, because the issue is open again either way. On a vanished row
/// the freshly reopened issue stays open — closing it would blame a project
/// deletion that never happened.
async fn reopen_one(
    db: &(impl ConnectionTrait + TransactionTrait),
    request: &FileRequest,
    number: i32,
    api: &Api,
) -> Reopened {
    let call = step(
        Operation::Reopen,
        Success::Fetched,
        // Boxed so the batch's frame holds a pointer rather than the request's own
        // future while `step` runs.
        Box::pin(api.reopen_issue(&request.repo, number)),
        |error| {
            tracing::warn!(
                name: "github.filing.reopen_unreachable",
                error = %error,
                entity = request.target.as_str(),
                "GitHub unreachable, skipping issue filing",
            );
        },
    )
    .await;
    let (answer, filing) = match call {
        Call::Answered { answer, action, .. } => (answer, action),
        // A reopen never carries on without an answer, which is the table's own
        // `Reopen`/`Unreachable` cell.
        Call::Unreachable { .. } => return Reopened::Decided(Filed::Stop(call.stop())),
    };
    match filing {
        Action::Use => {
            comment_on_reopen(request, number, api).await;
            store_reopened_link(db, request, number).await
        }
        Action::Stop => {
            tracing::warn!(
                name: "github.filing.rate_limited",
                remaining = %answer.remaining_text(),
                "GitHub rate limit; stopping the filing batch"
            );
            Reopened::Decided(Filed::Stop(batch_stop(answer.classify(Success::Fetched))))
        }
        Action::SkipRepo => {
            tracing::warn!(
                name: "github.filing.repo_skipped",
                repo = %request.repo,
                status = answer.status(),
                body = %answer.short_text(),
                "GitHub issue reopen skipped for this Repo"
            );
            Reopened::Decided(Filed::SkipRepo)
        }
        Action::SkipRow => {
            tracing::warn!(
                name: "github.filing.reopen_failed",
                entity = request.target.as_str(),
                entity_id = request.entity_id,
                repo = %request.repo,
                status = answer.status(),
                body = %answer.short_text(),
                "GitHub issue reopen failed"
            );
            Reopened::Decided(Filed::Next)
        }
        Action::Rearm => {
            tracing::info!(
                name: "github.filing.reopen_gone",
                entity = request.target.as_str(),
                entity_id = request.entity_id,
                repo = %request.repo,
                number,
                status = answer.status(),
                "the issue to reopen is gone; filing a fresh issue"
            );
            Reopened::Rearm
        }
    }
}

/// Stores the reopened issue's link, logging what became of the row.
///
/// The log names and the answers mirror `record_filed`'s, except that a vanished
/// row does not close the issue: it was already open before this batch ran.
async fn store_reopened_link(
    db: &(impl ConnectionTrait + TransactionTrait),
    request: &FileRequest,
    number: i32,
) -> Reopened {
    match commit_filed_link(db, request, number).await {
        FiledLink::Stored => {
            tracing::info!(
                name: "github.filing.reopened",
                repo = %request.repo,
                number,
                entity = request.target.as_str(),
                entity_id = request.entity_id,
                "reopened a GitHub issue"
            );
            Reopened::Decided(Filed::Linked(request.entity_id))
        }
        FiledLink::RowGone => {
            tracing::warn!(
                name: "github.filing.reopen_row_gone",
                entity = request.target.as_str(),
                entity_id = request.entity_id,
                repo = %request.repo,
                number,
                "row deleted (admin delete) while reopening; left the issue open"
            );
            Reopened::Decided(Filed::Next)
        }
        FiledLink::StoreFailed(error) => {
            // A link this server cannot store ends the outcome here: the reopened
            // issue stays open, and no later batch runs.
            tracing::error!(
                name: "github.filing.store_failed",
                error = %error,
                entity = request.target.as_str(),
                entity_id = request.entity_id,
                repo = %request.repo,
                number,
                "GitHub filing could not store its link; the reopened issue stays \
                 open and the batch stops"
            );
            Reopened::Decided(Filed::Stop(BatchStop::StoreFailed))
        }
    }
}

/// Posts the reopen comment, best effort: a failure is logged, nothing undone.
async fn comment_on_reopen(request: &FileRequest, number: i32, api: &Api) {
    match api.comment(&request.repo, number, REOPEN_COMMENT).await {
        Ok(answer) if answer.classify(Success::Created) == Verdict::Accepted => {}
        Ok(answer) => {
            tracing::warn!(
                name: "github.filing.reopen_comment_failed",
                repo = %request.repo,
                number,
                status = answer.status(),
                body = %answer.short_text(),
                "the reopen comment failed; the reopen and its link stand"
            );
        }
        Err(error) => {
            tracing::warn!(
                name: "github.filing.reopen_comment_failed",
                error = %error,
                repo = %request.repo,
                number,
                "the reopen comment failed; the reopen and its link stand"
            );
        }
    }
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
    api: &Api,
) -> Filed {
    let Some(number) = answer.issue_number() else {
        tracing::warn!(
            name: "github.filing.create_without_number",
            repo = %request.repo,
            body = %answer.short_text(),
            "GitHub issue creation returned 201 with no issue number"
        );
        return Filed::Next;
    };
    match commit_filed_link(db, request, number).await {
        FiledLink::Stored => {
            tracing::info!(
                name: "github.filing.filed",
                repo = %request.repo,
                number,
                entity = request.target.as_str(),
                entity_id = request.entity_id,
                "filed a GitHub issue"
            );
            Filed::Linked(request.entity_id)
        }
        FiledLink::RowGone => {
            // The row vanished mid-filing: the number is referenced nowhere, so
            // close the issue just created, best effort.
            let _ = close_issues_after_removal(&request.repo, &[number], api).await;
            tracing::warn!(
                name: "github.filing.row_gone",
                entity = request.target.as_str(),
                entity_id = request.entity_id,
                repo = %request.repo,
                number,
                "row deleted (admin delete) while filing; closed the new issue"
            );
            Filed::Next
        }
        FiledLink::StoreFailed(error) => {
            // A link this server cannot store ends the outcome here: the issue
            // just created stays open, and no later batch runs.
            tracing::error!(
                name: "github.filing.store_failed",
                error = %error,
                entity = request.target.as_str(),
                entity_id = request.entity_id,
                repo = %request.repo,
                number,
                "GitHub filing could not store its link; the issue stays open and the \
                 batch stops"
            );
            Filed::Stop(BatchStop::StoreFailed)
        }
    }
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
