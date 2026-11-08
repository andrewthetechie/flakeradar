# 09 — Read APIs

Read `00-shared-context.md` first. Tasks 01–08 have landed.

Python reference: `backend/app/queries.py`, `routers/tests.py`, `routers/jobs.py`, `routers/ci_runs.py`, `score_history.py`. README's API table.

## Goal

The dashboard, CI summary action and Fabro can read everything. REST and MCP share these query functions, so `flakeradar-db` holds them and they return the DTOs from `flakeradar-core`.

## Contracts

### Routes

Keep query parameters, defaults, limits, statuses and error text exactly as the routers have them. Read each router's signature, since these are the contract:

- `GET /api/repos`, `/api/tests`, `/api/summary`, `/api/tests/{id}/history`, `POST /api/tests/{id}/quarantine`
- `GET /api/quarantine` (token required)
- `GET /api/jobs`, `/api/jobs/summary`, `/api/jobs/{id}/history`
- `GET /api/ci-runs/{ci_run_id}/failures`

Quarantine has no token, by design. `PUT` and `DELETE .../issue` belong to task 10.

### Leaderboard

Port the descriptor design of `_list_leaderboard`: one engine, with a `LeaderboardEntity` per Test and Job, so a new filter or sort key extends a descriptor. Keep these rules together in the engine:

- `flaky_only` or `new_within_days` adds `score >= threshold`.
- Otherwise, `score > 0` unless `include_stable`, or there is a search term, or the descriptor keeps stable rows (Tests only: when quarantined or broken is requested).
- `q` search, `has_issue` and `new_within_days` (the lateral `newest_history_before` with `_became_flaky_clause`).
- `total` counts the filtered query before paging.
- Sort orders exactly as in `order`: `score` is `score DESC, last_seen DESC, id`, `last_seen` is `last_seen DESC, id DESC`, `proven` is `proven DESC, score DESC, id`.
- `escape_like` with `ESCAPE '\'` for every `ILIKE`.
- `broken` with `broken_streak = 0` matches nothing.
- `trend` comes from `past_scores` on the page's ids.

Keep the shapes of the performance-tuned queries from issues #20 and #24, and their indexes. Do not replace a lateral or a bounded query with a simpler one that scans.

### Details

Port `get_test`, `get_job`, `summary`, `job_summary`, `_runner_stats`, `_test_job_links`, `ci_run_failures` and its helpers, `set_quarantine`, `quarantine_list`, `search_tests`, `find_test_ids`, `latest_failure`, `search_jobs`, `find_job_ids`, including:

- 404 text
- the `truncated` flag
- `reports.pending`
- the choice of latest attempt when none is given
- `CiRunNotFound`

`issue_url`, `job_issue_url` (GitHub Pipelines only), `permalink` and the `explained_by` fields keep their logic.

### Quarantine

Body `{quarantined, reason?, expires_in_days?}`, with `reason` up to 500 characters and days 1–365. Unquarantining clears the reason and expiry. Clear stale values exactly as `set_quarantine` does.

Every route gets a `#[utoipa::path]`.

## Tests to port

`test_tests_api` (25), `test_test_detail_api` (8), `test_jobs_api` (26), `test_ci_runs_api` (30).

Add tests for:

- every filter and sort key combination the engine branches on
- ILIKE escaping of `%`, `_` and `\`
- a page past the end
- the Fabro fields listed in `00-shared-context.md`: deserialize responses into a copy of Fabro's structs and check `normalise_ts`'s rule on every timestamp

Enable all remaining REST scenarios in `parity.rs`, apart from Issue links and repo delete.

## Done when

- [ ] All verification in `00-shared-context.md` passes.
- [ ] Enabled parity scenarios pass.
- [ ] `frontend` works against the Rust server: run `npm run dev` with its proxy on port 8000, load the dashboard with data from `samples/simulate_ci.py`, and check the Tests and Jobs views, the drawers, filters and quarantine with the Playwright MCP tools.
- [ ] Manifest rows for this task are not `pending`, and gaps are logged.
- [ ] `STATUS.md` is updated, and the branch is pushed.
