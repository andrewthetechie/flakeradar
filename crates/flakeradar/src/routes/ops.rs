//! Operational reads: the readiness probe and the Prometheus scrape (ADR 0002).
//!
//! Both answer from the database alone, so every process returns the same
//! result, and neither needs a token, like the other read APIs. A database that
//! does not answer is reported rather than raised: the probe exists to say so.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use flakeradar_core::ReadinessOut;
use flakeradar_db::WORKER_LOCK_KEY;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde::Serialize;
use utoipa::ToSchema;

use crate::app::AppState;
use crate::error::ApiResult;

/// The media type of the Prometheus text exposition format, version 0.0.4.
pub const PROMETHEUS_MEDIA_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// The tiers the scrape reports, in the order it lists them.
const TIER_ORDER: [&str; 3] = ["flaky", "suspect", "stable"];

/// The Report statuses the scrape reports, in the order it lists them.
const REPORT_STATUSES: [&str; 3] = ["pending", "processed", "failed"];

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

// The tiers are the cached score columns, classified exactly as
// `flakeradar_core::tier_for` does: at or above the Flake threshold is flaky,
// above 0 suspect, otherwise stable.
const TEST_TIERS_SQL: &str = "
    SELECT CASE WHEN flakiness_score >= $1 THEN 'flaky'
                WHEN flakiness_score > 0 THEN 'suspect'
                ELSE 'stable' END AS tier,
           count(*) AS count
    FROM test_cases GROUP BY 1
";

const JOB_TIERS_SQL: &str = "
    SELECT CASE WHEN flakiness_score >= $1 THEN 'flaky'
                WHEN flakiness_score > 0 THEN 'suspect'
                ELSE 'stable' END AS tier,
           count(*) AS count
    FROM jobs GROUP BY 1
";

const REPORT_STATUSES_SQL: &str = "SELECT status, count(*) AS count FROM reports GROUP BY status";

const LAST_PROCESSED_SQL: &str =
    "SELECT extract(epoch FROM max(processed_at))::double precision AS last_processed FROM reports";

// Long HELP lines, kept as named constants so the render below stays readable.
const OLDEST_PENDING_HELP: &str = "# HELP flakeradar_oldest_pending_report_age_seconds Age in seconds of the oldest pending Report; 0 when the queue is empty.";
const LAST_PROCESSED_HELP: &str = "# HELP flakeradar_last_report_processed_timestamp_seconds Unix time of the most recently processed Report still retained; absent until the first Report is processed, and it can move backwards or disappear again once retention deletes processed Reports (REPORT_RETENTION_DAYS).";

/// The answer `GET /api/health` gives.
#[derive(Debug, Serialize, ToSchema)]
pub struct HealthOut {
    /// Always `ok`: reaching this answer is what the check is about.
    pub status: String,
}

/// Answers that the process is up.
#[utoipa::path(
    get,
    path = "/api/health",
    tag = "ops",
    responses((status = 200, body = HealthOut))
)]
pub async fn health() -> Json<HealthOut> {
    Json(HealthOut {
        status: "ok".to_owned(),
    })
}

/// Answers whether the database is reachable and what the Report queue holds.
///
/// A missing processor leader is body information only: during a redeploy the
/// lock moves between processes, and the replacement must not be kept unready.
///
/// # Errors
///
/// Returns the 500 the frozen server's unhandled query error became when the
/// database answered the connectivity check but the queue query then failed. A
/// database that does not answer at all is a 503 body rather than an error,
/// because the probe exists to report exactly that.
#[utoipa::path(
    get,
    path = "/api/health/ready",
    tag = "ops",
    responses((status = 200, body = ReadinessOut), (status = 503, description = "The database does not answer"))
)]
pub async fn ready(State(state): State<AppState>) -> ApiResult<Response> {
    let pool = state.pool();
    // Nothing else is knowable without a database.
    if pool
        .query_one_raw(Statement::from_string(DbBackend::Postgres, "SELECT 1"))
        .await
        .is_err()
    {
        return Ok(unavailable());
    }
    let facts = match queue_facts(pool).await {
        Ok(facts) => facts,
        Err(error) => return Ok(database_failure(&error)),
    };
    Ok(Json(ReadinessOut {
        database: true,
        processor_leader: facts.leader,
        pending_reports: facts.pending,
        oldest_pending_age_seconds: facts.oldest,
    })
    .into_response())
}

/// The Prometheus scrape.
///
/// A failed query answers 503 with an empty body, like the readiness probe: a
/// scrape during a database outage must fail rather than report zeros that look
/// like a healthy, idle instance.
#[utoipa::path(
    get,
    path = "/metrics",
    tag = "ops",
    responses((status = 200, body = String, content_type = "text/plain"), (status = 503, description = "The database does not answer"))
)]
pub async fn metrics(State(state): State<AppState>) -> Response {
    let threshold = state.settings().flake_threshold;
    match collect(state.pool(), threshold).await {
        Ok(snapshot) => (
            [(header::CONTENT_TYPE, PROMETHEUS_MEDIA_TYPE)],
            render(&snapshot),
        )
            .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, PROMETHEUS_MEDIA_TYPE)],
            String::new(),
        )
            .into_response(),
    }
}

