//! Flakiness scoring.
//!
//! A Test that fails every time is broken, not flaky: flakiness is instability,
//! so the primary signal is status flips between consecutive Executions,
//! weighted so recent flips matter more. A fail and a pass on the same commit
//! SHA prove nondeterminism, and that proof puts a floor under the score. The
//! rules and their constants are the frozen Python server's `scoring.py`.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;

use crate::model::ExecutionStatus;
use crate::py::round4;

/// The lowest score a Proven flake can have.
///
/// A same-SHA flip is proof of nondeterminism, so the score jumps to at least
/// this value even when the flip history looks calm.
pub const SAME_SHA_FLOOR: f64 = 0.6;

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
/// use flakeradar_core::{ExecutionStatus, flip_score};
///
/// let alternating = [ExecutionStatus::Passed, ExecutionStatus::Failed];
/// assert_eq!(flip_score(&alternating, 0.85, 50), 2.0 / 6.0);
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
    // step, which is what `decay**i` in the Python loop counts.
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
/// use flakeradar_core::{ExecutionStatus, count_same_sha_flips};
///
/// let executions = [("abc", ExecutionStatus::Passed), ("abc", ExecutionStatus::Failed)];
/// assert_eq!(count_same_sha_flips(&executions), 1);
/// ```
#[must_use]
pub fn count_same_sha_flips(executions: &[(&str, ExecutionStatus)]) -> usize {
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
/// nondeterminism, capped at 1 and rounded to four decimals as Python does.
///
/// # Examples
///
/// ```
/// use flakeradar_core::{ExecutionStatus, combined_score};
///
/// let statuses = [ExecutionStatus::Passed, ExecutionStatus::Failed];
/// let executions = [("abc", ExecutionStatus::Passed), ("abc", ExecutionStatus::Failed)];
/// assert_eq!(combined_score(&statuses, &executions, 0.85, 50), (0.6, 1));
/// ```
#[must_use]
pub fn combined_score(
    statuses: &[ExecutionStatus],
    executions: &[(&str, ExecutionStatus)],
    decay: f64,
    window: usize,
) -> (f64, usize) {
    let flips = flip_score(statuses, decay, window);
    let confirmed = count_same_sha_flips(executions);
    let mut score = flips;
    if confirmed > 0 {
        // Proven nondeterminism: floor the score and let repeated proof push it
        // toward 1.
        score = score.max(1.0_f64.min(proof_floor(confirmed)));
    }
    (round4(score.min(1.0)), confirmed)
}

/// The score floor repeated proof of nondeterminism earns.
///
/// Python evaluated `SAME_SHA_FLOOR + 0.1 * (count - 1)` with two roundings; a
/// fused multiply-add would differ in the last bit, so the expression stays
/// spelled out.
#[expect(
    clippy::suboptimal_flops,
    reason = "Python rounded the product and the sum separately; fusing them could \
              change the last bit of a served score"
)]
fn proof_floor(confirmed: usize) -> f64 {
    SAME_SHA_FLOOR + 0.1 * count_as_f64(confirmed.saturating_sub(1))
}

/// Flips on the Default branch only; Proven flakes on every branch.
///
/// `history` is `(commit_sha, branch, status)` newest first and must hold at
/// least the newest `window` rows overall and on the Default branch (see the
/// processor's rescore). `default_branch` of `None` means unknown, which counts
/// every branch.
#[must_use]
pub fn branch_scoped_score(
    history: &[(&str, &str, ExecutionStatus)],
    default_branch: Option<&str>,
    decay: f64,
    window: usize,
) -> (f64, usize) {
    let flips: Vec<ExecutionStatus> = history
        .iter()
        .filter(|(_, branch, _)| default_branch.is_none_or(|default| branch == &default))
        .map(|(_, _, status)| *status)
        .collect();
    let proven: Vec<(&str, ExecutionStatus)> = history
        .iter()
        .take(window)
        .map(|(sha, _, status)| (*sha, *status))
        .collect();
    combined_score(&flips, &proven, decay, window)
}

/// Non-skipped passing Executions since the newest failure.
///
/// # Examples
///
/// ```
/// use flakeradar_core::{ExecutionStatus, clean_streak};
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
pub fn branch_scoped_streak(
    history: &[(&str, &str, ExecutionStatus)],
    default_branch: Option<&str>,
) -> usize {
    let statuses: Vec<ExecutionStatus> = history
        .iter()
        .filter(|(_, branch, _)| default_branch.is_none_or(|default| branch == &default))
        .map(|(_, _, status)| *status)
        .collect();
    clean_streak(&statuses)
}

