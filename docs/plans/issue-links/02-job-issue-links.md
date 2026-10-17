# 02 — Issue links for Jobs: `PUT`/`DELETE /api/jobs/{id}/issue`

## Tracer-Bullet Outcome
A caller with the API key can link a GitHub issue to a Job and remove the link, with the same rules as Tests. A Job whose Pipeline is not on GitHub cannot be linked (`422`), but its link can always be cleared. `/api/jobs/{id}/history` and the jobs leaderboard then show the issue.

## User Story
As fabro's scout, which files issues for flaky CI jobs, I want to record the issue I filed for a Job, so that FlakeRadar links it and does not file a duplicate.

## Description
Two service functions in `issue_links.py`, two routes in `routers/jobs.py`, and tests appended to the task 01 test file. No schema change: task 01 added `IssueLinkIn`.

## Context Pack
- Source decisions: `00-shared-context.md`, "REST: `PUT …`" (rule 2 is the Job-only provider rule) and "REST: `DELETE …`".
- Repo facts:
  - Task 01 created `backend/app/issue_links.py` with the exceptions, `_check_link(kind, current, number, *, repo, url, replace)`, `link_test_issue` and `unlink_test_issue`. Follow the same shape.
  - `queries.jobs_select()` returns `select(Job, Pipeline, Repo)` joined (`Job.pipeline_id == Pipeline.id`, `Pipeline.repo_id == Repo.id`). `queries.to_job_out(job, pipeline, repo, threshold)` takes the `Repo` **row** (it reads `repo.name`), and sets `github_issue_url` only when `pipeline.provider == "github"`.
  - `routers/jobs.py` imports:
    ```python
    from fastapi import APIRouter, Depends, HTTPException, Query
    from sqlalchemy.ext.asyncio import AsyncSession

    from .. import queries, schemas
    from ..config import get_settings
    from ..db import get_db
    from ..identity import normalize_repo
    ```
    Its last route is `GET /api/jobs/{job_id}/history`, which answers `404 "Job not found"`. `GET /api/jobs/summary` is declared before the `{job_id}` routes; the new routes go after `job_history`.
  - `GET /api/jobs/{id}/history` returns `JobHistoryOut`, whose `job` field is a `JobOut`.
  - Factories: `make_pipeline(db, repo="acme/app", name=".github/workflows/ci.yml", provider="github")`, `make_job(db, pipeline, name="test", **fields)`.
- Non-goals: MCP (task 04); README (task 05); any change to the filer's provider rule.

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `feat/issue-links`

## Implementation Contract
- Expected files: `backend/app/issue_links.py`, `backend/app/routers/jobs.py`, `backend/tests/test_issue_links_api.py`.
- Interfaces and names:
  - `issue_links.py`: import `Job` from `.models` and `jobs_select`, `to_job_out` from `.queries`. Add after the Test functions:
    ```python
    async def link_job_issue(
        db: AsyncSession, job_id: int, number: int, *, url: str | None = None, replace: bool = False, threshold: float
    ) -> schemas.JobOut: ...


    async def unlink_job_issue(db: AsyncSession, job_id: int, *, threshold: float) -> schemas.JobOut: ...
    ```
  - `routers/jobs.py`: change the imports to `from .. import issue_links, queries, schemas` and add `from ..auth import require_token`. Add after `job_history`:
    ```python
    @router.put("/api/jobs/{job_id}/issue", response_model=schemas.JobOut, dependencies=[Depends(require_token)])
    async def link_issue(
        job_id: int,
        body: schemas.IssueLinkIn,
        replace: bool = Query(default=False),
        db: AsyncSession = Depends(get_db),
    ):
        """Link a GitHub issue filed elsewhere; FlakeRadar then files none for this Job."""


    @router.delete("/api/jobs/{job_id}/issue", response_model=schemas.JobOut, dependencies=[Depends(require_token)])
    async def unlink_issue(job_id: int, db: AsyncSession = Depends(get_db)):
        """Remove the Job's Issue link."""
    ```
    Same error mapping as in `routers/tests.py`.
- Behavior rules:
  1. `link_job_issue`:
     - `row = (await db.execute(jobs_select().where(Job.id == job_id).with_for_update(of=Job))).first()`.
     - `None` → `raise IssueLinkTargetNotFound("Job not found")`.
     - `job, pipeline, repo = row`.
     - `pipeline.provider != "github"` → `raise IssueNotLinkable("Only Jobs of GitHub Pipelines can link a GitHub issue")`. This check comes **before** `_check_link`.
     - `_check_link("Job", job.github_issue_number, number, repo=repo.name, url=url, replace=replace)`.
     - Set, commit, `logger.info("Linked issue %s#%s to job %s (was %s)", repo.name, number, job.id, previous)`, `return to_job_out(job, pipeline, repo, threshold)`.
  2. `unlink_job_issue`: the same load and `404`, **no** provider check. Clear, commit, log `"Unlinked issue %s#%s from job %s"` only when there was a link, return `to_job_out(...)`.
- Error and security rules: same as task 01.

## Steps
1. Append the tests below (red).
2. Add the two service functions and the two routes.
3. Run the Validator.

## Acceptance Criteria
- [ ] Every test below passes, and so do the task 01 tests.
- [ ] `/docs` lists `PUT` and `DELETE /api/jobs/{job_id}/issue`.

## Test Expectations
Append to `backend/tests/test_issue_links_api.py`. Seed with `pipe = await make_pipeline(db, "acme/app", ".github/workflows/ci.yml")` (or `provider="gitlab"`) and `job = await make_job(db, pipe, name="e2e", **fields)`, then commit.
1. `test_link_job_issue`: `PUT /api/jobs/{job.id}/issue` `{"number": 42}` with `AUTH` → `200`, `body["id"] == job.id`, `body["github_issue_number"] == 42`, `body["github_issue_url"] == "https://github.com/acme/app/issues/42"`. `GET /api/jobs/{job.id}/history` → `json()["job"]["github_issue_number"] == 42`.
2. `test_job_on_a_non_github_pipeline_cannot_be_linked`: `provider="gitlab"`, Job with `github_issue_number=3`. `PUT {"number": 4}` → `422`, `detail == "Only Jobs of GitHub Pipelines can link a GitHub issue"`, the database still has `3`. `DELETE` → `200`, `github_issue_number is None`.
3. `test_job_conflict_and_replace`: Job with `github_issue_number=4`. `PUT {"number": 5}` → `409`, `detail == "Job already links issue #4; pass replace=true to replace it"`. `?replace=true` → `200`, `5`.
4. `test_job_url_must_be_the_issue_in_the_jobs_repo`: `{"number": 6, "url": "https://github.com/other/repo/issues/6"}` → `422`, `detail == "url must be https://github.com/acme/app/issues/6: the issue must be in the Job's own Repo"`. `{"number": 6, "url": "https://github.com/acme/app/issues/6"}` → `200`.
5. `test_unknown_job_and_missing_token`: `PUT /api/jobs/99999/issue` (valid body, `AUTH`) → `404`, `detail == "Job not found"`. `DELETE` → `404`. `PUT` and `DELETE` on a real Job with no headers → `401`.
6. `test_unlink_job_issue`: Job with `github_issue_number=8`. `DELETE` → `200`, `github_issue_number is None`, `github_issue_url is None`. `DELETE` again → `200`.

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
