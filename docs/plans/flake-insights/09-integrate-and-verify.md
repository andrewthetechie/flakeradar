# 09 — README, end-to-end check, open the PR

## Tracer-Bullet Outcome
`feat/flake-insights` passes every check. The README documents Failure categories, Score history and trends, and the new API/MCP fields. A fresh Docker stack upgraded from `main` data, with the simulator run against it, shows all three features working: a retry-based Proven flake, likely-cause filtering, and a trend and sparkline. One PR is open against `main`.

## User Story
As the maintainer, I want one reviewable PR with evidence that the migrations and the whole flow work on real-shaped data.

## Description
Documentation plus verification, and then the PR. Code changes here are only fixes for problems the checks find.

## Context Pack
- Source decisions: `00-shared-context.md` (all). ADRs 0005–0007.
- Repo facts:
  - Local stack: `cp .env.example .env`, set `FLAKERADAR_API_TOKEN` (or `FLAKERADAR_ALLOW_INSECURE=1` for a throwaway run), then `docker compose up --build`. The app, API and MCP are on `http://localhost:8000`. Startup runs Alembic to `head` (`backend/app/migrate.py`).
  - Simulator: `python samples/simulate_ci.py http://localhost:8000 <token>` (repo `demo/shop`, Projects `backend` and `frontend`, Pipeline `.github/workflows/ci.yml`). After task 02 it includes `Checkout > shows the total`, which needs a retry every 4th run.
  - CI (`.github/workflows/ci.yml`) runs ruff 0.16.9, pytest, and the frontend typecheck, lint, format check and tests.
  - README anchors to update or extend: `## What it does` (bullet list), `## Retries` (added in task 02), `## MCP server for agents` → `### Tools` (table), `## How scoring works`, `## Configuration` (env table), `## API` (endpoint table), and the `## Contents` list near the top (add links for new top-level sections).
  - The README `### Tools` table today has the rows `list_repos`, `list_projects(repo)`, `top_flaky_tests(repo, project?, limit=20, include_suspect=true, file?)`, `search_tests(...)`, `get_test(...)` and the Job tools.
  - The README `## API` table has the rows `GET /api/tests?repo=&project=&include_stable=&sort=&page=&page_size=&file=`, `GET /api/tests/{id}/history?limit=`, `GET /api/jobs/{id}/history?limit=` and `GET /api/summary?repo=&project=`.
- Non-goals: new features; editing `CONTEXT.md` or ADRs. If the build had to differ from an ADR, **stop and ask the maintainer**.

## Delivery Strategy
- Shape: Normal tracer bullet (final verification; every earlier task is already green on its own)
- Valid-state scope: `feat/flake-insights`, and the PR's CI

