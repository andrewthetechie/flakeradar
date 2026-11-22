//! The Score-history day counts: what one rescore adds to today's history
//! row of each Test or Job it scores.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;

use flakeradar_core::model::EntityKind;
use sea_orm::ConnectionTrait;

use super::{ProcessingSettings, Update};
use crate::error::DbError;
use crate::score_history::{HistoryUpdate, write_today_history};

/// Executions and failures one day added to a Test's or Job's Score history.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DayCounts {
    /// Executions the day recorded.
    pub executions: i32,
    /// How many of them failed.
    pub failures: i32,
}

/// Adds this rescore to today's Score history row of every id.
pub(super) async fn write_day_counts(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    updates: &[Update],
    day_counts: Option<&BTreeMap<i32, DayCounts>>,
    settings: &ProcessingSettings,
) -> Result<(), DbError> {
    if updates.is_empty() {
        return Ok(());
    }
    let history_updates: Vec<HistoryUpdate> = updates
        .iter()
        .map(|update| HistoryUpdate {
            id: update.id,
            flakiness_score: update.flakiness_score,
            confirmed_flake_count: update.confirmed_flake_count,
            executions: day_counts
                .and_then(|counts| counts.get(&update.id))
                .map_or(0, |counts| counts.executions),
            failures: day_counts
                .and_then(|counts| counts.get(&update.id))
                .map_or(0, |counts| counts.failures),
        })
        .collect();
    write_today_history(db, kind, &history_updates, settings.clock.today()).await
}
