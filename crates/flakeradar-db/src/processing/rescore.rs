//! The rescoring job: recompute a Test's or Job's flakiness from its score
//! window, write the rows the rescore changes, and rescore everything a Report
//! or a Default branch change touched.
//!
//! Every rule a rescore applies lives here: the windows that read the Executions,
//! the `Update` row the bulk write binds, and the old states the Tier changes are
//! judged against.

// Rust guideline compliant 2026-07-21

use std::collections::{BTreeMap, BTreeSet};

use flakeradar_core::{
    classify::FailureCategory, classify::dominant_category, helpers::next_flaky_since,
    model::EntityKind, model::ExecutionStatus, notify::TierChange, scoring::ScoredExecution,
    scoring::branch_scoped_fail_streak, scoring::branch_scoped_score,
    scoring::branch_scoped_streak, types::Score,
};
use sea_orm::entity::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DbBackend, QueryResult, Statement, Value};

use super::history::{self, DayCounts};
use super::{ProcessingSettings, notices_of, tier_changes};

use crate::attribution::{
    int_array, job_execution_explained_sql, job_window_ids_sql, test_window_ids_sql,
};
use crate::batching::chunks;
use crate::error::DbError;
use crate::sql::{execution_status, get, placeholder_list, rows, score, value_row};

/// The score window as a bind parameter: a setting far past `i32` is capped,
/// because the window counts rows, not a database column.
fn window_param(window: usize) -> i32 {
    i32::try_from(window).unwrap_or(i32::MAX)
}

/// One entity's rescore result: the row a bulk update writes.
///
/// A Test and a Job write the same Flakiness score, Proven flake count, Clean
/// streak and fixed-note reset. A Test also writes its Fail streak and its
/// dominant Failure category, which a Job has no column for, so [`TEST_UPDATES`]
/// writes those and [`JOB_UPDATES`] leaves them out.
#[derive(Debug, Clone, Copy)]
pub(super) struct Update {
    pub(super) id: i32,
    pub(super) flakiness_score: Score,
    pub(super) confirmed_flake_count: i32,
    pub(super) clean_streak: i32,
    pub(super) clear_fixed_note: bool,
    /// The Fail streak. Only a Test scores one.
    pub(super) fail_streak: i32,
    /// The dominant Failure category. Only a Test is categorised.
    pub(super) failure_category: Option<FailureCategory>,
    /// The row's flaky state, moved by [`next_flaky_since`] at the two thresholds.
    pub(super) flaky_since: Option<DateTimeWithTimeZone>,
}

/// Recomputes flakiness for the given Tests from their score window.
///
/// Flip scoring looks only at Executions on the Repo's Default branch, while
/// Proven flakes still count on every branch. `day_counts` adds each Test's
/// executions and failures to today's Score history row. A Clean streak of 0
/// also clears `github_issue_fixed_noted_at`; any other streak leaves it alone,
/// which is what the bulk update's per-row reset flag does.
///
/// One entity's scoring window: its newest Executions and its Repo's branch.
#[derive(Debug, Clone, Default)]
struct Window {
    /// Executions, newest first.
    executions: Vec<ScoredExecution>,
    /// The Failure categories of the same Executions, newest first. Only a Test
    /// is categorised, so a Job's window leaves this empty.
    categories: Vec<Option<FailureCategory>>,
    /// The Default branch of the entity's Repo, or `None` while it is unknown.
    default_branch: Option<String>,
}

impl Window {
    /// Adds one newest-first row: its Execution, and the Repo's Default branch.
    fn push(&mut self, row: &QueryResult, status: ExecutionStatus) -> Result<(), DbError> {
        self.executions.push(ScoredExecution {
            commit_sha: get(row, "commit_sha")?,
            branch: get(row, "branch")?,
            status,
        });
        self.default_branch = get(row, "default_branch")?;
        Ok(())
    }
}

/// The binds both window statements take: the ids and the window, twice.
fn window_binds(chunk: &[i32], settings: &ProcessingSettings) -> Vec<Value> {
    vec![
        int_array(chunk),
        Value::from(window_param(settings.score_window)),
        int_array(chunk),
        Value::from(window_param(settings.score_window)),
    ]
}

