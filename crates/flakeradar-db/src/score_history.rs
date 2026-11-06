//! Score history (ADR 0007): daily snapshots, and the reads from them.
//!
//! The write happens in the same transaction as each rescore. The score and the
//! Proven flake count are replaced (the end-of-day value); executions and
//! failures are added. The reads are the past-score lookup, its lateral twin and
//! the detail window, per entity. The rules are the frozen Python server's
//! `score_history.py`.

// Rust guideline compliant 2026-07-21

use chrono::NaiveDate;
use sea_orm::{ConnectionTrait, DbBackend, Statement, Value};

use crate::attribution::int_array;
use crate::batching::chunks;
use crate::error::DbError;
use crate::sql::{get, rows, value_row};

/// How many days of history the detail responses embed.
pub const HISTORY_DAYS: i64 = 90;

/// Which Score-history table a read or write touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryKind {
    /// `test_score_history`, owned by a Test.
    Test,
    /// `job_score_history`, owned by a Job.
    Job,
}

impl HistoryKind {
    /// The table name.
    #[must_use]
    pub fn table(self) -> &'static str {
        match self {
            Self::Test => "test_score_history",
            Self::Job => "job_score_history",
        }
    }

    /// The column naming the owner: the Test's or the Job's id.
    #[must_use]
    pub fn owner_column(self) -> &'static str {
        match self {
            Self::Test => "test_case_id",
            Self::Job => "job_id",
        }
    }
}

/// One owner's Score history row.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreHistoryRow {
    /// The UTC day the snapshot closes.
    pub day: NaiveDate,
    /// The Flakiness score at the end of that day.
    pub flakiness_score: f64,
    /// The Proven flake count at the end of that day.
    pub confirmed_flake_count: i32,
    /// Executions recorded that day.
    pub executions: i32,
    /// Failures recorded that day.
    pub failures: i32,
}

/// The Score value a history write replaces and the counts it adds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HistoryUpdate {
    /// The Test's or Job's id.
    pub id: i32,
    /// The Flakiness score the rescore wrote.
    pub flakiness_score: f64,
    /// The Proven flake count the rescore wrote.
    pub confirmed_flake_count: i32,
    /// Executions to add to today's row.
    pub executions: i32,
    /// Failures to add to today's row.
    pub failures: i32,
}

/// Score of each owner's newest history row at least `TREND_DAYS` old.
///
/// `today` is the clock's day, so a test controls the cut-off. Owners with no
/// such row get no key at all, which is what makes `trend_for` answer `None`.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails, or when a stored
/// score cannot be read.
pub async fn past_scores(
    db: &impl ConnectionTrait,
    kind: HistoryKind,
    ids: &[i32],
    today: NaiveDate,
) -> Result<std::collections::BTreeMap<i32, f64>, DbError> {
    let cutoff = today - chrono::Duration::days(flakeradar_core::TREND_DAYS);
    let table = kind.table();
    let owner = kind.owner_column();
    let mut found = std::collections::BTreeMap::new();
    for chunk in chunks(ids) {
        let sql = format!(
            "SELECT entity_id, score FROM ( \
               SELECT {owner} AS entity_id, flakiness_score AS score, \
                      row_number() OVER (PARTITION BY {owner} ORDER BY day DESC) AS rn \
                 FROM {table} \
                WHERE {owner} = ANY($1::INTEGER[]) AND day <= $2::DATE \
             ) ranked WHERE rn = 1"
        );
        let query = vec![int_array(chunk), Value::from(cutoff)];
        for row in rows(db, sql, query).await? {
            found.insert(get::<i32>(&row, "entity_id")?, get::<f64>(&row, "score")?);
        }
    }
    Ok(found)
}

/// A per-row lateral subquery: the owner row's newest Score history at least
/// `days` old, or NULL.
///
/// The same lookup [`past_scores`] does, as a SQL condition a list query can
/// filter with before counting and paginating. `owner` is the table alias the
/// caller uses for the Test or Job (`t` or `j`), and `$1` is the cut-off day.
#[must_use]
pub fn newest_history_before(kind: HistoryKind, owner: &str) -> String {
    let table = kind.table();
    let owner_column = kind.owner_column();
    format!(
        "SELECT h.flakiness_score AS score FROM {table} h \
          WHERE h.{owner_column} = {owner}.id AND h.day <= $1::DATE \
          ORDER BY h.day DESC LIMIT 1"
    )
}

/// One owner's history rows inside the detail window, oldest first.
///
/// The rows the detail responses serialize: everything strictly newer than
/// [`HISTORY_DAYS`] back, the complement of the `day <=` cut-off the trend
/// lookup uses.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn window_rows(
    db: &impl ConnectionTrait,
    kind: HistoryKind,
    owner_id: i32,
    today: NaiveDate,
) -> Result<Vec<ScoreHistoryRow>, DbError> {
    let cutoff = today - chrono::Duration::days(HISTORY_DAYS);
    let table = kind.table();
    let owner_column = kind.owner_column();
    let sql = format!(
        "SELECT day, flakiness_score, confirmed_flake_count, executions, failures \
           FROM {table} WHERE {owner_column} = $1 AND day > $2::DATE ORDER BY day"
    );
    let mut history = Vec::new();
    for row in rows(db, sql, vec![Value::from(owner_id), Value::from(cutoff)]).await? {
        history.push(ScoreHistoryRow {
            day: get(&row, "day")?,
            flakiness_score: get(&row, "flakiness_score")?,
            confirmed_flake_count: get(&row, "confirmed_flake_count")?,
            executions: get(&row, "executions")?,
            failures: get(&row, "failures")?,
        });
    }
    Ok(history)
}

/// Upserts today's history row for each update.
///
/// The score and Proven flake count are replaced; executions and failures are
/// added, so a same-day rescore accumulates counts over one end-of-day score.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn write_today_history(
    db: &impl ConnectionTrait,
    kind: HistoryKind,
    updates: &[HistoryUpdate],
    today: NaiveDate,
) -> Result<(), DbError> {
    let table = kind.table();
    let owner = kind.owner_column();
    for chunk in chunks(updates) {
        let mut placeholders = Vec::with_capacity(chunk.len());
        let mut values = Vec::with_capacity(chunk.len() * 6);
        for (index, update) in chunk.iter().enumerate() {
            let base = index * 6;
            placeholders.push(value_row(base + 1, 6));
            values.extend([
                Value::from(update.id),
                Value::from(today),
                Value::from(update.flakiness_score),
                Value::from(update.confirmed_flake_count),
                Value::from(update.executions),
                Value::from(update.failures),
            ]);
        }
        let sql = format!(
            "INSERT INTO {table} ({owner}, day, flakiness_score, confirmed_flake_count, executions, failures) \
             VALUES {} \
             ON CONFLICT ({owner}, day) DO UPDATE SET \
               flakiness_score = EXCLUDED.flakiness_score, \
               confirmed_flake_count = EXCLUDED.confirmed_flake_count, \
               executions = {table}.executions + EXCLUDED.executions, \
               failures = {table}.failures + EXCLUDED.failures",
            placeholders.join(", ")
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
