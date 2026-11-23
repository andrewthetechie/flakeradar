//! The transition a rescore decides: the values a row moves to, and the events
//! that the move sends.
//!
//! [`decide`] takes one rescore's window data, the row's stored state, the
//! settings it decides against and a `now` instant, and answers the new stored
//! values plus whether a Tier crossing and whether a broken change fire (ADR
//! 0009). The flaky state follows the hysteresis (ADR 0012) and the Clean streak
//! its own rule (ADR 0007); a live acknowledgement answers no event at all. What
//! differs between a Test and a Job arrives as [`Rules`] data rather than as a
//! table of per-entity functions, and nothing here reads a row, a connection or a
//! clock, so a caller can ask what a rescore would do before it writes anything.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset};

use crate::classify::FailureCategory;
use crate::classify::dominant_category;
use crate::helpers::{is_broken, next_flaky_since, tier_for};
use crate::model::{EntityKind, Tier};
use crate::scoring::branch_scoped_streak;
use crate::scoring::{ScoredExecution, branch_scoped_fail_streak, branch_scoped_score};
use crate::types::Score;

/// What a rescore decides against: the window it scores, the decay it weighs the
/// flips with, and the three thresholds the state moves at.
///
/// The data layer carries more settings than these, and its day counts move no
/// transition, so this names only the values a transition reads. A
/// `broken_streak` of `0` disables the broken rule for every entity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// How many recent Executions the Flakiness score weighs.
    pub score_window: usize,
    /// The per-step decay of that weight.
    pub score_decay: f64,
    /// Consecutive Default-branch failures at which a Test counts as broken.
    pub broken_streak: i64,
    /// The Flakiness score at or above which a Test or Job counts as flaky.
    pub flake_threshold: f64,
    /// The Flakiness score below which a flaky Test or Job recovers.
    pub recover_threshold: f64,
}

/// One row's scoring window: its newest Executions, what they were categorised
/// as, and the branch the score is scoped to.
#[derive(Debug, Clone, Default)]
pub struct Window {
    /// Executions, newest first.
    pub executions: Vec<ScoredExecution>,
    /// The Failure categories of the same Executions, newest first. Only a Test is
    /// categorised, so a Job's window leaves this empty.
    pub categories: Vec<Option<FailureCategory>>,
    /// The Default branch of the entity's Repo, or `None` while it is unknown.
    pub default_branch: Option<String>,
}

/// What a row already stores, as far as the transition reads it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stored {
    /// The instant the row became flaky, or `None` when it is not flaky.
    pub flaky_since: Option<DateTime<FixedOffset>>,
    /// The Fail streak the row holds. A Job stores none, so its row counts as one
    /// that never was broken.
    pub fail_streak: Option<i32>,
    /// The instant a human acknowledged the row until, or `None`.
    pub acknowledged_until: Option<DateTime<FixedOffset>>,
}

/// The per-entity rules the decision reads instead of calling.
///
/// A Job stores no Fail streak, so no Job is ever broken: leaving the flaky tier
/// is always a recovery, and no broken change ever fires for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// Whether a row that left the flaky tier sends only when its Fail streak is
    /// under the broken threshold. A Job, which has no streak to be broken by,
    /// always sends.
    pub recovered_when_clean: bool,
    /// Whether a Fail streak that reaches the broken threshold is an event.
    pub breaks: bool,
}

impl Rules {
    /// The rules `kind`'s events follow.
    #[must_use]
    pub const fn for_kind(kind: EntityKind) -> Self {
        match kind {
            EntityKind::Test => Self {
                recovered_when_clean: true,
                breaks: true,
            },
            EntityKind::Job => Self {
                recovered_when_clean: false,
                breaks: false,
            },
        }
    }
}

/// The values a rescore writes for one row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Values {
    /// The score the window came to.
    pub flakiness_score: Score,
    /// How many commit SHAs recorded both a pass and a fail.
    pub confirmed_flake_count: i32,
    /// The Clean streak: passing Executions since the last failure.
    pub clean_streak: i32,
    /// Whether the Clean streak is 0, which is what clears a fixed note (ADR
    /// 0007).
    pub clear_fixed_note: bool,
    /// The Fail streak, newest failures first.
    pub fail_streak: i32,
    /// The dominant Failure category of the window, or `None` for a Job.
    pub failure_category: Option<FailureCategory>,
    /// The flaky state the thresholds came to, moved at the crossings and nowhere
    /// in between.
    pub flaky_since: Option<DateTime<FixedOffset>>,
}

/// One row's answer: what to store, and whether each event fires.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Answer {
    /// The Test's or Job's id, as `decide` was given it.
    pub id: i32,
    /// The values to write, answered whatever the events do.
    pub values: Values,
    /// Whether a Tier change fires: the flaky state moved, and the row either
    /// reached the flaky tier or recovered from it.
    pub crossing: bool,
    /// Whether a Test's Fail streak reached the broken threshold this rescore.
    pub broken_change: bool,
}

