# 06 — Read API: Jobs leaderboard, Job detail, summary, and "seen in Jobs"

## Tracer-Bullet Outcome
The dashboard and agents can:
- list a Repo's Jobs worst first;
- open one Job and see its recent Job executions, each tagged `passed`, `failed` (unexplained), `explained` (with the failing Tests listed) or `skipped`, with SHA, attempt, runner and a CI link;
- get Job summary counts;
- see, on a Test's history, which Jobs its Runs came from.

All of this goes through `queries.py`, so that MCP (task 09) reuses it.

## User Story
As a maintainer (or an agent), I want to see which CI jobs are flaky for reasons other than tests, with the evidence I need to act on it.

## Context Pack
- Read `00-shared-context.md`.
- Repo facts: `queries.py` has the patterns to copy. They are `Scope`, `tier_for`, `escape_like`, `to_test_out`, `list_tests` (paginated, with `total`), `summary`, `get_test` and `search_tests`. Routers stay thin (`routers/tests.py`). Schemas go in `schemas.py`.
- From task 05: `explained_ci_job_ids(db, repo_id, ci_job_ids)`.
- **Jobs are scoped by Repo only.** A `project` filter does not apply, so the Job endpoints have no `project` param.
- Non-goals: UI (08), MCP (09).

## Implementation Contract

### Schemas (`schemas.py`)
```python
class JobOut(BaseModel):
    id: int
    repo: str
    provider: str
    pipeline: str
    name: str
    flakiness_score: float
    tier: Tier
    confirmed_flake_count: int
    last_status: str
    last_seen_at: datetime
    github_issue_number: int | None
    github_issue_url: str | None   # only when provider == "github"

class JobPage(BaseModel):
    items: list[JobOut]; total: int; page: int; page_size: int

class ExplainingTestOut(BaseModel):
    test_id: int; project: str; classname: str; name: str; status: str   # failed | error

class JobExecutionOut(BaseModel):
    id: int
    status: str                         # passed | failed | skipped (as stored)
    outcome: Literal["passed", "failed", "explained", "skipped"]  # failed = unexplained
    commit_sha: str; branch: str
    ci_run_id: str; ci_run_attempt: int; ci_job_id: str
    url: str; runner_name: str; runner_labels: list[str]
    started_at: datetime | None; completed_at: datetime | None; created_at: datetime
    explained_by: list[ExplainingTestOut]   # empty unless outcome == "explained"; cap 20 per execution

class JobHistoryOut(BaseModel):
    job: JobOut
    unexplained_failures: int     # over the returned executions
    explained_failures: int
    executions: list[JobExecutionOut]   # newest first

class JobSummaryOut(BaseModel):
    total_jobs: int; flaky_jobs: int; suspect_jobs: int; confirmed_flaky_jobs: int
    total_job_executions: int; flake_threshold: float

class TestJobLinkOut(BaseModel):   # "seen in Jobs" on a Test
    job_id: int; pipeline: str; name: str
```
`HistoryOut` (the Test history) gains `jobs: list[TestJobLinkOut]`. These are the distinct Jobs whose Job executions share a `ci_job_id` with this Test's Runs in the returned window, ordered by name. For Runs that have a `ci_job_id` but no matching Job execution yet, nothing is shown.

### Queries (`queries.py`)
```python
JobSortKey = Literal["score", "last_seen", "proven"]
async def list_jobs(db, repo: str | None, *, threshold, include_stable=False, flaky_only=False,
                    sort: JobSortKey = "score", page=1, page_size=50) -> schemas.JobPage
async def job_summary(db, repo: str | None, *, threshold) -> schemas.JobSummaryOut
async def get_job(db, job_id: int, *, threshold, executions_limit=60) -> schemas.JobHistoryOut | None
async def search_jobs(db, repo: str, query: str, *, threshold, limit=20) -> list[schemas.JobOut]  # name or pipeline substring
async def find_job_ids(db, repo: str, name: str, pipeline: str | None = None) -> list[int]
```
Filtering and sorting mirror `list_tests` (the default hides stable Jobs, and sorts are the same). `get_job` computes `outcome` by combining the stored status with `explained_ci_job_ids`, and loads `explained_by` with one query for all returned failed executions (no N+1).

### Routes (new `backend/app/routers/jobs.py`, included in `main.py` next to `tests.router`)
| Endpoint | Auth | Notes |
|---|---|---|
| `GET /api/jobs?repo=&include_stable=&sort=&page=&page_size=` | — | `repo` optional. It is normalized, and an invalid value → 422. `page_size` ≤ 100. |
| `GET /api/jobs/summary?repo=` | — | |
| `GET /api/jobs/{job_id}/history?limit=` | — | 404 when unknown. `limit` 1–500, default 60. |

Declare `/api/jobs/summary` **before** `/api/jobs/{job_id}/...`. The Report router does the same for `/api/reports/summary`.

## Acceptance Criteria
- [ ] `GET /api/jobs?repo=acme/app` lists that Repo's non-stable Jobs, worst first, with `total`. `include_stable=true` includes the stable ones.
- [ ] Job history tags an execution `explained` when a linked Run has a failing Test, and lists that Test. An unlinked failure is `failed`, and a pass is `passed`.
- [ ] `unexplained_failures` and `explained_failures` count correctly.
- [ ] `github_issue_url` is null for non-GitHub providers.
- [ ] A Test's history includes `jobs` for Runs linked to known Job executions.
- [ ] `GET /api/jobs/summary` counts match the tiers.
- [ ] Unknown Job → 404. Bad `repo` → 422.

## Test Expectations
- New `backend/tests/test_jobs_api.py` (HTTP, with seeding through the factories).
- `tests/test_test_detail_api.py`: the `jobs` field.
- Frontend types are **not** changed here (task 08). The Test history gains a field, which the TypeScript client ignores.

## Dependencies
- Blocked by: 05.
- Blocks: 08, 09, 10.

## Estimate / Risk
Medium. Risk 2.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && ruff check backend && ruff format --check backend
```
