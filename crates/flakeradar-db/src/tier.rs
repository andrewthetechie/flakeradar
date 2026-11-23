//! The SQL shapes of the Flaky / Suspect / Stable classification and of the
//! broken filter, stated once so no gauge, count or filter can be left behind
//! by a change to the tier definition.
//!
//! These shapes mirror the Rust rules in `flakeradar_core::helpers`:
//! `tier_for` at `crates/flakeradar-core/src/helpers.rs:58` and `is_broken`
//! at `crates/flakeradar-core/src/helpers.rs:123`. The two cannot be one
//! expression: the Rust rules run in-process over a `Score` and a `bool`,
//! while these are SQL text PostgreSQL executes over columns, so joining them
//! would need a macro, a database function, or a generated fragment.
//!
//! The tier names [`tier_case`] answers are `flakeradar_core::model::Tier`'s
//! wire spellings — `flaky`, `suspect`, `stable`, the three arms of
//! `Tier::as_str` at `crates/flakeradar-core/src/model.rs:87-93` — so a
//! gauge groups by the same name a served row carries.

// Rust guideline compliant 2026-07-21

/// The flaky-tier condition over the row aliased `alias`: the stored
/// `flaky_since` state is set, which is what `tier_for` reads as Flaky.
#[must_use]
pub(crate) fn flaky_only(alias: &str) -> String {
    format!("{alias}.flaky_since IS NOT NULL")
}

/// The condition that excludes the stable tier over the row aliased `alias`:
/// a score above 0, or the stored flaky state set on its own. A row can be
/// flaky at a score of 0 when the Recover threshold is 0, so the state counts
/// by itself; the comparison is parenthesized so an `AND` in the surrounding
/// statement cannot read past it.
#[must_use]
pub(crate) fn not_stable(alias: &str) -> String {
    format!("({alias}.flakiness_score > 0 OR {alias}.flaky_since IS NOT NULL)")
}

/// The suspect-tier condition over the row aliased `alias`: a score above 0
/// without the stored flaky state, which is what `tier_for` reads as Suspect.
#[must_use]
pub(crate) fn suspect(alias: &str) -> String {
    format!("{alias}.flaky_since IS NULL AND {alias}.flakiness_score > 0")
}

/// The `CASE` answering the tier name of the row aliased `alias` — Flaky on
/// the stored state, Suspect on a score above 0, Stable otherwise — ending in
/// `AS tier` for a gauge that groups by it.
#[must_use]
pub(crate) fn tier_case(alias: &str) -> String {
    format!(
        "CASE WHEN {alias}.flaky_since IS NOT NULL THEN 'flaky' \
         WHEN {alias}.flakiness_score > 0 THEN 'suspect' \
         ELSE 'stable' END AS tier"
    )
}

/// The broken condition over the row aliased `alias`: its Fail streak has
/// reached the bound the caller binds as `placeholder`. `None` means the
/// comparison is disabled — the bound is 0, so `is_broken` is false for every
/// row and nothing matches: `FALSE`.
#[must_use]
pub(crate) fn broken(alias: &str, placeholder: Option<&str>) -> String {
    match placeholder {
        Some(placeholder) => format!("{alias}.fail_streak >= {placeholder}"),
        None => "FALSE".to_owned(),
    }
}
