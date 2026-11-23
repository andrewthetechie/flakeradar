//! The Tier-change events a rescore sends: the per-entity `Notices` descriptor,
//! and the read that names the rows an event reports.
//!
//! Scoring lives in the rescore and whether a move is worth a Notification lives
//! in the core decision (ADR 0009); this only names the rows and builds the
//! events, so the three can change without disturbing each other.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;

use flakeradar_core::helpers::tier_for;
use flakeradar_core::model::EntityKind;
use flakeradar_core::notify::TierChange;
use sea_orm::{ConnectionTrait, QueryResult, Value};

use crate::batching::chunks;
use crate::error::DbError;
use crate::sql::{get, placeholder_list, rows};

use super::rescore::{Planned, Update};

/// The `Notices` that report `kind`'s Tier changes.
pub(super) fn notices_of(kind: EntityKind) -> &'static Notices {
    match kind {
        EntityKind::Test => &TEST_NOTICES,
        EntityKind::Job => &JOB_NOTICES,
    }
}

/// What differs between a Test's and a Job's Tier change.
pub(super) struct Notices {
    /// The statement naming the rows an event reports, by id.
    names_sql: fn(&str) -> String,
    /// Builds the event from one row's name columns.
    change: fn(&Update, &QueryResult) -> Result<TierChange, DbError>,
}

/// The Tests' Tier changes.
const TEST_NOTICES: Notices = Notices {
    names_sql: test_names_sql,
    change: test_change,
};

/// The Jobs' Tier changes.
const JOB_NOTICES: Notices = Notices {
    names_sql: job_names_sql,
    change: job_change,
};

/// The statement naming the Tests an event reports, by id.
fn test_names_sql(placeholders: &str) -> String {
    format!(
        "SELECT t.id, t.classname, t.name, p.name AS project, r.name AS repo \
           FROM test_cases t \
           JOIN projects p ON p.id = t.project_id \
           JOIN repos r ON r.id = p.repo_id \
          WHERE t.id IN ({placeholders}) ORDER BY t.id"
    )
}

/// The statement naming the Jobs an event reports, by id.
fn job_names_sql(placeholders: &str) -> String {
    format!(
        "SELECT j.id, j.name, p.name AS pipeline, r.name AS repo \
           FROM jobs j \
           JOIN pipelines p ON p.id = j.pipeline_id \
           JOIN repos r ON r.id = p.repo_id \
          WHERE j.id IN ({placeholders}) ORDER BY j.id"
    )
}

/// The Tier change a Test's row names.
fn test_change(update: &Update, row: &QueryResult) -> Result<TierChange, DbError> {
    Ok(TierChange {
        kind: EntityKind::Test,
        id: i64::from(update.id),
        repo: get(row, "repo")?,
        project: Some(get(row, "project")?),
        name: get(row, "name")?,
        flakiness_score: update.flakiness_score,
        tier: tier_for(update.flakiness_score, update.flaky_since.is_some()),
        proven_flake_count: usize::try_from(update.confirmed_flake_count).unwrap_or(0),
        classname: Some(get(row, "classname")?),
        pipeline: None,
        failure_category: update.failure_category,
        broken: false,
    })
}

/// The Tier change a Job's row names.
fn job_change(update: &Update, row: &QueryResult) -> Result<TierChange, DbError> {
    Ok(TierChange {
        kind: EntityKind::Job,
        id: i64::from(update.id),
        repo: get(row, "repo")?,
        project: None,
        name: get(row, "name")?,
        flakiness_score: update.flakiness_score,
        tier: tier_for(update.flakiness_score, update.flaky_since.is_some()),
        proven_flake_count: usize::try_from(update.confirmed_flake_count).unwrap_or(0),
        classname: None,
        pipeline: Some(get(row, "pipeline")?),
        failure_category: None,
        broken: false,
    })
}

/// Notifications for the rows whose flaky state this rescore moves (ADR 0009).
///
/// The decision has already said which rows send what, so this only names them:
/// the id order and the read of the names are the same for both entities, and
/// the descriptor carries what differs in the event itself.
pub(super) async fn tier_changes(
    db: &impl ConnectionTrait,
    notices: &Notices,
    planned: &[Planned],
) -> Result<Vec<TierChange>, DbError> {
    // The named rows are read back into a map keyed by id, so the events go out
    // in id order whatever order the caller listed the ids in, and a tier
    // crossing comes before the broken change of the same id.
    let by_id: BTreeMap<i32, &Planned> =
        planned.iter().map(|plan| (plan.update.id, plan)).collect();
    if by_id.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i32> = by_id.keys().copied().collect();
    let mut named: BTreeMap<i32, Vec<TierChange>> = BTreeMap::new();
    for chunk in chunks(&ids) {
        let placeholders = placeholder_list(1, chunk.len());
        let sql = (notices.names_sql)(&placeholders);
        let values = chunk.iter().map(|id| Value::from(*id)).collect();
        for row in rows(db, sql, values).await? {
            let id: i32 = get(&row, "id")?;
            let Some(plan) = by_id.get(&id) else {
                continue;
            };
            // Both changes of one id are built from the same row: the crossing,
            // then the broken change that sets the flag on it.
            let mut changes = Vec::new();
            if plan.crossing {
                changes.push((notices.change)(&plan.update, &row)?);
            }
            if plan.broken_change {
                let mut change = (notices.change)(&plan.update, &row)?;
                change.broken = true;
                changes.push(change);
            }
            named.insert(id, changes);
        }
    }
    // A row deleted before this read names nothing, so it sends nothing.
    Ok(ids
        .iter()
        .filter_map(|id| named.get(id))
        .flatten()
        .cloned()
        .collect())
}
