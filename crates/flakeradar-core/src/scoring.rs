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

#[cfg(test)]
mod tests {
    use super::{
        FULL_CONFIDENCE_SAMPLES, PROVEN_FLAKE_FLOOR, Score, ScoredExecution,
        branch_scoped_fail_streak, branch_scoped_score, branch_scoped_streak, clean_streak,
        combined_score, count_proven_flakes, fail_streak, flip_score,
    };
    use crate::model::ExecutionStatus::{self, Error, Failed, Passed, Skipped};

    fn repeat(status: ExecutionStatus, count: usize) -> Vec<ExecutionStatus> {
        vec![status; count]
    }

    fn alternation(count: usize) -> Vec<ExecutionStatus> {
        (0..count)
            .map(|index| if index % 2 == 0 { Passed } else { Failed })
            .collect()
    }

    /// A history from `(commit_sha, branch, status)` rows, newest first.
    fn history(rows: &[(&str, &str, ExecutionStatus)]) -> Vec<ScoredExecution> {
        rows.iter()
            .map(|(sha, branch, status)| ScoredExecution {
                commit_sha: (*sha).to_owned(),
                branch: (*branch).to_owned(),
                status: *status,
            })
            .collect()
    }

    #[test]
    fn a_decay_that_is_not_a_number_scores_nothing() {
        // `f64::min` answers the other operand when one side is NaN, so the old
        // `score.min(1.0)` turned an undefined flip rate into a perfect score:
        // a typo in `FLAKERADAR_SCORE_DECAY` marked every Test flaky.
        let statuses = repeat(Passed, 4);
        for decay in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let (score, proven) = combined_score(&statuses, &[], decay, 50);
            assert_eq!(score.as_f64(), 0.0, "decay {decay}");
            assert_eq!(proven, 0);
        }
    }

    #[test]
    fn stable_passing_test_scores_zero() {
        assert_eq!(flip_score(&repeat(Passed, 20), 0.85, 50), 0.0);
    }

    #[test]
    fn always_failing_test_is_broken_not_flaky() {
        assert_eq!(flip_score(&repeat(Failed, 20), 0.85, 50), 0.0);
    }

    #[test]
    fn alternating_results_score_max() {
        assert_eq!(flip_score(&alternation(20), 0.85, 50), 1.0);
    }

    #[test]
    fn skipped_executions_ignored() {
        let statuses = [Passed, Skipped, Passed, Skipped, Passed];
        assert_eq!(flip_score(&statuses, 0.85, 50), 0.0);
    }

    #[test]
    fn fewer_than_two_executions_scores_zero() {
        assert_eq!(flip_score(&[Failed], 0.85, 50), 0.0);
        assert_eq!(flip_score(&[], 0.85, 50), 0.0);
    }

    #[test]
    fn error_counts_as_failing() {
        let as_errors = alternation(10)
            .iter()
            .map(|status| if *status == Failed { Error } else { *status })
            .collect::<Vec<_>>();
        assert_eq!(
            flip_score(&as_errors, 0.85, 50),
            flip_score(&alternation(10), 0.85, 50)
        );
    }

    #[test]
    fn recent_flip_outweighs_old_flip() {
        let mut recent_flip = vec![Failed];
        recent_flip.extend(repeat(Passed, 19));
        let mut old_flip = repeat(Passed, 19);
        old_flip.push(Failed);
        let recent = flip_score(&recent_flip, 0.85, 50);
        let old = flip_score(&old_flip, 0.85, 50);
        assert!(recent > old, "{recent} should exceed {old}");
        assert!(old > 0.0);
    }

    #[test]
    fn small_samples_are_damped() {
        let two_runs = flip_score(&[Failed, Passed], 0.85, 50);
        let many_runs = flip_score(&alternation(20), 0.85, 50);
        assert!(two_runs < 0.3, "one flip is not yet strong evidence");
        assert_eq!(many_runs, 1.0);
        assert_eq!(
            flip_score(&alternation(2), 0.85, 50),
            1.0 / super::count_as_f64(FULL_CONFIDENCE_SAMPLES - 1)
        );
    }

    #[test]
    fn window_caps_the_rows_considered() {
        // The newest row is a pass and the one beyond the window is a fail.
        let statuses = [Passed, Passed, Passed, Failed];
        assert_eq!(flip_score(&statuses, 0.85, 3), 0.0);
        // With the whole history the failure is still outside a two-row window.
        assert_eq!(flip_score(&statuses, 0.85, 2), 0.0);
    }

    #[test]
    fn a_sha_with_both_outcomes_is_a_proven_flake() {
        let executions = [("abc", Passed), ("abc", Failed), ("def", Passed)];
        assert_eq!(count_proven_flakes(&executions), 1);
    }

    #[test]
    fn a_proven_flake_needs_both_outcomes_on_the_sha() {
        let executions = [("abc", Failed), ("abc", Failed), ("def", Passed)];
        assert_eq!(count_proven_flakes(&executions), 0);
    }

    #[test]
    fn a_skipped_execution_does_not_prove_a_flake() {
        let executions = [("abc", Skipped), ("abc", Failed)];
        assert_eq!(count_proven_flakes(&executions), 0);
    }

    #[test]
    fn a_proven_flake_beats_sample_damping() {
        let statuses = [Passed, Failed];
        let with_sha = [("abc", Passed), ("abc", Failed)];
        let (score, proven) = combined_score(&statuses, &with_sha, 0.85, 50);
        assert_eq!(proven, 1);
        assert!(
            (score.as_f64() - PROVEN_FLAKE_FLOOR).abs() < 1e-12,
            "score {score} is not the floor"
        );
    }

    #[test]
    fn combined_score_floors_on_a_proven_flake() {
        let mut statuses = repeat(Passed, 30);
        statuses.push(Failed);
        let with_sha = [("abc", Passed), ("abc", Failed)];
        let (score, proven) = combined_score(&statuses, &with_sha, 0.85, 50);
        assert_eq!(proven, 1);
        assert!(score.as_f64() >= PROVEN_FLAKE_FLOOR);
    }

    #[test]
    fn combined_score_capped_at_one() {
        let statuses = alternation(50);
        let mut with_sha: Vec<(&str, ExecutionStatus)> = statuses
            .iter()
            .enumerate()
            .map(|(index, status)| (["sha0", "sha1", "sha2"][index % 3], *status))
            .collect();
        for _ in 0..10 {
            with_sha.push(("dup", Passed));
            with_sha.push(("dup", Failed));
        }
        let (score, _) = combined_score(&statuses, &with_sha, 0.85, 50);
        assert!(score <= Score::ONE);
        assert_eq!(score, Score::ONE);
    }

    #[test]
    fn clean_streak_counts_passes_since_the_newest_failure() {
        assert_eq!(clean_streak(&[Passed, Skipped, Passed, Failed, Passed]), 2);
        assert_eq!(clean_streak(&[Error, Passed]), 0);
        assert_eq!(clean_streak(&[]), 0);
    }

    #[test]
    fn fail_streak_counts_consecutive_failures_newest_first() {
        assert_eq!(fail_streak(&[Failed, Failed, Skipped, Error]), 3);
        assert_eq!(fail_streak(&[Failed]), 1);
        assert_eq!(fail_streak(&[Passed, Failed]), 0);
        assert_eq!(fail_streak(&[Passed]), 0);
        assert_eq!(fail_streak(&[]), 0);
    }

    #[test]
    fn fail_streak_resets_on_a_pass() {
        assert_eq!(fail_streak(&[Passed, Failed, Failed]), 0);
        assert_eq!(fail_streak(&[Passed, Error]), 0);
    }

    #[test]
    fn branch_scoped_none_equals_legacy() {
        let history = history(&[
            ("a", "main", Passed),
            ("a", "feat", Failed),
            ("b", "main", Passed),
        ]);
        let got = branch_scoped_score(&history, None, 0.85, 50);
        let want = combined_score(
            &[Passed, Failed, Passed],
            &[("a", Passed), ("a", Failed), ("b", Passed)],
            0.85,
            50,
        );
        assert_eq!(got, want);
    }

    #[test]
    fn branch_scoped_ignores_feature_branch_flips() {
        let history = history(&[
            ("c", "main", Passed),
            ("d", "main", Passed),
            ("a", "feat", Failed),
            ("b", "feat", Passed),
        ]);
        assert_eq!(
            branch_scoped_score(&history, Some("main"), 0.85, 50),
            (Score::ZERO, 0)
        );
    }

    #[test]
    fn branch_scoped_same_sha_flip_on_feature_branch_still_counts() {
        let history = history(&[("x", "feat", Passed), ("x", "feat", Failed)]);
        let (score, proven) = branch_scoped_score(&history, Some("main"), 0.85, 50);
        assert_eq!(proven, 1);
        assert!(score.as_f64() >= PROVEN_FLAKE_FLOOR);
    }

    #[test]
    fn branch_scoped_history_slicing() {
        let history = history(&[
            ("a", "main", Passed),
            ("a", "main", Passed),
            ("a", "main", Passed),
            ("x", "main", Failed),
            ("x", "main", Passed),
        ]);
        let (_, proven) = branch_scoped_score(&history, Some("main"), 0.85, 3);
        assert_eq!(proven, 0);
    }

    #[test]
    fn branch_scoped_streak_filters_by_default_branch() {
        let history = history(&[
            ("a", "feat", Failed),
            ("b", "main", Passed),
            ("c", "main", Passed),
        ]);
        assert_eq!(branch_scoped_streak(&history, Some("main")), 2);
        assert_eq!(branch_scoped_streak(&history, None), 0);
    }

    #[test]
    fn branch_scoped_fail_streak_filters_by_default_branch() {
        let history = history(&[
            ("a", "feat", Passed),
            ("b", "main", Failed),
            ("c", "main", Failed),
        ]);
        assert_eq!(branch_scoped_fail_streak(&history, Some("main")), 2);
        // The newest row is a feat pass: on an unknown branch it stops the count.
        assert_eq!(branch_scoped_fail_streak(&history, None), 0);
    }

    #[test]
    fn branch_scoped_fail_streak_unknown_branch_counts_every_row() {
        let history = history(&[
            ("a", "main", Failed),
            ("b", "main", Failed),
            ("c", "dev", Failed),
        ]);
        assert_eq!(branch_scoped_fail_streak(&history, None), 3);
        assert_eq!(branch_scoped_fail_streak(&history, Some("main")), 2);
    }

    #[test]
    fn a_commit_and_a_branch_are_named_apart() {
        // The row has no positional constructor, so a caller says which value is
        // which instead of counting arguments.
        let row = ScoredExecution {
            commit_sha: "abc".to_owned(),
            branch: "main".to_owned(),
            status: Failed,
        };
        assert_eq!(row.commit_sha, "abc");
        assert_eq!(row.branch, "main");
    }
}
