"""`D2`: every read route, generated from the route's own parameter list.

The earlier battery sent `limit` and `offset` to the leaderboards, where they are
not parameters, so it compared the unfiltered page twice. This one reads each
route's parameters out of `backend/app/routers/*.py` (listed in `PARAMETERS`
below, with the reference's own constraints) and sends every one of them alone
and then in combination, with valid, boundary and refused values.
"""

from __future__ import annotations

from harness import API_KEY, Battery

from seed import FRONTEND, OTHER, PIPELINE, PROJECT, REPO, job_id_of, run_id_of, test_id_of

# The reference's own declarations, one line per query parameter:
#   route: [(name, values), ...]
# Values are the ones a client can send: a valid value, each boundary the
# declaration allows, and a refused one.
SORTS = ["score", "last_seen", "proven", "bogus", "", "SCORE"]
CATEGORIES = ["timing", "network", "environment", "assertion", "other", "bogus", ""]
BOOLS = ["true", "false", "1", "0", "True", "FALSE", "yes", "maybe", "1.0", ""]
INTS = ["1", "2", "0", "-1", "abc", ""]


def _call(battery: Battery, path: str, params: list[tuple[str, str]], *, headers=None, known=None, label=None):
    battery.call("GET", path, params=params, headers=headers, known=known, label=label or f"GET {path}?{params}")


