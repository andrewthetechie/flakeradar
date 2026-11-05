//! Retention: delete old Reports, Executions and Score history in bounded batches.
//!
//! This is the frozen Python server's `retention.py`. Each round deletes at most
//! `batch` rows per table, oldest id first, and is its own committed
//! transaction, so a large backlog drains a few thousand rows per interval
//! instead of issuing one delete that the statement timeout would cancel. A run
//! repeats until a round deletes less than a full batch from every table, or
//! until its time budget is spent; running out of budget is a normal return, and
//! the next interval resumes where the committed batches left off.
//!
//! Every round rescores the Tests and Jobs whose Executions it deleted, in the
//! same transaction, so a Test or Job that stopped running decays to a score of
//! 0 and leaves the leaderboard instead of keeping its last cached score
//! forever. Losing old Executions is not news, so the Tier changes that rescore
//! produces are dropped and only their count is logged (ADR 0009).

// Rust guideline compliant 2026-07-21

use chrono::{NaiveDate, TimeDelta};
use sea_orm::entity::prelude::DateTimeWithTimeZone;
use sea_orm::sea_query::{Expr, ExprTrait as _, Order, Query};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder as _,
    QuerySelect, TransactionTrait,
};
use tracing::info;

use crate::clock::MonotonicClock;
use crate::entities::{
    job_executions, job_score_history, reports, test_cases, test_executions, test_runs,
    test_score_history,
};
use crate::error::DbError;
use crate::processing::{ProcessingSettings, rescore, rescore_jobs};
use crate::report_intake::{REPORT_FAILED, REPORT_PROCESSED};

/// The largest retention window a setting may ask for, in days.
///
/// The cutoff is `now - days`; `chrono` cannot represent an instant more than
/// about 262 000 years from the epoch, so a larger setting is clamped rather than
/// panicking. A window this wide keeps every row, which is what a setting that
/// large meant anyway.
const MAX_RETENTION_DAYS: i64 = 100_000;

/// The settings one retention run reads.
///
/// The binary's `Settings` builds one of these; the database layer cannot depend
/// on the binary's config module, and a test states only the values it changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetentionSettings {
    /// How many days a processed Report is kept.
    pub report_days: i64,
    /// How many days a failed Report is kept; `0` keeps them until a human acts.
    pub failed_report_days: i64,
    /// How many days an Execution, and a Run left empty, is kept.
    pub execution_days: i64,
    /// How many days a Score history row is kept.
    pub history_days: i64,
    /// How many rows one round deletes per table.
    pub batch: i64,
    /// How long one run may keep pruning before the next interval takes over.
    pub time_budget_seconds: f64,
}

impl Default for RetentionSettings {
    fn default() -> Self {
        Self {
            report_days: 7,
            failed_report_days: 0,
            execution_days: 90,
            history_days: 365,
            batch: 5000,
            time_budget_seconds: 60.0,
        }
    }
}

/// What one retention round, or one whole run, deleted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneResult {
    /// Processed Reports deleted.
    pub reports: u64,
    /// Failed Reports deleted by the optional `failed_report_days` setting.
    pub failed_reports: u64,
    /// Executions deleted.
    pub executions: u64,
    /// Runs deleted once their last Execution was gone.
    pub runs: u64,
    /// Job executions deleted.
    pub job_executions: u64,
    /// Test Score history rows deleted.
    pub test_score_history: u64,
    /// Job Score history rows deleted.
    pub job_score_history: u64,
    /// Quarantines whose human-set expiry had passed, cleared in this round.
    pub expired_quarantines: u64,
}

impl PruneResult {
    /// Score history rows deleted, from both history tables.
    #[must_use]
    pub fn score_history(&self) -> u64 {
        self.test_score_history + self.job_score_history
    }

    /// Adds one round's counts to a run's totals.
    fn accumulate(&mut self, round: &Self) {
        self.reports += round.reports;
        self.failed_reports += round.failed_reports;
        self.executions += round.executions;
        self.runs += round.runs;
        self.job_executions += round.job_executions;
        self.test_score_history += round.test_score_history;
        self.job_score_history += round.job_score_history;
        self.expired_quarantines += round.expired_quarantines;
    }

    /// Whether one of this round's tables deleted a full batch, so more may remain.
    ///
    /// The two Score history tables are checked separately: a full batch in each
    /// sums to twice the batch size, which a check on the sum would miss.
    fn is_full(&self, batch: u64) -> bool {
        [
            self.reports,
            self.failed_reports,
            self.executions,
            self.runs,
            self.job_executions,
            self.test_score_history,
            self.job_score_history,
        ]
        .into_iter()
        .any(|count| count == batch)
    }
}

