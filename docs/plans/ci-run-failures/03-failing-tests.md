# 03 — `failures`: the failing Tests of a CI run attempt

## Tracer-Bullet Outcome
`GET /api/ci-runs/{ci_run_id}/failures` lists every Test that failed or errored in the CI run attempt, one entry per (Test, Run). Each entry has its retry result, its Failure message, and the Test's current tier and score. A CI run with two attempts gives different lists for `ci_run_attempt=1` and the default (latest). More than 500 failing pairs are cut, and the response says so.

## User Story
As a CI gate, I want the failing Tests of my CI run attempt with their current tier, so that I can decide whether the build is red only because of Tests FlakeRadar already knows are flaky.

## Description
Add one private helper to the `# --- CI run failures (issue #5) ---` section of `queries.py` and call it from `ci_run_failures`. No schema, router or migration change. Task 02 already defined `CiRunTestFailureOut`, `FAILURES_LIMIT` and `MESSAGE_MAX`.

## Context Pack
- Source decisions: `00-shared-context.md`, "`failures`: failing Tests".
- Repo facts:
  - `queries.ci_run_failures` (task 02) already has the Repo row (`repo_row`), the resolved `attempt`, `threshold` and `limit`. It returns `total_failures=0, truncated=False, failures=[]`.
  - `scoring.FAILING = {"failed", "error"}`. `queries.py` already imports it (`from .scoring import FAILING`).
  - `queries.tier_for(score, threshold)` and `queries.issue_url(repo_name, number)`. Tests get `github_issue_url = issue_url(repo_name, tc.github_issue_number)` whatever the provider. This is the same as `to_test_out`.
  - `TestExecution.attempt` is unique per (Test, Run) for data written by the processor, but a factory can write duplicates. Break ties with `TestExecution.id`.
  - `batching.chunks(items, size=1000)` splits IN lists. It is already imported in `queries.py`.
- Non-goals: `failed_jobs` (task 04); Failure details; `trend`.

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `feat/ci-run-failures`

## Implementation Contract
- Expected files: `backend/app/queries.py`, `backend/tests/test_ci_runs_api.py`.
- Interfaces and names:
  ```python
  async def _ci_run_test_failures(
      db: AsyncSession, repo: Repo, ci_run_id: str, attempt: int, *, threshold: float, limit: int
  ) -> tuple[list[schemas.CiRunTestFailureOut], int]:
      """Failing (Test, Run) pairs of one CI run attempt, capped at `limit`; also the uncapped count."""
  ```
  `ci_run_failures` calls it and sets `failures=items`, `total_failures=total`, `truncated=total > len(items)`.
