# Implement summary Jobs (issue #9)

## Tracer-Bullet Outcome

A failed summary Job is `explained` when another failed non-summary Job exists in
the same CI run attempt, never moves its own score, and is still `unexplained`
when its own step broke with no failed sibling. `summary` round-trips from
`POST /api/ingest/pipeline` onto the Job and into the read API.

## User Story

As a CI gate, I want a summary Job's failure to be explained by the Job that
actually failed, so that a red run caused only by a known flake is not blocked by
an `unexplained` job.

## Context Pack

- Read `00-shared-context.md`. ADR 0004 (amended) gives the reasons.
- Non-goals: UI copy for summary Jobs; any change to how Test scores work.

## Implementation Contract

### Schema and model

- `JobResultIn`: `summary: bool = False`.
- `JobOut`, `CiRunFailedJobOut`: `summary: bool`.
- `Job.summary`: `Boolean`, NOT NULL, server default false.
- Migration `0010_summary_jobs.py` (`down_revision = "0009"`): add the column.
  No backfill; a Job becomes a summary Job only when its reporter says so.

### Attribution (`attribution.py`)

- `summary_explained_clause(ci_run_id, ci_run_attempt, repo_id, summary_flag) -> ColumnElement[bool]`
  returns `summary_flag.is_(True) AND EXISTS(<failed non-summary sibling in the same attempt>)`,
  using `aliased()` sibling tables.
- `explained_ci_job_ids`: after the Test-explained pass, union the summary Job
  executions whose `summary_explained_clause` is true.
- `unexplained_failure_counts`: the `explained` label becomes
  `or_(explained_clause(...), summary_explained_clause(...))`.

### Scoring (`processing.py`)

- `_job_history`: select `summary_explained_clause(...)` and feed status
  `JOB_SKIPPED` when `(is_explained or is_summary_explained) and status == JOB_FAILED`.
- `process_pipeline_report`: build `summary_by_name` and upsert with
  `on_conflict_do_update(constraint="uq_jobs_pipeline_name", set_={"summary": ...})`
  so the newest report wins. `last_seen_at` is left alone on conflict.

### Read API (`queries.py`)

- `to_job_out` sets `summary=job.summary`.
- `_ci_run_failed_jobs` sets `summary=job.summary`. The `outcome` needs no change
  once `explained_ci_job_ids` covers summary Jobs.

### Reporter and docs

- `samples/flakeradar-jobs.yml`: `const SUMMARY_JOBS = ["alls-green"]` and
  `summary: SUMMARY_JOBS.includes(j.name)`.
- `docs/adr/0004-...md`: a "summary Job" paragraph, plus rejected options and
  consequences.
- `README.md` and `CONTEXT.md`: the rule in one place each.
- `frontend/src/api.ts`: `summary: boolean` on `Job`.

## Behavior Rules

1. A failed summary Job with a failed non-summary Job in the same attempt is
   explained, whether or not the sibling is itself explained.
2. A failed summary Job with no failed non-summary Job in the attempt is
   unexplained.
3. Two failed summary Jobs do not explain each other.
4. A passing summary Job is never affected: attribution only turns `failed` into
   `skipped`.
5. A Job's `summary` flag follows the newest Pipeline report (not sticky).

## Test Expectations

- `tests/test_attribution.py`
  1. `test_summary_job_explained_by_a_failed_sibling`: `explained_ci_job_ids` is
     `{"J2"}` for `J1` (real, unexplained) and `J2` (summary); `_job_history`
     gives `J2` status `skipped`.
  2. `test_summary_job_without_a_failed_sibling_is_unexplained`: set is empty;
     history status is `failed`.
  3. `test_summary_jobs_do_not_explain_each_other`: two summary Jobs, no
     non-summary failure → empty set.
  4. `test_summary_job_explained_when_the_real_failure_is_a_flaky_test`: the
     issue-#9 setup with a linked failing JUnit report → `{"J1", "J2"}`.
  5. `test_summary_job_failure_does_not_move_its_score`: same-SHA fail then pass
     → score `0.0`, `confirmed_flake_count == 0`, `clean_streak == 1`.
  6. `test_unexplained_failure_counts_excludes_summary_explained`: the gate count
     is `{}`; the real Job still counts `1`.
- `tests/test_ci_runs_api.py`
  1. `test_summary_job_explained_by_a_failed_job`: the issue-#9 run →
     `[("alls-green-playwright", "explained", True), ("test-playwright", "explained", False)]`.
  2. `test_summary_job_is_unexplained_without_a_failed_sibling` →
     `[("alls-green", "unexplained", True)]`.
  3. `test_failed_job_fields` gains `"summary": False`.
- `tests/test_pipeline_processing.py::test_summary_flag_defaults_false_and_flips`:
  absent → false, `summary=True` → true, a later report without it → false.
- `tests/test_jobs_api.py::test_job_out_includes_summary_flag`: `JobOut` carries it.
- `tests/test_db.py::test_migrations_are_idempotent`: head is `0010`.

## Validator

```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
cd frontend && npx tsc --noEmit && npm test -- --run
```

## Risks

- **Alias correlation**: `summary_explained_clause` must use `aliased()` tables,
  or the EXISTS silently correlates with the caller's joins. Covered by the
  agreement tests and `test_explained_ci_job_ids_agrees_with_scoring`.
- **False explained**: a summary Job whose own step broke while an unrelated
  non-summary Job also failed is hidden. Accepted per the issue's "no failed
  needed job" wording.
- **Last-write-wins flag**: a workflow that drops the marker flips the flag back.
  Accepted (self-healing); no sticky-true.
- **Rollback**: revert the commit. `summary` defaults to `False`, so old callers
  and old rows behave exactly as before.
