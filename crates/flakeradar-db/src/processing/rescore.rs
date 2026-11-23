//! The rescoring job: recompute a Test's or Job's flakiness from its score
//! window, write the rows the rescore changes, and rescore everything a Report
//! or a Default branch change touched.
//!
//! The I/O a rescore needs lives here: the windows that read the Executions, the
//! stored states the decision reads, the `Update` row the bulk write binds, and
//! today's Score history. What a rescore decides — the new values, the Tier
//! crossing and the broken change — is [`decision::decide`], which needs no
//! connection.

// Rust guideline compliant 2026-07-21

use std::collections::{BTreeMap, BTreeSet};

use flakeradar_core::{
    classify::FailureCategory, decision, model::EntityKind, model::ExecutionStatus,
    notify::TierChange, scoring::ScoredExecution, types::Score,
};
use sea_orm::entity::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DbBackend, QueryResult, Statement, Value};

use super::ProcessingSettings;
use super::events::{notices_of, tier_changes};
use super::history::{self, DayCounts};

use crate::attribution::{
    int_array, job_execution_explained_sql, job_window_ids_sql, test_window_ids_sql,
};
use crate::batching::chunks;
use crate::error::DbError;
use crate::sql::{execution_status, get, placeholder_list, rows, value_row};

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
    /// The row's flaky state, moved by [`flakeradar_core::helpers::next_flaky_since`] at
    /// the two thresholds.
    pub(super) flaky_since: Option<DateTimeWithTimeZone>,
}

/// The rows whose events the decision approved, each with the events it sends.
///
/// The decision answers the crossing and the broken change; this carries them
/// beside the row so the event query only reads names and writes events.
pub(super) struct Planned {
    pub(super) update: Update,
    /// Whether the row's flaky state moved, so it sends a Tier change.
    pub(super) crossing: bool,
    /// Whether the row's Fail streak reached the broken threshold this rescore.
    pub(super) broken_change: bool,
}

/// Adds one newest-first row to a scoring window: its Execution, and the Repo's
/// Default branch.
fn push(
    window: &mut decision::Window,
    row: &QueryResult,
    status: ExecutionStatus,
) -> Result<(), DbError> {
    window.executions.push(ScoredExecution {
        commit_sha: get(row, "commit_sha")?,
        branch: get(row, "branch")?,
        status,
    });
    window.default_branch = get(row, "default_branch")?;
    Ok(())
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

impl From<&decision::Answer> for Update {
    /// The row the bulk write binds for one answer of the decision: the values
    /// it came to, beside the id `decide` was given.
    fn from(answer: &decision::Answer) -> Self {
        let values = answer.values;
        Self {
            id: answer.id,
            flakiness_score: values.flakiness_score,
            confirmed_flake_count: values.confirmed_flake_count,
            clean_streak: values.clean_streak,
            clear_fixed_note: values.clear_fixed_note,
            fail_streak: values.fail_streak,
            failure_category: values.failure_category,
            flaky_since: values.flaky_since,
        }
    }
}

impl From<&ProcessingSettings> for decision::Settings {
    /// The fields the decision decides against: the window it scores, the decay
    /// it weighs the flips with, and the three thresholds the state moves at.
    /// What else [`ProcessingSettings`] holds is the processor's own.
    fn from(settings: &ProcessingSettings) -> Self {
        Self {
            score_window: settings.score_window,
            score_decay: settings.score_decay,
            broken_streak: settings.broken_streak,
            flake_threshold: settings.flake_threshold,
            recover_threshold: settings.recover_threshold,
        }
    }
}

/// The windows of every id, newest first.
async fn windows(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    ids: &[i32],
    settings: &ProcessingSettings,
) -> Result<BTreeMap<i32, decision::Window>, DbError> {
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
) -> Result<BTreeMap<i32, decision::Window>, DbError> {
    let mut windows: BTreeMap<i32, decision::Window> = BTreeMap::new();
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
            push(window, &row, status)?;
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
) -> Result<BTreeMap<i32, decision::Window>, DbError> {
    let mut windows: BTreeMap<i32, decision::Window> = BTreeMap::new();
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
            push(windows.entry(id).or_default(), &row, status)?;
        }
    }
    Ok(windows)
}

/// Recomputes flakiness for `kind`'s rows, and writes the Tier changes it makes.
///
/// This is the I/O around [`decision::decide`], once per batch: the windows read
/// the Executions the row scores, the stored states read what the row holds
/// before the write, the bulk write stores the values the decision came to, and
/// today's Score history gains the day's counts. What differs between a Test and
/// a Job is a window statement, a stored-state statement, a write statement and
/// a set of Notices.
///
/// Flip scoring looks only at Executions on the Repo's Default branch, while
/// Proven flakes still count on every branch. `day_counts` adds each row's
/// executions and failures to today's Score history row. A Clean streak of 0 also
/// clears `github_issue_fixed_noted_at`; any other streak leaves it alone, which
/// is what the bulk update's per-row reset flag does.
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
    // the thresholds relative to what the row holds now, and because the decision
    // suppresses an event with the row's own acknowledgement.
    let stored = old_states(db, kind, ids).await?;
    let answers = decision::decide(
        kind,
        ids,
        &windows,
        &stored,
        &decision::Settings::from(settings),
        settings.clock.now(),
    );
    let updates: Vec<Update> = answers.iter().map(Update::from).collect();
    // Only the rows the decision approved an event for name anything, so the
    // event query reads no row it would not send for.
    let planned: Vec<Planned> = answers
        .iter()
        .filter(|answer| answer.crossing || answer.broken_change)
        .map(|answer| Planned {
            update: Update::from(answer),
            crossing: answer.crossing,
            broken_change: answer.broken_change,
        })
        .collect();
    let changes = tier_changes(db, notices_of(kind), &planned, settings).await?;
    write_updates(db, updates_of(kind), &updates).await?;
    history::write_day_counts(db, kind, &updates, day_counts, settings).await?;
    Ok(changes)
}

/// The flaky state a row holds before a rescore replaces it, as the decision
/// reads it. A Job stores no Fail streak, so its row reads none.
///
/// A row the caller lists that this read does not return has no stored state, so
/// it counts as not flaky, with no acknowledgement.
async fn old_states(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    ids: &[i32],
) -> Result<BTreeMap<i32, decision::Stored>, DbError> {
    // Only a Test stores a Fail streak, so a Job's row reads none.
    let (from, stores_streak) = match kind {
        EntityKind::Test => (
            "SELECT id, flaky_since, fail_streak, acknowledged_until FROM test_cases WHERE id IN ",
            true,
        ),
        EntityKind::Job => (
            "SELECT id, flaky_since, acknowledged_until FROM jobs WHERE id IN ",
            false,
        ),
    };
    let mut states = BTreeMap::new();
    for chunk in chunks(ids) {
        let sql = format!("{}({})", from, placeholder_list(1, chunk.len()));
        let values = chunk.iter().map(|id| Value::from(*id)).collect();
        for row in rows(db, sql, values).await? {
            states.insert(
                get(&row, "id")?,
                decision::Stored {
                    flaky_since: get::<Option<DateTimeWithTimeZone>>(&row, "flaky_since")?,
                    fail_streak: if stores_streak {
                        Some(get::<i32>(&row, "fail_streak")?)
                    } else {
                        None
                    },
                    acknowledged_until: get::<Option<DateTimeWithTimeZone>>(
                        &row,
                        "acknowledged_until",
                    )?,
                },
            );
        }
    }
    Ok(states)
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