/// The transition of each of `ids`, in the order they were given.
///
/// A row with no window scores as an empty one, and a row with no stored state
/// counts as not flaky, with no Fail streak and no acknowledgement. The score
/// maths, the hysteresis and the tier of [`Values`] all come from the pure
/// helpers this calls, so an answer cannot disagree with a served row.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
///
/// use flakeradar_core::defaults::{BROKEN_STREAK, FLAKE_THRESHOLD, RECOVER_THRESHOLD, SCORE_DECAY, SCORE_WINDOW};
/// use flakeradar_core::decision::{Settings, Window, decide};
/// use flakeradar_core::model::{EntityKind, ExecutionStatus};
/// use flakeradar_core::scoring::ScoredExecution;
///
/// let now = chrono::DateTime::parse_from_rfc3339("2026-07-21T12:00:00+00:00").expect("an instant");
/// let row = |sha: &str, status: ExecutionStatus| ScoredExecution {
///     commit_sha: sha.to_owned(),
///     branch: "main".to_owned(),
///     status,
/// };
/// let window = Window {
///     // One commit SHA that both failed and passed: a Proven flake.
///     executions: vec![row("g", ExecutionStatus::Failed), row("g", ExecutionStatus::Passed)],
///     default_branch: Some("main".to_owned()),
///     ..Window::default()
/// };
/// let settings = Settings {
///     score_window: SCORE_WINDOW,
///     score_decay: SCORE_DECAY,
///     broken_streak: BROKEN_STREAK,
///     flake_threshold: FLAKE_THRESHOLD,
///     recover_threshold: RECOVER_THRESHOLD,
/// };
/// let windows = BTreeMap::from([(1, window)]);
/// let answers = decide(EntityKind::Test, &[1], &windows, &BTreeMap::new(), &settings, now);
/// let answer = answers.first().expect("one answer for one id");
/// assert_eq!(answer.values.flaky_since, Some(now));
/// assert!(answer.crossing, "the row reached the flaky tier");
/// assert!(!answer.broken_change, "one failure is under the broken threshold");
/// ```
#[must_use]
pub fn decide(
    kind: EntityKind,
    ids: &[i32],
    windows: &BTreeMap<i32, Window>,
    stored: &BTreeMap<i32, Stored>,
    settings: &Settings,
    now: DateTime<FixedOffset>,
) -> Vec<Answer> {
    let rules = Rules::for_kind(kind);
    ids.iter()
        .map(|id| answer(*id, windows.get(id), stored.get(id), rules, settings, now))
        .collect()
}

/// One row's answer, from its window and its stored state, either of which a
/// caller may not have read.
fn answer(
    id: i32,
    window: Option<&Window>,
    old: Option<&Stored>,
    rules: Rules,
    settings: &Settings,
    now: DateTime<FixedOffset>,
) -> Answer {
    let no_window = Window::default();
    let window = window.unwrap_or(&no_window);
    let no_state = Stored::default();
    let old = old.unwrap_or(&no_state);

    let values = values_of(window, old.flaky_since, settings, now);
    // A row under a live acknowledgement is dropped like a deleted one, so it
    // sends no event; its values are still the ones to write, so nothing arrives
    // when the acknowledgement ends.
    let sends = old.acknowledged_until.is_none_or(|until| until <= now);
    Answer {
        id,
        values,
        crossing: sends && tier_change(old, &values, rules, settings),
        broken_change: sends && broken_change(old, &values, rules, settings),
    }
}

/// The values `window` scores to, starting from the row's stored flaky state.
fn values_of(
    window: &Window,
    flaky_since: Option<DateTime<FixedOffset>>,
    settings: &Settings,
    now: DateTime<FixedOffset>,
) -> Values {
    let branch = window.default_branch.as_deref();
    let (score, proven) = branch_scoped_score(
        &window.executions,
        branch,
        settings.score_decay,
        settings.score_window,
    );
    let streak = branch_scoped_streak(&window.executions, branch);
    let categories: Vec<Option<FailureCategory>> = window
        .categories
        .iter()
        .take(settings.score_window)
        .copied()
        .collect();
    Values {
        flakiness_score: score,
        confirmed_flake_count: window_count(proven),
        clean_streak: window_count(streak),
        clear_fixed_note: streak == 0,
        fail_streak: window_count(branch_scoped_fail_streak(&window.executions, branch)),
        failure_category: dominant_category(&categories),
        flaky_since: next_flaky_since(
            flaky_since,
            score,
            settings.flake_threshold,
            settings.recover_threshold,
            now,
        ),
    }
}

/// A count of Executions, as the column's `i32`.
///
/// A window wider than `i32::MAX` rows cannot be read, so an out-of-range count
/// caps rather than failing the rescore.
fn window_count(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// Whether a Tier change fires: the flaky state moved, and the row either
/// reached the flaky tier or left it as a recovery.
///
/// A score that drifts between the two thresholds moves no state and sends
/// nothing, which is what keeps a Notification off every flip (ADR 0012).
fn tier_change(old: &Stored, values: &Values, rules: Rules, settings: &Settings) -> bool {
    old.flaky_since.is_some() != values.flaky_since.is_some()
        && (tier_for(values.flakiness_score, values.flaky_since.is_some()) == Tier::Flaky
            || recovered(values, rules, settings))
}

/// Whether a row that left the flaky tier has recovered.
///
/// A Test that is now broken has not fixed itself, so it sends nothing; an
/// entity that cannot be broken sends whenever the tier clears.
fn recovered(values: &Values, rules: Rules, settings: &Settings) -> bool {
    !rules.recovered_when_clean || !is_broken(i64::from(values.fail_streak), settings.broken_streak)
}

/// Whether this rescore is the one whose Fail streak reached the broken
/// threshold, so that the event fires once and not on every later run.
fn broken_change(old: &Stored, values: &Values, rules: Rules, settings: &Settings) -> bool {
    let before = i64::from(old.fail_streak.unwrap_or(0));
    rules.breaks
        && !is_broken(before, settings.broken_streak)
        && is_broken(i64::from(values.fail_streak), settings.broken_streak)
}