/// The 503 a probe answers when the database does not.
fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({"database": false})),
    )
        .into_response()
}

/// The queue facts one roundtrip returns.
#[derive(Debug, Clone, Copy, PartialEq)]
struct QueueFacts {
    leader: bool,
    pending: i64,
    oldest: f64,
}

/// Reads the queue facts.
async fn queue_facts(pool: &sea_orm::DatabaseConnection) -> Result<QueueFacts, sea_orm::DbErr> {
    let statement =
        Statement::from_sql_and_values(DbBackend::Postgres, QUEUE_SQL, [WORKER_LOCK_KEY.into()]);
    // The query has no FROM clause, so it always returns exactly one row; the
    // arm for a missing one cannot run and is logged in coverage-gaps.md.
    let row = pool.query_one_raw(statement).await?.ok_or_else(|| {
        sea_orm::DbErr::RecordNotFound("the queue query returned no row".to_owned())
    })?;
    Ok(QueueFacts {
        leader: row.try_get("", "leader")?,
        pending: row.try_get("", "pending")?,
        oldest: row.try_get("", "oldest")?,
    })
}

/// Every number one scrape reports.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    facts: QueueFacts,
    statuses: BTreeMap<String, i64>,
    tests: BTreeMap<String, i64>,
    jobs: BTreeMap<String, i64>,
    last_processed: Option<f64>,
}

/// Reads every number one scrape reports.
async fn collect(
    pool: &sea_orm::DatabaseConnection,
    threshold: f64,
) -> Result<Snapshot, sea_orm::DbErr> {
    let facts = queue_facts(pool).await?;
    let statuses = count_by(pool, REPORT_STATUSES_SQL, "status", None).await?;
    let tests = count_by(pool, TEST_TIERS_SQL, "tier", Some(threshold)).await?;
    let jobs = count_by(pool, JOB_TIERS_SQL, "tier", Some(threshold)).await?;
    let last_processed = last_processed(pool).await?;
    Ok(Snapshot {
        facts,
        statuses,
        tests,
        jobs,
        last_processed,
    })
}

/// Runs a `GROUP BY` count query into a map keyed by its `key` column.
async fn count_by(
    pool: &sea_orm::DatabaseConnection,
    sql: &str,
    key: &str,
    threshold: Option<f64>,
) -> Result<BTreeMap<String, i64>, sea_orm::DbErr> {
    let statement = match threshold {
        Some(value) => Statement::from_sql_and_values(DbBackend::Postgres, sql, [value.into()]),
        None => Statement::from_string(DbBackend::Postgres, sql),
    };
    let rows = pool.query_all_raw(statement).await?;
    let mut counts = BTreeMap::new();
    for row in rows {
        let group: String = row.try_get("", key)?;
        let count: i64 = row.try_get("", "count")?;
        let _ = counts.insert(group, count);
    }
    Ok(counts)
}

/// The Unix time of the most recently processed Report still retained.
async fn last_processed(pool: &sea_orm::DatabaseConnection) -> Result<Option<f64>, sea_orm::DbErr> {
    let row = pool
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            LAST_PROCESSED_SQL,
        ))
        .await?;
    match row {
        Some(row) => {
            let value: Option<f64> = row.try_get("", "last_processed")?;
            Ok(value)
        }
        None => Ok(None),
    }
}

/// Renders one scrape.
fn render(snapshot: &Snapshot) -> String {
    let mut lines = vec![
        "# HELP flakeradar_reports Reports by current status.".to_owned(),
        "# TYPE flakeradar_reports gauge".to_owned(),
    ];
    for status in REPORT_STATUSES {
        let count = snapshot.statuses.get(status).copied().unwrap_or_default();
        lines.push(format!("flakeradar_reports{{status=\"{status}\"}} {count}"));
    }
    lines.extend([
        OLDEST_PENDING_HELP.to_owned(),
        "# TYPE flakeradar_oldest_pending_report_age_seconds gauge".to_owned(),
        format!(
            "flakeradar_oldest_pending_report_age_seconds {}",
            python_float(snapshot.facts.oldest)
        ),
        "# HELP flakeradar_processor_leader 1 when a process holds the Report processor advisory lock."
            .to_owned(),
        "# TYPE flakeradar_processor_leader gauge".to_owned(),
        format!(
            "flakeradar_processor_leader {}",
            u8::from(snapshot.facts.leader)
        ),
    ]);
    for (metric, counts, help) in [
        (
            "flakeradar_tests",
            &snapshot.tests,
            "Tests by tier (flaky, suspect, stable).",
        ),
        (
            "flakeradar_jobs",
            &snapshot.jobs,
            "Jobs by tier (flaky, suspect, stable).",
        ),
    ] {
        lines.push(format!("# HELP {metric} {help}"));
        lines.push(format!("# TYPE {metric} gauge"));
        for tier in TIER_ORDER {
            let count = counts.get(tier).copied().unwrap_or_default();
            lines.push(format!("{metric}{{tier=\"{tier}\"}} {count}"));
        }
    }
    lines.push(LAST_PROCESSED_HELP.to_owned());
    lines.push("# TYPE flakeradar_last_report_processed_timestamp_seconds gauge".to_owned());
    if let Some(value) = snapshot.last_processed {
        lines.push(format!(
            "flakeradar_last_report_processed_timestamp_seconds {}",
            python_float(value)
        ));
    }
    let mut body = lines.join("\n");
    body.push('\n');
    body
}

