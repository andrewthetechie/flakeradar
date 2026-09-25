from app import scoring


def test_stable_passing_test_scores_zero():
    assert scoring.flip_score(["passed"] * 20, decay=0.85, window=50) == 0.0


def test_always_failing_test_is_broken_not_flaky():
    assert scoring.flip_score(["failed"] * 20, decay=0.85, window=50) == 0.0


def test_alternating_results_score_max():
    statuses = ["passed", "failed"] * 10
    assert scoring.flip_score(statuses, decay=0.85, window=50) == 1.0


def test_same_sha_flake_beats_sample_damping():
    # Two runs only, but on the SAME sha: proof outranks sample-size caution.
    statuses = ["passed", "failed"]
    with_sha = [("abc", "passed"), ("abc", "failed")]
    score, confirmed = scoring.combined_score(statuses, with_sha, 0.85, 50)
    assert confirmed == 1
    assert score == scoring.SAME_SHA_FLOOR


def test_recent_flip_outweighs_old_flip():
    recent_flip = ["failed"] + ["passed"] * 19
    old_flip = ["passed"] * 19 + ["failed"]
    recent = scoring.flip_score(recent_flip, decay=0.85, window=50)
    old = scoring.flip_score(old_flip, decay=0.85, window=50)
    assert recent > old > 0


def test_skipped_executions_ignored():
    statuses = ["passed", "skipped", "passed", "skipped", "passed"]
    assert scoring.flip_score(statuses, decay=0.85, window=50) == 0.0


def test_fewer_than_two_executions_scores_zero():
    assert scoring.flip_score(["failed"], decay=0.85, window=50) == 0.0
    assert scoring.flip_score([], decay=0.85, window=50) == 0.0


def test_error_counts_as_failing():
    as_errors = ["error", "passed"] * 5
    as_failures = ["failed", "passed"] * 5
    assert scoring.flip_score(as_errors, 0.85, 50) == scoring.flip_score(as_failures, 0.85, 50)


def test_small_samples_are_damped():
    two_runs = scoring.flip_score(["failed", "passed"], decay=0.85, window=50)
    many_runs = scoring.flip_score(["failed", "passed"] * 10, decay=0.85, window=50)
    assert two_runs < 0.3  # one flip is not yet strong evidence
    assert many_runs == 1.0  # sustained alternation is maximal flakiness


def test_same_sha_flip_detection():
    execs = [("abc", "passed"), ("abc", "failed"), ("def", "passed")]
    assert scoring.count_same_sha_flips(execs) == 1


def test_same_sha_requires_both_outcomes():
    execs = [("abc", "failed"), ("abc", "failed"), ("def", "passed")]
    assert scoring.count_same_sha_flips(execs) == 0


def test_same_sha_skips_dont_count():
    execs = [("abc", "skipped"), ("abc", "failed")]
    assert scoring.count_same_sha_flips(execs) == 0


def test_combined_score_floors_on_confirmed_flake():
    # One mild flip far in the past, but a proven same-SHA flake.
    statuses = ["passed"] * 30 + ["failed"]
    with_sha = [("abc", "passed"), ("abc", "failed")]
    score, confirmed = scoring.combined_score(statuses, with_sha, 0.85, 50)
    assert confirmed == 1
    assert score >= scoring.SAME_SHA_FLOOR


def test_combined_score_capped_at_one():
    statuses = ["passed", "failed"] * 25
    with_sha = [(f"sha{i}", s) for i, s in enumerate(statuses)]
    # Add 10 confirmed same-sha flakes.
    with_sha += [("dup", "passed"), ("dup", "failed")] * 10
    score, _ = scoring.combined_score(statuses, with_sha, 0.85, 50)
    assert score <= 1.0