/// The instant `days` before `now`.
pub(crate) fn cutoff(now: DateTimeWithTimeZone, days: i64) -> DateTimeWithTimeZone {
    now - TimeDelta::days(days.clamp(-MAX_RETENTION_DAYS, MAX_RETENTION_DAYS))
}

/// A row limit as a count PostgreSQL accepts: a negative setting deletes nothing
/// per round rather than failing the query.
fn batch_limit(batch: i64) -> u64 {
    u64::try_from(batch).unwrap_or_default()
}

/// Deletes one round of expired rows: at most `retention.batch` per table.
///
/// The caller owns the transaction, so the deletes and the rescore they cause
/// commit together. A count equal to the batch size means that table may have
/// more expired rows, so another round is worth running.
///
/// Failed Reports are kept forever unless `failed_report_days` is positive;
/// their age starts when they failed, or when they were ingested if they never
/// got that far.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails; the caller's
/// transaction is then rolled back and nothing in this round is kept.
pub async fn prune(
    db: &impl ConnectionTrait,
    now: DateTimeWithTimeZone,
    retention: &RetentionSettings,
    processing: &ProcessingSettings,
) -> Result<PruneResult, DbError> {
    let batch = batch_limit(retention.batch);
    let reports = delete_processed_reports(db, cutoff(now, retention.report_days), batch).await?;
    // Zero and negative values keep the frozen server's behaviour of never
    // pruning a failed Report, so a misconfigured zero cannot empty the queue.
    let failed_reports = if retention.failed_report_days > 0 {
        delete_failed_reports(db, cutoff(now, retention.failed_report_days), batch).await?
    } else {
        0
    };

    let execution_cutoff = cutoff(now, retention.execution_days);
    // The ids of this batch are read before the delete, so the Tests and Jobs
    // that lose Executions can still be rescored afterwards.
    let (executions, stale_tests) = expire_test_executions(db, execution_cutoff, batch).await?;
    let (job_executions, stale_jobs) = expire_job_executions(db, execution_cutoff, batch).await?;
    // Runs only become deletable once their last Execution is gone.
    let runs = delete_empty_runs(db, execution_cutoff, batch).await?;
    let test_score_history =
        delete_test_history(db, cutoff(now, retention.history_days).date_naive(), batch).await?;
    let job_score_history =
        delete_job_history(db, cutoff(now, retention.history_days).date_naive(), batch).await?;

    drop_silent_tier_changes(db, &stale_tests, &stale_jobs, processing).await?;
    let expired_quarantines = clear_expired_quarantines(db, now).await?;

    Ok(PruneResult {
        reports,
        failed_reports,
        executions,
        runs,
        job_executions,
        test_score_history,
        job_score_history,
        expired_quarantines,
    })
}

/// Deletes one batch of processed Reports older than `cutoff`.
async fn delete_processed_reports(
    db: &impl ConnectionTrait,
    cutoff: DateTimeWithTimeZone,
    batch: u64,
) -> Result<u64, DbError> {
    Ok(reports::Entity::delete_many()
        .filter(
            reports::Column::Id.in_subquery(
                Query::select()
                    .column(reports::Column::Id)
                    .from(reports::Entity)
                    .and_where(reports::Column::Status.eq(REPORT_PROCESSED))
                    .and_where(reports::Column::ProcessedAt.lt(cutoff))
                    .order_by(reports::Column::Id, Order::Asc)
                    .limit(batch)
                    .to_owned(),
            ),
        )
        .exec(db)
        .await?
        .rows_affected)
}

/// Deletes one batch of failed Reports older than `cutoff`.
///
/// A failed Report carries the moment it failed; the fallback covers a row that
/// failed before it was processed.
async fn delete_failed_reports(
    db: &impl ConnectionTrait,
    cutoff: DateTimeWithTimeZone,
    batch: u64,
) -> Result<u64, DbError> {
    Ok(reports::Entity::delete_many()
        .filter(
            reports::Column::Id.in_subquery(
                Query::select()
                    .column(reports::Column::Id)
                    .from(reports::Entity)
                    .and_where(reports::Column::Status.eq(REPORT_FAILED))
                    .and_where(Expr::cust_with_values(
                        "COALESCE(processed_at, created_at) < $1",
                        [cutoff],
                    ))
                    .order_by(reports::Column::Id, Order::Asc)
                    .limit(batch)
                    .to_owned(),
            ),
        )
        .exec(db)
        .await?
        .rows_affected)
}