/// Consecutive non-skipped failures, newest first, the reverse of a Clean streak.
///
/// # Examples
///
/// ```
/// use flakeradar_core::{ExecutionStatus, fail_streak};
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
    history: &[(&str, &str, ExecutionStatus)],
    default_branch: Option<&str>,
) -> usize {
    let statuses: Vec<ExecutionStatus> = history
        .iter()
        .filter(|(_, branch, _)| default_branch.is_none_or(|default| branch == &default))
        .map(|(_, _, status)| *status)
        .collect();
    fail_streak(&statuses)
}

#[cfg(test)]
mod tests {
    use super::{
        FULL_CONFIDENCE_SAMPLES, SAME_SHA_FLOOR, branch_scoped_fail_streak, branch_scoped_score,
        branch_scoped_streak, clean_streak, combined_score, count_same_sha_flips, fail_streak,
        flip_score,
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
    fn same_sha_flip_detection() {
        let executions = [("abc", Passed), ("abc", Failed), ("def", Passed)];
        assert_eq!(count_same_sha_flips(&executions), 1);
    }

    #[test]
    fn same_sha_requires_both_outcomes() {
        let executions = [("abc", Failed), ("abc", Failed), ("def", Passed)];
        assert_eq!(count_same_sha_flips(&executions), 0);
    }

    #[test]
    fn same_sha_skips_dont_count() {
        let executions = [("abc", Skipped), ("abc", Failed)];
        assert_eq!(count_same_sha_flips(&executions), 0);
    }

    #[test]
    fn same_sha_flake_beats_sample_damping() {
        let statuses = [Passed, Failed];
        let with_sha = [("abc", Passed), ("abc", Failed)];
        let (score, confirmed) = combined_score(&statuses, &with_sha, 0.85, 50);
        assert_eq!(confirmed, 1);
        assert!(
            (score - SAME_SHA_FLOOR).abs() < 1e-12,
            "score {score} is not the floor"
        );
    }

    #[test]
    fn combined_score_floors_on_confirmed_flake() {
        let mut statuses = repeat(Passed, 30);
        statuses.push(Failed);
        let with_sha = [("abc", Passed), ("abc", Failed)];
        let (score, confirmed) = combined_score(&statuses, &with_sha, 0.85, 50);
        assert_eq!(confirmed, 1);
        assert!(score >= SAME_SHA_FLOOR);
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
        assert!(score <= 1.0);
        assert_eq!(score, 1.0);
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
        let history = [
            ("a", "main", Passed),
            ("a", "feat", Failed),
            ("b", "main", Passed),
        ];
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
        let history = [
            ("c", "main", Passed),
            ("d", "main", Passed),
            ("a", "feat", Failed),
            ("b", "feat", Passed),
        ];
        assert_eq!(
            branch_scoped_score(&history, Some("main"), 0.85, 50),
            (0.0, 0)
        );
    }

    #[test]
    fn branch_scoped_same_sha_flip_on_feature_branch_still_counts() {
        let history = [("x", "feat", Passed), ("x", "feat", Failed)];
        let (score, confirmed) = branch_scoped_score(&history, Some("main"), 0.85, 50);
        assert_eq!(confirmed, 1);
        assert!(score >= SAME_SHA_FLOOR);
    }

    #[test]
    fn branch_scoped_history_slicing() {
        let history = [
            ("a", "main", Passed),
            ("a", "main", Passed),
            ("a", "main", Passed),
            ("x", "main", Failed),
            ("x", "main", Passed),
        ];
        let (_, confirmed) = branch_scoped_score(&history, Some("main"), 0.85, 3);
        assert_eq!(confirmed, 0);
    }

    #[test]
    fn branch_scoped_streak_filters_by_default_branch() {
        let history = [
            ("a", "feat", Failed),
            ("b", "main", Passed),
            ("c", "main", Passed),
        ];
        assert_eq!(branch_scoped_streak(&history, Some("main")), 2);
        assert_eq!(branch_scoped_streak(&history, None), 0);
    }

    #[test]
    fn branch_scoped_fail_streak_filters_by_default_branch() {
        let history = [
            ("a", "feat", Passed),
            ("b", "main", Failed),
            ("c", "main", Failed),
        ];
        assert_eq!(branch_scoped_fail_streak(&history, Some("main")), 2);
        // The newest row is a feat pass: on an unknown branch it stops the count.
        assert_eq!(branch_scoped_fail_streak(&history, None), 0);
    }

    #[test]
    fn branch_scoped_fail_streak_unknown_branch_counts_every_row() {
        let history = [
            ("a", "main", Failed),
            ("b", "main", Failed),
            ("c", "dev", Failed),
        ];
        assert_eq!(branch_scoped_fail_streak(&history, None), 3);
        assert_eq!(branch_scoped_fail_streak(&history, Some("main")), 2);
    }
}
