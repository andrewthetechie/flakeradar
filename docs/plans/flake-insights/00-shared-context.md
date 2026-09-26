# 00 — Shared context for the flake-insights plan

**Read this whole file before you start any task in `docs/plans/flake-insights/`.** Every task file assumes you know what is here. Each task file gives the exact contracts for its own step. This file gives the ground rules, the vocabulary, the decisions, and the current code that all the tasks share.

## What we are building

Three features, decided in ADRs that are already on `main`. Read them before you start:

1. **Retry attempts** (`docs/adr/0005-retry-attempts-from-junit.md`). JUnit reports from Playwright 1.59+ (`includeRetries`) and Maven Surefire write each failed retry as a `<flakyFailure>`, `<flakyError>`, `<rerunFailure>` or `<rerunError>` child of `<testcase>`. FlakeRadar ignores them today. We store each retry as its own failing **Execution** in the same Run, with an `attempt` number. A fail and a pass on the same SHA already make a **Proven flake**, so retries give evidence from the first Run.
2. **Failure categories** (`docs/adr/0006-rule-based-failure-categories.md`). Each failing Execution gets a **Failure category** (`network`, `environment`, `timing`, `assertion`, `other`) from ordered regex rules, assigned at ingest. A Test's Failure category is the most common one in its recent failing Executions. The API, UI and MCP can filter by it.
3. **Score history and Clean streak** (`docs/adr/0007-daily-score-history.md`). One row per Test and per Job per UTC day stores the score and Proven flake count at the end of the day, plus that day's Executions and failures. Each Test and Job caches a **Clean streak**. The API computes a **trend** (worsening / improving / steady). The UI shows a sparkline.

**This is a live instance with real data.** Nothing is wiped. Migrations only add columns and tables. There is **no backfill**: old Executions have no category, and Score history starts on the day this ships.

## Delivery rules (read carefully)

- **Branch:** all tasks land in order on `feat/flake-insights`. Create it from `main` if it does not exist yet (`git switch -c feat/flake-insights main`), otherwise `git switch feat/flake-insights`. Commit your task as one or more commits on that branch. Do not merge to `main`. The whole series ships as **one PR**, and task 09 opens it.
- **Order:** tasks run **one after another** in number order (01 … 09). You can assume every task with a lower number has landed. Backend tasks 01 → 03 → 06 → 07 each add one Alembic revision, and the revision chain is why the order is fixed.
- **Keep the suite green.** Every task must leave these passing:
  ```bash
  cd backend && .venv/bin/python -m pytest -q          # needs Docker (testcontainers Postgres 17)
  cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
  ```
  Every task that touches `frontend/` must also leave this passing:
  ```bash
  cd frontend && npm run typecheck && npm run lint && npm run format:check && npm test
  ```
  Use `uvx ruff@0.16.9` (or `pip install ruff==0.16.9`). An older system `ruff` fails on `ruff.toml` with `unknown field 'lint'`.
- **Do not edit** `CONTEXT.md` or `docs/adr/*`. They are already up to date for this feature.
- **Commit messages** end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. If you are a different model, use your own name in the same format.
- **If a contract in a task file is wrong** (for example, a library behaves differently), do the smallest correct thing, keep the public names, and say what you changed in the commit message.
- **Frontend type changes belong to the UI tasks** (02, 05, 08). Backend tasks add JSON fields, and the frontend ignores fields it does not know, so backend tasks do not touch `frontend/`.

## Domain vocabulary (from `CONTEXT.md`: use these words in code, UI text and docs)