/// Deletes one batch of old Test executions, naming the Tests that owned them.
async fn expire_test_executions(
    db: &impl ConnectionTrait,
    cutoff: DateTimeWithTimeZone,
    batch: u64,
) -> Result<(u64, Vec<i32>), DbError> {
    let ids: Vec<i64> = test_executions::Entity::find()
        .select_only()
        .column(test_executions::Column::Id)
        .filter(test_executions::Column::CreatedAt.lt(cutoff))
        .order_by_asc(test_executions::Column::Id)
        .limit(batch)
        .into_tuple::<i64>()
        .all(db)
        .await?;
    if ids.is_empty() {
        return Ok((0, Vec::new()));
    }
    let mut owners: Vec<i32> = test_executions::Entity::find()
        .select_only()
        .column(test_executions::Column::TestCaseId)
        .filter(test_executions::Column::Id.is_in(ids.clone()))
        .distinct()
        .into_tuple::<i32>()
        .all(db)
        .await?;
    owners.sort_unstable();
    let deleted = test_executions::Entity::delete_many()
        .filter(test_executions::Column::Id.is_in(ids))
        .exec(db)
        .await?
        .rows_affected;
    Ok((deleted, owners))
}

/// Deletes one batch of old Job executions, naming the Jobs that owned them.
async fn expire_job_executions(
    db: &impl ConnectionTrait,
    cutoff: DateTimeWithTimeZone,
    batch: u64,
) -> Result<(u64, Vec<i32>), DbError> {
    let ids: Vec<i64> = job_executions::Entity::find()
        .select_only()
        .column(job_executions::Column::Id)
        .filter(job_executions::Column::CreatedAt.lt(cutoff))
        .order_by_asc(job_executions::Column::Id)
        .limit(batch)
        .into_tuple::<i64>()
        .all(db)
        .await?;
    if ids.is_empty() {
        return Ok((0, Vec::new()));
    }
    let mut owners: Vec<i32> = job_executions::Entity::find()
        .select_only()
        .column(job_executions::Column::JobId)
        .filter(job_executions::Column::Id.is_in(ids.clone()))
        .distinct()
        .into_tuple::<i32>()
        .all(db)
        .await?;
    owners.sort_unstable();
    let deleted = job_executions::Entity::delete_many()
        .filter(job_executions::Column::Id.is_in(ids))
        .exec(db)
        .await?
        .rows_affected;
    Ok((deleted, owners))
}

/// Deletes one batch of Runs old enough to expire and left with no Execution.
async fn delete_empty_runs(
    db: &impl ConnectionTrait,
    cutoff: DateTimeWithTimeZone,
    batch: u64,
) -> Result<u64, DbError> {
    Ok(test_runs::Entity::delete_many()
        .filter(
            test_runs::Column::Id.in_subquery(
                Query::select()
                    .column(test_runs::Column::Id)
                    .from(test_runs::Entity)
                    .and_where(test_runs::Column::CreatedAt.lt(cutoff))
                    .and_where(
                        Expr::exists(
                            Query::select()
                                .expr(Expr::val(1))
                                .from(test_executions::Entity)
                                .and_where(
                                    Expr::col((
                                        test_executions::Entity,
                                        test_executions::Column::TestRunId,
                                    ))
                                    .eq(Expr::col((test_runs::Entity, test_runs::Column::Id))),
                                )
                                .to_owned(),
                        )
                        .not(),
                    )
                    .order_by(test_runs::Column::Id, Order::Asc)
                    .limit(batch)
                    .to_owned(),
            ),
        )
        .exec(db)
        .await?
        .rows_affected)
}

/// Deletes one batch of Test Score history rows by their (Test, day) key.
async fn delete_test_history(
    db: &impl ConnectionTrait,
    cutoff: NaiveDate,
    batch: u64,
) -> Result<u64, DbError> {
    Ok(test_score_history::Entity::delete_many()
        .filter(
            Expr::tuple([
                Expr::col((
                    test_score_history::Entity,
                    test_score_history::Column::TestCaseId,
                )),
                Expr::col((test_score_history::Entity, test_score_history::Column::Day)),
            ])
            .in_subquery(
                Query::select()
                    .column(test_score_history::Column::TestCaseId)
                    .column(test_score_history::Column::Day)
                    .from(test_score_history::Entity)
                    .and_where(test_score_history::Column::Day.lt(cutoff))
                    .order_by(test_score_history::Column::Day, Order::Asc)
                    .limit(batch)
                    .to_owned(),
            ),
        )
        .exec(db)
        .await?
        .rows_affected)
}

