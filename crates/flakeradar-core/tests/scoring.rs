//! The Flakiness score, Clean streak, Fail streak and Proven flake rules.
//!
//! Every served score comes from these functions, so the tests state the values
//! the rules name: the Proven flake floor, the full-confidence sample count, the
//! decay window, and what a branch that is not the Default branch may prove.

// Rust guideline compliant 2026-07-21

use flakeradar_core::model::ExecutionStatus::{self, Error, Failed, Passed, Skipped};
use flakeradar_core::scoring::{
    FULL_CONFIDENCE_SAMPLES, PROVEN_FLAKE_FLOOR, ScoredExecution, branch_scoped_fail_streak,
    branch_scoped_score, branch_scoped_streak, clean_streak, combined_score, count_proven_flakes,
    fail_streak, flip_score,
};
use flakeradar_core::types::Score;

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
        // The one flip the two rows can carry, weighted by the newest row over
        // `FULL_CONFIDENCE_SAMPLES - 1`: the rule's own arithmetic, not a call
        // into the private helper that casts the counts.
        flip_score(&alternation(2), 0.85, 50),
        1.0 / 6.0,
        "a small sample takes the confidence weight of one over {FULL_CONFIDENCE_SAMPLES}"
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