| Term | Meaning | Avoid |
|---|---|---|
| **Repo** | A source repository, lowercase `owner/name`. | |
| **Project** | A named test suite inside a Repo. | |
| **Test** | One logical test case in one Repo and Project. DB table `test_cases`, model `TestCase`. | test case (in UI text) |
| **Report** | One upload from CI. Kind `junit` becomes a Run. Kind `pipeline` becomes Job executions. | upload, payload |
| **Run** | The processed result of one JUnit Report. DB table `test_runs`. | build, Job |
| **Execution** | One try of a Test by the test runner, and its outcome, within one Run. A retry inside the same Run is its own Execution, numbered from 0 (`attempt`). A CI re-run is **not** a retry: it produces a new Run. DB table `test_executions`. | result |
| **Job / Job execution / Pipeline** | A CI job, one of its outcomes, and the workflow that groups Jobs (ADR 0004). | |
| **Failure message / Failure details** | The one-line reason / the full traceback and captured output of a failing Execution. | stack trace |
| **Failure category** | The likely cause of a failing Execution: `network`, `environment`, `timing`, `assertion` or `other`. A Test's Failure category is the most common one among its recent failing Executions. UI text says "likely cause". | root cause |
| **Flakiness score / Proven flake / tiers** | Unchanged. flaky ≥ `flake_threshold`, suspect > 0, stable = 0. | same-SHA flip (UI text) |
| **Default branch** | The Repo's main line. `repos.default_branch`; `NULL` means unknown, so every branch counts. | |
| **Score history** | A Test's or Job's Flakiness score and Proven flake count at the end of each UTC day, plus that day's counts of Executions and failures. | |
| **Clean streak** | Non-skipped Executions of a Test or Job on the Default branch since its last failure. | |
| **Trend** | Read-time comparison of the current score with the newest Score history row at least 14 days old: `worsening`, `improving` or `steady`, or `null` when there is no such row. | |

## Decisions (the ledger): treat as requirements

**Retries (tasks 01–02)**
- The parser reads the four retry elements through `case._elem` (the raw `xml.etree.ElementTree.Element`). junitparser 5.0.3 does not model them: for a `<testcase>` that holds only a `<flakyFailure>`, `case.result == []`, so today the test counts as a clean pass. This was verified on 2026-09-26.
- Order of emitted cases for one `<testcase>`: every `flaky*` element in document order, then the `<testcase>`'s own outcome, then every `rerun*` element in document order. Playwright writes `flaky*` for a test that passed on retry, and `rerun*` for a test that failed every attempt.
- A `*Failure` element becomes status `failed`, and a `*Error` element becomes `error`.
- Processing numbers attempts per Test in report order, starting at 0. This also numbers duplicate `<testcase>` entries with the same identity.
- `Report.counts` gains `retried`: the number of Tests in the Report with more than one Execution.
- The GitHub `min_failures` gate counts Executions, so each failed retry counts. Docs must say so. The code does not change.

**Failure categories (tasks 03–05)**
- Rule order: `network`, then `environment`, then `timing`, then `assertion`. No match gives `other`. The Failure message is checked against all categories first, and the Failure details only when the message matches nothing.
- The category is assigned once, at ingest, and stored on `test_executions.failure_category`. It is `NULL` for `passed` and `skipped`.
- `test_cases.failure_category` is cached at rescore. It is the most common non-null category over the Test's newest `score_window` Executions (any branch). The newest wins a tie. It is `NULL` when those Executions have no failure.
- No backfill and no reclassify command.
- Jobs have no Failure category.

**Score history, Clean streak, trend (tasks 06–08)**
- Tables `test_score_history` and `job_score_history`, primary key (owner id, `day`). `day` is the UTC date of processing (`utcnow().date()`, because `utcnow()` is timezone-aware UTC).
- Written in the same transaction as every rescore, as an upsert. `flakiness_score` and `confirmed_flake_count` are **replaced**. `executions` and `failures` are **added**. A rescore with no new Executions (a Default-branch change, or attribution for Jobs) adds 0.
- Test `failures` = Executions with status `failed` or `error`. Job `failures` = Job executions with status `failed`, **raw** (explained or not).
- Clean streak = `scoring.clean_streak` over the Default-branch-filtered statuses, newest first. Skipped rows are ignored. For Jobs, an explained failure is already turned into `skipped` before scoring, so it does not break the streak. The streak can be at most the number of rows the rescore fetched (about `score_window`).
- Trend: `TREND_DAYS = 14`, `TREND_DELTA = 0.05`. `past` = `flakiness_score` of the newest history row with `day <= today - 14 days`. `current - past >= 0.05` gives `worsening`, `<= -0.05` gives `improving`, anything else gives `steady`, and no row gives `null`.
- The Test and Job detail responses embed `score_history` (the last 90 days, oldest first). There is no new endpoint.
- Retention: `FLAKERADAR_SCORE_HISTORY_RETENTION_DAYS`, default `365`. Pruned by the existing hourly prune.

