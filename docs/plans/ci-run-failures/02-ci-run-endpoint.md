# 02 — The CI run endpoint: lookup, attempt, report counts, 404s

## Tracer-Bullet Outcome
`GET /api/ci-runs/{ci_run_id}/failures?repo=…[&ci_run_attempt=N]` exists. It finds the CI run, picks the CI run attempt (the highest one by default), and returns the final response shape with real `reports` counts. `failures` and `failed_jobs` are still empty lists (tasks 03 and 04 fill them). Unknown Repo, CI run or attempt gives `404`. A caller that uploads a JUnit Report and asks at once sees `reports.pending == 1`.

## User Story
As a CI gate, I want to know whether FlakeRadar has seen my CI run attempt and whether its Reports are still being processed, so that I never read "not processed yet" as "nothing failed".

## Description
Add the response schemas (all of them, including the entry types that later tasks fill), one read service in `queries.py`, a new router file, and its registration in `main.py`.

## Context Pack
- Source decisions: `00-shared-context.md`, "Endpoint and identity", "Which CI run attempt", "404 rules", "`reports` counts", "Top-level" and "The response shape".
- Repo facts:
  - Router style (`backend/app/routers/jobs.py`):
    ```python
    from fastapi import APIRouter, Depends, HTTPException, Query
    from sqlalchemy.ext.asyncio import AsyncSession

    from .. import queries, schemas
    from ..config import get_settings
    from ..db import get_db
    from ..identity import normalize_repo

    router = APIRouter()
    ```
  - `backend/app/main.py` imports `from .routers import jobs, reports, tests` and calls `app.include_router(jobs.router)` just before `app.mount("/mcp", mcp_app)`.
  - `queries.py` imports `select`, `func` from sqlalchemy and the models it needs from `.models`. It already imports `JobExecution`, `Job`, `Pipeline`, `Project`, `Repo`, `TestRun`. Add `Report` and `REPORT_PENDING`, `REPORT_PROCESSED`, `REPORT_FAILED` to that import.
  - `Report.status` is one of `pending | processed | failed`.
- Non-goals: filling `failures` (task 03) or `failed_jobs` (task 04); the MCP tool (task 05); README (task 06).

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `feat/ci-run-failures`

## Implementation Contract
- Expected files: `backend/app/schemas.py`, `backend/app/queries.py`, `backend/app/routers/ci_runs.py` (new), `backend/app/main.py`, `backend/tests/test_ci_runs_api.py` (new).
- Interfaces and names:
  - `schemas.py`: add a section `# --- CI run failures (issue #5) ---` at the end of the file:
    ```python
    class ReportCountsOut(BaseModel):
        pending: int
        processed: int
        failed: int


    class CiRunTestFailureOut(BaseModel):
        test_id: int
        project: str
        suite: str
        classname: str
        name: str
        run_id: int
        ci_job_id: str | None
        status: str  # the Test's last Execution in this Run: failed | error | passed (passed on retry) | skipped
        failed_attempts: int  # failed + error Executions of this Test in this Run
        failure_category: str | None  # of the last failing Execution
        message: str  # Failure message of the last failing Execution, first 1024 characters
        tier: Tier
        flakiness_score: float
        confirmed_flake_count: int
        clean_streak: int
        quarantined: bool
        github_issue_number: int | None
        github_issue_url: str | None


    class CiRunFailedJobOut(BaseModel):
        job_id: int
        pipeline: str
        name: str
        ci_job_id: str
        url: str
        outcome: Literal["explained", "unexplained"]
        tier: Tier
        flakiness_score: float
        confirmed_flake_count: int
        clean_streak: int
        github_issue_number: int | None
        github_issue_url: str | None  # only when the Pipeline's provider is github


    class CiRunFailuresOut(BaseModel):
        repo: str
        ci_run_id: str
        ci_run_attempt: int
        flake_threshold: float
        reports: ReportCountsOut  # both Report kinds, this CI run attempt only
        total_failures: int  # (Test, Run) pairs before the cap
        truncated: bool
        failures: list[CiRunTestFailureOut]
        failed_jobs: list[CiRunFailedJobOut]
    ```
  - `queries.py`: add a section `# --- CI run failures (issue #5) ---` at the end of the file:
    ```python
    FAILURES_LIMIT = 500
    MESSAGE_MAX = 1024


    class CiRunNotFound(LookupError):
        """The Repo, CI run or CI run attempt is unknown; str(exc) is the 404 detail."""


    async def ci_run_failures(
        db: AsyncSession,
        repo: str,
        ci_run_id: str,
        ci_run_attempt: int | None,
        *,
        threshold: float,
        limit: int = FAILURES_LIMIT,
    ) -> schemas.CiRunFailuresOut:
        """The failing Tests and failed Jobs of one CI run attempt (issue #5).

        `repo` is already normalized. ci_run_attempt=None means the highest known attempt.
        Raises CiRunNotFound.
        """
    ```
    plus two private helpers in the same section:
    ```python
    async def _ci_run_attempts(db: AsyncSession, repo_id: int, ci_run_id: str) -> set[int]: ...
    async def _ci_run_report_counts(db: AsyncSession, repo_id: int, ci_run_id: str, attempt: int) -> schemas.ReportCountsOut: ...
    ```
    `MESSAGE_MAX` and `limit` are used in task 03. Define them now anyway.
  - `routers/ci_runs.py`: module docstring `"""CI run read API: the failing Tests and failed Jobs of one CI run attempt."""`.
    ```python
    @router.get("/api/ci-runs/{ci_run_id}/failures", response_model=schemas.CiRunFailuresOut)
    async def ci_run_failures(
        ci_run_id: str = Path(..., min_length=1, max_length=255),
        repo: str = Query(..., max_length=255),
        ci_run_attempt: int | None = Query(default=None, ge=1),
        db: AsyncSession = Depends(get_db),
    ):
    ```
  - `main.py`: `from .routers import ci_runs, jobs, reports, tests` and `app.include_router(ci_runs.router)` right after `app.include_router(jobs.router)`.
