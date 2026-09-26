"""Flakiness scoring.

A test that fails 100% of the time is broken, not flaky. Flakiness is
*instability*, so the primary signal is status FLIPS between consecutive
executions, weighted so recent flips matter more (geometric decay).

Signals combined:
1. flip score  — decayed rate of pass<->fail transitions over the last
   `window` executions (skipped executions are ignored).
2. same-SHA flips — a fail and a pass recorded on the SAME commit SHA is
   proof of nondeterminism (identical code, different outcome). Any such
   evidence puts a floor under the score.

Final score is in [0, 1].
"""

from collections import defaultdict

FAILING = {"failed", "error"}
SAME_SHA_FLOOR = 0.6
# A flip rate computed from few executions is low-confidence: damp it until
# we have seen at least this many non-skipped executions.
FULL_CONFIDENCE_SAMPLES = 7


def _is_fail(status: str) -> bool:
    return status in FAILING


def flip_score(statuses_newest_first: list[str], decay: float, window: int) -> float:
    """Decayed transition rate over the most recent `window` executions.

    `statuses_newest_first[0]` is the latest execution. A transition is a
    change between pass and fail across consecutive (non-skipped) executions.
    """
    relevant = [s for s in statuses_newest_first[:window] if s != "skipped"]
    if len(relevant) < 2:
        return 0.0

    num = 0.0
    den = 0.0
    for i in range(len(relevant) - 1):
        weight = decay**i
        den += weight
        if _is_fail(relevant[i]) != _is_fail(relevant[i + 1]):
            num += weight
    raw = num / den if den else 0.0
    # One flip across two runs is 100% flip rate but near-zero evidence;
    # scale confidence linearly with sample size.
    confidence = min(1.0, (len(relevant) - 1) / (FULL_CONFIDENCE_SAMPLES - 1))
    return raw * confidence


def count_same_sha_flips(executions: list[tuple[str, str]]) -> int:
    """Count commit SHAs that recorded BOTH a pass and a fail.

    `executions` is a list of (commit_sha, status) tuples.
    """
    by_sha: dict[str, set[bool]] = defaultdict(set)
    for sha, status in executions:
        if status == "skipped":
            continue
        by_sha[sha].add(_is_fail(status))
    return sum(1 for outcomes in by_sha.values() if len(outcomes) == 2)


def combined_score(
    statuses_newest_first: list[str],
    executions_with_sha: list[tuple[str, str]],
    decay: float,
    window: int,
) -> tuple[float, int]:
    """Return (flakiness_score, confirmed_same_sha_flake_count)."""
    flips = flip_score(statuses_newest_first, decay, window)
    confirmed = count_same_sha_flips(executions_with_sha)
    score = flips
    if confirmed > 0:
        # Proven nondeterminism: floor the score, and let repeated proof
        # push it toward 1.0.
        score = max(score, min(1.0, SAME_SHA_FLOOR + 0.1 * (confirmed - 1)))
    return round(min(score, 1.0), 4), confirmed


def branch_scoped_score(
    history: list[tuple[str, str, str]],  # (commit_sha, branch, status), newest first
    default_branch: str | None,
    decay: float,
    window: int,
) -> tuple[float, int]:
    """Flips on the Default branch only; Proven flakes on every branch.

    `history` must hold at least the newest `window` rows overall AND the
    newest `window` rows on the Default branch (see processing.rescore).
    default_branch=None means "unknown": every branch counts (old behaviour).
    """
    flips = [s for _, b, s in history if default_branch is None or b == default_branch]
    proven = [(sha, s) for sha, _, s in history[:window]]
    return combined_score(flips, proven, decay, window)
