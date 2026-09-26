# 00 — Shared context for the CI-jobs plan

**Read this whole file before you start any task in `docs/plans/ci-jobs/`.** Every task file assumes you know what is here. Each task file gives the exact contracts for its own step. This file gives the ground rules, the vocabulary, the schema that task 01 creates, and the facts about external systems that the plan relies on.

## What we are building

FlakeRadar scores Tests from JUnit reports. We are adding **Jobs**: CI jobs that are scored for flakiness with the same rules, even when they produce no JUnit. The decisions are in `docs/adr/0004-pushed-ci-jobs-and-attribution.md`. Read it. In short:

1. **Pipeline reports.** CI pushes the Job results of one attempt of one Pipeline run to a new endpoint, `POST /api/ingest/pipeline`, as JSON. FlakeRadar never calls the CI provider. The payload does not depend on any provider. GitHub Actions is the first provider, through a reporter workflow triggered by `workflow_run` (task 07).
2. **Jobs are scored like Tests.** Each Job execution is `passed`, `failed` or `skipped`. A re-run attempt is a new Job execution on the same SHA, so it produces Proven flakes.
3. **Attribution.** A JUnit upload can name the Job execution that produced it (`ci_job_id`). A failed Job execution is **explained** when a Test execution from the same Job execution failed. Explained failures are scored as `skipped`, so only **unexplained** failures count against a Job.
4. **Default-branch rule, for Tests and Jobs.** Flips count only on the Repo's Default branch. On other branches, only Proven flakes count. Until the Default branch is known (`repos.default_branch IS NULL`), every branch counts, which is the old behaviour.
5. **Surfaces:** a read API, a Tests / Jobs toggle in the UI, read-only MCP tools, and GitHub issue filing for Jobs.

**This is a live instance with real data.** Unlike the repo-split plan, nothing is wiped. The migration must keep existing rows (see task 01).

## Delivery rules (read carefully)

- **Branch:** all tasks land in order on `feat/ci-jobs`. Create it from `main` if it does not exist yet (`git switch -c feat/ci-jobs`), otherwise `git switch feat/ci-jobs`. Commit your task as one or more commits on that branch. Do not merge to `main`. The whole series ships as **one PR**, and task 11 opens it.
- **Order:** tasks run **one after another** in number order (01 … 11). You can assume every task with a lower number has landed.
- **Keep the suite green.** Unlike the repo-split plan, every task must leave `pytest` passing in `backend/`, and every task that touches `frontend/` must leave `npm run typecheck && npm run lint && npm run format:check && npm test` passing. Check each task's "Validator Stopping Point".
- **Do not edit** `CONTEXT.md` or `docs/adr/*` unless your task says to. They are already up to date for this feature.
- **Lint:** from the repo root, `ruff check backend && ruff format --check backend` must pass (ruff 0.16.9, config in `ruff.toml`).
- **Commit messages** end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. If you are a different model, use your own name in the same format.
- **If a contract in a task file is wrong** (for example, a library behaves differently), do the smallest correct thing, keep the public names, and say what you changed in the commit message.

## Domain vocabulary (from `CONTEXT.md`: use these words in code, UI text and docs)

| Term | Meaning | Avoid |
|---|---|---|
| **Repo** | A source repository, lowercase `owner/name`. Unique. | |
| **Project** | A named test suite inside a Repo. | |
| **Test** | One logical test case in one Repo and Project. The DB table is `test_cases`. | |
| **Report** | One upload from CI, of one of two **kinds**. A `junit` Report becomes a Run. A `pipeline` Report becomes Job executions. It is `pending`, then `processed` or `failed`. | upload, payload |
| **Run** | The processed result of one JUnit Report. The DB table is `test_runs`. | build, Job |
| **Execution** | One Test's outcome in one Run. | |
| **Pipeline** | A named CI workflow in a Repo. On GitHub it is identified by the workflow **file path**, e.g. `.github/workflows/ci.yml`. It groups Jobs and is **not scored**. | workflow (in provider-neutral code), build |
| **Job** | One job within a Pipeline, identified by its **display name**. Each matrix leg is its own Job, e.g. `test (ubuntu-latest, 3.12)`. It belongs to a Repo through its Pipeline, **not** to a Project. | check, step, Run |
| **Job execution** | One Job's outcome (`passed`, `failed`, `skipped`) in one attempt at one commit SHA. | job run, build |
| **CI job id** (`ci_job_id`) | The provider's id for one Job execution. On GitHub it is `job.check_run_id` inside the job, and the job `id` in the REST API. It is the same number in both places. | |
| **Explained failure** | A failed Job execution where some Test execution from a Run with the same `ci_job_id` (same Repo) failed or errored. | |
| **Unexplained failure** | A failed Job execution that is not explained. Only these count against a Job's score. | |
| **Default branch** | The Repo's main line, e.g. `main`, as CI last reported it. `NULL` means not known yet. | |
| **Flakiness score / Proven flake / Flake threshold / tiers** | Unchanged meanings. They now apply to Jobs too (flaky ≥ threshold, suspect > 0, stable = 0). | |
| **Quarantine** | Tests only. **Jobs have no Quarantine.** | |