/// Renders a float the way Python's `str` does, with a decimal point.
///
/// A sample line prints `0.0` for an empty queue, where Rust's own `Display`
/// would print `0`. Every value here is a number of seconds, so none reaches the
/// magnitude where Python switches to exponent notation.
fn python_float(value: f64) -> String {
    let text = format!("{value}");
    if value.is_finite() && !text.contains('.') && !text.contains('e') && !text.contains('E') {
        return format!("{text}.0");
    }
    text
}

/// The 500 a readiness probe answers when its queue query fails.
///
/// The frozen server ran that query outside its `try`, so the unhandled
/// `SQLAlchemyError` reached Starlette's error handler and became a plain-text
/// 500, not the `{"detail": ...}` document every other failure carries. The
/// scrape answers 503 instead, because its whole query block was inside the
/// `try`.
fn database_failure(error: &sea_orm::DbErr) -> Response {
    tracing::error!(error = %error, "readiness query failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "Internal Server Error",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    //! The scrape's rendering and the query text need no database.

    use super::{
        JOB_TIERS_SQL, LAST_PROCESSED_SQL, QUEUE_SQL, QueueFacts, REPORT_STATUSES_SQL, Snapshot,
        TEST_TIERS_SQL, python_float, render,
    };
    use std::collections::BTreeMap;

    #[test]
    fn a_float_is_printed_the_way_python_prints_it() {
        assert_eq!(python_float(0.0), "0.0");
        assert_eq!(python_float(90.0), "90.0");
        assert_eq!(python_float(0.125), "0.125");
        assert_eq!(python_float(1_700_000_000.5), "1700000000.5");
    }

    #[test]
    fn an_empty_instance_reports_zeroes_and_no_processed_timestamp() {
        let snapshot = Snapshot {
            facts: QueueFacts {
                leader: false,
                pending: 0,
                oldest: 0.0,
            },
            statuses: BTreeMap::new(),
            tests: BTreeMap::new(),
            jobs: BTreeMap::new(),
            last_processed: None,
        };
        let body = render(&snapshot);
        assert!(body.contains("flakeradar_oldest_pending_report_age_seconds 0.0"));
        assert!(body.contains("flakeradar_processor_leader 0"));
        assert!(body.contains("flakeradar_reports{status=\"pending\"} 0"));
        assert!(body.contains("flakeradar_tests{tier=\"stable\"} 0"));
        // A zero would mean the epoch, so the sample is absent until a Report is
        // processed; its HELP and TYPE lines stay, keeping the scrape stable.
        assert!(!body.lines().any(|line| line
            .starts_with("flakeradar_last_report_processed_timestamp_seconds ")));
        assert!(body.contains("# TYPE flakeradar_last_report_processed_timestamp_seconds gauge"));
        assert!(body.ends_with('\n'));
    }

    #[test]
    fn a_busy_instance_reports_every_series_once() {
        let snapshot = Snapshot {
            facts: QueueFacts {
                leader: true,
                pending: 2,
                oldest: 90.5,
            },
            statuses: BTreeMap::from([("pending".to_owned(), 2), ("failed".to_owned(), 1)]),
            tests: BTreeMap::from([("flaky".to_owned(), 3)]),
            jobs: BTreeMap::from([("suspect".to_owned(), 4)]),
            last_processed: Some(1_700_000_000.0),
        };
        let body = render(&snapshot);
        assert!(body.contains("flakeradar_reports{status=\"pending\"} 2"));
        assert!(body.contains("flakeradar_reports{status=\"failed\"} 1"));
        assert!(body.contains("flakeradar_reports{status=\"processed\"} 0"));
        assert!(body.contains("flakeradar_oldest_pending_report_age_seconds 90.5"));
        assert!(body.contains("flakeradar_processor_leader 1"));
        assert!(body.contains("flakeradar_tests{tier=\"flaky\"} 3"));
        assert!(body.contains("flakeradar_jobs{tier=\"suspect\"} 4"));
        assert!(body.contains("flakeradar_last_report_processed_timestamp_seconds 1700000000.0"));
        for metric in [
            "flakeradar_reports",
            "flakeradar_oldest_pending_report_age_seconds",
            "flakeradar_processor_leader",
            "flakeradar_tests",
            "flakeradar_jobs",
            "flakeradar_last_report_processed_timestamp_seconds",
        ] {
            assert_eq!(body.matches(&format!("# TYPE {metric} gauge")).count(), 1);
        }
    }

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
