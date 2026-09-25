# 00 — Shared context for the repo-split plan

**Read this whole file before you start any task in `docs/plans/repo-split/`.** Every task file assumes you know what is here. The task file gives the exact contracts for its own step. This file gives the ground rules, the vocabulary, the final schema and the verified facts about the libraries.

## What we are building

FlakeRadar is a self-hosted flaky-test detector. CI uploads JUnit XML. FlakeRadar tracks each test's pass/fail history by commit SHA and scores how often each test flips between pass and fail. This repo is a fork, and we are changing it in these ways:

1. **Repo and Project are two separate things.** A Repo (`owner/name`) has one or more Projects (`frontend`, `backend`, `e2e`). The same Project name can exist in many Repos. See `docs/adr/0001-repo-project-split.md`.
2. **Ingest is queued.** CI gets `202` as soon as the raw Report is stored. A single background processor turns Reports into Runs. See `docs/adr/0002-queued-ingest.md`.
3. **Postgres only, fully async.** SQLite is gone. Tests run against a real Postgres. See `docs/adr/0003-postgres-only-async.md`.
4. **Evidence for agents.** Each Test stores the file and line where it is defined. Each failing Execution stores its full Failure details. Tests get GitHub permalinks.
5. **An MCP server** (fastmcp) at `/mcp` for agents. It is read-only.
6. **A UI that scales:** Repo and Project pickers, state kept in the URL, pagination, a slide-over detail panel, and a queue indicator.

The existing data is being **wiped**. You do not need to migrate data, and you do not need backward compatibility with the old API.

## Delivery rules (read carefully)