## Current code you will touch (facts, checked 2026-09-26 on `main` @ `6042374`)

**Backend** (`backend/app/`):
- `batching.py`: `CHUNK = 1000` and `def chunks(items: Sequence[T], size: int = CHUNK) -> Iterator[Sequence[T]]`. Use it for every bulk statement and `IN (...)` list.
- `models.py`: `Repo`, `Project`, `TestCase`, `TestRun`, `TestExecution`, `Report`, `Pipeline`, `Job`, `JobExecution`, `utcnow()`, and the constants `REPORT_*`, `JOB_PASSED/JOB_FAILED/JOB_SKIPPED`, `JOB_STATUSES`, `JobStatus`. No ORM relationships: every read is an explicit `select()` with joins.
- `scoring.py`: `FAILING = {"failed", "error"}`, `_is_fail(status)`, `flip_score`, `count_same_sha_flips`, `combined_score`, and `branch_scoped_score(history, default_branch, decay, window) -> tuple[float, int]`, where `history` is `list[tuple[str, str, str]]` = `(commit_sha, branch, status)` newest first.
- `parsing.py`: `ParsedCase`, `fingerprint`, `parse_junit_xml(content: bytes) -> list[ParsedCase]`, `MESSAGE_MAX = 2000`, `DETAILS_MAX = 16384`.
- `processing.py`: `rescore(db, test_case_ids)`, `rescore_jobs(db, job_ids)`, `_job_history(db, job_ids)`, `rescore_repo(db, repo_id)`, `apply_default_branch`, `process_pipeline_report(db, report)`, `process_report(db, report)`, `process_next(session_factory)`. The single processor runs one Report at a time (ADR 0002).
- `attribution.py`: `explained_clause(ci_job_id, repo_id)`, `explained_ci_job_ids`, `unexplained_failure_counts`.
- `queries.py`: read services shared by REST and MCP: `Scope`, `SortKey`, `tier_for`, `issue_url`, `to_test_out`, `tests_select`, `list_tests`, `summary`, `get_test`, `search_tests`, `latest_failure`, `to_job_out`, `jobs_select`, `list_jobs`, `get_job`, `search_jobs`.
- `schemas.py`: Pydantic response models (`TestOut`, `ExecutionOut`, `HistoryOut`, `SummaryOut`, `JobOut`, `JobHistoryOut`, …).
- `routers/tests.py`, `routers/jobs.py`: thin REST routes over `queries`.
- `mcp_server.py`: `build_mcp(session_factory, *, api_token)` with tools `list_repos`, `list_projects`, `top_flaky_tests`, `search_tests`, `get_test`, `top_flaky_jobs`, `search_jobs`, `get_job`, plus an `INSTRUCTIONS` string.
- `retention.py`: `prune(db, *, now, report_days, execution_days) -> PruneResult(reports, executions, runs, job_executions)` and `run_prune(session_factory)`.
- `config.py`: `Settings` (env prefix `FLAKERADAR_`) with `score_window = 50`, `score_decay = 0.85`, `flake_threshold = 0.30`, `report_retention_days = 7`, `execution_retention_days = 90`, `github_issue_min_failures = 0`.
- `backend/migrations/versions/`: `0001_baseline.py` (revision `"0001"`) and `0002_ci_jobs.py` (revision `"0002"`, `down_revision = "0001"`). New revisions: `0003` (task 01), `0004` (task 03), `0005` (task 06), `0006` (task 07).

