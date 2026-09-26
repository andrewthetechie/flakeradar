# 01 — Schema, models and factories for Pipelines, Jobs and Job executions

## Tracer-Bullet Outcome
Migration `0002` adds everything later tasks need, and it keeps every existing row:
- `repos.default_branch`.
- A Report `kind` and `repo_id`, with `project_id` made nullable.
- Enrichment columns on `reports` and `test_runs`.
- The new tables `pipelines`, `jobs` and `job_executions`.

`models.py` matches the migrations exactly (the autogenerate test finds no diff), and the factories can seed the new rows. Nothing uses the new columns yet. The app behaves exactly as before, except that JUnit ingest now also sets `reports.repo_id`.

## User Story
As the implementer of tasks 02–10, I want the final schema in place first, so that each later task changes behaviour only and not the database.

## Context Pack
- Read `00-shared-context.md`: the ledger, and the warning that this instance has real data.
- Repo facts:
  - `backend/migrations/versions/0001_baseline.py` is revision `"0001"`. It is the style to copy: `TS = sa.DateTime(timezone=True)`, explicit constraint and index names.
  - `backend/tests/test_db.py::test_models_match_migrations` runs `alembic.autogenerate.compare_metadata` and requires an empty diff. Column `index=True` in the models produces `ix_<table>_<column>`. Name the migration indexes the same way, or declare them in `__table_args__` with explicit names.
  - `backend/tests/test_db.py::test_migrations_are_idempotent` asserts `version == "0001"`. Change it to `"0002"`.
  - `backend/tests/conftest.py::_TABLES` must list every table, children first.
  - `backend/app/routers/reports.py::ingest_endpoint` creates `Report(project_id=..., ...)`. It must now also set `repo_id=proj.repo_id`.
- Non-goals: any behaviour change. No new endpoints, no processing, no scoring.

## Implementation Contract

### Migration `backend/migrations/versions/0002_ci_jobs.py`
`revision = "0002"`, `down_revision = "0001"`. The upgrade does the following, in this order:

1. `repos`: add `default_branch` `String(255)`, nullable.
2. `reports`:
   - Add `kind` `String(16)`, not null, `server_default="junit"`.
   - Add `repo_id` `Integer`, FK `repos.id` `ON DELETE CASCADE`. Add it **nullable** first, backfill it with `UPDATE reports SET repo_id = projects.repo_id FROM projects WHERE projects.id = reports.project_id`, then set it `NOT NULL`. Index `ix_reports_repo_id`.
   - Make `project_id` nullable.
   - Add the nullable columns `ci_job_id String(255)`, `ci_run_attempt Integer`, `pipeline String(512)` and `default_branch String(255)`.
   - Add a check constraint `ck_reports_junit_has_project`: `kind <> 'junit' OR project_id IS NOT NULL`.
3. `test_runs`: add the nullable columns `ci_job_id String(255)`, `ci_run_attempt Integer` and `pipeline String(512)`. Add index `ix_test_runs_ci_job_id` on `ci_job_id`.
4. Create `pipelines`:
   - `id` Integer PK.
   - `repo_id` FK `repos.id` CASCADE, not null.
   - `provider` `String(32)`, not null.
   - `name` `String(512)`, not null.
   - `created_at` TS, not null.
   - Unique `uq_pipelines_repo_provider_name` (`repo_id`, `provider`, `name`), and index `ix_pipelines_repo_id`.
5. Create `jobs`:
   - `id` Integer PK.
   - `pipeline_id` FK `pipelines.id` CASCADE, not null.
   - `name` `String(512)`, not null.
   - `flakiness_score` Float, not null, `server_default="0"`.
   - `confirmed_flake_count` Integer, not null, `server_default="0"`.
   - `last_status` `String(16)`, not null, `server_default="passed"`.
   - `last_seen_at` TS, not null.
   - `github_issue_number` Integer, nullable.
   - `created_at` TS, not null.
   - Unique `uq_jobs_pipeline_name` (`pipeline_id`, `name`). Indexes `ix_jobs_pipeline_id` and `ix_jobs_pipeline_score` (`pipeline_id`, `flakiness_score`).
