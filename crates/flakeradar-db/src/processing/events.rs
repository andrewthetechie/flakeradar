//! The Tier-change events a rescore sends: the crossing and broken rules, the
//! per-entity `Notices` descriptor, and the read that names the rows.
//!
//! Scoring lives in the rescore; deciding what is worth a Notification lives
//! here (ADR 0009), so the two can change without disturbing each other.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;

use flakeradar_core::helpers::{is_broken, tier_for};
use flakeradar_core::model::{EntityKind, Tier};
use flakeradar_core::notify::TierChange;
use sea_orm::entity::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, QueryResult, Value};

use crate::batching::chunks;
use crate::error::DbError;
use crate::sql::{get, placeholder_list, rows};

use super::ProcessingSettings;
use super::rescore::{OldRow, Update};

/// The `Notices` that report `kind`'s Tier changes.
pub(super) fn notices_of(kind: EntityKind) -> &'static Notices {
    match kind {
        EntityKind::Test => &TEST_NOTICES,
        EntityKind::Job => &JOB_NOTICES,
    }
}

/// Whether one update moved the row's flaky state, in either direction.
///
/// A Tier change is `flaky_since` going from NULL to set or from set to NULL: a
/// score that drifts between the two thresholds changes no state and sends no
/// Notification. A row with no stored state counts as not flaky.
fn crossed(update: &Update, old: &BTreeMap<i32, OldRow>) -> bool {
    let before = old
        .get(&update.id)
        .is_some_and(|(_, flaky_since, _)| flaky_since.is_some());
    before != update.flaky_since.is_some()
}

/// The Fail streak a row held before this rescore, or 0 when it had no state.
///
/// A Job stores no Fail streak, so its row reads as one that never was broken.
fn old_fail_streak(old: &BTreeMap<i32, OldRow>, id: i32) -> i64 {
    i64::from(old.get(&id).and_then(|&(_, _, streak)| streak).unwrap_or(0))
}

/// What differs between a Test's and a Job's Tier change.
pub(super) struct Notices {
    /// The statement naming the rows an event reports, by id, and reading their
    /// acknowledgement.
    names_sql: fn(&str) -> String,
    /// Whether a row that left the flaky tier has recovered.
    ///
    /// A Job always has. A Test that is now broken has not fixed itself, so it
    /// sends no event.
    recovered: fn(&Update, &ProcessingSettings) -> bool,
    /// Whether a row whose Fail streak was `old` has just reached the broken
    /// threshold, so it sends a broken change.
    ///
    /// Only a Test stores a Fail streak, so a Job's row never breaks.
    broken: fn(i64, &Update, &ProcessingSettings) -> bool,
    /// Builds the event from one row's name columns.
    change: fn(&Update, &QueryResult) -> Result<TierChange, DbError>,
}

/// The Tests' Tier changes.
const TEST_NOTICES: Notices = Notices {
    names_sql: test_names_sql,
    recovered: test_recovered,
    broken: test_broken,
    change: test_change,
};

/// The Jobs' Tier changes.
const JOB_NOTICES: Notices = Notices {
    names_sql: job_names_sql,
    recovered: job_recovered,
    broken: job_broken,
    change: job_change,
};

/// The statement naming the Tests an event reports, by id.
fn test_names_sql(placeholders: &str) -> String {
    format!(
        "SELECT t.id, t.classname, t.name, t.acknowledged_until, p.name AS project, r.name AS repo \
           FROM test_cases t \
           JOIN projects p ON p.id = t.project_id \
           JOIN repos r ON r.id = p.repo_id \
          WHERE t.id IN ({placeholders}) ORDER BY t.id"
    )
}

/// The statement naming the Jobs an event reports, by id.
fn job_names_sql(placeholders: &str) -> String {
    format!(
        "SELECT j.id, j.name, j.acknowledged_until, p.name AS pipeline, r.name AS repo \
           FROM jobs j \
           JOIN pipelines p ON p.id = j.pipeline_id \
           JOIN repos r ON r.id = p.repo_id \
          WHERE j.id IN ({placeholders}) ORDER BY j.id"
    )
}

/// Whether a Test that left the flaky tier has recovered.
fn test_recovered(update: &Update, settings: &ProcessingSettings) -> bool {
    !is_broken(i64::from(update.fail_streak), settings.broken_streak)
}

/// Whether a Job that left the flaky tier has recovered.
fn job_recovered(_update: &Update, _settings: &ProcessingSettings) -> bool {
    true
}

/// Whether a Test whose Fail streak was `old` has just become broken.
fn test_broken(old_fail_streak: i64, update: &Update, settings: &ProcessingSettings) -> bool {
    !is_broken(old_fail_streak, settings.broken_streak)
        && is_broken(i64::from(update.fail_streak), settings.broken_streak)
}

/// A Job stores no Fail streak, so no Job ever becomes broken.
fn job_broken(_old_fail_streak: i64, _update: &Update, _settings: &ProcessingSettings) -> bool {
    false
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
/// The crossing rule, the id order and the read of the names are the same for
/// both entities; the descriptor carries what differs.
pub(super) async fn tier_changes(
    db: &impl ConnectionTrait,
    notices: &Notices,
    updates: &[Update],
    old: &BTreeMap<i32, OldRow>,
    settings: &ProcessingSettings,
) -> Result<Vec<TierChange>, DbError> {
    let crossing: BTreeMap<i32, Update> = updates
        .iter()
        .filter(|update| {
            crossed(update, old)
                && (tier_for(update.flakiness_score, update.flaky_since.is_some()) == Tier::Flaky
                    || (notices.recovered)(update, settings))
        })
        .map(|update| (update.id, *update))
        .collect();
    // A Fail streak that reaches the broken threshold is an event even when no
    // tier moved; only a Test has a streak to cross.
    let broken: BTreeMap<i32, Update> = updates
        .iter()
        .filter(|update| (notices.broken)(old_fail_streak(old, update.id), update, settings))
        .map(|update| (update.id, *update))
        .collect();
    let by_id: BTreeMap<i32, Update> = crossing
        .iter()
        .chain(broken.iter())
        .map(|(id, update)| (*id, *update))
        .collect();
    if by_id.is_empty() {
        return Ok(Vec::new());
    }
    // The events go out in id order whatever order the caller listed the ids in:
    // the named rows are read back into a map keyed by id, and the keys of that
    // union order the output, so a tier crossing comes before the broken change
    // of the same id.
    let ids: Vec<i32> = by_id.keys().copied().collect();
    let now = settings.clock.now();
    let mut named: BTreeMap<i32, Vec<TierChange>> = BTreeMap::new();
    for chunk in chunks(&ids) {
        let placeholders = placeholder_list(1, chunk.len());
        let sql = (notices.names_sql)(&placeholders);
        let values = chunk.iter().map(|id| Value::from(*id)).collect();
        for row in rows(db, sql, values).await? {
            let id: i32 = get(&row, "id")?;
            let Some(update) = by_id.get(&id) else {
                continue;
            };
            // An acknowledged row is dropped like a deleted one, so it sends no
            // event. Its caller still writes the Tier, so nothing arrives when
            // the acknowledgement ends.
            let acknowledged = get::<Option<DateTimeWithTimeZone>>(&row, "acknowledged_until")?;
            if acknowledged.is_some_and(|until| until > now) {
                continue;
            }
            // Both changes of one id are built from the same row: the crossing,
            // then the broken change that sets the flag on it.
            let mut changes = Vec::new();
            if crossing.contains_key(&id) {
                changes.push((notices.change)(update, &row)?);
            }
            if broken.contains_key(&id) {
                let mut change = (notices.change)(update, &row)?;
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