def reads(battery: Battery) -> None:
    battery.phase = "d2"
    flaky = test_id_of(battery, "test_timing_flake")
    broken = test_id_of(battery, "test_broken")
    stable = test_id_of(battery, "test_stable")
    job = job_id_of(battery, "lint")

    # GET /api/repos: no parameters.
    _call(battery, "/api/repos", [])

    # GET /api/tests — repo, project, include_stable, sort, page, page_size,
    # file, category, q, quarantined, has_issue, new_within_days, broken.
    for scope in ([], [("repo", REPO)], [("repo", REPO), ("project", PROJECT)],
                  [("repo", REPO), ("project", FRONTEND)], [("repo", OTHER)],
                  [("repo", "ACME/APP")], [("repo", "acme/app ")], [("repo", "")],
                  [("repo", "bad")], [("repo", "a/"), ("project", "")],
                  [("project", PROJECT)]):
        _call(battery, "/api/tests", scope)
    for value in BOOLS:
        _call(battery, "/api/tests", [("repo", REPO), ("include_stable", value)])
    for value in SORTS:
        _call(battery, "/api/tests", [("repo", REPO), ("include_stable", "true"), ("sort", value)])
    for value in ["1", "2", "3", "0", "1.0", "abc"]:
        _call(battery, "/api/tests", [("repo", REPO), ("include_stable", "true"), ("page", value)])
    for value in ["1", "2", "10", "100", "0", "101", "abc", ""]:
        _call(battery, "/api/tests", [("repo", REPO), ("include_stable", "true"), ("page_size", value)])
    for value in ["tests/test_cron.py", "test_cron", ".py", "./tests/", "TEST_CRON", "%", "_", "cron%", "", "x" * 1024]:
        _call(battery, "/api/tests", [("repo", REPO), ("include_stable", "true"), ("file", value)])
    for value in CATEGORIES:
        _call(battery, "/api/tests", [("repo", REPO), ("include_stable", "true"), ("category", value)])
    for value in ["test", "TIMING", "tests.test_cron", "Card", "", "card", "nomatch", "x" * 200]:
        _call(battery, "/api/tests", [("repo", REPO), ("include_stable", "true"), ("q", value)])
    for value in BOOLS:
        _call(battery, "/api/tests", [("repo", REPO), ("include_stable", "true"), ("quarantined", value)])
        _call(battery, "/api/tests", [("repo", REPO), ("include_stable", "true"), ("has_issue", value)])
        _call(battery, "/api/tests", [("repo", REPO), ("include_stable", "true"), ("broken", value)])
    for value in ["1", "90", "0", "91", "-1", "abc", ""]:
        _call(battery, "/api/tests", [("repo", REPO), ("include_stable", "true"), ("new_within_days", value)])
    # The combinations the dashboard sends.
    for params in (
        [("repo", REPO), ("project", PROJECT), ("page", "1"), ("page_size", "10"), ("sort", "score")],
        [("repo", REPO), ("project", PROJECT), ("page", "2"), ("page_size", "10"), ("sort", "last_seen")],
        [("repo", REPO), ("sort", "proven"), ("page_size", "5")],
        [("repo", REPO), ("include_stable", "true"), ("category", "timing"), ("sort", "score")],
        [("repo", REPO), ("include_stable", "true"), ("file", "test_cron"), ("category", "timing")],
        [("repo", REPO), ("include_stable", "true"), ("q", "flake"), ("sort", "proven")],
        [("repo", REPO), ("include_stable", "true"), ("quarantined", "true"), ("has_issue", "false")],
        [("repo", REPO), ("include_stable", "true"), ("broken", "true"), ("category", "other")],
        [("repo", REPO), ("include_stable", "true"), ("new_within_days", "7"), ("sort", "score")],
        [("repo", REPO), ("project", FRONTEND), ("include_stable", "true"), ("file", "Card")],
        [("repo", REPO), ("page", "2"), ("page_size", "1"), ("category", "timing")],
    ):
        _call(battery, "/api/tests", params)

    # GET /api/summary — repo, project.
    for params in ([], [("repo", REPO)], [("repo", REPO), ("project", PROJECT)],
                   [("repo", REPO), ("project", FRONTEND)], [("repo", OTHER)],
                   [("repo", "")], [("repo", "nope/nope")], [("project", "backend")]):
        _call(battery, "/api/summary", params)

    # GET /api/tests/{test_id}/history — limit (1..500).
    for test, label in ((flaky, "flaky"), (broken, "broken"), (stable, "stable"), (999_999, "missing")):
        for value in [None, "1", "3", "60", "500", "0", "501", "abc", ""]:
            params = [] if value is None else [("limit", value)]
            _call(battery, f"/api/tests/{test}/history", params, label=f"history of {label} Test limit={value}")

    # GET /api/jobs — repo, include_stable, sort, page, page_size, q, pipeline,
    # has_issue, new_within_days.
    for params in ([], [("repo", REPO)], [("repo", OTHER)], [("repo", "")], [("repo", "bad")]):
        _call(battery, "/api/jobs", params)
    for value in BOOLS:
        _call(battery, "/api/jobs", [("repo", REPO), ("include_stable", value)])
        _call(battery, "/api/jobs", [("repo", REPO), ("include_stable", "true"), ("has_issue", value)])
    for value in SORTS:
        _call(battery, "/api/jobs", [("repo", REPO), ("include_stable", "true"), ("sort", value)])
    for value in ["1", "2", "0", "1.0", "abc"]:
        _call(battery, "/api/jobs", [("repo", REPO), ("include_stable", "true"), ("page", value)])
    for value in ["1", "100", "0", "101", "abc"]:
        _call(battery, "/api/jobs", [("repo", REPO), ("include_stable", "true"), ("page_size", value)])
    for value in ["lint", "LINT", "backend", "", "nomatch", "x" * 200]:
        _call(battery, "/api/jobs", [("repo", REPO), ("include_stable", "true"), ("q", value)])
    for value in [PIPELINE, ".github", "ci.yml", "", "nomatch"]:
        _call(battery, "/api/jobs", [("repo", REPO), ("include_stable", "true"), ("pipeline", value)])
    for value in ["1", "90", "0", "91", "abc"]:
        _call(battery, "/api/jobs", [("repo", REPO), ("include_stable", "true"), ("new_within_days", value)])
    for params in (
        [("repo", REPO), ("include_stable", "true"), ("sort", "score"), ("page", "1"), ("page_size", "2")],
        [("repo", REPO), ("include_stable", "true"), ("pipeline", PIPELINE), ("q", "unit")],
        [("repo", REPO), ("include_stable", "true"), ("has_issue", "true"), ("sort", "proven")],
    ):
        _call(battery, "/api/jobs", params)

    # GET /api/jobs/summary — repo.
    for params in ([], [("repo", REPO)], [("repo", OTHER)], [("repo", "")], [("repo", "bad")]):
        _call(battery, "/api/jobs/summary", params)

    # GET /api/jobs/{job_id}/history — limit (1..500).
    for value in [None, "1", "3", "500", "0", "501", "abc"]:
        params = [] if value is None else [("limit", value)]
        _call(battery, f"/api/jobs/{job}/history", params, label=f"job history limit={value}")
    _call(battery, "/api/jobs/999999/history", [], label="job history of a missing Job")

    # GET /api/ci-runs/{ci_run_id}/failures — repo (required), ci_run_attempt (>= 1).
    for run in ("run-explain", "run-unexplained", "run-summary", "run-cancel", "nope", ""):
        _call(battery, f"/api/ci-runs/{run}/failures", [("repo", REPO)])
    for value in [None, "1", "2", "0", "-1", "abc"]:
        params = [("repo", REPO)] + ([] if value is None else [("ci_run_attempt", value)])
        _call(battery, "/api/ci-runs/run-explain/failures", params, label=f"ci-run failures attempt={value}")
    for params in ([], [("repo", "")], [("repo", "bad")], [("repo", OTHER)]):
        _call(battery, "/api/ci-runs/run-explain/failures", params, label=f"ci-run failures {params}")

    # GET /api/reports — status, limit (1..200).
    for params in ([], [("status", "pending")], [("status", "processed")], [("status", "failed")],
                   [("status", "PENDING")], [("status", "bogus")], [("status", "")]):
        _call(battery, "/api/reports", params)
    for value in ["1", "200", "0", "201", "abc", ""]:
        _call(battery, "/api/reports", [("limit", value)])
        _call(battery, "/api/reports", [("status", "processed"), ("limit", value)])

    # GET /api/reports/{id} and the summary.
    for report in (1, 2, 999_999):
        _call(battery, f"/api/reports/{report}", [])
    _call(battery, "/api/reports/summary", [])

    # GET /api/quarantine — repo (required), project (default "default"), token.
    for params in ([("repo", REPO)], [("repo", REPO), ("project", PROJECT)],
                   [("repo", REPO), ("project", FRONTEND)], [("repo", REPO), ("project", "")],
                   [("repo", OTHER)], [("repo", "")], [("repo", "bad")],
                   [("project", PROJECT)], []):
        _call(battery, "/api/quarantine", params, headers=API_KEY)
    _call(battery, "/api/quarantine", [("repo", REPO)], label="quarantine list without a token")

    # The paths a parameter cannot be given to; a repeated one takes the last.
    _call(battery, "/api/tests", [("repo", REPO), ("repo", OTHER)])
    _call(battery, "/api/tests", [("repo", OTHER), ("repo", REPO)])
    _call(battery, "/api/tests", [("repo", REPO), ("page", "1"), ("page", "2")])
    _call(battery, "/api/tests", [("repo", REPO), ("unknown", "1")])
    _call(battery, "/api/tests", [("repo", REPO), ("include_stable", "")])

    # The dumps the reads are built from: any query this battery missed would
    # still have to produce the same rows.
    battery.dump(
        [
            "test_cases",
            "test_runs",
            "test_executions",
            "jobs",
            "job_executions",
            ("test_score_history", "test_case_id, day"),
            ("job_score_history", "job_id, day"),
        ],
        label="d2: rows behind every read",
    )
    run = run_id_of(battery, "c1")
    battery.rows(
        "SELECT e.test_case_id, e.status, e.attempt, e.message, c.file, c.line, c.name, "
        "e.failure_category, e.duration "
        f"FROM test_executions e JOIN test_cases c ON c.id = e.test_case_id "
        f"WHERE e.test_run_id = {run} ORDER BY e.id",
        label="d2: one Run's Executions",
    )
