# 04 — `failed_jobs`: the failed Jobs of a CI run attempt

## Tracer-Bullet Outcome
The endpoint lists every failed Job execution of the CI run attempt. Each one is marked `explained` (a failing Test in the same Job explains it) or `unexplained` (setup or infrastructure), with the Job's current tier and score. A CI run that is red only because of a setup failure now shows `failures: []` and one `unexplained` entry in `failed_jobs`. It no longer looks all-clear.

## User Story
As a CI gate, I want to see failed Jobs that no failing Test explains, so that I do not treat an infrastructure failure as a green build.

## Description
Add one private helper to the `# --- CI run failures (issue #5) ---` section of `queries.py` and call it from `ci_run_failures`. No schema change: task 02 defined `CiRunFailedJobOut`.

## Context Pack
- Source decisions: `00-shared-context.md`, "`failed_jobs`: failed Jobs". ADR 0004 (explained and unexplained failures).
- Repo facts:
  - `attribution.explained_ci_job_ids(db, repo_id, ci_job_ids) -> set[str]`, already imported in `queries.py`.
  - `queries.jobs_select()` returns `select(Job, Pipeline, Repo)` with the joins. `to_job_out` sets `github_issue_url=issue_url(repo.name, job.github_issue_number) if pipeline.provider == "github" else None`. Use the same rule.
  - `models.JOB_FAILED == "failed"`, already imported in `queries.py`.
  - `JobExecution.ci_run_attempt` is `NOT NULL`, so no `coalesce`.
- Non-goals: listing passed or skipped Job executions; `explained_by` lists (the caller can match `ci_job_id` with `failures[].ci_job_id`).

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `feat/ci-run-failures`

## Implementation Contract
- Expected files: `backend/app/queries.py`, `backend/tests/test_ci_runs_api.py`.
- Interfaces and names:
  ```python
  async def _ci_run_failed_jobs(
      db: AsyncSession, repo: Repo, ci_run_id: str, attempt: int, *, threshold: float
  ) -> list[schemas.CiRunFailedJobOut]:
      """Failed Job executions of one CI run attempt, each explained or unexplained."""
  ```
  `ci_run_failures` calls it and sets `failed_jobs` to the result.
- Behavior rules:
  1. Query:
     ```python
     select(JobExecution, Job, Pipeline)
     .join(Job, Job.id == JobExecution.job_id)
     .join(Pipeline, Pipeline.id == Job.pipeline_id)
     .where(
         Pipeline.repo_id == repo.id,
         JobExecution.ci_run_id == ci_run_id,
         JobExecution.ci_run_attempt == attempt,
         JobExecution.status == JOB_FAILED,
     )
     .order_by(Pipeline.name, Job.name, JobExecution.id)
     ```
  2. `explained = await explained_ci_job_ids(db, repo.id, [e.ci_job_id for e, _, _ in rows])`.
  3. Each row becomes `CiRunFailedJobOut(job_id=job.id, pipeline=pipeline.name, name=job.name, ci_job_id=e.ci_job_id, url=e.url, outcome="explained" if e.ci_job_id in explained else "unexplained", tier=tier_for(job.flakiness_score, threshold), flakiness_score=job.flakiness_score, confirmed_flake_count=job.confirmed_flake_count, clean_streak=job.clean_streak, github_issue_number=job.github_issue_number, github_issue_url=issue_url(repo.name, job.github_issue_number) if pipeline.provider == "github" else None)`.
- Error and security rules: None new.

## Steps
1. Write the tests below first (red).
2. Add `_ci_run_failed_jobs` and wire it into `ci_run_failures`.
3. Run the Validator.

## Acceptance Criteria
- [ ] Every test below passes, and so do the task 02 and 03 tests.

## Test Expectations
Append to `backend/tests/test_ci_runs_api.py`. Seed with `make_pipeline(db, "acme/app", ".github/workflows/ci.yml")`, `make_job(db, p, name=..., **fields)` and `make_job_execution(db, job, status, ci_job_id=..., ci_run_id="100", ci_run_attempt=..., url=...)`. For "explained", add a Run in the same Repo with the same `ci_job_id` and a failing Execution, as `test_jobs_api.py::test_job_history_outcomes_and_explained_by` does.
1. `test_failed_jobs_explained_and_unexplained`: Jobs `backend` (fails, `ci_job_id="J-B"`, explained by a failing Test in a Run with `ci_run_id="100", ci_run_attempt=1, ci_job_id="J-B"`), `e2e` (fails, `"J-E"`, no Test), `lint` (passes, `"J-L"`). All in `ci_run_id="100"`, attempt 1. Result: `[(j["name"], j["outcome"]) for j in failed_jobs] == [("backend", "explained"), ("e2e", "unexplained")]`. `failures` has the one failing Test.
2. `test_red_without_failing_tests`: only an `e2e` Job execution, failed, `ci_run_id="100"` → `failures == []`, `reports` all 0, `failed_jobs` has one entry with `outcome "unexplained"`.
3. `test_failed_job_fields`: Job `e2e` with `flakiness_score=0.1, confirmed_flake_count=1, clean_streak=2, github_issue_number=4`, execution `url="https://ci/1"`. The entry `== {"job_id": job.id, "pipeline": ".github/workflows/ci.yml", "name": "e2e", "ci_job_id": "J-E", "url": "https://ci/1", "outcome": "unexplained", "tier": "suspect", "flakiness_score": 0.1, "confirmed_flake_count": 1, "clean_streak": 2, "github_issue_number": 4, "github_issue_url": "https://github.com/acme/app/issues/4"}`.
4. `test_failed_job_issue_url_null_for_non_github`: Pipeline with `provider="gitlab"`, Job with `github_issue_number=4` → `github_issue_url is None`.
5. `test_failed_jobs_of_other_attempts_are_ignored`: `e2e` failed at attempt 1, passed at attempt 2 (`ci_run_id="100"` both) → default (attempt 2) gives `failed_jobs == []`. `ci_run_attempt=1` gives one entry.

## Validator
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```

## Dependencies
- Blocked by: 03 (same function and test file; run them in order to avoid conflicts).
- Blocks: 05, 06.

## Labels
`feature`, `backend`, `priority:high`