## Decisions (the ledger): treat as requirements

**Identity**
- A Pipeline is unique by (`repo_id`, `provider`, `name`). `provider` is lowercase `^[a-z0-9_-]{1,32}$`, e.g. `github`, `gitlab`, `forgejo`.
- A Job is unique by (`pipeline_id`, `name`). If a job is renamed, it becomes a new Job. That is accepted.
- A Job execution is unique by (`job_id`, `ci_job_id`). A duplicate is **ignored**: no error, and no second row.

**Pipeline report payload** (task 03 has the exact schema)
- Status is already normalized by the reporter: `passed | failed | skipped`. The server knows nothing about provider conclusions.
- The GitHub reporter maps conclusions like this:
  - `success` → `passed`.
  - `failure` and `timed_out` → `failed`.
  - `cancelled`, `skipped` and `neutral` → `skipped`.
  - `startup_failure`, `action_required`, and anything else or null → **the job is left out**.
- Each Job execution also carries `url`, `runner_name`, `runner_labels`, `started_at` and `completed_at`, all optional. `runner_labels` holds the runner labels, e.g. `["ubuntu-latest"]`, and on GitHub it carries the OS.

**Default-branch rule** (tasks 02 and 04)
- History for scoring is taken newest first by row id (arrival order), as it is today.
- **Flip score** = `flip_score` over the newest `score_window` executions **on the Default branch**. If the Default branch is `NULL`, every branch counts.
- **Proven flakes** = `count_same_sha_flips` over the newest `score_window` executions **on any branch**.
- The score combines them exactly as `combined_score` does today (same floor, same rounding).
- The Default branch comes from `default_branch` on either kind of Report. It is applied **when the Report is processed**, not at ingest. When it changes (including `NULL` → a value), every Test and every Job in that Repo is rescored.

**Attribution** (task 05)
- A failed Job execution `E` is explained if and only if a `test_runs` row exists with `ci_job_id = E.ci_job_id`, whose Project is in the same Repo as `E`'s Pipeline, and which has at least one `test_executions` row with status `failed` or `error`.
- Attribution is **recomputed at every rescore**. It is never stored.
- Processing a JUnit Report that has a `ci_job_id` rescores every Job with a Job execution carrying that `ci_job_id` in the same Repo. Processing a Pipeline Report rescores its own Jobs, and that rescore already sees any Runs that arrived earlier.

**Retention**
- Job executions are pruned with `execution_retention_days`, by `created_at`, like Test executions. Pipeline Reports are pruned with `report_retention_days`, like JUnit Reports.
- Pipelines and Jobs are never pruned.

**Auth** (unchanged philosophy)
- `POST /api/ingest/pipeline` needs `X-API-Key`. The read endpoints do not. MCP needs the Bearer token.

**GitHub issues** (task 10)
- These are filed only for Jobs whose Pipeline `provider == "github"`, using the same gate settings as Tests. The failures signal counts **unexplained** failures only.

## Current code you will touch (facts, checked 2026-09-25 on `main` @ `fea6f57`)

- `backend/app/models.py`: `Repo`, `Project`, `TestCase`, `TestRun`, `TestExecution`, `Report`, `REPORT_*` constants, `utcnow()`. No ORM relationships.
- `backend/app/scoring.py`: pure functions `flip_score`, `count_same_sha_flips`, `combined_score(statuses_newest_first, executions_with_sha, decay, window) -> (score, confirmed)`, and `FAILING = {"failed", "error"}`. `tests/test_scoring.py` must keep passing unchanged.
- `backend/app/processing.py`: `CHUNK = 1000`, `ProcessOutcome(report_id, status, run_id, counts, touched_test_ids, error)`, `claim_next_report`, `rescore(db, test_case_ids)`, `process_report(db, report)` and `process_next(session_factory)`. The work runs in a SAVEPOINT, and failures mark the Report `failed`.
- `backend/app/identity.py`: `normalize_repo`, `normalize_project`, `normalize_root`, `get_or_create_project(db, repo, project)`.
- `backend/app/routers/reports.py`: `POST /api/ingest` with query params `repo, project, root, commit_sha, branch, ci_run_id` and a raw or multipart body. The Report read endpoints and retry use `_report_query()`, which **inner-joins Project**.
- `backend/app/queries.py`: the read services shared by REST and MCP (`Scope`, `tier_for`, `list_tests`, `summary`, `get_test`, …).
- `backend/app/github_integration.py`: `file_issues_for`, `sync_closed_issues`, `on_report_processed` and `_FilingGate`. It never raises.
- `backend/app/retention.py`: `prune(db, *, now, report_days, execution_days) -> PruneResult(reports, executions, runs)`.
- `backend/app/main.py`: the lifespan starts `ReportWorker` with `on_processed=github_integration.on_report_processed` and `prune=_maintenance`.
- `backend/tests/conftest.py`: `_TABLES = "reports, test_executions, test_runs, test_cases, projects, repos"` is truncated before each test. **New tables must be added to this list.** Also `TOKEN`, `AUTH` and `make_junit(cases, suite=..., classname=...)`.
- `backend/tests/factories.py`: `make_project`, `make_test_case`, `make_run`, `make_execution`, `make_report`. Each flushes but does not commit.
- `backend/migrations/versions/0001_baseline.py`: revision `"0001"`. The new migration is `0002`.
- Frontend: `src/api.ts` (types plus fetchers), `src/urlState.ts` (the view lives in the URL), `src/App.tsx`, and components in `src/components/` (`Leaderboard`, `TestDrawer`, `TestDetail`, `StatTiles`, `ScopePicker`, `QueueIndicator` …). Vitest with Testing Library.