impl Update {
    /// The row `id`'s window scores to, starting from its stored flaky state.
    ///
    /// A row with no window scores as an empty one: no flip, no Proven flake and
    /// a Clean streak of 0. A row with no stored state is not flaky, so its
    /// `old_flaky_since` is `None`.
    fn of(
        id: i32,
        window: Option<&Window>,
        old_flaky_since: Option<DateTimeWithTimeZone>,
        settings: &ProcessingSettings,
    ) -> Self {
        let empty = Window::default();
        let window = window.unwrap_or(&empty);
        let branch = window.default_branch.as_deref();
        let (score, proven) = branch_scoped_score(
            &window.executions,
            branch,
            settings.score_decay,
            settings.score_window,
        );
        let streak = branch_scoped_streak(&window.executions, branch);
        let categories: Vec<Option<FailureCategory>> = window
            .categories
            .iter()
            .take(settings.score_window)
            .copied()
            .collect();
        Self {
            id,
            flakiness_score: score,
            confirmed_flake_count: i32::try_from(proven).unwrap_or(i32::MAX),
            clean_streak: i32::try_from(streak).unwrap_or(i32::MAX),
            clear_fixed_note: streak == 0,
            fail_streak: i32::try_from(branch_scoped_fail_streak(&window.executions, branch))
                .unwrap_or(i32::MAX),
            failure_category: dominant_category(&categories),
            flaky_since: next_flaky_since(
                old_flaky_since,
                score,
                settings.flake_threshold,
                settings.recover_threshold,
                settings.clock.now(),
            ),
        }
    }
}

/// The windows of every id, newest first.
async fn windows(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    ids: &[i32],
    settings: &ProcessingSettings,
) -> Result<BTreeMap<i32, Window>, DbError> {
    match kind {
        EntityKind::Test => test_windows(db, ids, settings).await,
        EntityKind::Job => job_windows(db, ids, settings).await,
    }
}

/// The Tests' scoring windows, with the Failure category of each Execution.
async fn test_windows(
    db: &impl ConnectionTrait,
    ids: &[i32],
    settings: &ProcessingSettings,
) -> Result<BTreeMap<i32, Window>, DbError> {
    let mut windows: BTreeMap<i32, Window> = BTreeMap::new();
    for chunk in chunks(ids) {
        let sql = format!(
            "SELECT e.test_case_id, r.commit_sha, r.branch, e.status, e.failure_category, ro.default_branch \
               FROM test_executions e \
               JOIN test_runs r ON r.id = e.test_run_id \
               JOIN projects p ON p.id = r.project_id \
               JOIN repos ro ON ro.id = p.repo_id \
              WHERE e.id IN (SELECT w.id FROM ({}) AS w) \
              ORDER BY e.test_case_id, e.id DESC",
            test_window_ids_sql()
        );
        for row in rows(db, sql, window_binds(chunk, settings)).await? {
            let id: i32 = get(&row, "test_case_id")?;
            let status = execution_status(&get::<String>(&row, "status")?)?;
            let window = windows.entry(id).or_default();
            window.push(&row, status)?;
            window.categories.push(
                get::<Option<String>>(&row, "failure_category")?
                    .as_deref()
                    .and_then(FailureCategory::from_wire),
            );
        }
    }
    Ok(windows)
}