6. Create `job_executions`:
   - `id` BigInteger PK.
   - `job_id` FK `jobs.id` CASCADE, not null.
   - `ci_job_id` `String(255)`, not null.
   - `ci_run_id` `String(255)`, not null, `server_default=""`.
   - `ci_run_attempt` Integer, not null, `server_default="1"`.
   - `commit_sha` `String(64)`, not null.
   - `branch` `String(255)`, not null.
   - `status` `String(16)`, not null.
   - `url` Text, not null, `server_default=""`.
   - `runner_name` `String(255)`, not null, `server_default=""`.
   - `runner_labels` JSONB, not null, `server_default=sa.text("'[]'::jsonb")`.
   - `started_at` TS, nullable. `completed_at` TS, nullable.
   - `created_at` TS, not null.
   - Unique `uq_job_executions_job_ci_job_id` (`job_id`, `ci_job_id`). Indexes `ix_job_exec_job_id` (`job_id`, `id`), `ix_job_executions_ci_job_id` (`ci_job_id`) and `ix_job_executions_created_at` (`created_at`).

`downgrade()` reverses these steps in reverse order. It first deletes `kind='pipeline'` Reports so that `project_id` can be `NOT NULL` again.

### `backend/app/models.py`
- Add the constants `REPORT_KIND_JUNIT = "junit"`, `REPORT_KIND_PIPELINE = "pipeline"` and `REPORT_KINDS = (REPORT_KIND_JUNIT, REPORT_KIND_PIPELINE)`. Also add `JOB_STATUSES = ("passed", "failed", "skipped")`.
- `Repo.default_branch: Mapped[str | None]`.
- `Report`: `kind`, `repo_id`, `project_id: Mapped[int | None]`, `ci_job_id`, `ci_run_attempt`, `pipeline`, `default_branch`, plus the check constraint in `__table_args__`.
- `TestRun`: `ci_job_id`, `ci_run_attempt`, `pipeline`.
- New classes `Pipeline`, `Job` and `JobExecution`, mirroring the tables above. Give `JobExecution` `__test__ = False` only if pytest tries to collect it (it doesn't, because the name does not start with `Test`). Update the module docstring with one paragraph on Pipelines, Jobs and Job executions.

### `backend/tests/factories.py`
Add these helpers. Each flushes and does not commit:
```python
async def make_pipeline(db, repo: str = "acme/app", name: str = ".github/workflows/ci.yml",
                        provider: str = "github") -> Pipeline: ...   # get-or-create the Repo, like make_project
async def make_job(db, pipeline: Pipeline, name: str = "test", **fields) -> Job: ...
async def make_job_execution(db, job: Job, status: str = "passed", *, ci_job_id: str | None = None,
                             commit_sha: str = "sha1", branch: str = "main", ci_run_id: str = "1",
                             ci_run_attempt: int = 1, created_at: datetime | None = None,
                             **fields) -> JobExecution: ...   # ci_job_id defaults to a unique counter string
```
Also extend `make_report(...)` with the keyword arguments `kind="junit"`, `ci_job_id=None`, `ci_run_attempt=None`, `pipeline=None` and `default_branch=None`. It sets `repo_id=project.repo_id`. Extend `make_run(...)` with `ci_job_id=None`, `ci_run_attempt=None` and `pipeline=None`.

### Other edits
- `conftest.py`: `_TABLES = "reports, job_executions, jobs, pipelines, test_executions, test_runs, test_cases, projects, repos"`.
- `routers/reports.py::ingest_endpoint`: set `repo_id=proj.repo_id` on the new Report.

## Acceptance Criteria
- [ ] `alembic upgrade head` on a database with `0001` data keeps every Report, and every Report gets `repo_id` equal to its Project's Repo and `kind == "junit"`.
- [ ] `test_models_match_migrations` passes (empty diff).
- [ ] A `junit` Report with `project_id = NULL` raises `IntegrityError`. A `pipeline` Report with `project_id = NULL` is accepted.
- [ ] Two Job executions with the same (`job_id`, `ci_job_id`) raise `IntegrityError`.
- [ ] Downgrade to `0001` and upgrade again both succeed.
- [ ] The whole existing suite still passes.

## Test Expectations
Extend `backend/tests/test_db.py`:
- `test_migrations_create_every_table`: add `pipelines`, `jobs`, `job_executions`.
- `test_migrations_are_idempotent`: expect `"0002"`.
- `test_junit_report_requires_project`, `test_pipeline_report_allows_no_project`, `test_job_execution_unique_per_ci_job_id`, `test_job_unique_per_pipeline`.
- `test_upgrade_backfills_report_repo_id`: use `alembic.command.downgrade`/`upgrade` in a `run_sync` in the same way `app/migrate.py` runs migrations, on a throwaway schema or with care to restore `head`. If that is awkward with the shared session engine, test the backfill SQL directly against seeded rows instead, and say so in the commit message.

## Dependencies
- Blocked by: nothing.
- Blocks: every later task.

## Estimate / Risk
Medium. Risk 3: this is the only task that migrates live data. The backfill and the downgrade path are the risky parts.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && ruff check backend && ruff format --check backend
```
