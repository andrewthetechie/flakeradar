# 05 — Link JUnit reports to Job executions, and handle explained failures

## Tracer-Bullet Outcome
JUnit ingest accepts `ci_job_id`, `ci_run_attempt` and `pipeline`, and processing copies them onto the Run. When a Job is scored, a failed Job execution with a failing Test from the same `ci_job_id` counts as `skipped`. Only unexplained failures move a Job's score. This works in either arrival order: when a linked JUnit report arrives after the Pipeline report, the processor rescores the Job.

## User Story
As a maintainer, I want the Jobs leaderboard to show failures that tests do not explain (setup, network, runners), so that a flaky test does not also make its job look flaky.

## Context Pack
- Read `00-shared-context.md`, section "Attribution". ADR 0004 gives the reasons.
- From task 01: `reports.ci_job_id`, `reports.ci_run_attempt`, `reports.pipeline`, `test_runs.ci_job_id` (indexed), `test_runs.ci_run_attempt`, `test_runs.pipeline` and `job_executions.ci_job_id` (indexed).
- From task 04: `rescore_jobs(db, job_ids)` and its history helper.
- The JUnit side is in `routers/reports.py::ingest_endpoint` (query params) and `processing.py::process_report` (creates the `TestRun`).
- Non-goals: UI and API for the explained or unexplained breakdown (06); docs for the new params (07).

## Implementation Contract

### JUnit ingest (`routers/reports.py`)
New optional query params, all stored on the Report:
- `ci_job_id: str | None = Query(default=None, max_length=255)`. Strip it, and empty becomes `None`.
- `ci_run_attempt: int | None = Query(default=None, ge=1)`.
- `pipeline: str | None = Query(default=None, max_length=512)`. Strip it, and empty becomes `None`. On GitHub the snippet derives it from `GITHUB_WORKFLOW_REF` (task 07). It is stored for display only. Attribution never matches on it.

### Processing (`processing.py`)
1. In the JUnit path, copy `ci_job_id`, `ci_run_attempt` and `pipeline` from the Report onto the new `TestRun`.
2. After the JUnit path rescores Tests, if `report.ci_job_id` is set, find the Jobs to rescore:
   ```sql
   SELECT DISTINCT je.job_id FROM job_executions je
   JOIN jobs j ON j.id = je.job_id JOIN pipelines p ON p.id = j.pipeline_id
   WHERE je.ci_job_id = :ci_job_id AND p.repo_id = :repo_id
   ```
   Then call `rescore_jobs` on them. Put the ids in `ProcessOutcome.touched_job_ids` too, so that task 10 can file issues.
3. In the Job history used by `rescore_jobs`, add an `explained` flag per Job execution. The status fed to scoring is `"skipped"` when `status == "failed" and explained`, and the stored status otherwise. `explained` is:
   ```sql
   EXISTS (SELECT 1 FROM test_runs tr
           JOIN projects pr ON pr.id = tr.project_id
           JOIN test_executions te ON te.test_run_id = tr.id
           WHERE tr.ci_job_id = job_executions.ci_job_id
             AND pr.repo_id = pipelines.repo_id
             AND te.status IN ('failed', 'error'))
   ```
   Compute it in the same query as the history (a correlated `exists()` column) and not per row in Python.
4. Expose a reusable read helper for task 06:
   ```python
   async def explained_ci_job_ids(db: AsyncSession, repo_id: int, ci_job_ids: list[str]) -> set[str]:
       """The subset of ci_job_ids that have a failing Test execution in this Repo."""
   ```
   Put it in `processing.py` or a small new module `attribution.py`. Either is fine. It must use the same definition as the scoring query. Add a test that asserts both agree on the same data.

### Edge cases (required behaviour)
- A JUnit report with a `ci_job_id` that matches no Job execution is a no-op for Jobs. Later, when the Pipeline report arrives, its rescore sees the Run.
- One Job execution can have several linked Runs (one per Project). **Any** failing Test execution in any of them explains the failure.
- A **passing** Job execution with failing Tests (for example `continue-on-error` or retries inside the runner) stays `passed`. Attribution only ever changes `failed` to `skipped`.
- Retention: once linked Test executions are pruned, a failure becomes unexplained again. Accept this. Test and Job executions share `execution_retention_days`, so they age out together.

## Acceptance Criteria
- [ ] Pipeline report first: Job execution `J1` failed, then a JUnit report with `ci_job_id=J1` containing a failed Test → after the JUnit report is processed, the Job's history treats `J1` as skipped, and the score is recomputed.
- [ ] JUnit first, then the Pipeline report → the same result.
- [ ] A Job alternating fail/pass on `main`, where every failure is explained → score 0. The same history with no JUnit → score > 0.
- [ ] A Run with the same `ci_job_id` in **another Repo** does not explain the failure.
- [ ] A JUnit report with only passing Tests does not explain the failure.
- [ ] `ci_job_id`, `ci_run_attempt` and `pipeline` round-trip from the ingest query onto `test_runs`.
- [ ] `explained_ci_job_ids` agrees with the scoring query.

## Test Expectations
- New `backend/tests/test_attribution.py` for the criteria above (both arrival orders, cross-Repo isolation, passing-tests case, and agreement between the helper and scoring).
- `tests/test_ingest_api.py`: the new params are stored, validated (`ci_run_attempt=0` → 422), and empty strings become `None`.

## Dependencies
- Blocked by: 04.
- Blocks: 06, 07, 10.

## Estimate / Risk
Medium. Risk 3: the correlated query and the arrival-order handling.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && ruff check backend && ruff format --check backend
```
