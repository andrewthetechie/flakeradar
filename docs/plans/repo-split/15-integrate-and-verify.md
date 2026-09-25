# 15 — Integrate and verify

## Tracer-Bullet Outcome
The `feat/repo-split` branch is fully working: backend tests, frontend tests and the strict build all pass. `docker compose up --build` brings up Postgres and the app with 2 workers. The demo simulator's Reports are all processed by exactly one processor. The dashboard, REST API and MCP server all show the same flaky test with its Location and permalink. This is the only task that promises the entire repository passes.

## User Story
As the maintainer, I want a single checkpoint that proves the whole fork works end-to-end before it is merged to `main`.

## Description
Run the full verification below on `feat/repo-split`. If something fails, fix it in the file that owns the behavior (the task files `01`–`14` in this folder describe the intended contract of each area). **Do not add features.** Record every fix in the commit message.

When everything passes, stop and report the results. **Do not merge to `main` and do not open a pull request unless the user explicitly asks.**

## Context Pack
- Source decisions: all of `00-shared-context.md`; ADRs `docs/adr/0001-repo-project-split.md`, `0002-queued-ingest.md` and `0003-postgres-only-async.md`; vocabulary from `CONTEXT.md`.
- Expected totals when every task has landed:
  - backend: **87** pytest tests across `test_db` (8), `test_scoring` (14), `test_parsing` (11), `test_identity` (4), `test_ingest_api` (8), `test_reports_api` (3), `test_processing` (10), `test_worker` (3), `test_retention` (2), `test_github` (6), `test_tests_api` (6), `test_test_detail_api` (6) and `test_mcp` (6);
  - frontend: **25** Vitest tests in 8 files.
- Known facts, established while these tasks were written and verified:
  - The processor election log line is `This process is now the Report processor` (logger `flakeradar.worker`). It shows only because `main.py` pins the `flakeradar` logger to INFO.
  - With `--workers 2`, this line must appear **exactly once** while both workers are healthy.
  - The demo simulator uploads 29 Reports to repo `demo/shop` (projects `backend` and `frontend`). `test_checkout_total_rounding` ends up `flaky` with `confirmed_flake_count == 1`.
- Non-goals: new features; refactors beyond what is needed to pass; performance tuning; merging.

## Delivery Strategy
- Shape: Wide refactor: Integrate and verify.
- Valid-state scope: the whole repository on `feat/repo-split` must pass after this task.

## Implementation Contract
- Expected files: only files that need a fix. There are no planned changes.
- Interfaces and names: none new.
- Verified external contracts: this MCP smoke script works against a live server (the fastmcp 4 HTTP client with a Bearer token, verified during planning). Save it as `/tmp/mcp_smoke.py` (do **not** commit it):
```python
import asyncio
import os

from fastmcp import Client

EXPECTED_TOOLS = ["get_test", "list_projects", "list_repos", "search_tests", "top_flaky_tests"]


async def main() -> None:
    async with Client("http://localhost:8000/mcp/", auth=os.environ["FLAKERADAR_API_TOKEN"]) as c:
        tools = sorted(t.name for t in await c.list_tools())
        assert tools == EXPECTED_TOOLS, tools
        top = (await c.call_tool("top_flaky_tests", {"repo": "demo/shop"})).data
        assert top[0]["name"] == "test_checkout_total_rounding", top
        assert top[0]["confirmed_flake_count"] == 1, top[0]
        detail = (await c.call_tool("get_test", {"test_id": top[0]["id"]})).data
        url = detail["location"]["url"]
        assert url.startswith("https://github.com/demo/shop/blob/"), url
        assert url.endswith("/tests/e2e/test_shop.py#L88"), url
        assert "AssertionError" in detail["latest_failure"]["details"], detail
        print("mcp ok")


asyncio.run(main())
```
- Behavior rules: a failing check must be fixed at its root, not by weakening the check. If a check itself turns out to be wrong (e.g. a count that changed because of a legitimate fix), update it and explain why in the commit message.
- Error and security rules: use a real random token for the compose run (`python -c "import secrets; print(secrets.token_urlsafe(32))"`). Never commit `.env`.

## Acceptance Criteria
- [ ] `cd backend && .venv/bin/python -m pytest -q` → `87 passed`.
- [ ] `cd frontend && npm ci && npm test && npm run build` → 25 tests passed, build ok.
- [ ] Leftover scan prints nothing:
  `grep -rnE "ingest_report|UTCDateTime|fetchProjects|/api/projects|github_repo|sqlite" backend/app backend/migrations frontend/src README.md CONTRIBUTING.md samples`
- [ ] `docker compose up --build -d` → both services are healthy, and `curl -s localhost:8000/api/health` gives `{"status":"ok"}`.
- [ ] `docker compose logs flakeradar | grep -c "This process is now the Report processor"` → `1`.
- [ ] `backend/.venv/bin/python samples/simulate_ci.py http://localhost:8000 "$FLAKERADAR_API_TOKEN"` → `uploaded 29 reports; failed to process: 0`.
- [ ] `curl -s "localhost:8000/api/reports/summary"` → `{"pending":0,"failed":0}`.
- [ ] `FLAKERADAR_API_TOKEN=… backend/.venv/bin/python /tmp/mcp_smoke.py` → `mcp ok`.
- [ ] `curl -s -o /dev/null -w "%{http_code}" -X POST localhost:8000/mcp/` → `401`.
- [ ] Manual UI check at `http://localhost:8000/?repo=demo%2Fshop`:
  - the tiles and leaderboard show only `demo/shop`;
  - the Project picker lists `backend` and `frontend`;
  - clicking `test_checkout_total_rounding` opens the slide-over, with the breadcrumb `demo/shop / backend`, a permalink ending `tests/e2e/test_shop.py:88`, and the traceback;
  - Esc closes it;
  - browser Back restores the previous view;
  - at 390 px wide, test names wrap and stay readable.
- [ ] `docker compose down` (keep the volume), then `docker compose up -d`. The data is still there (`/api/summary` shows the same `total_runs`).

## Test Expectations
- Frameworks: pytest (backend, real Postgres via testcontainers), Vitest (frontend), and a docker compose end-to-end run with the simulator and the MCP smoke script above.
- Behavior under test: the full ingest → queue → single processor → scoring → REST/MCP/UI path, using the concrete demo data from `samples/simulate_ci.py`. The expected literal outcomes are listed in the Acceptance Criteria.

## Dependencies
- Blocked by: 01, 02, 03, 04, 05, 06, 07, 08, 09, 10, 11, 12, 13, 14 (all tasks)
- Why blocked: this is the integration checkpoint for the whole integration branch.
- Blocks: None (next step: the user decides on a PR/merge to `main`)

## Labels
`test`, `chore`, `priority:high`

## Estimate
Small (Medium if fixes are needed)

## Risk
2 - Verification only. Fixes are small and targeted.

## Validator Stopping Point
Every Acceptance Criteria item above is checked. Report the output of each command back to the user, then stop.
