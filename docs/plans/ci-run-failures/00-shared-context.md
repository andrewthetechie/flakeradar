# 00 — Shared context for the ci-run-failures plan

**Read this whole file before you start any task in `docs/plans/ci-run-failures/`.** Every task file assumes you know what is here. Each task file gives the exact contracts for its own step. This file gives the ground rules, the vocabulary, the decisions and the current code that all the tasks share.

## What we are building

GitHub issue #5: "API: failed tests of one CI run, with each test's current tier".

A CI gate (the first user is fabro's merge phase) has just seen a red CI run. It needs to ask FlakeRadar: "Which tests failed in **this** CI run attempt, and are they known to be flaky?" Today it cannot. It has to scrape test names from the CI log and search for each one.

We add one read-only endpoint and one MCP tool:

```
GET /api/ci-runs/{ci_run_id}/failures?repo=OWNER/NAME[&ci_run_attempt=N]
MCP: ci_run_failures(repo, ci_run_id, ci_run_attempt?)
```

The response gives:

- the report counts of that CI run attempt (so the caller can tell "nothing failed" from "not processed yet");
- every failing Test in it, with the Test's **current** tier and score;
- every failed Job in it, marked explained or unexplained, with the Job's current tier and score. A CI run that is red only because of a setup failure then does not look all-clear.

No UI change. No change to ingest or processing. One additive migration (indexes only).

## Delivery rules (read carefully)

- **Branch:** all tasks land in order on `feat/ci-run-failures`. Create it from `main` if it does not exist yet (`git switch -c feat/ci-run-failures main`), otherwise `git switch feat/ci-run-failures`. Commit each task as one or more commits on that branch. Do not merge to `main`. The whole series ships as **one PR**, and task 06 opens it.
- **Order:** tasks run **one after another** in number order (01 … 06). You can assume every task with a lower number has landed.
- **Keep the suite green.** Every task must leave these passing:
  ```bash
  cd backend && .venv/bin/python -m pytest -q          # needs Docker (testcontainers Postgres 17)
  cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
  ```
  Use `uvx ruff@0.16.9` (or `pip install ruff==0.16.9`). An older system `ruff` fails on `ruff.toml` with `unknown field 'lint'`. No task touches `frontend/`.
- **Do not edit** `CONTEXT.md` or `docs/adr/*`. `CONTEXT.md` already defines **CI run** and **CI run attempt** for this feature. No ADR is needed.
- **Commit messages** end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. If you are a different model, use your own name in the same format.
- **If a contract in a task file is wrong** (for example, a library behaves differently), do the smallest correct thing, keep the public names, and say what you changed in the commit message.
- **Match the surrounding code.** Comment density, naming and idiom like `backend/app/queries.py` and `backend/app/routers/jobs.py`. No ORM relationships: every read is an explicit `select()`/join (see the `models.py` docstring).

## Domain vocabulary (from `CONTEXT.md`: use these words in code, docs and error text)

| Term | Meaning | Avoid |
|---|---|---|
| **Repo** | A source repository, lowercase `owner/name`. Table `repos`. | |
| **Project** | A named test suite inside a Repo. Table `projects`. | |
| **Test** | One logical test case in one Repo and Project. Table `test_cases`, model `TestCase`. | test case (in text) |
| **Report** | One upload from CI. Kind `junit` becomes a Run. Kind `pipeline` becomes Job executions. Status `pending` → `processed` or `failed`. Table `reports`. | upload |
| **Run** | The processed result of **one JUnit Report**. Table `test_runs`, model `TestRun`. | build |
| **Execution** | One try of a Test inside one Run. A retry inside the same Run is its own Execution, with `attempt` numbered **from 0**. Table `test_executions`. | result |
| **Job / Job execution / Pipeline** | A CI job, one outcome of it in one CI run attempt, and the workflow that groups Jobs. Tables `jobs`, `job_executions`, `pipelines`. | |
| **Explained / Unexplained failure** | A failed Job execution is *explained* when a Run with the same `ci_job_id` in the same Repo has a failed or errored Execution. Otherwise it is *unexplained*. The rule lives in `backend/app/attribution.py`. | |
| **CI run** (new) | One run of a Pipeline in the CI provider, identified by its `ci_run_id` within a Repo. One CI run produces many Runs and Job executions. | **Run** (that is a processed Report) |
| **CI run attempt** (new) | One try of a CI run, numbered **from 1** (`ci_run_attempt`). A Report with no attempt number counts as attempt 1. | "attempt" alone (an Execution's `attempt` is a retry inside one Run, from 0) |

**Naming rule for this feature:** never write bare "run" or "attempt" for the new concepts. Use `ci_run_id` and `ci_run_attempt` in code, params and JSON, and "CI run" / "CI run attempt" in prose. The path segment is `ci-runs`.

## Decisions (the ledger): treat as requirements

**Endpoint and identity**
- `GET /api/ci-runs/{ci_run_id}/failures`. Query: `repo` (required, `max_length=255`, normalized with `identity.normalize_repo`; a `ValueError` gives `422`), `ci_run_attempt` (optional int, `ge=1`). Path `ci_run_id`: `min_length=1`, `max_length=255`, matched **exactly** (no strip, no case change).
- No authentication, like the other read `GET`s.
- A CI run is identified by (Repo, `ci_run_id`). The same `ci_run_id` in another Repo is a different CI run.
- `NULL` `ci_run_attempt` (allowed on JUnit Reports and Runs) counts as **1** everywhere: `func.coalesce(col, 1)`. `job_executions.ci_run_attempt` is `NOT NULL` already.

**Which CI run attempt**
- The set of known attempts of a CI run = the union of `coalesce(ci_run_attempt, 1)` over (a) its Reports (any kind, any status), (b) its Runs (`test_runs` joined to `projects` on the Repo), and (c) its Job executions (`job_executions` → `jobs` → `pipelines` on the Repo).
- When `ci_run_attempt` is omitted, use the **highest** known attempt. A `pending` attempt-2 Report therefore makes attempt 2 the default before it is processed.
- The response always echoes the resolved `ci_run_attempt`.

**404 rules** (`detail` strings are exact)
- Repo name not in `repos` → `404 "Repo not found"`.
- Repo known, set of known attempts empty → `404 "CI run not found"`.
- `ci_run_attempt` given, CI run known, that attempt not in the set → `404 "CI run attempt not found"`.
- Retention consequence (documented, not handled): processed Reports are pruned after 7 days, so an older CI run answers with `reports` all 0 but still lists its failures. Runs and Executions are pruned after 90 days, after which the CI run is `404`.

**`reports` counts**
- `{pending, processed, failed}`: counts of Reports of **both kinds** (`junit` and `pipeline`) with this Repo, this `ci_run_id` and `coalesce(ci_run_attempt, 1)` equal to the resolved attempt. Every key is always present (0 when none).
- A caller polls until `pending == 0`. `failed > 0` means some results are missing. The caller decides what to do.

**`failures`: failing Tests**
- One entry per **(Test, Run)** pair where at least one Execution of that Test in that Run has status `failed` or `error` (`scoring.FAILING`). Runs = `test_runs` of this Repo (via `projects.repo_id`) with this `ci_run_id` and `coalesce(ci_run_attempt, 1)` = resolved attempt.
- Retries: `status` = the status of the pair's **last** Execution (highest `attempt`, then highest `id`). It can be `passed` (the Test passed on retry: flaky, but it did not make the build red), `failed` or `error`. `failed_attempts` = count of the pair's `failed`/`error` Executions. `message` and `failure_category` come from the pair's **last failing** Execution.
- The same Test in two Runs (matrix legs uploading the same Project) gives **two** entries, told apart by `run_id` and `ci_job_id`.
- `message` is cut to the first **1024** characters. Failure details are **not** included (the caller uses `/api/tests/{id}/history`).
- `failure_category` is the **Execution's** category, not the Test's dominant one.
- Tier, score, Proven flake count, Clean streak, quarantine and issue are the Test's **current** cached values. Tier = `queries.tier_for(score, threshold)`.
- Order: `Project.name, TestCase.suite, TestCase.classname, TestCase.name, TestRun.id` (all ascending).
- Cap: at most **500** entries (`FAILURES_LIMIT`). `total_failures` = number of pairs before the cap, `truncated` = `total_failures > len(failures)`. No pagination.

**`failed_jobs`: failed Jobs**
- One entry per Job execution of this Repo with this `ci_run_id`, `ci_run_attempt` = resolved attempt and `status == "failed"`. Passed and skipped Job executions are not listed.
- `outcome` is `"explained"` or `"unexplained"`, decided by `attribution.explained_ci_job_ids`. (Note: `/api/jobs/{id}/history` calls an unexplained failure `"failed"`. This endpoint says `"unexplained"` on purpose, because every entry here failed.)
- Tier, score, Proven flake count, Clean streak and issue are the Job's current values. `github_issue_url` only when the Pipeline's provider is `github` (same rule as `queries.to_job_out`).
- Order: `Pipeline.name, Job.name, JobExecution.id`. Not capped.

**Top-level**
- `flake_threshold` = `get_settings().flake_threshold`, so the caller sees the tier cut-off.
- No `trend` anywhere (it needs extra history queries).

**MCP**
- Tool `ci_run_failures(repo: str, ci_run_id: str, ci_run_attempt: int | None = None)` returns the same JSON as REST. The not-found cases become `ToolError` with the same text. The MCP `INSTRUCTIONS` and the README tools table each gain a line.

**Indexes**
- Migration `0008` adds plain (not `CONCURRENTLY`) indexes: `test_runs (ci_run_id)`, `reports (repo_id, ci_run_id)`, `job_executions (ci_run_id)`.

## The response shape (final; task 02 defines all of it)

```json
{
  "repo": "acme/app",
  "ci_run_id": "38965631903",
  "ci_run_attempt": 2,
  "flake_threshold": 0.3,
  "reports": {"pending": 0, "processed": 3, "failed": 0},
  "total_failures": 1,
  "truncated": false,
  "failures": [
    {"test_id": 11235, "project": "backend", "suite": "pytest",
     "classname": "tests.test_migration.TestMigration074", "name": "test_upgrade",
     "run_id": 901, "ci_job_id": "51234", "status": "failed", "failed_attempts": 1,
     "failure_category": "timing", "message": "TimeoutError: ...",
     "tier": "suspect", "flakiness_score": 0.018, "confirmed_flake_count": 0, "clean_streak": 4,
     "quarantined": false, "github_issue_number": null, "github_issue_url": null}
  ],
  "failed_jobs": [
    {"job_id": 7, "pipeline": ".github/workflows/ci.yml", "name": "backend", "ci_job_id": "51234",
     "url": "https://github.com/acme/app/actions/runs/38965631903/job/51234", "outcome": "explained",
     "tier": "stable", "flakiness_score": 0.0, "confirmed_flake_count": 0, "clean_streak": 12,
     "github_issue_number": null, "github_issue_url": null}
  ]
}
```

## Current code you will touch (facts, checked 2026-10-16 on `main` @ `a01ee9d`)

- `backend/app/models.py`: `Repo`, `Project`, `TestCase`, `TestRun`, `TestExecution`, `Report`, `Pipeline`, `Job`, `JobExecution`. Relevant columns:
  - `TestRun`: `project_id`, `ci_run_id: str` (default `""`), `ci_job_id: str | None`, `ci_run_attempt: int | None`. **No `__table_args__` today.**
  - `TestExecution`: `test_case_id`, `test_run_id`, `status` (`passed|failed|error|skipped`), `message`, `attempt` (from 0), `failure_category: str | None`.
  - `Report`: `kind` (`junit|pipeline`), `repo_id` (NOT NULL), `project_id` (NULL for pipeline), `ci_run_id`, `ci_run_attempt: int | None`, `status`. `__table_args__` already has `Index("ix_reports_status_id", "status", "id")` and a `CheckConstraint`.
  - `JobExecution`: `job_id`, `ci_job_id`, `ci_run_id`, `ci_run_attempt: int` (NOT NULL, default 1), `status` (`passed|failed|skipped`), `url`. `__table_args__` has a `UniqueConstraint` and `Index("ix_job_exec_job_id", "job_id", "id")`.
  - Constants: `REPORT_PENDING`, `REPORT_PROCESSED`, `REPORT_FAILED`, `JOB_FAILED`.
- `backend/app/scoring.py`: `FAILING = {"failed", "error"}`.
- `backend/app/attribution.py`: `async def explained_ci_job_ids(db, repo_id: int, ci_job_ids: list[str]) -> set[str]` (already chunks its IN lists).
- `backend/app/queries.py` ("read services shared by the REST routers and the MCP server"): `tier_for(score, threshold)`, `issue_url(repo, number)`, `to_job_out(...)`, section headers like `# --- Jobs (task 06) ---`. Every function takes an `AsyncSession` and returns `schemas.*` models.
- `backend/app/schemas.py`: Pydantic response models. `Tier = Literal["flaky", "suspect", "stable"]`. Classes whose name starts with `Test` set `__test__ = False`.
- `backend/app/routers/jobs.py`: the router style to copy (`APIRouter()`, `Depends(get_db)`, `get_settings().flake_threshold`, `HTTPException(404, detail=...)`).
- `backend/app/main.py`: routers are included **above** the static `Mount("/")`: `app.include_router(reports.router)`, `tests.router`, `jobs.router`, then `app.mount("/mcp", mcp_app)`.
- `backend/app/mcp_server.py`: `build_mcp(session_factory, *, api_token)`, helper `_scope(repo, project)` (normalizes, raises `ToolError`), tools return `model_dump(mode="json")`.
- `backend/migrations/versions/`: latest is `0007_report_retry_count.py` (`revision = "0007"`, `down_revision = "0006"`).
- Tests: `backend/tests/conftest.py` gives fixtures `client` (httpx against the app, **no worker runs**, so ingested Reports stay `pending`), `db` (an `AsyncSession`) and `session_factory`; `AUTH = {"X-API-Key": "changeme"}`; `make_junit(cases, suite=..., classname=...)`. `backend/tests/factories.py` gives `make_project`, `make_test_case(db, project, name=..., classname=..., suite=..., **fields)`, `make_run(db, project, commit_sha=..., ci_run_id=..., ci_job_id=..., ci_run_attempt=...)`, `make_execution(db, tc, run, status=..., message=..., attempt=..., failure_category=...)`, `make_report(db, project, body, ci_run_id=..., status=..., kind=..., ci_run_attempt=...)`, `make_pipeline(db, repo, name, provider=...)`, `make_job(db, pipeline, name=..., **fields)`, `make_job_execution(db, job, status, ci_job_id=..., ci_run_id="1", ci_run_attempt=1, ...)`. Factories flush but do not commit: call `await db.commit()` before using `client`.
- `backend/tests/test_db.py`: `test_models_match_migrations` (autogenerate must find no diff) and `test_migrations_are_idempotent` (asserts the head revision string).
