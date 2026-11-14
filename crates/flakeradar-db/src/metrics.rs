//! The operational reads behind the readiness probe and the Prometheus scrape.
//!
//! Both answers come from the database alone, so every replica reports the same
//! numbers. The reads stay off the Execution tables and the Run tables: the queue
//! and the cached score columns are all a scrape needs, which keeps its cost
//! independent of how much history a Repo holds.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;

use sea_orm::ConnectionTrait;
use sea_orm::sea_query::Value;

use crate::error::DbError;
use crate::sql::{get, one, required_one, rows};

// One roundtrip for the queue facts.
//
// A one-argument session advisory lock (the form the Report processor holds)
// appears in pg_locks as classid = 0, objid = key, which is distinct from the
// transaction-scoped migration lock on another key. The probe is scoped to the
// current database because pg_locks is a cluster-wide view: the same key in
// another database of the cluster belongs to that environment's leader.
const QUEUE_SQL: &str = "
    SELECT
      EXISTS(SELECT 1 FROM pg_locks
             WHERE locktype = 'advisory' AND classid = 0 AND objid::bigint = $1
               AND database = (SELECT oid FROM pg_database WHERE datname = current_database())) AS leader,
      (SELECT count(*) FROM reports WHERE status = 'pending') AS pending,
      (SELECT coalesce(extract(epoch FROM (now() - min(created_at))), 0)::double precision
         FROM reports WHERE status = 'pending') AS oldest
";

// The tiers are the stored tier state: a row with `flaky_since` set is flaky,
// one above 0 without it is suspect, otherwise stable. The rescore stamps
// `flaky_since` from `FLAKE_THRESHOLD` and clears it at `RECOVER_THRESHOLD`, so
// these gauges count exactly the tier every API row reports.
const TEST_TIERS_SQL: &str = "
    SELECT CASE WHEN flaky_since IS NOT NULL THEN 'flaky'
                WHEN flakiness_score > 0 THEN 'suspect'
                ELSE 'stable' END AS tier,
           count(*) AS count
    FROM test_cases GROUP BY 1
";

const JOB_TIERS_SQL: &str = "
    SELECT CASE WHEN flaky_since IS NOT NULL THEN 'flaky'
                WHEN flakiness_score > 0 THEN 'suspect'
                ELSE 'stable' END AS tier,
           count(*) AS count
    FROM jobs GROUP BY 1
";

const REPORT_STATUSES_SQL: &str = "SELECT status, count(*) AS count FROM reports GROUP BY status";

const LAST_PROCESSED_SQL: &str =
    "SELECT extract(epoch FROM max(processed_at))::double precision AS last_processed FROM reports";

/// The queue facts one roundtrip returns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QueueFacts {
    /// Whether this database has a processor leader.
    pub leader: bool,
    /// How many Reports are pending.
    pub pending: i64,
    /// The age in seconds of the oldest pending Report, `0` when none is.
    pub oldest: f64,
}

/// Whether the database answers at all.
pub async fn reachable(db: &impl ConnectionTrait) -> bool {
    one(db, "SELECT 1".to_owned(), Vec::new()).await.is_ok()
}

/// Reads the queue facts the probe and the scrape report.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails or answers no row.
pub async fn queue_facts(db: &impl ConnectionTrait, lock_key: i64) -> Result<QueueFacts, DbError> {
    let row = required_one(db, QUEUE_SQL, vec![Value::from(lock_key)], "queue").await?;
    Ok(QueueFacts {
        leader: get(&row, "leader")?,
        pending: get(&row, "pending")?,
        oldest: get(&row, "oldest")?,
    })
}

/// How many Reports hold each status.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn report_status_counts(
    db: &impl ConnectionTrait,
) -> Result<BTreeMap<String, i64>, DbError> {
    counts(db, REPORT_STATUSES_SQL, "status", None).await
}

/// How many Tests hold each tier, by their stored `flaky_since` state.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn test_tier_counts(db: &impl ConnectionTrait) -> Result<BTreeMap<String, i64>, DbError> {
    counts(db, TEST_TIERS_SQL, "tier", None).await
}

/// How many Jobs hold each tier, by their stored `flaky_since` state.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn job_tier_counts(db: &impl ConnectionTrait) -> Result<BTreeMap<String, i64>, DbError> {
    counts(db, JOB_TIERS_SQL, "tier", None).await
}

/// The Unix time of the most recently processed Report still retained.
///
/// The aggregate answers one row whether or not a Report was processed, so the
/// value is `None` only while the table holds none that was.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails or answers no row.
pub async fn last_processed_at(db: &impl ConnectionTrait) -> Result<Option<f64>, DbError> {
    let row = required_one(db, LAST_PROCESSED_SQL, Vec::new(), "last processed").await?;
    get(&row, "last_processed")
}

/// Runs a `GROUP BY` count query into a map keyed by its `key` column.
async fn counts(
    db: &impl ConnectionTrait,
    sql: &str,
    key: &str,
    threshold: Option<f64>,
) -> Result<BTreeMap<String, i64>, DbError> {
    let values = threshold.map(Value::from).into_iter().collect();
    let mut counts = BTreeMap::new();
    for row in rows(db, sql, values).await? {
        let group: String = get(&row, key)?;
        let count: i64 = get(&row, "count")?;
        let _ = counts.insert(group, count);
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    //! The query text needs no database.

    use super::{
        JOB_TIERS_SQL, LAST_PROCESSED_SQL, QUEUE_SQL, REPORT_STATUSES_SQL, TEST_TIERS_SQL,
    };

    #[test]
    fn the_ops_queries_stay_off_the_execution_tables() {
        // The probe and the scrape read the queue and the cached score columns;
        // a query here that reached the Execution tables would make every scrape
        // and every readiness check as expensive as the whole history.
        for sql in [
            QUEUE_SQL,
            TEST_TIERS_SQL,
            JOB_TIERS_SQL,
            REPORT_STATUSES_SQL,
            LAST_PROCESSED_SQL,
        ] {
            let lowered = sql.to_lowercase();
            for table in ["test_executions", "test_runs", "job_executions"] {
                assert!(!lowered.contains(table), "{table} in {sql}");
            }
        }
        // The capture really sees the ops queries themselves.
        assert!(QUEUE_SQL.contains("pg_locks"));
        assert!(QUEUE_SQL.contains("reports"));
        assert!(TEST_TIERS_SQL.contains("test_cases"));
        assert!(JOB_TIERS_SQL.contains("jobs"));
    }
}
