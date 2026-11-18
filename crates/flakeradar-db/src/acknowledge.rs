//! Acknowledge: a human's decision that a flaky Test or Job is owned for now.
//!
//! This module owns the `(acknowledged_until, acknowledge_reason)` pair for both
//! acknowledgeable entities. The REST route turns an [`AcknowledgeIn`] body into
//! the [`AcknowledgeDecision`] it describes — the one conversion in the
//! workspace — and writes it through [`set_acknowledge`]; the retention loop
//! clears the same pair when the expiry passes. An acknowledged row still lists
//! in the leaderboard until the expiry clears, so the decision is reversible by
//! waiting or by a new request.
//!
//! The rules live here, once for both targets. The two targets select genuinely
//! different row shapes, which is the one place the engine switches on the
//! target: the select prefix, the table and the row mapper.

// Rust guideline compliant 2026-07-21

use chrono::{DateTime, Duration, Utc};
use flakeradar_core::{dto::AcknowledgeIn, dto::JobOut, dto::TestOut, model::EntityKind};
use sea_orm::{ConnectionTrait, DbBackend, Statement, Value};
use serde::Serialize;

use crate::error::DbError;
use crate::reads::{
    IMPACT_COLUMNS, JOBS_FROM, JOBS_SELECT, ReadSettings, TESTS_FROM, TESTS_SELECT,
    impact_lateral_sql, impact_since, job_out, test_out,
};
use crate::sql::{get, one};

/// A human's acknowledgement decision for one Test or Job.
///
/// The body states a flag beside two values that only mean something when it is
/// set, so the decision is one value: what the person decided, and what they said
/// and for how long when they acknowledged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcknowledgeDecision {
    /// Acknowledge the Test or Job, with the reason and the day count the person gave.
    Acknowledge {
        /// Why the person acknowledged it.
        reason: Option<String>,
        /// How many days the acknowledgement lasts.
        days: i64,
    },
    /// Withdraw the acknowledgement: clear the expiry and the reason.
    Clear,
}

/// What an acknowledgement or its withdrawal answered: the target's own response
/// shape.
///
/// A caller asks for one target and gets that target's row, so the variant is
/// settled by the query that ran and no handler has to guess. `untagged` means the
/// wire is the wrapped DTO exactly: the same `TestOut` or `JobOut` body its
/// leaderboard serves, which is why the routes answer `Json<AcknowledgedOut>`
/// rather than reaching into a variant. `utoipa` still names the concrete body
/// per route.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum AcknowledgedOut {
    /// A Test, in the leaderboard shape.
    Test(TestOut),
    /// A Job, in the leaderboard shape.
    Job(JobOut),
}

/// The table that holds `kind`'s acknowledge columns.
const fn acknowledge_table(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Test => "test_cases",
        EntityKind::Job => "jobs",
    }
}

/// The select of one target's row, as SQL.
///
/// It carries the impact columns because the row feeds the leaderboard's own
/// mapper: without the lateral the answer names unknown columns.
fn select_sql(kind: EntityKind, since: DateTime<Utc>) -> String {
    match kind {
        EntityKind::Test => format!(
            "{TESTS_SELECT}{IMPACT_COLUMNS} {TESTS_FROM}{} WHERE t.id = $1",
            impact_lateral_sql(EntityKind::Test, since)
        ),
        EntityKind::Job => format!(
            "{JOBS_SELECT}{IMPACT_COLUMNS} {JOBS_FROM}{} WHERE j.id = $1",
            impact_lateral_sql(EntityKind::Job, since)
        ),
    }
}

/// Turns a request body into the decision it describes.
///
/// The body states a flag and the two values that belong to it; the engine takes
/// the decision they describe, so it never sees a flag.
///
/// # Panics
///
/// Only when an acknowledged body carries no day count. The body reader requires
/// `days` on an acknowledgement, so a missing one is a reader bug, never a body a
/// client can send.
#[must_use]
pub fn decision(body: AcknowledgeIn) -> AcknowledgeDecision {
    if body.acknowledged {
        #[expect(
            clippy::expect_used,
            reason = "the body reader requires a day count on an acknowledgement, so \
                      a missing one is a reader bug, never a body a client can send"
        )]
        let days = body.days.expect("an acknowledged body carries a day count");
        AcknowledgeDecision::Acknowledge {
            reason: body.reason,
            days,
        }
    } else {
        AcknowledgeDecision::Clear
    }
}

/// Writes one target's acknowledge columns and answers the refreshed row.
///
/// Selects the row, computes the expiry from the decision and the clock, stores
/// both values in one statement, and patches the mapped output with exactly what
/// this call wrote. Answers `None` when the Test or Job is gone.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn set_acknowledge(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    id: i64,
    decision: AcknowledgeDecision,
    settings: &ReadSettings,
) -> Result<Option<AcknowledgedOut>, DbError> {
    let Some(row) = one(
        db,
        select_sql(kind, impact_since(settings)),
        vec![Value::from(id)],
    )
    .await?
    else {
        return Ok(None);
    };
    let stored_id: i32 = get(&row, "id")?;
    let now = settings.clock.now();
    let (reason, until) = match decision {
        AcknowledgeDecision::Acknowledge { reason, days } => {
            (reason, Some(now + Duration::days(days)))
        }
        AcknowledgeDecision::Clear => (None, None),
    };
    let sql = format!(
        "UPDATE {} SET acknowledged_until = $2, acknowledge_reason = $3 WHERE id = $1",
        acknowledge_table(kind)
    );
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        [
            Value::from(stored_id),
            Value::from(until),
            Value::from(reason.clone()),
        ],
    );
    db.execute_raw(statement).await?;
    let out = match kind {
        EntityKind::Test => {
            let mut test = test_out(&row, settings, None)?;
            test.acknowledged_until = until.map(|at| at.with_timezone(&Utc));
            test.acknowledge_reason = reason;
            AcknowledgedOut::Test(test)
        }
        EntityKind::Job => {
            let mut job = job_out(&row, None)?;
            job.acknowledged_until = until.map(|at| at.with_timezone(&Utc));
            job.acknowledge_reason = reason;
            AcknowledgedOut::Job(job)
        }
    };
    Ok(Some(out))
}