- **Branch:** all tasks land in order on the integration branch `feat/repo-split`. Create it from `main` if it does not exist yet (`git switch -c feat/repo-split`), otherwise `git switch feat/repo-split`. Commit your task as one or more commits on that branch. Do not merge to `main`.
- **Order:** tasks run **one after another** in number order (01, 02, …, 15). You can assume every task with a lower number has landed.
- **Broken states are allowed.** The move from sync SQLite to async Postgres breaks the whole app at once. After a task lands, **other parts of the app may be broken** (for example, the frontend build fails because the API shape changed). That is expected. A later task fixes it. Only task 15 ("Integrate and verify") promises that everything passes.
- **Your validator is scoped.** Each task has a "Validator Stopping Point" that names the exact commands that must pass for *that* task. Run those commands. Do not try to fix breakage that belongs to a later task, but do not add new breakage outside your scope either.
- **Do not edit** `CONTEXT.md` or `docs/adr/*` unless your task says to.
- **Commit messages** end with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. (This is the user's attribution convention for this work. If you are a different model, use your own name in the same format.)

## Domain vocabulary (from `CONTEXT.md`, use these words in code, UI text and docs)

| Term | Meaning | Avoid |
|---|---|---|
| **Repo** | A source repository, identified by lowercase `owner/name` (e.g. `andrewthetechie/writers-app`). Unique. | "project" for a repository |
| **Project** | A named test suite inside a Repo (`frontend`, `backend`, `e2e`). Unique within its Repo. A single-suite Repo uses the project `default`. | suite, component |
| **Project root** | The directory of a Project inside its Repo (e.g. `frontend`). File paths that the Project reports are relative to it. | |
| **Test** | One logical test case, identified by suite, classname and name within one Repo and Project. (The DB table is `test_cases` and the ORM class is `TestCase`; user-facing text says "test".) | spec |
| **Location** | The file (relative to the Project root) and line where a Test is defined, as the latest report gave them. Not part of the Test's identity. | |
| **Report** | One JUnit XML file that CI uploads. It is `pending` until processed, then `processed` (it became a Run) or `failed` (kept with its error). | upload, payload |
| **Run** | The processed result of one Report: the Executions for one Repo and Project at one commit SHA. | build, job |
| **Execution** | One Test's outcome (`passed`, `failed`, `error`, `skipped`) in one Run. | result, attempt |
| **Failure message** | The short, one-line reason for a failing Execution. | |
| **Failure details** | The full output for a failing Execution: the traceback plus captured stdout/stderr. | stack trace |
| **Flakiness score** | A number from 0 to 1 that measures how often pass/fail flips, with recent flips weighted more. A test that always fails scores 0. | |
| **Proven flake** | A commit SHA on which the same Test both passed and failed. It puts a floor under the score. The DB column is `confirmed_flake_count`; the UI says "proven flakes". | confirmed flake, same-SHA flip (in UI text) |
| **Flake threshold** | `FLAKERADAR_FLAKE_THRESHOLD` (default `0.30`). | |
| **Flaky test** | score ≥ threshold. | |
| **Suspect test** | 0 < score < threshold. | |
| **Stable test** | score = 0. | |
| **Tier** | One of `flaky`, `suspect`, `stable`. Computed from the score, never stored. | |
| **Quarantine** | A reversible human decision that the test runner may skip a Test. FlakeRadar never quarantines on its own. The MCP server cannot quarantine. | |

## Decisions (the ledger) — treat as requirements

**Identity and naming**
- The `repo` query parameter is **required** on ingest. It is trimmed and lowercased, and must then match `^[a-z0-9_.-]+/[a-z0-9_.-]+$` (at most 255 chars). If it is missing or invalid, the response is **422**.
- `project` is trimmed and lowercased, must match `^[a-z0-9_.-]{1,100}$`, and defaults to `default`. The same rule applies everywhere `project` is accepted.
- `root` (the Project root) is optional on ingest. It is normalized: trimmed, leading `./` and leading/trailing `/` removed. A value containing a `..` path segment returns 422. The latest Report that sends `root` sets it on the Project. A Report without `root` leaves it unchanged.
- Repos and Projects are **auto-created** on first ingest.
- On read endpoints, `project` is valid only together with `repo`. `project` without `repo` → 422.

**Ingest and the queue**
- `POST /api/ingest` (token required) validates, stores a `pending` Report, and returns `202 {"report_id": <int>, "status": "pending"}`. An empty body → 400, a body over 20 MB → 413, and XML that doesn't parse → 422.
- One processor at a time, chosen by a Postgres advisory lock, processes pending Reports oldest-first (`FOR UPDATE SKIP LOCKED`). Every uvicorn worker runs a standby processor task. The Dockerfile runs `--workers 2`.
- A Report that fails processing becomes `failed` with its `error` text and can be retried.
- Report read endpoints need **no token**: `GET /api/reports/summary`, `GET /api/reports`, `GET /api/reports/{id}`. Only `POST /api/reports/{id}/retry` needs the token.

**Evidence**
- Location: the latest report wins, but a report with no `file` attribute does **not** erase a known Location.
- Size caps: Failure message 2,000 chars; Failure details 16,384 chars; MCP `get_test` returns at most 4,096 chars of details.
- Permalink: `https://github.com/{repo}/blob/{last_failing_sha}/{path}#L{line}`, where `path` is `{root}/{file}` (or just `{file}` when the root is empty). It is built only when the file is known and at least one failing Execution (`failed` or `error`) exists. `#L{line}` is added only when `line >= 1`.
- **Known runner quirk (verified):** pytest's `junit_family=xunit1` reports **0-based** lines (a test on line 1 reports `line="0"`). Store the line exactly as reported. Do not guess per-runner offsets.
- No guessing file paths from classnames.

**Retention**
- Processed Reports are deleted 7 days after `processed_at` (`FLAKERADAR_REPORT_RETENTION_DAYS=7`). Failed Reports are kept until someone deletes them.
- Executions are deleted 90 days after `created_at` (`FLAKERADAR_EXECUTION_RETENTION_DAYS=90`). Runs left with no Executions are deleted with them.
- Pruning runs hourly on the processor that holds the lock.

**GitHub issues**
- Issues are filed in **the Test's own Repo**, using the global `FLAKERADAR_GITHUB_TOKEN`. `FLAKERADAR_GITHUB_REPO` is removed.

**MCP**
- fastmcp, mounted at `/mcp`. Clients authenticate with `Authorization: Bearer <FLAKERADAR_API_TOKEN>`; REST keeps `X-API-Key`. Read-only tools: `list_repos`, `list_projects`, `top_flaky_tests`, `search_tests`, `get_test`.

**Auth**
- Unchanged philosophy: the dashboard is internal only. Read endpoints and the quarantine toggle need no token. CI-facing endpoints (`/api/ingest`, `/api/quarantine`, `/api/reports/{id}/retry`) and the MCP server need the token.

## Target backend layout (after all tasks)

```
backend/
  alembic.ini
  migrations/env.py                  async-aware env (task 01)
  migrations/versions/0001_baseline.py   squashed fresh baseline (task 01)
  app/
    config.py        Settings (pydantic-settings, env prefix FLAKERADAR_)
    db.py            async engine, SessionLocal, get_db, lock keys (task 01)
    models.py        ORM models, final schema (task 01)
    migrate.py       run_migrations(engine) (task 01)
    auth.py          require_token dependency (task 01)
    scoring.py       pure scoring functions (unchanged)
    parsing.py       pure JUnit parsing (task 02)
    identity.py      repo/project/root normalization + get_or_create_project (task 03)
    processing.py    claim + process one Report (task 04)
    worker.py        ReportWorker: leader lock, loop, hooks (task 05, 06, 07)
    retention.py     prune() (task 06)
    github_integration.py  async, per-repo (task 07)
    queries.py       read services shared by REST and MCP (task 08, 09, 10)
    schemas.py       Pydantic DTOs
    routers/__init__.py
    routers/reports.py     ingest + report endpoints (task 03)
    routers/tests.py       repos/tests/summary/history/quarantine (task 08, 09)
    mcp_server.py    build_mcp() (task 10)
    main.py          FastAPI app, lifespan, routers, /mcp mount, static files
  tests/
    conftest.py      Postgres container, engine, db, client fixtures, make_junit (task 01)
    factories.py     async seed helpers (task 01)
    test_*.py
```

`app/ingest.py` is deleted in task 01. Its parsing moves to `parsing.py` (task 02), and its persistence moves to `processing.py` (task 04).

## Final database schema (created in task 01 — every later task relies on it)

All timestamps are `timestamptz` (`DateTime(timezone=True)`), stored and returned in UTC. There are no ORM relationships: code uses explicit `select()` joins, because lazy loading does not work in async SQLAlchemy.

| Table | Columns | Constraints / indexes |
|---|---|---|
| `repos` | `id` int PK, `name` varchar(255) not null, `created_at` | unique(`name`) |
| `projects` | `id` int PK, `repo_id` → repos.id ON DELETE CASCADE, `name` varchar(100), `root` varchar(1024) not null default `''`, `created_at` | unique(`repo_id`,`name`) named `uq_projects_repo_name`; index `repo_id` |
| `test_cases` | `id` int PK, `project_id` → projects.id CASCADE, `fingerprint` varchar(40), `suite` text default '', `classname` text default '', `name` text, `file` text NULL, `line` int NULL, `flakiness_score` float default 0, `confirmed_flake_count` int default 0, `last_status` varchar(16) default 'passed', `last_seen_at`, `quarantined` bool default false, `quarantined_at` NULL, `github_issue_number` int NULL | unique(`project_id`,`fingerprint`) named `uq_test_cases_project_fingerprint`; index `project_id`; index (`project_id`,`flakiness_score`) |
| `test_runs` | `id` int PK, `project_id` → projects.id CASCADE, `commit_sha` varchar(64), `branch` varchar(255), `ci_run_id` varchar(255) default '', `created_at` | index `project_id`; index `commit_sha` |
| `test_executions` | `id` bigint PK, `test_case_id` → test_cases.id CASCADE, `test_run_id` → test_runs.id CASCADE, `status` varchar(16), `duration` float default 0, `message` text default '', `details` text default '', `created_at` | index (`test_case_id`,`id`) named `ix_exec_case_id`; index `test_run_id`; index `created_at` |
| `reports` | `id` int PK, `project_id` → projects.id CASCADE, `commit_sha` varchar(64), `branch` varchar(255), `ci_run_id` varchar(255) default '', `root` varchar(1024) NULL, `body` bytea, `status` varchar(16) default 'pending', `error` text NULL, `counts` jsonb NULL, `run_id` → test_runs.id ON DELETE SET NULL NULL, `created_at`, `processed_at` NULL | index (`status`,`id`) named `ix_reports_status_id`; index `project_id` |

The report status constants are defined in `app/models.py`: `REPORT_PENDING = "pending"`, `REPORT_PROCESSED = "processed"`, `REPORT_FAILED = "failed"`.

## Coding conventions

- Python 3.12. Type hints everywhere (`X | None`, not `Optional`). Match the existing style: module docstrings that explain *why*, short comments, no comment noise.
- **Async everywhere in the backend.** Route handlers are `async def`. DB access uses `AsyncSession` and `await session.execute(select(...))`. Never touch a relationship attribute (there are none). Never call blocking code in an `async def`. CPU-heavy work such as XML parsing goes through `await asyncio.to_thread(fn, ...)`.
- Postgres-specific SQL is fine and expected: `from sqlalchemy.dialects.postgresql import insert as pg_insert` for `ON CONFLICT`, `FOR UPDATE SKIP LOCKED`, advisory locks, `JSONB`.
- **asyncpg parameter limit:** one statement can bind at most 32,767 parameters. Chunk bulk inserts and `IN (...)` lists at **1,000 rows** per statement.
- Settings come from `get_settings()` (cached). Tests change settings through `monkeypatch` or by passing values as arguments. Do not mutate the cached object.
- Logging: `logging.getLogger("flakeradar")` or a child logger (`flakeradar.worker`, `flakeradar.github`). Never log the API token, the GitHub token, or a raw Report body.

## Test harness (created in task 01)

- **Backend runner:** pytest 8+ with **pytest-asyncio** (`asyncio_mode = auto`, session-scoped loops) and **httpx `AsyncClient` + `ASGITransport`** for HTTP tests. `ASGITransport` does **not** run the app lifespan, so migrations and the worker do not start in HTTP tests. That is intended.
- **Database:** a `postgres:17-alpine` container, started once per test session by **testcontainers** (Docker must be running). If `FLAKERADAR_TEST_DATABASE_URL` is set (e.g. `postgresql+asyncpg://flakeradar:flakeradar@localhost:5432/flakeradar_test`), that database is used instead. Every test starts with all tables truncated.
- **Fixtures** (in `backend/tests/conftest.py`):
  - `database_url` (session): the URL.
  - `engine` (session): an `AsyncEngine` with migrations applied.
  - `session_factory` (function): an `async_sessionmaker[AsyncSession]` over clean tables.
  - `db` (function): one `AsyncSession`.
  - `client` (function): `httpx.AsyncClient` against `app.main.app`, with `get_db` overridden.
  - Constants `TOKEN = "changeme"` and `AUTH = {"X-API-Key": TOKEN}`, plus the helper `make_junit(...)`.
- **Seed helpers** in `backend/tests/factories.py`: `make_project`, `make_test_case`, `make_run`, `make_execution`, `make_report`. Their signatures are in task 01.
- Test functions are plain `async def test_...(client, db): ...`. With `asyncio_mode = auto`, no decorator is needed.
- Backend commands (run from `backend/`):
  ```bash
  python3.12 -m venv .venv            # once
  .venv/bin/pip install -r requirements.txt
  .venv/bin/python -m pytest -q                      # everything
  .venv/bin/python -m pytest -q tests/test_parsing.py   # one file
  ```
- **Frontend:** React 18 + Vite 6.4 + strict TypeScript. From task 11 on, the frontend uses **Vitest 5 + Testing Library + jsdom**. Commands (from `frontend/`): `npm install`, `npm test` (= `vitest run`), `npm run build` (= `tsc -b && vite build`).

## Verified external contracts (checked against installed packages on 2026-09-24)

Versions installed and exercised: fastapi 0.141.1, starlette 1.7.0, SQLAlchemy 2.1.0, asyncpg 0.31.0, alembic 1.20.0, pydantic 2.13.5, pydantic-settings 2.15.0, junitparser 5.0.3, httpx 0.28.1, pytest 9.1.1, pytest-asyncio 1.4.0, testcontainers 4.15.0, fastmcp 4.0.9 (mcp 2.2.0). Frontend: vite 6.4.3 (lockfile), vitest 5.0.1, @testing-library/react 16.3.3, @testing-library/jest-dom 7.0.1, @testing-library/user-event 14.6.7, jsdom 30.1.1.

**testcontainers** — the import path is `testcontainers.community.postgres` (the old `testcontainers.postgres` is deprecated):
```python
from testcontainers.community.postgres import PostgresContainer
with PostgresContainer("postgres:17-alpine", driver="asyncpg") as pg:
    url = pg.get_connection_url()   # "postgresql+asyncpg://test:test@<host>:<port>/test"
```

**pytest-asyncio 1.4 ini keys** (verified in `pytest_asyncio/plugin.py`): `asyncio_mode`, `asyncio_default_fixture_loop_scope`, `asyncio_default_test_loop_scope`. Setting both scopes to `session` is required so a session-scoped async engine can be used from function-scoped tests (verified working).

**Alembic from inside a running event loop.** The stock async `env.py` template calls `asyncio.run()`, which **crashes** when it's called from the FastAPI lifespan. The verified pattern is to pass a sync connection through `config.attributes["connection"]`:
```python
# env.py
connection = config.attributes.get("connection")
if connection is not None:
    do_run_migrations(connection)          # sync Connection handed in by run_sync
else:
    asyncio.run(run_async_migrations())    # CLI use: alembic upgrade head / revision
# caller
async with engine.begin() as conn:
    await conn.run_sync(lambda sync_conn: _upgrade(sync_conn))
```

**Postgres queue and lock behavior** (verified with a real postgres:17-alpine container):
- Two sessions running `SELECT id FROM reports WHERE status='pending' ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED` at the same time get **different** rows.
- `SELECT pg_try_advisory_lock(k)` returns `True` for the first connection and `False` for any other.
- ⚠ **A session-level advisory lock survives when its connection is returned to the SQLAlchemy pool.** Closing the `AsyncConnection` does *not* release it. The leader must therefore hold **one dedicated connection** for as long as it leads, and release it explicitly with `SELECT pg_advisory_unlock(k)` before closing.
- A dedicated lock connection must use autocommit, so it does not sit "idle in transaction": `conn = await engine.connect(); await conn.execution_options(isolation_level="AUTOCOMMIT")`. Verified: the holder's `pg_stat_activity.state` is `idle`, and the lock is still held.
- ON CONFLICT upsert, verified: `await s.execute(pg_insert(TestCase).values([...]).on_conflict_do_nothing(constraint="uq_test_cases_project_fingerprint"))`.
- Bulk UPDATE by primary key, verified: `await s.execute(update(TestCase), [{"id": 1, "last_status": "failed", "file": "x.py"}, {"id": 2, "last_status": "failed"}])`. Rows may have different key sets, and a row that omits a column leaves that column unchanged.
- Bulk INSERT: `await s.execute(insert(TestExecution), [ {...}, {...} ])`.

**junitparser 5.0.3** (verified):
```python
from junitparser import Error, Failure, JUnitXml, Skipped, TestSuite
xml = JUnitXml.fromstring(text)            # JUnitXml (a <testsuites>) or TestSuite (bare <testsuite>)
case.name, case.classname, case.time       # str | None, str | None, float
case._elem.get("file"), case._elem.get("line")   # raw attribute strings or None (no public accessor)
case.system_out, case.system_err           # str | None
for r in case.result:                      # Failure | Error | Skipped
    r.message, r.type, r.text              # message attr, type attr, element body text (the traceback)
```
Example: `<failure message="assert 1 == 2" type="AssertionError">Traceback...\ntests/test_a.py:14: AssertionError</failure>` gives `message='assert 1 == 2'` and `text='Traceback...\ntests/test_a.py:14: AssertionError'`.

**Runners that emit `file` (verified):**
- pytest: `-o junit_family=xunit1` gives `<testcase classname="test_a" name="test_a" file="test_a.py" line="0">`. The default (`xunit2`) has no file or line.
- Vitest: the `junit` reporter option `addFileAttribute?: boolean` (default `false`), found in the vitest 5.0.1 types (`JUnitOptions`).
- jest-junit: environment variable `JEST_JUNIT_ADD_FILE_ATTRIBUTE="true"` (reporter option `addFileAttribute`), found in the jest-junit 17 README.

**fastmcp 4.0.9** (verified with a running prototype):
```python
from fastmcp import FastMCP, Client
from fastmcp.exceptions import ToolError
from fastmcp.server.auth.providers.jwt import StaticTokenVerifier
from fastmcp.utilities.lifespan import combine_lifespans

mcp = FastMCP("FlakeRadar", instructions="...", auth=StaticTokenVerifier(
    tokens={"<token>": {"client_id": "flakeradar", "scopes": []}}))

@mcp.tool
async def top_flaky_tests(repo: str, limit: int = 20) -> list[dict]:
    """Docstring becomes the tool description."""
    ...

mcp_app = mcp.http_app(path="/")                    # Starlette app
app = FastAPI(lifespan=combine_lifespans(app_lifespan, mcp_app.lifespan))
app.mount("/mcp", mcp_app)                          # endpoint: /mcp/ (POST /mcp -> 307 -> /mcp/)
```
- Over HTTP: no token → 401, wrong token → 401, `Authorization: Bearer <token>` → OK. The 401 also comes back through `httpx.ASGITransport` without the lifespan running.
- `raise ToolError("message")` inside a tool → the client sees an error result with that text.
- In-memory testing: `async with Client(mcp) as c: r = await c.call_tool("name", {...})`. `r.data` is the returned Python value (a `dict` or `list[dict]`). `await c.call_tool(..., raise_on_error=False)` gives `r.is_error` and `r.content[0].text`. `await c.list_tools()` gives objects with a `.name`.

**Claude Code MCP registration** (verified from `claude mcp add --help`):
```bash
claude mcp add --transport http flakeradar https://<host>/mcp/ --header "Authorization: Bearer <token>"
```

## Existing code that is kept unchanged

`backend/app/scoring.py` is pure and stays as is. Its public contract:
```python
FAILING = {"failed", "error"}
SAME_SHA_FLOOR = 0.6
FULL_CONFIDENCE_SAMPLES = 7

def flip_score(statuses_newest_first: list[str], decay: float, window: int) -> float: ...
def count_same_sha_flips(executions: list[tuple[str, str]]) -> int: ...   # (commit_sha, status)
def combined_score(
    statuses_newest_first: list[str],
    executions_with_sha: list[tuple[str, str]],
    decay: float,
    window: int,
) -> tuple[float, int]:   # (flakiness_score rounded to 4 dp, confirmed_flake_count)
```
`backend/tests/test_scoring.py` must keep passing.

The fingerprint function (moved into `parsing.py` in task 02) must stay byte-identical:
```python
def fingerprint(suite: str, classname: str, name: str) -> str:
    raw = f"{suite}::{classname}::{name}".encode()
    return hashlib.sha1(raw).hexdigest()
```

## Task index

| # | File | Title |
|---|---|---|
| 01 | `01-postgres-async-foundation.md` | Postgres + async foundation |
| 02 | `02-junit-parsing-evidence.md` | JUnit parsing with Location and Failure details |
| 03 | `03-queued-ingest-and-report-api.md` | Queued ingest and the Report API |
| 04 | `04-report-processor.md` | Report processor |
| 05 | `05-background-worker.md` | Background processor with leader lock |
| 06 | `06-retention-pruning.md` | Retention pruning |
| 07 | `07-per-repo-github-issues.md` | Per-repo GitHub issues, async |
| 08 | `08-leaderboard-read-api.md` | Repos, leaderboard and summary read API |
| 09 | `09-test-detail-and-quarantine-api.md` | Test detail, permalink and quarantine API |
| 10 | `10-mcp-server.md` | MCP server |
| 11 | `11-ui-pickers-url-state-pagination.md` | UI: pickers, URL state, paginated leaderboard |
| 12 | `12-ui-slide-over-detail.md` | UI: slide-over test detail |
| 13 | `13-ui-queue-indicator.md` | UI: queue indicator |
| 14 | `14-docs-snippets-simulator.md` | Docs, CI snippets and demo simulator |
| 15 | `15-integrate-and-verify.md` | Integrate and verify |