- Behavior rules:
  1. `ci_run_failures` loads the Repo with `select(Repo).where(Repo.name == repo)`. If there is none, it raises `CiRunNotFound("Repo not found")`.
  2. `_ci_run_attempts` returns the union of three `select(...).distinct()` queries (run all three, then union the sets in Python):
     - `func.coalesce(Report.ci_run_attempt, 1)` where `Report.repo_id == repo_id`, `Report.ci_run_id == ci_run_id`;
     - `func.coalesce(TestRun.ci_run_attempt, 1)` from `TestRun` joined to `Project` on `Project.id == TestRun.project_id`, where `Project.repo_id == repo_id`, `TestRun.ci_run_id == ci_run_id`;
     - `JobExecution.ci_run_attempt` from `JobExecution` joined to `Job` (`Job.id == JobExecution.job_id`) and `Pipeline` (`Pipeline.id == Job.pipeline_id`), where `Pipeline.repo_id == repo_id`, `JobExecution.ci_run_id == ci_run_id`.
  3. Empty set → `CiRunNotFound("CI run not found")`. If `ci_run_attempt is None`, use `max(attempts)`. If it is given and not in the set → `CiRunNotFound("CI run attempt not found")`.
  4. `_ci_run_report_counts`: `select(Report.status, func.count()).where(Report.repo_id == repo_id, Report.ci_run_id == ci_run_id, func.coalesce(Report.ci_run_attempt, 1) == attempt).group_by(Report.status)`. Build `ReportCountsOut` with `.get(status, 0)` for each of `REPORT_PENDING`, `REPORT_PROCESSED`, `REPORT_FAILED`.
  5. Return `CiRunFailuresOut(repo=repo_row.name, ci_run_id=ci_run_id, ci_run_attempt=attempt, flake_threshold=threshold, reports=counts, total_failures=0, truncated=False, failures=[], failed_jobs=[])`. Tasks 03 and 04 replace the last four values.
  6. The router normalizes `repo` with `normalize_repo`, and a `ValueError` becomes `HTTPException(422, detail=str(exc))` (copy `_repo` in `routers/jobs.py`, but `repo` is required here). It calls `queries.ci_run_failures(db, name, ci_run_id, ci_run_attempt, threshold=get_settings().flake_threshold)`. It catches `queries.CiRunNotFound as exc` and raises `HTTPException(status_code=404, detail=str(exc)) from exc`.
- Error and security rules: read-only, no auth (like `/api/jobs`). Do not echo the Repo name back in a 404 detail.

