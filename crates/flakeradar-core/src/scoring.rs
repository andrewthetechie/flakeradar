//! Flakiness scoring.
//!
//! A Test that fails every time is broken, not flaky: flakiness is instability,
//! so the primary signal is status flips between consecutive Executions,
//! weighted so recent flips matter more. A fail and a pass on the same commit
//! SHA prove nondeterminism, and that proof puts a floor under the score. Every
//! served score, and so every tier and every Notification, comes from these
//! rules; `tests/parity` replays the recorded answers.

use std::collections::BTreeMap;

use crate::model::ExecutionStatus;
use crate::py::round4;
use crate::types::Score;

/// One Execution as the branch-scoped rules read it.
///
/// The commit SHA and the branch are named fields rather than two bare strings in a
/// row, so a caller cannot pass the branch where the commit belongs. The strings
/// are owned because they come straight from a database read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoredExecution {
    /// The commit the Execution ran.
    pub commit_sha: String,
    /// The branch the commit is on.
    pub branch: String,
    /// What the Execution did.
    pub status: ExecutionStatus,
}

/// The lowest score a Proven flake can have.
///
/// One commit SHA that both passed and failed is a Proven flake: proof of
/// nondeterminism, so the score jumps to at least this value even when the flip
/// history looks calm.
pub const PROVEN_FLAKE_FLOOR: f64 = 0.6;

/// The number of non-skipped Executions at which a flip rate is fully trusted.
///
/// A flip rate from few Executions is low-confidence evidence, so it is damped
/// linearly until this many Executions have been seen.
pub const FULL_CONFIDENCE_SAMPLES: usize = 7;

/// A sample or Execution count as a float.
///
/// Every count here is bounded by the score window, so the cast is exact; the
/// lint is about `usize` widths, not about this value.
#[expect(
    clippy::cast_precision_loss,
    reason = "an Execution count is far below 2^52, so the cast is exact"
)]
fn count_as_f64(count: usize) -> f64 {
    count as f64
}

/// The decayed transition rate over the newest `window` Executions.
///
/// `statuses` is newest first. A transition is a change between passing and
/// failing across consecutive non-skipped Executions, weighted by
/// `decay.powi(i)` so the newest transition counts most. Fewer than two
/// non-skipped Executions cannot flip, so they score zero.
///
/// # Examples
///
/// ```
/// use flakeradar_core::{model::ExecutionStatus, scoring::flip_score};
///
/// let alternating = [ExecutionStatus::Passed, ExecutionStatus::Failed];
/// assert_eq!(flip_score(&alternating, 0.85, 50), 1.0 / 6.0);
/// ```
#[must_use]
pub fn flip_score(statuses: &[ExecutionStatus], decay: f64, window: usize) -> f64 {
    let relevant: Vec<ExecutionStatus> = statuses
        .iter()
        .take(window)
        .copied()
        .filter(|status| *status != ExecutionStatus::Skipped)
        .collect();
    if relevant.len() < 2 {
        return 0.0;
    }

    let mut numerator = 0.0;
    let mut denominator = 0.0;
    // Walking newest to oldest means each step back in time is one more decay
    // step: the offset is the exponent, so the newest flip weighs `decay^0`.
    for (offset, (newer, older)) in relevant.iter().zip(relevant.iter().skip(1)).enumerate() {
        let weight = decay.powf(count_as_f64(offset));
        denominator += weight;
        if newer.is_failing() != older.is_failing() {
            numerator += weight;
        }
    }
    // The newest weight is `decay.powf(0)`, which is 1, so the denominator is
    // never zero.
    let raw = numerator / denominator;
    // One flip across two runs is a 100% flip rate but near-zero evidence; scale
    // confidence linearly with the sample size.
    let confidence =
        1.0_f64.min(count_as_f64(relevant.len() - 1) / count_as_f64(FULL_CONFIDENCE_SAMPLES - 1));
    raw * confidence
}

/// The number of commit SHAs that recorded both a pass and a fail.
///
/// `executions` is a list of `(commit_sha, status)` pairs; skipped Executions
/// say nothing about the SHA.
///
/// # Examples
///
/// ```
/// use flakeradar_core::{model::ExecutionStatus, scoring::count_proven_flakes};
///
/// let executions = [("abc", ExecutionStatus::Passed), ("abc", ExecutionStatus::Failed)];
/// assert_eq!(count_proven_flakes(&executions), 1);
/// ```
#[must_use]
pub fn count_proven_flakes(executions: &[(&str, ExecutionStatus)]) -> usize {
    let mut by_sha: BTreeMap<&str, (bool, bool)> = BTreeMap::new();
    for (sha, status) in executions {
        if *status == ExecutionStatus::Skipped {
            continue;
        }
        let entry = by_sha.entry(sha).or_insert((false, false));
        if status.is_failing() {
            entry.1 = true;
        } else {
            entry.0 = true;
        }
    }
    by_sha
        .values()
        .filter(|(passed, failed)| *passed && *failed)
        .count()
}

