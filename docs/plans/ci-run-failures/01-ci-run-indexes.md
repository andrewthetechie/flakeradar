# 01 — Indexes for CI run lookups

## Tracer-Bullet Outcome
Migration `0008` adds three indexes, so you can look up a CI run by `ci_run_id` without a table scan. `models.py` declares the same indexes, so `test_models_match_migrations` stays green. No behavior changes.

## User Story
As the maintainer, I want the CI run lookup in later tasks to stay fast on a live instance with months of Runs and Job executions.

## Description
Schema only. One Alembic revision, three `Index` declarations in `models.py`, and a one-line test update.

## Context Pack
- Source decisions: `00-shared-context.md`, "Indexes".
- Repo facts:
  - Latest migration: `backend/migrations/versions/0007_report_retry_count.py`. Its header style:
    ```python
    """report retry cap

    Adds `reports.retry_count` ...

    Revision ID: 0007
    Revises: 0006
    Create Date: 2026-10-xx
    """

    import sqlalchemy as sa
    from alembic import op

    revision = "0007"
    down_revision = "0006"
    branch_labels = None
    depends_on = None
    ```
  - Index creation style used in `0006`: `op.create_index("ix_job_score_history_day", "job_score_history", ["day"])` and `op.drop_index("ix_job_score_history_day", table_name="job_score_history")`.
  - `TestRun` in `backend/app/models.py` has no `__table_args__`. `Report.__table_args__` is:
    ```python
    __table_args__ = (
        Index("ix_reports_status_id", "status", "id"),
        CheckConstraint("kind <> 'junit' OR project_id IS NOT NULL", name="ck_reports_junit_has_project"),
    )
    ```
    `JobExecution.__table_args__` is:
    ```python
    __table_args__ = (
        UniqueConstraint("job_id", "ci_job_id", name="uq_job_executions_job_ci_job_id"),
        Index("ix_job_exec_job_id", "job_id", "id"),
    )
    ```
  - `backend/tests/test_db.py::test_migrations_are_idempotent` ends with `assert version == "0007"`.
- Non-goals: any query or endpoint; `CONCURRENTLY` index builds.

## Delivery Strategy
- Shape: Prep (schema only, no behavior)
- Valid-state scope: `feat/ci-run-failures` (create it from `main` in this task)

## Implementation Contract
- Expected files: `backend/migrations/versions/0008_ci_run_indexes.py` (new), `backend/app/models.py`, `backend/tests/test_db.py`.
- Interfaces and names:
  - Migration: `revision = "0008"`, `down_revision = "0007"`. Docstring first line `ci run indexes`, body: "Indexes `ci_run_id` on `test_runs`, `reports` (with `repo_id`) and `job_executions`, for `GET /api/ci-runs/{ci_run_id}/failures`. Plain `CREATE INDEX`: these tables are small next to `test_executions`.", `Create Date: 2026-10-16`.
  - `upgrade()`:
    ```python
    op.create_index("ix_test_runs_ci_run_id", "test_runs", ["ci_run_id"])
    op.create_index("ix_reports_repo_ci_run_id", "reports", ["repo_id", "ci_run_id"])
    op.create_index("ix_job_executions_ci_run_id", "job_executions", ["ci_run_id"])
    ```
  - `downgrade()` drops the three in reverse order with `op.drop_index(name, table_name=...)`.
  - `models.py`:
    - `TestRun` gains `__table_args__ = (Index("ix_test_runs_ci_run_id", "ci_run_id"),)`. Put it right after `__test__ = False`, the same placement as `TestExecution`.
    - `Report.__table_args__` gains `Index("ix_reports_repo_ci_run_id", "repo_id", "ci_run_id")`, after `ix_reports_status_id`.
    - `JobExecution.__table_args__` gains `Index("ix_job_executions_ci_run_id", "ci_run_id")`, after `ix_job_exec_job_id`.
    - Do **not** add `index=True` to the `ci_run_id` columns. That would generate different index names.
  - `test_db.py`: `assert version == "0008"`.
- Behavior rules: None.
- Error and security rules: None.

## Steps
1. `git switch -c feat/ci-run-failures main`.
2. Write the migration.
3. Add the three `Index` entries to `models.py`.
4. Change the asserted head revision in `test_db.py` to `"0008"`.
5. Run the Validator.

## Acceptance Criteria
- [ ] `test_models_match_migrations` passes (autogenerate finds no diff).
- [ ] `test_migrations_are_idempotent` passes with `"0008"`.
- [ ] The whole backend suite and ruff pass.

## Test Expectations
- No new tests. The two `test_db.py` tests cover the migration and the model agreement.

## Validator
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```

## Dependencies
- Blocked by: None.
- Blocks: 02 (its queries rely on these indexes for speed, not for correctness).

## Labels
`chore`, `backend`, `priority:high`
