# 09 — MCP: `top_flaky_jobs`, `search_jobs` and `get_job`

## Tracer-Bullet Outcome
An agent connected to `/mcp/` can list a Repo's worst Jobs, find a Job by name, and read one Job's recent Job executions with the explained and unexplained breakdown and the Tests behind each explained failure. `get_test` also reports which Jobs the Test was seen in. All tools stay read-only.

## User Story
As an agent fixing CI, I want to tell whether a job fails because of flaky tests or because of infrastructure, so that I fix the right thing.

## Context Pack
- Read `00-shared-context.md`.
- `backend/app/mcp_server.py`: `build_mcp(session_factory, *, api_token)`, the helpers `_scope`, `_check_limit` (1–100) and `_cap` (4 KB), and the `INSTRUCTIONS` string. The tools return `model_dump(mode="json")` dicts. Errors are raised as `ToolError`.
- Queries from task 06: `list_jobs`, `search_jobs`, `get_job`, `find_job_ids`. `HistoryOut.jobs`.
- The tests in `backend/tests/test_mcp.py` use `async with Client(mcp) as c: await c.call_tool(...)`, and `r.data` is the value returned.
- Non-goals: any tool that writes.

## Implementation Contract
```python
@mcp.tool
async def top_flaky_jobs(repo: str, limit: int = 20, include_suspect: bool = True) -> list[dict]:
    """Worst CI jobs first in a Repo. Job scores count only UNEXPLAINED failures
    (failures with no failing test in the same job)."""

@mcp.tool
async def search_jobs(repo: str, query: str, limit: int = 20) -> list[dict]:
    """Find Jobs whose name or Pipeline (workflow file) contains `query`."""

@mcp.tool
async def get_job(job_id: int | None = None, repo: str | None = None, name: str | None = None,
                  pipeline: str | None = None, executions_limit: int = 20) -> dict:
    """One Job: score, tier, unexplained vs explained failures, and recent
    executions (outcome, sha, branch, attempt, runner, CI url, explaining tests)."""
```
- Name resolution in `get_job` works like `get_test`: no match → `ToolError`, and several matches → `ToolError` that lists the ids and suggests passing `pipeline`.
- Cap `explained_by` at 5 Tests per execution in the MCP output, to keep the result small.
- `get_test` adds a `"jobs": [{"job_id", "pipeline", "name"}]` key.
- Extend `INSTRUCTIONS` by two or three sentences: Jobs and Pipelines exist, Job scores count only unexplained failures, and agents should start with `top_flaky_jobs` for CI-level problems.

## Acceptance Criteria
- [ ] `list_tools()` includes the three new tools, and no tool writes.
- [ ] `top_flaky_jobs` respects `include_suspect` and `limit`. `limit=0` → error.
- [ ] `get_job` by id and by (`repo`, `name`) return the same data. An ambiguous name → an error that lists the ids.
- [ ] `get_job` shows an `explained` execution with its Tests.
- [ ] `get_test` includes `jobs`.
- [ ] The HTTP auth tests still pass (401 without the Bearer token).

## Test Expectations
Extend `backend/tests/test_mcp.py` with the criteria above.

## Dependencies
- Blocked by: 06.
- Blocks: 11.

## Estimate / Risk
Small. Risk 1.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && ruff check backend && ruff format --check backend
```