/// The Jobs' scoring windows, newest first.
///
/// Explained failed Executions become `skipped`, so they never break a streak.
async fn job_windows(
    db: &impl ConnectionTrait,
    ids: &[i32],
    settings: &ProcessingSettings,
) -> Result<BTreeMap<i32, Window>, DbError> {
    let mut windows: BTreeMap<i32, Window> = BTreeMap::new();
    for chunk in chunks(ids) {
        // The table names are written out: `job_execution_explained_sql` reads
        // `job_executions`, `jobs` and `pipelines` by name, and a query that named
        // them by alias could not be spliced into this one.
        let sql = format!(
            "SELECT job_executions.job_id, job_executions.commit_sha, job_executions.branch, \
                    job_executions.status, ({}) AS explained, repos.default_branch \
               FROM job_executions \
               JOIN jobs ON jobs.id = job_executions.job_id \
               JOIN pipelines ON pipelines.id = jobs.pipeline_id \
               JOIN repos ON repos.id = pipelines.repo_id \
              WHERE job_executions.id IN (SELECT w.id FROM ({}) AS w) \
              ORDER BY job_executions.job_id, job_executions.id DESC",
            job_execution_explained_sql(),
            job_window_ids_sql()
        );
        for row in rows(db, sql, window_binds(chunk, settings)).await? {
            let id: i32 = get(&row, "job_id")?;
            let explained: bool = get(&row, "explained")?;
            let mut status = execution_status(&get::<String>(&row, "status")?)?;
            if explained && status == ExecutionStatus::Failed {
                status = ExecutionStatus::Skipped;
            }
            windows.entry(id).or_default().push(&row, status)?;
        }
    }
    Ok(windows)
}

/// Recomputes flakiness for `kind`'s rows, and writes the Tier changes it makes.
///
/// Every rule a rescore applies is here, once: the window scores the row, the
/// row's own stored state says whether its flaky state crossed, the bulk write
/// stores the row and today's Score history gains the day's counts. What differs
/// between a Test and a Job is a window statement, a write statement and a set of
/// Notices.
///
/// A row Acknowledged past the clock is still written, but sends no Notification,
/// and the skipped Notification is not sent when the acknowledgement ends.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a read or write fails.
pub async fn rescore(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    ids: &[i32],
    day_counts: Option<&BTreeMap<i32, DayCounts>>,
    settings: &ProcessingSettings,
) -> Result<Vec<TierChange>, DbError> {
    let windows = windows(db, kind, ids, settings).await?;
    // The stored state is read before the write, because `flaky_since` moves at
    // the thresholds relative to what the row holds now.
    let old = old_states(db, kind, ids).await?;
    let updates: Vec<Update> = ids
        .iter()
        .map(|id| {
            Update::of(
                *id,
                windows.get(id),
                old.get(id).and_then(|&(_, flaky_since, _)| flaky_since),
                settings,
            )
        })
        .collect();
    let changes = tier_changes(db, notices_of(kind), &updates, &old, settings).await?;
    write_updates(db, updates_of(kind), &updates).await?;
    history::write_day_counts(db, kind, &updates, day_counts, settings).await?;
    Ok(changes)
}

/// What one row holds before a rescore replaces it: its Score, its flaky state,
/// and its Fail streak. A Job stores no Fail streak, so its row carries `None`.
pub(super) type OldRow = (Score, Option<DateTimeWithTimeZone>, Option<i32>);

/// The scores and flaky states a rescore is about to replace, read before the update.
///
/// A row the caller lists that this read does not return has no stored state, so
/// it counts as not flaky.
async fn old_states(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    ids: &[i32],
) -> Result<BTreeMap<i32, OldRow>, DbError> {
    // Only a Test stores a Fail streak, so a Job's row reads none.
    let (from, fail_streak) = match kind {
        EntityKind::Test => (
            "SELECT id, flakiness_score, flaky_since, fail_streak FROM test_cases WHERE id IN ",
            true,
        ),
        EntityKind::Job => (
            "SELECT id, flakiness_score, flaky_since FROM jobs WHERE id IN ",
            false,
        ),
    };
    let mut scores = BTreeMap::new();
    for chunk in chunks(ids) {
        let sql = format!("{}({})", from, placeholder_list(1, chunk.len()));
        let values = chunk.iter().map(|id| Value::from(*id)).collect();
        for row in rows(db, sql, values).await? {
            scores.insert(
                get(&row, "id")?,
                (
                    score(get(&row, "flakiness_score")?, "flakiness_score")?,
                    get::<Option<DateTimeWithTimeZone>>(&row, "flaky_since")?,
                    if fail_streak {
                        Some(get::<i32>(&row, "fail_streak")?)
                    } else {
                        None
                    },
                ),
            );
        }
    }
    Ok(scores)
}