- Behavior rules (build the queries like this):
  1. Runs of the CI run attempt, as a scalar subquery:
     ```python
     run_ids = (
         select(TestRun.id)
         .join(Project, Project.id == TestRun.project_id)
         .where(
             Project.repo_id == repo.id,
             TestRun.ci_run_id == ci_run_id,
             func.coalesce(TestRun.ci_run_attempt, 1) == attempt,
         )
         .scalar_subquery()
     )
     ```
  2. Failing pairs, as a subquery:
     ```python
     pairs = (
         select(TestExecution.test_case_id, TestExecution.test_run_id)
         .where(TestExecution.test_run_id.in_(run_ids), TestExecution.status.in_(FAILING))
         .distinct()
         .subquery()
     )
     ```
  3. `total = (await db.execute(select(func.count()).select_from(pairs))).scalar_one()`. If `total == 0`, return `([], 0)`.
  4. The page of pairs:
     ```python
     select(TestCase, Project.name, TestRun.id, TestRun.ci_job_id)
     .select_from(pairs)
     .join(TestCase, TestCase.id == pairs.c.test_case_id)
     .join(TestRun, TestRun.id == pairs.c.test_run_id)
     .join(Project, Project.id == TestCase.project_id)
     .order_by(Project.name, TestCase.suite, TestCase.classname, TestCase.name, TestRun.id)
     .limit(limit)
     ```
  5. All Executions of the page's pairs: `select(TestExecution).where(TestExecution.test_run_id.in_(page_run_ids), TestExecution.test_case_id.in_(page_test_ids)).order_by(TestExecution.attempt, TestExecution.id)`. Both IN lists have at most `limit` (500) distinct ids, so no chunking is needed. Group them in Python by `(test_case_id, test_run_id)`. Skip rows whose key is not a page pair (the two IN lists can cross-match).
  6. For each page pair, in page order, with `execs` = its Executions in `(attempt, id)` order:
     - `last = execs[-1]`, `failing = [e for e in execs if e.status in FAILING]`, `last_failing = failing[-1]`;
     - `status=last.status`, `failed_attempts=len(failing)`, `failure_category=last_failing.failure_category`, `message=last_failing.message[:MESSAGE_MAX]`;
     - `test_id=tc.id`, `project`, `suite`, `classname`, `name`, `run_id`, `ci_job_id` (the Run's), `tier=tier_for(tc.flakiness_score, threshold)`, `flakiness_score`, `confirmed_flake_count`, `clean_streak`, `quarantined`, `github_issue_number`, `github_issue_url=issue_url(repo.name, tc.github_issue_number)`.
- Error and security rules: None new. Messages come from CI output. Return them as data and never interpret them.

## Steps
1. Write the tests below first (red).
2. Add `_ci_run_test_failures` and wire it into `ci_run_failures`.
3. Run the Validator.

## Acceptance Criteria
- [ ] Every test below passes, and so do the task 02 tests.
- [ ] A Test that passed on retry is listed with `status == "passed"`. A caller can drop it by filtering `status in ("failed", "error")`.

## Test Expectations
Append to `backend/tests/test_ci_runs_api.py` (reuse `_url`). Seed with `make_project`, `make_test_case`, `make_run(..., ci_run_id=..., ci_run_attempt=..., ci_job_id=...)` and `make_execution(..., status=..., message=..., attempt=..., failure_category=...)`, then `await db.commit()`.
1. `test_failures_of_two_attempts`: Project `backend`, Tests `t1`, `t2`. Run A (`ci_run_id="100"`, attempt 1): `t1` failed, `t2` passed. Run B (`"100"`, attempt 2): `t1` passed, `t2` failed. Default → `ci_run_attempt == 2`, names `["t2"]`. `ci_run_attempt=1` → names `["t1"]`.
2. `test_failure_entry_fields`: Test `t1` with `flakiness_score=0.5, confirmed_flake_count=2, clean_streak=3, quarantined=True, github_issue_number=9`. One Run (`"100"`, attempt 1, `ci_job_id="J1"`) with `t1` `error`, `message="boom"`, `failure_category="network"`. Assert the entry equals (compare with `==` on a dict) `{"test_id": t1.id, "project": "backend", "suite": "unit", "classname": "tests.test_mod", "name": "t1", "run_id": run.id, "ci_job_id": "J1", "status": "error", "failed_attempts": 1, "failure_category": "network", "message": "boom", "tier": "flaky", "flakiness_score": 0.5, "confirmed_flake_count": 2, "clean_streak": 3, "quarantined": True, "github_issue_number": 9, "github_issue_url": "https://github.com/acme/app/issues/9"}`. The default `flake_threshold` is `0.30` (`backend/app/config.py`), so 0.5 is `flaky`. Also assert `total_failures == 1` and `truncated is False`.
3. `test_retries_in_one_run`: one Run. `t1`: attempt 0 `failed` (`"first"`, `"timing"`), attempt 1 `passed`. `t2`: attempt 0 `failed` (`"a"`), attempt 1 `failed` (`"b"`, `"assertion"`). `t3`: attempt 0 `passed` only. Result: two entries. `t1`: `status "passed"`, `failed_attempts 1`, `message "first"`, `failure_category "timing"`. `t2`: `status "failed"`, `failed_attempts 2`, `message "b"`, `failure_category "assertion"`. No `t3`.
4. `test_same_test_in_two_runs_is_two_entries`: `t1` fails in two Runs of the same attempt (`ci_job_id` `"A"` and `"B"`). Two entries, in `run_id` order, with `ci_job_id` `["A", "B"]`.
5. `test_skipped_and_passing_tests_are_not_listed`: one Run with `t1` passed and `t2` skipped → `failures == []`, `total_failures == 0`.
6. `test_message_is_capped`: `message="x" * 5000` → `len(entry["message"]) == 1024`.
7. `test_order`: Projects `backend` and `api`. Failing Tests `backend/tests.b::z`, `backend/tests.a::y`, `api/tests.a::x` (pass `classname=` to `make_test_case`) → order `[("api","x"), ("backend","y"), ("backend","z")]`.
8. `test_other_ci_runs_are_ignored`: a failing `t1` in `ci_run_id="999"` and a passing `t1` in `"100"` → asking `"100"` gives `failures == []`.
9. `test_cap_and_truncated` (calls the service directly, so it needs no 501 rows): 3 failing Tests in one Run, `await queries.ci_run_failures(db, "acme/app", "100", None, threshold=0.3, limit=2)` (`from app import queries`) → `total_failures == 3`, `truncated is True`, `len(failures) == 2`, names `["t1", "t2"]`.

## Validator
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```

## Dependencies
- Blocked by: 02.
- Blocks: 05, 06.

## Labels
`feature`, `backend`, `priority:high`
