# 06 — README, simulator, end-to-end check, open the PR

## Tracer-Bullet Outcome
The README documents `GET /api/ci-runs/{ci_run_id}/failures` and the `ci_run_failures` MCP tool. The simulator gives a CI run with two attempts. A fresh Docker stack shows the endpoint working on simulated data. One PR is open against `main` and closes issue #5.

## User Story
As the maintainer, I want one reviewable PR with docs and evidence that the endpoint answers the merge gate's question on real-shaped data.

## Description
Documentation, one small simulator change, verification, then the PR. Other code changes here are only fixes for problems the checks find.

## Context Pack
- Source decisions: `00-shared-context.md` (all). Issue #5 "Done when": the endpoint returns the shape for a CI run with failures, empty `failures` with `reports.pending > 0` while a Report is queued, and `404` for an unknown CI run. Tests cover the attempt default and a CI run with two attempts. The README's API section lists the endpoint.
- Repo facts:
  - README `## Contents` lists sections in order. `- [CI jobs](#ci-jobs)` is followed by `- [Repos and projects](#repos-and-projects)`.
  - README `## CI jobs` ends with its subsection `### Other CI providers` (with a JSON example). Then `## Repos and projects` starts.
  - README `### Tools` table (under `## MCP server for agents`): the last row is `get_job(job_id | repo+name[+pipeline], executions_limit=20)`.
  - README `## API` table: the row `GET /api/jobs/{id}/history?limit=` is followed by `GET /api/summary?repo=&project=`.
  - `samples/simulate_ci.py` (repo `demo/shop`, `random.Random(42)`): for `i == 6` it posts an attempt-2 JUnit Report and Pipeline report with `ci_run_id "run-6-retry"`. Those are lines with `post_junit(client, "backend", "", sha, "run-6-retry", f"b-6-2", 2, ...)` and `post_pipeline(client, sha, "run-6-retry", 2, [...])`. A real re-run keeps its CI run ID.
  - Local stack: `cp .env.example .env`, set `FLAKERADAR_API_TOKEN` (or `FLAKERADAR_ALLOW_INSECURE=1` for a throwaway run), then `docker compose up --build`. App, API and MCP on `http://localhost:8000`. Simulator: `python samples/simulate_ci.py http://localhost:8000 <token>`. It waits until no Report is pending.
- Non-goals: new features; editing `CONTEXT.md` or ADRs. If the build had to differ from `00-shared-context.md`, list the difference in the PR body.

## Delivery Strategy
- Shape: Normal tracer bullet (final verification; every earlier task is already green on its own)
- Valid-state scope: `feat/ci-run-failures`, and the PR's CI

## Implementation Contract
- Expected files: `README.md`, `samples/simulate_ci.py`. Also any file that needs a fix found by the checks (each fix in its own commit that names the task it belongs to).
- Interfaces and names: None new.
- Verified external contracts: `gh pr create --base main --head feat/ci-run-failures --title … --body-file …` (GitHub CLI).
- Behavior rules:
  - `samples/simulate_ci.py`: replace `"run-6-retry"` with `"run-6"` in both calls, so run 6 is one CI run with attempts 1 and 2. Change nothing else.
  - README `## Contents`: add `- [Failures of one CI run](#failures-of-one-ci-run)` after `- [CI jobs](#ci-jobs)`.
  - README: new section after `## CI jobs` (after its `### Other CI providers` subsection) and before `## Repos and projects`, with this text:
    ````markdown
    ## Failures of one CI run

    A CI gate or bot that sees a red CI run can ask which tests failed in that
    run, and whether FlakeRadar already knows them as flaky:

    ```bash
    curl -s "https://flakeradar.example.com/api/ci-runs/$GITHUB_RUN_ID/failures?repo=owner/name&ci_run_attempt=$GITHUB_RUN_ATTEMPT"
    ```

    The response is for one attempt of the CI run. Without `ci_run_attempt`, it
    is the latest attempt that FlakeRadar has seen. Pass the attempt when you
    know it.

    - `reports` counts the run attempt's reports (JUnit and pipeline) as
      `pending`, `processed` and `failed`. Ingest answers before it processes a
      report. Poll until `pending` is 0 before you trust an empty list.
      `failed` above 0 means some results are missing.
    - `failures` has one entry for each test that failed or errored in a run of
      that attempt. Each entry has the test's current `tier`, `flakiness_score`,
      `confirmed_flake_count`, `clean_streak` and `quarantined`, so you can
      apply your own threshold. `status` is the test's last try in that run. A
      test that passed on retry has `status: "passed"` and `failed_attempts`
      above 0. `message` is the first 1 KB of the failure message. The list
      holds at most 500 entries. Then `truncated` is `true`, and
      `total_failures` gives the full count.
    - `failed_jobs` lists the failed jobs of that attempt. `outcome` is
      `explained` when a failing test in the same job explains the failure, and
      `unexplained` for a failure in setup or infrastructure. A run can be red
      with an empty `failures` list and an `unexplained` job.
    - An unknown repo, CI run or attempt returns `404`. Processed reports are
      deleted after `REPORT_RETENTION_DAYS`. After that, `reports` shows 0 but
      the failures stay until `EXECUTION_RETENTION_DAYS`.
    ````
    (`REPORT_RETENTION_DAYS` and `EXECUTION_RETENTION_DAYS` are the names in the README `## Configuration` table, which leaves out the `FLAKERADAR_` prefix.)
  - README `### Tools` table: add after the `get_job` row:
    `| `ci_run_failures(repo, ci_run_id, ci_run_attempt?)` | The failing tests of one CI run attempt (the latest by default) with their current tier and score, its failed jobs (explained or unexplained), and its report counts. |`
  - README `## API` table: add after the `GET /api/jobs/{id}/history?limit=` row:
    `| `GET /api/ci-runs/{ci_run_id}/failures?repo=&ci_run_attempt=` | none | The failing tests and failed jobs of one CI run attempt, with each one's current tier and the attempt's report counts. See [Failures of one CI run](#failures-of-one-ci-run). |`