/// One rescore write: the statement, and the values one row binds into it.
///
/// The two halves are the SQL as PostgreSQL receives it, so a statement can be
/// grepped for what it sets. The rows go between them, and one row's own length
/// numbers the next row's placeholders, so the two statements cannot drift from
/// their binds.
struct BulkUpdate {
    /// The `UPDATE ... FROM (VALUES` the rows follow.
    head: &'static str,
    /// The `) AS v(...) WHERE ...` the rows precede.
    tail: &'static str,
    /// The values one update binds, in the order `tail` names its columns.
    row: fn(&Update) -> Vec<Value>,
}

/// The Tests' rescore write, which also stores the Failure category and Fail streak.
const TEST_UPDATES: BulkUpdate = BulkUpdate {
    head: "UPDATE test_cases AS t SET flakiness_score = v.flakiness_score, \
             confirmed_flake_count = v.confirmed_flake_count, \
             failure_category = v.failure_category, clean_streak = v.clean_streak, \
             fail_streak = v.fail_streak, flaky_since = v.flaky_since, \
             github_issue_fixed_noted_at = CASE WHEN v.reset_note THEN NULL ELSE t.github_issue_fixed_noted_at END \
           FROM (VALUES ",
    tail: ") AS v(id, flakiness_score, confirmed_flake_count, failure_category, clean_streak, \
             fail_streak, flaky_since, reset_note) WHERE t.id = v.id",
    row: |update| {
        vec![
            Value::from(update.id),
            Value::from(update.flakiness_score.as_f64()),
            Value::from(update.confirmed_flake_count),
            update
                .failure_category
                .map_or(Value::String(None), |category| {
                    Value::from(category.as_str())
                }),
            Value::from(update.clean_streak),
            Value::from(update.fail_streak),
            update
                .flaky_since
                .map_or(Value::ChronoDateTimeWithTimeZone(None), Value::from),
            Value::from(update.clear_fixed_note),
        ]
    },
};

/// The Jobs' rescore write: no Failure category is scored and no Fail streak kept.
const JOB_UPDATES: BulkUpdate = BulkUpdate {
    head: "UPDATE jobs AS j SET flakiness_score = v.flakiness_score, \
             confirmed_flake_count = v.confirmed_flake_count, clean_streak = v.clean_streak, \
             flaky_since = v.flaky_since, \
             github_issue_fixed_noted_at = CASE WHEN v.reset_note THEN NULL ELSE j.github_issue_fixed_noted_at END \
           FROM (VALUES ",
    tail: ") AS v(id, flakiness_score, confirmed_flake_count, clean_streak, flaky_since, \
             reset_note) WHERE j.id = v.id",
    row: |update| {
        vec![
            Value::from(update.id),
            Value::from(update.flakiness_score.as_f64()),
            Value::from(update.confirmed_flake_count),
            Value::from(update.clean_streak),
            update
                .flaky_since
                .map_or(Value::ChronoDateTimeWithTimeZone(None), Value::from),
            Value::from(update.clear_fixed_note),
        ]
    },
};

/// The rescore write for `kind`'s rows.
const fn updates_of(kind: EntityKind) -> &'static BulkUpdate {
    match kind {
        EntityKind::Test => &TEST_UPDATES,
        EntityKind::Job => &JOB_UPDATES,
    }
}

/// Writes one chunk-set of rescores, one statement per chunk.
///
/// The fixed-note reset is the same `CASE` in both statements: a Clean streak of 0
/// clears the note and any other streak leaves it alone.
async fn write_updates(
    db: &impl ConnectionTrait,
    updates: &BulkUpdate,
    rows: &[Update],
) -> Result<(), DbError> {
    for chunk in chunks(rows) {
        let mut values = Vec::new();
        let mut placeholders = Vec::with_capacity(chunk.len());
        for update in chunk {
            let row = (updates.row)(update);
            placeholders.push(value_row(values.len() + 1, row.len()));
            values.extend(row);
        }
        let sql = format!(
            "{}{}{}",
            updates.head,
            placeholders.join(", "),
            updates.tail
        );
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await?;
    }
    Ok(())
}