## Coding conventions (unchanged from the repo-split plan)

- Python 3.12, type hints (`X | None`). Module docstrings explain *why*. Short comments.
- Async everywhere. `AsyncSession` with explicit `select()` joins. No relationships.
- Postgres-specific SQL is fine (`pg_insert(...).on_conflict_do_nothing(constraint=...)`, JSONB, window functions).
- Chunk bulk statements and `IN (...)` lists at `CHUNK = 1000`.
- Logging goes to `flakeradar.*` child loggers. Never log tokens or raw Report bodies.
- Read services go in `queries.py` and return `schemas.*`. REST routers and MCP tools stay thin.

## External facts the plan relies on

**GitHub Actions** (from docs.github.com, 2026-09-25):
- `${{ job.check_run_id }}` is available in the job context on github.com. It is the numeric job id, the same `id` that the jobs REST API returns.
- `GITHUB_WORKFLOW_REF` = `owner/repo/.github/workflows/ci.yml@refs/heads/main`. The Pipeline name is the part between `owner/repo/` and `@`.
- A job **cannot** read its own display name, such as `test (ubuntu, 3.12)`. Only the REST API returns it.
- `GET /repos/{owner}/{repo}/actions/runs/{run_id}/attempts/{attempt_number}/jobs` (`per_page` max 100, paginated) returns `jobs[]` with `id, run_id, run_attempt, name, status, conclusion, html_url, head_sha, head_branch, started_at, completed_at, runner_name, labels, workflow_name`. With `GITHUB_TOKEN` the reporter workflow needs `permissions: actions: read`.
- The `workflow_run` trigger's `workflows:` filter takes **exact workflow names** (the `name:` key), with no globs. The workflow file must be on the default branch. `types: [completed]` fires again for each re-run attempt. You can chain at most three levels.
- The `workflow_run` event payload has `github.event.workflow_run.{id, run_attempt, path, head_sha, head_branch, event, html_url, head_repository.full_name}` and `github.event.repository.{full_name, default_branch}`.
- **Fork PRs:** `workflow_run.head_branch` is the branch name **in the fork**, which may be `main`. The reporter must therefore send `branch = "<fork owner>:<head_branch>"` when `head_repository.full_name != repository.full_name`, so that a fork's `main` is never taken for the Default branch.
- **Commit SHAs:** for `pull_request` events, `${{ github.sha }}` (which the JUnit snippet uses) is the **merge commit**, while `workflow_run.head_sha` is the PR head. This does not matter for attribution, which matches on `ci_job_id` and not on SHA. Do not try to reconcile the two.

**GitLab** (for docs only, not built): `CI_JOB_ID`, `CI_JOB_NAME` (includes the matrix values), `CI_PIPELINE_ID`, `CI_DEFAULT_BRANCH`.

## Task index

| # | File | Title |
|---|---|---|
| 01 | `01-schema-and-models.md` | Schema, models, factories for Pipelines, Jobs, Job executions |
| 02 | `02-default-branch-scoring.md` | Default-branch rule for Test scoring |
| 03 | `03-pipeline-report-ingest.md` | `POST /api/ingest/pipeline` and Report API kinds |
| 04 | `04-pipeline-report-processing.md` | Process Pipeline reports; score Jobs; retention |
| 05 | `05-job-attribution.md` | Link JUnit reports to Job executions; explained failures |
| 06 | `06-jobs-read-api.md` | Jobs leaderboard, Job detail, summary, "seen in Jobs" |
| 07 | `07-github-reporter-and-docs.md` | GitHub reporter workflow, JUnit snippet, README, simulator |
| 08 | `08-ui-jobs-view.md` | UI: Tests / Jobs toggle, Jobs leaderboard, Job drawer |
| 09 | `09-mcp-job-tools.md` | MCP: `top_flaky_jobs`, `search_jobs`, `get_job` |
| 10 | `10-job-issue-filing.md` | GitHub issue filing for Jobs |
| 11 | `11-integrate-and-verify.md` | End-to-end verification and the PR |
