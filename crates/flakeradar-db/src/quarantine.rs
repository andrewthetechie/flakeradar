//! Quarantine: a human's decision that a flaky Test is skipped for now.
//!
//! This module owns the `(quarantined, quarantined_at, quarantine_reason,
//! quarantine_expires_at)` state for `test_cases`. The REST route turns a
//! [`QuarantineIn`] body into the [`QuarantineDecision`] it describes — the one
//! conversion in the workspace — and writes it through [`set_quarantine`];
//! [`quarantine_list`] serves the skip list the runners poll, and the retention
//! loop's lapse rule will live here too. A quarantined Test keeps listing in
//! the leaderboard, and the runner skip list serves it until the expiry clears
//! or a new request releases it.

// Rust guideline compliant 2026-07-21

use chrono::{Duration, Utc};
use flakeradar_core::{
    dto::{QuarantineIn, QuarantineItem, TestOut},
    model::EntityKind,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement, Value};

use crate::error::DbError;
use crate::reads::{
    IMPACT_COLUMNS, ReadSettings, TESTS_FROM, TESTS_SELECT, id, impact_lateral_sql, impact_since,
    optional_timestamp, test_out,
};
use crate::sql::{fingerprint, get, one, rows};

/// A human's quarantine decision for one Test.
///
/// The body states a flag beside two values that only mean something when it is
/// set, so the decision is one value: what the person decided, and what they said
/// and for how long when they quarantined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuarantineDecision {
    /// Quarantine the Test, with the reason and the expiry the person gave.
    Quarantine {
        /// Why the person quarantined it.
        reason: Option<String>,
        /// How many days the quarantine lasts, when they set an expiry.
        expires_in_days: Option<i64>,
    },
    /// Release the Test: clear the flag, the reason and the expiry.
    Release,
}

/// Turns a request body into the decision it describes.
///
/// The body states a flag and the two values that belong to it; the engine takes
/// the decision they describe, so it never sees a flag.
#[must_use]
pub fn decision(body: QuarantineIn) -> QuarantineDecision {
    if body.quarantined {
        QuarantineDecision::Quarantine {
            reason: body.reason,
            expires_in_days: body.expires_in_days,
        }
    } else {
        QuarantineDecision::Release
    }
}

/// Stores a human's quarantine decision for one Test.
///
/// Every write stores the decision's values, so re-quarantining without a reason
/// or an expiry clears the old ones instead of keeping a stale decision.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn set_quarantine(
    db: &impl ConnectionTrait,
    test_id: i64,
    decision: QuarantineDecision,
    settings: &ReadSettings,
) -> Result<Option<TestOut>, DbError> {
    let impact = impact_lateral_sql(EntityKind::Test, impact_since(settings));
    let sql = format!("{TESTS_SELECT}{IMPACT_COLUMNS} {TESTS_FROM}{impact} WHERE t.id = $1");
    let Some(row) = one(db, sql, vec![Value::from(test_id)]).await? else {
        return Ok(None);
    };
    let stored_id: i32 = get(&row, "id")?;
    let now = settings.clock.now();
    let (quarantined, quarantined_at, reason, expires_at) = match decision {
        QuarantineDecision::Quarantine {
            reason,
            expires_in_days,
        } => (
            true,
            Some(now),
            reason,
            expires_in_days.map(|days| now + Duration::days(days)),
        ),
        QuarantineDecision::Release => (false, None, None, None),
    };
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE test_cases SET quarantined = $2, quarantined_at = $3, \
            quarantine_reason = $4, quarantine_expires_at = $5 WHERE id = $1",
        [
            Value::from(stored_id),
            Value::from(quarantined),
            Value::from(quarantined_at),
            Value::from(reason.clone()),
            Value::from(expires_at),
        ],
    );
    db.execute_raw(statement).await?;
    let mut test = test_out(&row, settings, None)?;
    test.quarantined = quarantined;
    test.quarantined_at = quarantined_at.map(|at| at.with_timezone(&Utc));
    test.quarantine_reason = reason;
    test.quarantine_expires_at = expires_at.map(|at| at.with_timezone(&Utc));
    Ok(Some(test))
}

/// For test runners: the quarantined Tests to skip in one Repo and Project.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn quarantine_list(
    db: &impl ConnectionTrait,
    repo: &str,
    project: &str,
) -> Result<Vec<QuarantineItem>, DbError> {
    let sql = "SELECT t.* FROM test_cases t \
           JOIN projects p ON p.id = t.project_id \
           JOIN repos r ON r.id = p.repo_id \
          WHERE r.name = $1 AND p.name = $2 AND t.quarantined \
          ORDER BY t.name, t.id";
    let mut items = Vec::new();
    for row in rows(
        db,
        sql,
        vec![
            Value::from(repo.to_owned()),
            Value::from(project.to_owned()),
        ],
    )
    .await?
    {
        items.push(QuarantineItem {
            suite: get(&row, "suite")?,
            classname: get(&row, "classname")?,
            name: get(&row, "name")?,
            fingerprint: fingerprint(&get::<String>(&row, "fingerprint")?, "fingerprint")?,
            file: get(&row, "file")?,
            line: get::<Option<i32>>(&row, "line")?.map(id),
            quarantined_at: optional_timestamp(&row, "quarantined_at")?,
            reason: get(&row, "quarantine_reason")?,
            expires_at: optional_timestamp(&row, "quarantine_expires_at")?,
        });
    }
    Ok(items)
}