/// One Job's scoring input: its newest-first history and its Repo's Default branch.
#[derive(Debug, Clone, Default)]
pub struct JobHistory {
    /// The Default branch of the Job's Repo, or `None` while it is unknown.
    pub default_branch: Option<String>,
    /// Executions, newest first, with explained failures already turned into
    /// `skipped` so they never break a streak.
    pub executions: Vec<ScoredExecution>,
}

/// Newest-first Job-execution history for each Job, plus its Repo's Default branch.
///
/// Explained failed executions become `skipped`, so they never break a streak.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a read fails.
pub async fn job_history(
    db: &impl ConnectionTrait,
    job_ids: &[i32],
    settings: &ProcessingSettings,
) -> Result<BTreeMap<i32, JobHistory>, DbError> {
    let mut windows = job_windows(db, job_ids, settings).await?;
    Ok(job_ids
        .iter()
        .map(|id| {
            // The window is consumed: a Job's whole history is large, so it is
            // taken out of the map rather than copied out of it.
            let window = windows.remove(id).unwrap_or_default();
            (
                *id,
                JobHistory {
                    default_branch: window.default_branch,
                    executions: window.executions,
                },
            )
        })
        .collect())
}

/// Rescales every Job that carries this `ci_job_id` in the Repo.
pub(super) async fn rescore_jobs_for_ci_job_id(
    db: &impl ConnectionTrait,
    repo_id: i32,
    ci_job_id: &str,
    settings: &ProcessingSettings,
) -> Result<(Vec<i32>, Vec<TierChange>), DbError> {
    let mut job_ids: BTreeSet<i32> = BTreeSet::new();
    let sql = "SELECT job_executions.job_id FROM job_executions \
               JOIN jobs ON jobs.id = job_executions.job_id \
               JOIN pipelines ON pipelines.id = jobs.pipeline_id \
              WHERE job_executions.ci_job_id = $1 AND pipelines.repo_id = $2";
    for row in rows(db, sql, vec![Value::from(ci_job_id), Value::from(repo_id)]).await? {
        job_ids.insert(get(&row, "job_id")?);
    }
    let job_ids: Vec<i32> = job_ids.into_iter().collect();
    if job_ids.is_empty() {
        return Ok((job_ids, Vec::new()));
    }
    let changes = rescore(db, EntityKind::Job, &job_ids, None, settings).await?;
    Ok((job_ids, changes))
}

/// Rescores every Test and every Job in the Repo (chunked).
///
/// The Tier changes are dropped on purpose: a Default branch change rescores the
/// whole Repo at once, so notifying about it would send hundreds of events for
/// one Report (ADR 0009). Only their count is reported.
pub(super) async fn rescore_repo(
    db: &impl ConnectionTrait,
    repo_id: i32,
    settings: &ProcessingSettings,
) -> Result<(), DbError> {
    let mut test_ids = Vec::new();
    for row in rows(
        db,
        "SELECT t.id FROM test_cases t JOIN projects p ON p.id = t.project_id WHERE p.repo_id = $1"
            .to_owned(),
        vec![Value::from(repo_id)],
    )
    .await?
    {
        test_ids.push(get(&row, "id")?);
    }
    let mut job_ids = Vec::new();
    for row in rows(
        db,
        "SELECT j.id FROM jobs j JOIN pipelines p ON p.id = j.pipeline_id WHERE p.repo_id = $1"
            .to_owned(),
        vec![Value::from(repo_id)],
    )
    .await?
    {
        job_ids.push(get(&row, "id")?);
    }
    test_ids.sort_unstable();
    job_ids.sort_unstable();
    let mut dropped = rescore(db, EntityKind::Test, &test_ids, None, settings)
        .await?
        .len();
    dropped += rescore(db, EntityKind::Job, &job_ids, None, settings)
        .await?
        .len();
    if dropped > 0 {
        // One event per changed Test would flood the channel, so only the count
        // is reported (ADR 0009).
        tracing::info!(
            name: "db.processing.notifications_dropped",
            repo_id,
            dropped,
            "Rescore of a Repo dropped Notifications",
        );
    }
    Ok(())
}
