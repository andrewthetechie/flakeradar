//! Acknowledge: a human's decision that a flaky Test or Job is owned for now.
//!
//! This module owns the `(acknowledged_until, acknowledge_reason)` pair for both
//! acknowledgeable entities. The REST route turns an [`AcknowledgeIn`] body into
//! the [`AcknowledgeDecision`] it describes — the one conversion in the
//! workspace — and writes it through [`set_acknowledge`]; the retention loop
//! clears the same pair through [`clear_expired_acknowledgements`] when the
//! expiry passes. An acknowledged row still lists
//! in the leaderboard until the expiry clears, so the decision is reversible by
//! waiting or by a new request.
//!
//! The rules live here, once for both targets: `live_condition` and
//! `lapsed_condition` state when an acknowledgement counts, and
//! [`clear_expired_acknowledgements`] states the lapse rule the retention loop's
//! prune consumes — the same arrangement `crate::quarantine` keeps for a
//! quarantine. The listings and the filing gate call the condition instead of
//! spelling the comparison, so no statement can drift from the others.
//!
//! The row is read and mapped through the shared entry points in `crate::reads`;
//! the one place this module still switches on the target is the table the write
//! updates.

// Rust guideline compliant 2026-07-21

use chrono::{Duration, Utc};
use flakeradar_core::{dto::AcknowledgeIn, dto::JobOut, dto::TestOut, model::EntityKind};
use sea_orm::entity::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DbBackend, Statement, Value};
use serde::Serialize;

use crate::error::DbError;
use crate::reads::{EntityOut, ReadSettings, entity_out, entity_row};
use crate::sql::{get, one, rows};

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

/// The condition for a live acknowledgement, over the row aliased `alias`.
///
/// This is the one place the rule is stated: an acknowledgement is live while its
/// expiry is in the future. `placeholder` is the text of the bind that carries the
/// instant — `$4` or `$5` in the filing statements, whose position depends on
/// whether the Clean-streak clause is there, and a numbered placeholder in a
/// listing — because the comparison always takes the same value the caller binds.
#[must_use]
pub(crate) fn live_condition(alias: &str, placeholder: &str) -> String {
    format!("{alias}.acknowledged_until > {placeholder}")
}

/// The condition for a lapsed or absent acknowledgement, over the row aliased
/// `alias`: everything [`live_condition`] rejects, parenthesized so an `OR` in the
/// surrounding statement cannot read past it.
#[must_use]
pub(crate) fn lapsed_condition(alias: &str, placeholder: &str) -> String {
    format!("({alias}.acknowledged_until IS NULL OR {alias}.acknowledged_until <= {placeholder})")
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
    let Some(row) = entity_row(db, kind, id, settings).await? else {
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
    let out = match entity_out(&row, kind, settings)? {
        EntityOut::Test(mut test) => {
            test.acknowledged_until = until.map(|at| at.with_timezone(&Utc));
            test.acknowledge_reason = reason;
            AcknowledgedOut::Test(test)
        }
        EntityOut::Job(mut job) => {
            job.acknowledged_until = until.map(|at| at.with_timezone(&Utc));
            job.acknowledge_reason = reason;
            AcknowledgedOut::Job(job)
        }
    };
    Ok(Some(out))
}

/// One acknowledgement the lapse cleared.
///
/// A named row, so the caller states which entity it is logging rather than
/// reading a table name out of a tuple.
#[derive(Debug, Clone)]
pub struct ExpiredAcknowledgement {
    /// Whether the cleared row was a Test or a Job.
    pub kind: EntityKind,
    /// The cleared row's id.
    pub id: i32,
    /// The cleared row's name.
    pub name: String,
}

/// Clears every acknowledgement whose human-set end has passed, all in this round.
///
/// This is the one place the lapse rule is stated: a row whose expiry passed loses
/// the same pair [`set_acknowledge`] stores. One `UPDATE ... RETURNING` per table
/// clears that table's whole set, so the number of statements no longer grows with
/// the number of expired rows, and a table is only touched when it has a due row,
/// so a round with nothing due sends no `UPDATE` at all. The caller logs each
/// cleared row it returns, which is where the operator-facing line stays.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn clear_expired_acknowledgements(
    db: &impl ConnectionTrait,
    now: DateTimeWithTimeZone,
) -> Result<Vec<ExpiredAcknowledgement>, DbError> {
    let mut cleared = Vec::new();
    for kind in [EntityKind::Test, EntityKind::Job] {
        let table = acknowledge_table(kind);
        let due = one(
            db,
            format!("SELECT 1 FROM {table} WHERE acknowledged_until <= $1 LIMIT 1"),
            vec![Value::from(now)],
        )
        .await?;
        if due.is_none() {
            continue;
        }
        let expired = rows(
            db,
            format!(
                "UPDATE {table} SET acknowledged_until = NULL, acknowledge_reason = NULL \
                  WHERE acknowledged_until <= $1 \
                  RETURNING id, name"
            ),
            vec![Value::from(now)],
        )
        .await?;
        for row in &expired {
            cleared.push(ExpiredAcknowledgement {
                kind,
                id: get(row, "id")?,
                name: get(row, "name")?,
            });
        }
    }
    Ok(cleared)
}