/// The Flakiness score and the Proven flake count of one history.
///
/// The score is the flip score, floored and then raised by repeated proof of
/// nondeterminism, capped at 1 and rounded to four decimals with [`round4`]. A
/// flip rate that is not a number proves nothing, so it scores 0: `f64::min`
/// answers the other operand for NaN, which would otherwise be served as a
/// perfect score.
///
/// # Examples
///
/// ```
/// use flakeradar_core::{model::ExecutionStatus, scoring::combined_score};
///
/// let statuses = [ExecutionStatus::Passed, ExecutionStatus::Failed];
/// let executions = [("abc", ExecutionStatus::Passed), ("abc", ExecutionStatus::Failed)];
/// let (score, proven) = combined_score(&statuses, &executions, 0.85, 50);
/// assert_eq!((score.as_f64(), proven), (0.6, 1));
/// ```
///
/// # Panics
///
/// Panics only if [`Score::new`] refuses the rate this function just capped at 1
/// and rounded to four decimals, which cannot happen: a capped, rounded rate is
/// finite and falls inside 0 to 1.
#[must_use]
pub fn combined_score(
    statuses: &[ExecutionStatus],
    executions: &[(&str, ExecutionStatus)],
    decay: f64,
    window: usize,
) -> (Score, usize) {
    let flips = flip_score(statuses, decay, window);
    let proven = count_proven_flakes(executions);
    let mut score = flips;
    if proven > 0 {
        // Proven nondeterminism: floor the score and let repeated proof push it
        // toward 1.
        score = score.max(1.0_f64.min(proof_floor(proven)));
    }
    // A rate that is not a number scores nothing: `min` answers the other side
    // for NaN, so an undefined rate would otherwise be served as a perfect one.
    let score = round4(if score.is_finite() {
        score.min(1.0)
    } else {
        0.0
    });
    #[expect(
        clippy::expect_used,
        reason = "a rounded rate capped at 1 is a score: it is finite and in 0 to 1, \
                  which is exactly what Score::new refuses to accept otherwise"
    )]
    let score = Score::new(score).expect("a capped, rounded rate is a score");
    (score, proven)
}
/// The score floor repeated proof of nondeterminism earns.
///
/// The product is rounded and then added, as two separate steps: a fused
/// multiply-add would differ in the last bit, and a served score has no tolerance
/// for that.
#[expect(
    clippy::suboptimal_flops,
    reason = "the product is rounded and then added; fusing the two steps could \
              change the last bit of a served score"
)]
fn proof_floor(proven: usize) -> f64 {
    PROVEN_FLAKE_FLOOR + 0.1 * count_as_f64(proven.saturating_sub(1))
}

/// Flips on the Default branch only; Proven flakes on every branch.
///
/// `history` is newest first and must hold at least the newest `window` rows
/// overall and on the Default branch (see the processor's rescore).
/// `default_branch` of `None` means unknown, which counts every branch.
#[must_use]
pub fn branch_scoped_score(
    history: &[ScoredExecution],
    default_branch: Option<&str>,
    decay: f64,
    window: usize,
) -> (Score, usize) {
    let flips: Vec<ExecutionStatus> = history
        .iter()
        .filter(|execution| default_branch.is_none_or(|default| execution.branch == *default))
        .map(|execution| execution.status)
        .collect();
    let proven: Vec<(&str, ExecutionStatus)> = history
        .iter()
        .take(window)
        .map(|execution| (execution.commit_sha.as_str(), execution.status))
        .collect();
    combined_score(&flips, &proven, decay, window)
}

/// Non-skipped passing Executions since the newest failure.
///
/// # Examples
///
/// ```
/// use flakeradar_core::{model::ExecutionStatus, scoring::clean_streak};
///
/// let statuses = [ExecutionStatus::Passed, ExecutionStatus::Skipped, ExecutionStatus::Failed];
/// assert_eq!(clean_streak(&statuses), 1);
/// ```
#[must_use]
pub fn clean_streak(statuses: &[ExecutionStatus]) -> usize {
    let mut streak = 0;
    for status in statuses {
        if *status == ExecutionStatus::Skipped {
            continue;
        }
        if status.is_failing() {
            break;
        }
        streak += 1;
    }
    streak
}

/// The Clean streak over Default-branch rows only.
#[must_use]
pub fn branch_scoped_streak(history: &[ScoredExecution], default_branch: Option<&str>) -> usize {
    clean_streak(&default_branch_statuses(history, default_branch))
}

/// Consecutive non-skipped failures, newest first, the reverse of a Clean streak.
///
/// # Examples
///
/// ```
/// use flakeradar_core::{model::ExecutionStatus, scoring::fail_streak};
///
/// let statuses = [ExecutionStatus::Failed, ExecutionStatus::Skipped, ExecutionStatus::Error];
/// assert_eq!(fail_streak(&statuses), 2);
/// ```
#[must_use]
pub fn fail_streak(statuses: &[ExecutionStatus]) -> usize {
    let mut streak = 0;
    for status in statuses {
        if *status == ExecutionStatus::Skipped {
            continue;
        }
        if !status.is_failing() {
            break;
        }
        streak += 1;
    }
    streak
}

/// The Fail streak over Default-branch rows only.
#[must_use]
pub fn branch_scoped_fail_streak(
    history: &[ScoredExecution],
    default_branch: Option<&str>,
) -> usize {
    fail_streak(&default_branch_statuses(history, default_branch))
}

/// The statuses on the Default branch, or on every branch while it is unknown.
fn default_branch_statuses(
    history: &[ScoredExecution],
    default_branch: Option<&str>,
) -> Vec<ExecutionStatus> {
    history
        .iter()
        .filter(|execution| default_branch.is_none_or(|default| execution.branch == *default))
        .map(|execution| execution.status)
        .collect()
}
