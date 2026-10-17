# 02 — Correct stored nextest flaky markers (migration 0009)

## Tracer-Bullet Outcome
Migration `0009` rewrites Executions that were stored before task 01 with the nextest flaky marker as a failure. They become `passed`, so each affected Test's next rescore finds its Proven flake.

## User Story
As the maintainer, I want writers-app's Tests that already have stored flaky-fail retries to score as flakes without waiting for 90 days of new data.

## Description
Data only. One Alembic revision with one `UPDATE`, no schema change. A test runs the migration's SQL against seeded rows.

## Context Pack
- Source decisions: `00-shared-context.md`, "Existing data (migration `0009`)".
- Repo facts:
  - Latest migration `backend/migrations/versions/0008_ci_run_indexes.py`. Header style:
    ```python
    """ci run indexes

    Indexes `ci_run_id` on ...

    Revision ID: 0008
    Revises: 0007
    Create Date: 2026-10-16
    """

    from alembic import op

    revision = "0008"
    down_revision = "0007"
    branch_labels = None
    depends_on = None
    ```
  - Data-migration style in `0002_ci_jobs.py`: `op.execute("UPDATE reports SET repo_id = projects.repo_id FROM projects WHERE projects.id = reports.project_id")`.
  - Table `test_executions` columns used: `id`, `test_case_id`, `test_run_id`, `status`, `message`, `details`, `attempt`, `failure_category`.
  - Migrations run at app startup under an advisory lock (`backend/app/migrate.py`). The `engine` test fixture is already at head.
  - `backend/tests/test_db.py::test_migrations_are_idempotent` ends with `assert version == "0008"`.
  - `backend/tests/test_db.py::test_upgrade_backfills_report_repo_id` seeds rows with factories, runs the backfill SQL with `await db.execute(text(...))`, commits and checks the rows.
- Non-goals: rescoring in the migration; fixing `test_score_history`, `reports.counts` or `test_cases.last_status`; any schema change.

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `fix/nextest-flaky-fail`

## Implementation Contract
- Expected files: `backend/migrations/versions/0009_nextest_flaky_marker.py` (new), `backend/tests/test_db.py`.
- Interfaces and names:
  - Migration: `revision = "0009"`, `down_revision = "0008"`, `Create Date: 2026-10-16`. Docstring:
    ```
    nextest flaky marker

    cargo-nextest with `flaky-result = "fail"` writes the final try of a flaky
    test, which passed, as a <failure> with the message "test passed on attempt
    N/M but is configured to fail when flaky". Reports parsed before issue #8
    stored that try as failed. This marks those Executions passed, the same as
    the parser now does. Each Test is rescored from its stored Executions on its
    next Report, so no rescore runs here. Score history and report counts keep
    their old values.
    ```
  - A module constant the test reuses (SQL as written, Postgres regex operator `~`):
    ```python
    BACKFILL_SQL = """
    UPDATE test_executions AS e
    SET status = 'passed', message = '', details = '', failure_category = NULL
    WHERE e.status IN ('failed', 'error')
      AND e.attempt > 0
      AND e.message ~ '^test passed on attempt [0-9]+/[0-9]+ but is configured to fail when flaky$'
      AND EXISTS (
        SELECT 1 FROM test_executions AS prev
        WHERE prev.test_run_id = e.test_run_id
          AND prev.test_case_id = e.test_case_id
          AND prev.attempt < e.attempt
          AND prev.status IN ('failed', 'error')
      )
    """
    ```
  - `upgrade()`: `op.execute(BACKFILL_SQL)`.
  - `downgrade()`: a body of `pass`, with the comment `# Data only: the old (wrong) statuses are not restored.`
  - `test_db.py`: change the head assertion to `assert version == "0009"`.
- Behavior rules: `00-shared-context.md`, "Existing data". The `EXISTS` clause is the migration's form of the parser rule "only with earlier `flaky*` tries".
- Error and security rules: None. The SQL has no parameters.

## Steps
1. Write the migration.
2. Write the test below. Load `BACKFILL_SQL` from the migration file with `importlib`, because the file name starts with a digit:
   ```python
   import importlib.util
   from pathlib import Path

   def _migration(name: str):
       path = Path(__file__).resolve().parents[1] / "migrations" / "versions" / f"{name}.py"
       spec = importlib.util.spec_from_file_location(name, path)
       module = importlib.util.module_from_spec(spec)
       spec.loader.exec_module(module)
       return module
   ```
   Put `_migration` near the top of `test_db.py`, and the imports with the other imports. `test_db.py` today imports `Job, JobExecution, Report, TestCase` from `app.models` and `make_job, make_job_execution, make_pipeline, make_project, make_report` from `tests.factories`. Add `TestExecution`, and `make_execution, make_run, make_test_case`.
3. Update the head assertion in `test_db.py` to `"0009"`.
4. Run the Validator.

## Acceptance Criteria
- [ ] `test_migrations_are_idempotent` passes with `"0009"`. `test_models_match_migrations` still passes (no schema change).
- [ ] The new test passes: only the marker row that follows an earlier failing try in the same Run and Test changes.
- [ ] The whole backend suite and ruff pass.

## Test Expectations
`backend/tests/test_db.py`, new test `test_upgrade_marks_nextest_flaky_marker_passed(db)`:
- Seed with factories: `proj = await make_project(db)`, `tc = await make_test_case(db, proj, name="t")`, `other = await make_test_case(db, proj, name="u")`, `run = await make_run(db, proj)`.
- Rows (`MARKER = "test passed on attempt 2/2 but is configured to fail when flaky"`):
  1. `make_execution(db, tc, run, status="failed", message="panicked", attempt=0, failure_category="assertion")` → unchanged.
  2. `make_execution(db, tc, run, status="failed", message=MARKER, details="d", attempt=1, failure_category="other")` → becomes `("passed", "", "", None)`.
  3. `make_execution(db, other, run, status="failed", message=MARKER, attempt=0)` → unchanged (attempt 0).
  4. `make_execution(db, other, run, status="failed", message=MARKER, attempt=1)` → becomes passed: row 3 is an earlier failing try of the same Test in the same Run.
  5. A second Run `run2 = await make_run(db, proj, commit_sha="sha2")`: `make_execution(db, tc, run2, status="passed", attempt=0)` and `make_execution(db, tc, run2, status="failed", message=MARKER, attempt=1)` → the second stays `failed` (the earlier try passed).
  6. `make_execution(db, other, run2, status="failed", message=MARKER + " and more", attempt=1)` with `make_execution(db, other, run2, status="failed", message="x", attempt=0)` → stays `failed` (not a full match).
- `await db.commit()`, then `await db.execute(text(_migration("0009_nextest_flaky_marker").BACKFILL_SQL))`, `await db.commit()`.
- Assert each row's `(status, message, details, failure_category)` by its id. Re-select with `select(TestExecution)...` and call `db.expire_all()` first, or select the columns directly, so you do not read stale ORM state.

## Validator
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```

## Dependencies
- Blocked by: 01 (same rule; the branch exists).
- Blocks: 04.

## Labels
`bug`, `backend`, `migration`, `priority:high`