## Steps
1. Add the schemas.
2. Add `FAILURES_LIMIT`, `MESSAGE_MAX`, `CiRunNotFound`, the two helpers and `ci_run_failures` to `queries.py`.
3. Add `routers/ci_runs.py` and register it in `main.py`.
4. Write the tests below, then run the Validator.

## Acceptance Criteria
- [ ] Every test below passes.
- [ ] `GET /api/ci-runs/x/failures` without `repo` gives `422`.
- [ ] The OpenAPI page (`/docs`) lists the endpoint (it does if the router is registered above the static mount).

## Test Expectations
New file `backend/tests/test_ci_runs_api.py`, module docstring `"""CI run failures API: which CI run attempt, report counts, 404s (issue #5)."""`. Use the `client` and `db` fixtures and the factories. Commit before calling `client`. URL helper inside the file:
```python
def _url(ci_run_id: str, repo: str = "acme/app", **params) -> str:
    query = "&".join(f"{k}={v}" for k, v in {"repo": repo, **params}.items())
    return f"/api/ci-runs/{ci_run_id}/failures?{query}"
```
Tests (one `async def test_…` each):
1. `test_unknown_repo_is_404`: no data → `404`, `detail == "Repo not found"`.
2. `test_invalid_repo_is_422`: `repo=nope` → `422`.
3. `test_unknown_ci_run_is_404`: `make_project(db, "acme/app")` only → `404`, `"CI run not found"`.
4. `test_unknown_attempt_is_404`: a Run with `ci_run_id="100", ci_run_attempt=1`; ask `ci_run_attempt=3` → `404`, `"CI run attempt not found"`.
5. `test_attempt_defaults_to_latest_including_pending`: a `processed` junit Report at attempt 1 and a `pending` junit Report at attempt 2, both `ci_run_id="100"` (`make_report(db, proj, b"<x/>", ci_run_id="100", status=..., ci_run_attempt=...)`). Default → `ci_run_attempt == 2`, `reports == {"pending": 1, "processed": 0, "failed": 0}`. With `ci_run_attempt=1` → `reports == {"pending": 0, "processed": 1, "failed": 0}`.
6. `test_null_attempt_counts_as_one`: a Report with `ci_run_attempt=None`, `status="processed"` → `ci_run_attempt == 1`, `reports.processed == 1`.
7. `test_pipeline_reports_are_counted`: one junit Report (`processed`) and one `make_pipeline_report(db, "acme/app")` (pending; its JSON has `ci_run_id "1"`, attempt 1). Ask `ci_run_id "1"`, with the junit Report also at `ci_run_id="1"`, attempt 1 → `reports == {"pending": 1, "processed": 1, "failed": 0}`.
8. `test_found_by_run_after_reports_are_pruned`: only a Run (`make_run(db, proj, ci_run_id="200", ci_run_attempt=1)`), no Report → `200`, `reports` all 0, `failures == []`, `failed_jobs == []`, `total_failures == 0`, `truncated is False`.
9. `test_found_by_job_execution`: only a Job execution (`make_job_execution(db, job, "passed", ci_run_id="300", ci_run_attempt=2)`) → `200`, `ci_run_attempt == 2`.
10. `test_ci_run_id_is_scoped_to_repo`: a Report with `ci_run_id="100"` in `other/repo` only; ask `acme/app` (Repo exists via `make_project`) → `404 "CI run not found"`.
11. `test_ingest_then_ask_sees_pending`: `POST /api/ingest?repo=acme/app&project=backend&commit_sha=abc&ci_run_id=555&ci_run_attempt=1` with `content=make_junit([("t1", "failed")])` and `headers=AUTH` → `202`. Then `GET _url("555")` → `200`, `reports == {"pending": 1, "processed": 0, "failed": 0}`, `failures == []`. (The test client runs no worker, so the Report stays pending.)
12. `test_top_level_fields`: Repo given as `ACME/App` → `repo == "acme/app"`; `ci_run_id` echoed; `flake_threshold == get_settings().flake_threshold` (`from app.config import get_settings`).

## Validator
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```

## Dependencies
- Blocked by: 01.
- Blocks: 03, 04, 05.

## Labels
`feature`, `backend`, `priority:high`