**Backend tests** (`backend/tests/`):
- `conftest.py`: fixtures `db`, `session_factory`, `client` (httpx against the app, no lifespan), `TOKEN`, `AUTH`, `make_junit(cases, suite=..., classname=...)`, and
  ```python
  _TABLES = "reports, job_executions, jobs, pipelines, test_executions, test_runs, test_cases, projects, repos"
  ```
  This list is truncated before each test. **Add every new table to it.**
- `factories.py`: `make_project`, `make_test_case(db, project, name=..., **fields)`, `make_run`, `make_execution(db, test_case, run, status=..., message=..., details=..., created_at=...)`, `make_report`, `make_pipeline_report`, `make_pipeline`, `make_job(db, pipeline, name=..., **fields)`, `make_job_execution`. Each flushes but does not commit. Call `await db.commit()` before another session (processor, API client) reads the rows.
- Tests use `pytest-asyncio` (auto mode: plain `async def test_...`).

**Frontend** (`frontend/src/`): React 18, Vite, Tailwind 4, Vitest + Testing Library. `api.ts` holds the types and fetchers that mirror `schemas.py`. `urlState.ts` keeps the view in the URL. `App.tsx` wires everything. The components are in `components/`: `Leaderboard`, `LeaderboardControls`, `TestDrawer`, `TestDetail`, `JobLeaderboard`, `JobDrawer`, `StatusMark`, `scoreStyle` and others. Tailwind colour classes include `text-text`, `text-text-2`, `text-muted`, `text-signal`, `text-link`, `text-good`, `text-critical`, `bg-surface`, `bg-surface-2`, `border-line`. CSS variables include `--seq-250 … --seq-650`, `--baseline`, `--signal`. Test fixtures typed as `TestCase`, `Execution`, `Summary`, `Job`, `History` or `JobHistory` live in `App.test.tsx`, `components/Leaderboard.test.tsx`, `components/TestDrawer.test.tsx`, `components/JobDrawer.test.tsx`, `components/JobLeaderboard.test.tsx`, `components/StatTiles.test.tsx` and `components/QueueIndicator.test.tsx`. **When you add a required field to a TS type, add it to every fixture of that type**, or typecheck fails.

## Coding conventions

- Python 3.12, type hints (`X | None`). Module docstrings explain *why*. Short comments.
- Async everywhere. `AsyncSession` with explicit `select()` joins. Postgres-specific SQL is fine (`pg_insert(...).on_conflict_do_update(...)`, `DISTINCT ON`, window functions).
- Read services go in `queries.py` and return `schemas.*`. REST routers and MCP tools stay thin.
- Logging goes to `flakeradar.*` loggers. Never log tokens or raw Report bodies.
- Frontend: function components, Prettier formatting, no new npm dependencies (the sparkline is hand-written SVG).

## Task index

| # | File | Title | Blocked by |
|---|---|---|---|
| 01 | `01-retry-attempts-backend.md` | Retry attempts become Executions | — |
| 02 | `02-retry-attempts-ui-and-docs.md` | Retry attempts in the drawer, README and simulator | 01 |
| 03 | `03-failure-categories-at-ingest.md` | Classify failures at ingest; cache a Test's category | 01 |
| 04 | `04-category-filter-and-counts.md` | Filter and count by Failure category (REST + MCP) | 03 |
| 05 | `05-categories-ui.md` | Likely-cause badge, filter and drawer column | 04 |
| 06 | `06-test-score-history.md` | Test Score history, Clean streak, trend (backend + MCP) | 03 |
| 07 | `07-job-score-history.md` | Job Score history, Clean streak, trend | 06 |
| 08 | `08-trend-ui.md` | Sparkline, trend marks and Clean streak in the UI | 06, 07 |
| 09 | `09-integrate-and-verify.md` | README, end-to-end check, open the PR | all |