- Error and security rules: None.

## Steps
1. Make the README and simulator changes.
2. Run every check in the Validator and fix anything that fails.
3. **Migration on existing data:**
   1. `git switch main`, `docker compose up --build -d`, then run the simulator so `0007` data exists.
   2. `git switch feat/ci-run-failures`, `docker compose up --build -d`. Check `docker compose logs flakeradar`: startup migrates `0007 → 0008` without errors.
4. **End-to-end checks** (run the simulator again against the same stack, then):
   - `curl -s "localhost:8000/api/ci-runs/run-6/failures?repo=demo/shop" | python -m json.tool` → `ci_run_attempt: 2`, `reports.pending: 0`, `failures` includes `test_schema_migration_v42` (it always fails) with a `tier`, and `failed_jobs` is consistent with the Pipeline report for attempt 2.
   - The same with `&ci_run_attempt=1` → `failures` includes `test_checkout_total_rounding` and `test_schema_migration_v42`, and `failed_jobs` includes `backend` with `outcome: "explained"`.
   - `curl -si "localhost:8000/api/ci-runs/nope/failures?repo=demo/shop"` → `404` with `"CI run not found"`.
   - MCP: `ci_run_failures(repo="demo/shop", ci_run_id="run-6")` returns the same JSON. Use `claude mcp add --transport http flakeradar http://localhost:8000/mcp/ --header "Authorization: Bearer <token>"` or a fastmcp `Client`.
5. **Downgrade sanity** (on a throwaway DB only): `cd backend && FLAKERADAR_DATABASE_URL=… .venv/bin/alembic downgrade 0007`, then `upgrade head`. Both succeed.
6. **Open the PR:** `gh pr create --base main --head feat/ci-run-failures --title "API and MCP: failures of one CI run attempt" --body-file <file>`. The body has:
   - a summary: the endpoint, the MCP tool, the response shape (paste the example from `00-shared-context.md`);
   - the naming change from the issue (`/api/ci-runs` and `ci_run_attempt` in place of `/api/runs` and `attempt`) and why (**Run** and an Execution's `attempt` already mean other things; see `CONTEXT.md`);
   - the additions beyond the issue: `failed_jobs`, retry handling (`status` / `failed_attempts`), `run_id` / `ci_job_id`, the 500 cap with `truncated`, and `flake_threshold`;
   - migration notes (`0008` adds three indexes, nothing else);
   - the task list (01–05) with commit ranges;
   - the manual checks from steps 3–5 and what you saw;
   - `Closes #5`;
   - and it ends with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.

## Acceptance Criteria
- [ ] Every command in the Validator passes locally.
- [ ] The migration and end-to-end checks in steps 3–5 are done, and the results are in the PR body.
- [ ] The README has the new section, the Contents link and both table rows.
- [ ] The PR is open against `main`, its body contains `Closes #5`, and its CI is green.

## Test Expectations
- No new automated tests unless a check finds a bug. A bug fix comes with a regression test in `backend/tests/test_ci_runs_api.py` (REST) or `backend/tests/test_mcp.py` (MCP).

## Validator
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```

## Dependencies
- Blocked by: 01, 02, 03, 04, 05.
- Blocks: None.

## Labels
`chore`, `docs`, `priority:high`