## Implementation Contract
- Expected files: `README.md`. Also any file that needs a fix found by the checks (each fix in its own commit that names the task it belongs to).
- Interfaces and names: None new.
- Verified external contracts: `gh pr create --base main --head feat/flake-insights --title … --body-file …` (GitHub CLI).
- Behavior rules (README content):
  - `## What it does`: add two bullets:
    - `**Likely cause for every failure** — failures are sorted into network, environment, timing, assertion or other by fixed rules over the failure message, so you can fix one kind of flake at a time.`
    - `**Trends and fix verification** — a daily score history per test and job, a worsening/improving trend on the leaderboard, and a clean streak that shows whether a fix held.`
  - New section `## Failure categories` after `## Retries`: the five categories, the rule order (network → environment → timing → assertion → other), the fact that the message is checked before the details, two examples (`Timed out 5000ms waiting for expect(...)` → timing; `connect ECONNREFUSED` → network), a Test's category = the most common in its last `SCORE_WINDOW` executions, "likely, not a diagnosis", and the note that failures stored before this release have no category.
  - New section `## Score history and trends` after `## How scoring works`: a daily row per Test and Job (end-of-day score and Proven flake count, plus that day's executions and failures), the trend rule (±0.05 vs the newest row ≥ 14 days old), the Clean streak (non-skipped Default-branch executions since the last failure; explained Job failures do not break it), the 90 days shown in the drawer, and the 365-day retention.
  - `## Configuration` table: add the row `| `SCORE_HISTORY_RETENTION_DAYS` | `365` | daily score history rows older than this are deleted |`.
  - `## API` table: the tests row becomes `GET /api/tests?repo=&project=&include_stable=&sort=&page=&page_size=&file=&category=`, with the description gaining "; `category` filters by likely cause". The history rows' descriptions gain "; `score_history` (90 days), `clean_streak`, `trend`". The summary row gains "; `category_counts`". Add one sentence under the table: Test executions carry `attempt` (0 = first try in the Run) and `failure_category`.
  - `### Tools` table: `top_flaky_tests(repo, project?, limit=20, include_suspect=true, file?, category?)`. The `get_test` and `get_job` rows gain "score history (30 days)".
- Error and security rules: None.

## Steps
1. Run every check in the Validator and fix anything that fails.
2. **Migration on real-shaped data:**
   1. `git switch main`, `docker compose up --build -d`, then run the simulator so `0002` data exists (Tests, Runs, Jobs).
   2. `git switch feat/flake-insights`, `docker compose up --build -d`. Check the logs (`docker compose logs flakeradar`): startup migrates `0002 → 0006` without errors.
   3. `curl -s localhost:8000/api/tests?repo=demo/shop | python -m json.tool`: old Tests have `failure_category: null`, `clean_streak: 0`, `trend: null`, and unchanged scores.
3. **End-to-end checks** (run the new simulator again against the same stack):
   - `Checkout > shows the total` (Project `frontend`) has `confirmed_flake_count >= 1`, and its history has Executions with `attempt: 1`. Report `counts` include `retried`.
   - `GET /api/tests?repo=demo/shop&category=timing` includes `Checkout > shows the total`. `GET /api/summary?repo=demo/shop` has `category_counts`.
   - `GET /api/tests/{id}/history` for a Test touched today has one `score_history` point for today, and `executions` equal to the Executions added today.
   - UI (desktop and ~390 px wide): the likely-cause badge and filter (`?cause=timing` in the URL), `retry N` in the drawer, the Clean streak and sparkline in the Test and Job drawers, and trend marks (they appear only after 14 days of history; confirm they are absent now and do not break the layout).
   - MCP: `top_flaky_tests(repo="demo/shop", category="timing")` and `get_test(...)` show `failure_category`, `attempt`, `score_history` and `clean_streak`. Use `claude mcp add --transport http flakeradar http://localhost:8000/mcp/ --header "Authorization: Bearer <token>"`, or a fastmcp `Client`.
4. **Downgrade sanity** (on a throwaway DB only): `cd backend && FLAKERADAR_DATABASE_URL=… .venv/bin/alembic downgrade 0002`, then `upgrade head`. Both succeed. Note in the PR that a downgrade drops the attempt numbers, categories and score history.
5. **Open the PR:** `gh pr create --base main --head feat/flake-insights --title "Retry attempts, failure categories and score history (ADR 0005–0007)" --body-file <file>`. The body has:
   - a summary of the three features;
   - the task list (01–08) with commit ranges;
   - migration notes (`0003`–`0006` are additive, there is no backfill, and old data shows `null` categories and no history);
   - the manual checks from steps 2–4 and what you saw;
   - known limits: category rules are heuristic and not configurable; trends need 14 days of history; `min_failures` now counts failed retries;
   - and it ends with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.

## Acceptance Criteria
- [ ] Every command in the Validator passes locally.
- [ ] The migration from real `0002` data works as described in step 2.
- [ ] The end-to-end checks in step 3 are done, and the results are in the PR body.
- [ ] The README has the new sections and table rows listed above.
- [ ] The PR is open against `main`, and its CI is green.

## Test Expectations
- No new automated tests unless a check finds a bug. A bug fix comes with a regression test in the test file of the task that owns the code (for example `backend/tests/test_processing.py` for processing).

## Dependencies
- Blocked by: 01, 02, 03, 04, 05, 06, 07, 08.
- Why blocked: this task verifies and documents the whole series.
- Blocks: None.

## Labels
`chore`, `docs`, `priority:high`

## Estimate
Medium

## Risk
2 - Mostly verification. The migration check protects live data.

## Validator Stopping Point
```bash
uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
cd backend && .venv/bin/python -m pytest -q
cd ../frontend && npm ci && npm run typecheck && npm run lint && npm run format:check && npm test && npm run build
```