/// Deletes one batch of Job Score history rows by their (Job, day) key.
async fn delete_job_history(
    db: &impl ConnectionTrait,
    cutoff: NaiveDate,
    batch: u64,
) -> Result<u64, DbError> {
    Ok(job_score_history::Entity::delete_many()
        .filter(
            Expr::tuple([
                Expr::col((job_score_history::Entity, job_score_history::Column::JobId)),
                Expr::col((job_score_history::Entity, job_score_history::Column::Day)),
            ])
            .in_subquery(
                Query::select()
                    .column(job_score_history::Column::JobId)
                    .column(job_score_history::Column::Day)
                    .from(job_score_history::Entity)
                    .and_where(job_score_history::Column::Day.lt(cutoff))
                    .order_by(job_score_history::Column::Day, Order::Asc)
                    .limit(batch)
                    .to_owned(),
            ),
        )
        .exec(db)
        .await?
        .rows_affected)
}

/// Rescores what lost Executions, dropping the Tier changes that causes.
///
/// A Test with no Executions left scores 0 and leaves the leaderboard, and one
/// with Executions left is scored on what remains. Losing old Executions is not
/// news, so the Notifications this would send are counted and dropped (ADR 0009).
async fn drop_silent_tier_changes(
    db: &impl ConnectionTrait,
    stale_tests: &[i32],
    stale_jobs: &[i32],
    processing: &ProcessingSettings,
) -> Result<(), DbError> {
    let mut dropped = 0;
    if !stale_tests.is_empty() {
        dropped += rescore(db, stale_tests, None, processing).await?.len();
    }
    if !stale_jobs.is_empty() {
        dropped += rescore_jobs(db, stale_jobs, None, processing).await?.len();
    }
    if dropped > 0 {
        info!(dropped, "Prune dropped Notifications");
    }
    Ok(())
}

/// Clears every quarantine whose human-set expiry has passed, all in this round.
async fn clear_expired_quarantines(
    db: &impl ConnectionTrait,
    now: DateTimeWithTimeZone,
) -> Result<u64, DbError> {
    let due = test_cases::Entity::find()
        .filter(test_cases::Column::Quarantined.eq(true))
        .filter(test_cases::Column::QuarantineExpiresAt.lte(now))
        .all(db)
        .await?;
    for test in &due {
        info!(test_id = test.id, name = %test.name, "Quarantine expired; cleared");
        test_cases::Entity::update(test_cases::ActiveModel {
            id: sea_orm::Set(test.id),
            quarantined: sea_orm::Set(false),
            quarantined_at: sea_orm::Set(None),
            quarantine_reason: sea_orm::Set(None),
            quarantine_expires_at: sea_orm::Set(None),
            ..Default::default()
        })
        .exec(db)
        .await?;
    }
    Ok(u64::try_from(due.len()).unwrap_or(u64::MAX))
}

/// Deletes the expired backlog in committed batches, stopping at the time budget.
///
/// Every round is committed before the next one starts, so a later failure
/// cannot roll back the rows already deleted. Running out of budget is a normal
/// return: the caller sees no error and the next interval picks up the rest.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails, after rolling that
/// round back.
pub async fn run_prune(
    db: &DatabaseConnection,
    retention: &RetentionSettings,
    processing: &ProcessingSettings,
    monotonic: &dyn MonotonicClock,
) -> Result<PruneResult, DbError> {
    let batch = batch_limit(retention.batch);
    let deadline = monotonic.now() + retention.time_budget_seconds;
    let mut totals = PruneResult::default();
    loop {
        let transaction = db.begin().await?;
        let round = match prune(&transaction, processing.clock.now(), retention, processing).await {
            Ok(round) => round,
            Err(error) => {
                transaction.rollback().await?;
                return Err(error);
            }
        };
        transaction.commit().await?;
        totals.accumulate(&round);
        if !round.is_full(batch) {
            break;
        }
        if monotonic.now() >= deadline {
            info!(
                "Retention prune hit its {}s budget; the rest waits for the next interval",
                retention.time_budget_seconds
            );
            break;
        }
    }
    // The frozen server's own summary line, with the counts in the same order.
    info!(
        "Pruned {} reports, {} failed reports, {} executions, {} job executions, {} runs, \
         {} score history rows, cleared {} expired quarantines",
        totals.reports,
        totals.failed_reports,
        totals.executions,
        totals.job_executions,
        totals.runs,
        totals.score_history(),
        totals.expired_quarantines
    );
    Ok(totals)
}
