# 03 — `POST /api/ingest/pipeline` and Report API kinds

## Tracer-Bullet Outcome
CI can POST a JSON Pipeline report. It is validated and stored as a `pending` Report of kind `pipeline`, and the response is `202 {"report_id", "status": "pending"}`. The Report endpoints list both kinds, and they show `kind` and a nullable `project`. The queue indicator in the UI still renders (with no project for Pipeline reports). **The processor does not handle Pipeline reports yet.** Until task 04, `process_report` must mark them `failed` with a clear error rather than crashing on a missing project.

## User Story
As a CI author on any provider, I want one small JSON contract for reporting job results, so that FlakeRadar can track my jobs without knowing my CI system.

## Context Pack
- Read `00-shared-context.md`, sections "Pipeline report payload" and "Identity".
- Repo facts:
  - JUnit ingest is in `backend/app/routers/reports.py`. It validates, calls `get_or_create_project` and stores a `Report`. ADR 0002 says to validate at ingest and do the work in the processor.
  - `_report_query()` inner-joins `Project`, so Pipeline reports would vanish from `GET /api/reports`. It must join `Repo` through `Report.repo_id`, with an **outer** join to `Project`.
  - `schemas.ReportOut.project: str` → becomes `str | None`. Add `kind: str`.
  - Frontend `src/api.ts::ReportInfo.project: string` → `string | null`, plus `kind`. `src/components/QueueIndicator.tsx` line ~59 renders `{r.repo} / {r.project}`. Render `{r.repo} / {r.project}` for JUnit and `{r.repo} · pipeline` when `project` is null.
- Non-goals: processing (task 04); the GitHub reporter (task 07).

## Implementation Contract

### `backend/app/identity.py`
Add `normalize_provider(raw: str) -> str` (trim, lowercase, `^[a-z0-9_-]{1,32}$`, else `ValueError`). Split out `get_or_create_repo(db, repo) -> Repo` (race-safe, `ON CONFLICT DO NOTHING`). Make `get_or_create_project` use it.

### `backend/app/schemas.py`
```python
class JobResultIn(BaseModel):
    ci_job_id: str = Field(min_length=1, max_length=255)
    name: str = Field(min_length=1, max_length=512)
    status: Literal["passed", "failed", "skipped"]
    url: str = Field(default="", max_length=2048)
    runner_name: str = Field(default="", max_length=255)
    runner_labels: list[Annotated[str, Field(max_length=255)]] = Field(default_factory=list, max_length=50)
    started_at: datetime | None = None
    completed_at: datetime | None = None

class PipelineReportIn(BaseModel):
    repo: str = Field(max_length=255)
    provider: str = Field(max_length=32)
    pipeline: str = Field(min_length=1, max_length=512)
    commit_sha: str = Field(min_length=1, max_length=64)
    branch: str = Field(min_length=1, max_length=255)
    default_branch: str | None = Field(default=None, max_length=255)
    ci_run_id: str = Field(default="", max_length=255)
    ci_run_attempt: int = Field(default=1, ge=1)
    jobs: list[JobResultIn] = Field(min_length=1, max_length=1000)
```
`ReportOut` gains `kind: str` and `project: str | None`.

### `backend/app/routers/reports.py`
```python
@router.post("/api/ingest/pipeline", status_code=202, response_model=schemas.IngestAccepted,
             dependencies=[Depends(require_token)])
async def ingest_pipeline(body: schemas.PipelineReportIn, db: AsyncSession = Depends(get_db)): ...
```
- Normalize `repo` (`normalize_repo`) and `provider` (`normalize_provider`). Strip `pipeline`, `branch` and each job `name`. `default_branch` is stripped, and empty becomes `None`. Any `ValueError` → 422.
- Duplicate `ci_job_id` values inside one payload → 422 `"duplicate ci_job_id in jobs"`.
- Store `Report(kind="pipeline", repo_id=repo.id, project_id=None, commit_sha, branch, ci_run_id, ci_run_attempt, pipeline, default_branch, body=<normalized payload>.model_dump_json().encode())`. Store the **normalized** payload, so the processor never has to validate it again.
- Return `202 {"report_id": id, "status": "pending"}`.
- Pydantic validation errors return FastAPI's default 422. That is fine.

Update the Report read endpoints (`_report_query`, `_report_out`) for `kind` and the nullable project.

### `backend/app/processing.py` (temporary guard; task 04 replaces it)
At the top of `process_report`: `if report.kind != REPORT_KIND_JUNIT: raise ValueError("pipeline reports are not processed yet")`. The existing failure path marks the Report `failed`.

### Frontend
Update `api.ts` (`ReportInfo.kind: "junit" | "pipeline"`, `project: string | null`) and `QueueIndicator.tsx` as described. Add a test to `QueueIndicator.test.tsx` for the pipeline row.

## Acceptance Criteria
- [ ] A valid payload → 202, with a `pending` Report of `kind == "pipeline"`, the right `repo_id`, and `project_id` NULL.
- [ ] No token → 401. Bad repo → 422. Bad provider → 422. Empty `jobs` → 422. Unknown `status` → 422. Duplicate `ci_job_id` → 422.
- [ ] `GET /api/reports` and `GET /api/reports/{id}` return Pipeline reports with `kind: "pipeline"` and `project: null`. JUnit reports return `kind: "junit"`.
- [ ] The Repo is auto-created on first Pipeline report.
- [ ] Until task 04, processing a Pipeline report marks it `failed` and does not crash the worker.

## Test Expectations
- New `backend/tests/test_pipeline_ingest_api.py` covering the criteria above.
- `tests/test_reports_api.py`: add `kind` assertions, and a Pipeline report in the list.
- `tests/test_identity.py`: `normalize_provider` cases.
- Frontend: `QueueIndicator.test.tsx` pipeline row.

## Dependencies
- Blocked by: 01.
- Blocks: 04, 07.

## Estimate / Risk
Small–medium. Risk 2.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && ruff check backend && ruff format --check backend
cd frontend && npm run typecheck && npm run lint && npm run format:check && npm test
```
