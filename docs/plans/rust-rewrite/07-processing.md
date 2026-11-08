# 07 — Processing, attribution, Score history

Read `00-shared-context.md` first. Tasks 01–06 have landed.

Python reference: `backend/app/processing.py`, `attribution.py`, `score_history.py`. Read their docstrings closely, because they state the rules. Also read `docs/perf/20-test-rescore-explain.md` and `21-job-rescore-explain.md`, and ADRs 0002, 0004, 0005, 0007 and 0009.

## Goal

One pending Report becomes a Run or Job executions, and Tests and Jobs get rescored. This is the core of the product, so port it statement by statement, in `crates/flakeradar-db`.

## Contracts

### `process_next(db, settings) -> Option<ProcessOutcome>`

- One transaction: claim the oldest pending Report with `FOR UPDATE SKIP LOCKED`, then do the work in a SAVEPOINT.
- On an error, roll back to the savepoint, and in the same outer transaction either requeue or fail.
- Error text is capped at 2000 characters.
- `ProcessOutcome` has the same fields as Python.

**Transient or permanent.** Python retries only SQLAlchemy `OperationalError` and `InterfaceError`. In Rust, treat these as transient:

- connection, I/O, pool timeout and pool closed errors
- SQLSTATE classes `08`, `53` and `57` (including `57014` from `statement_timeout`)
- `40001` and `40P01`

Everything else is permanent, including every parse and validation failure. A transient failure requeues only while `retry_count < max_report_retries`. A permanent failure spends no budget.

### JUnit path (`process_report`)

Keep this order, because scores depend on it:

1. Parse and classify in `spawn_blocking`.
2. `apply_default_branch`, before inserting anything, so the rescore reads the new value.
3. Update `projects.root` **only when it differs**, so the row is not rewritten by every Report.
4. Insert the Run.
5. Upsert Tests with `ON CONFLICT DO NOTHING`, one row per distinct fingerprint in report order, first occurrence wins, in chunks.
6. Build Executions in report order. `attempt` counts per Test from 0. `counts` has `passed`, `failed`, `error`, `skipped` and `retried`, where `retried` is the number of Tests with more than one Execution.
7. Update Tests with the **last** occurrence: `last_status` and `last_seen_at`, and `file` and `line` **only if the report gave a file**.
8. Rescore touched Tests, sorted, with `day_counts`.
9. If the branch changed, rescore the whole Repo.
10. If `ci_job_id` is set, rescore Jobs carrying it in this Repo.
11. Complete the Report. Tier changes are empty if the branch changed.

Keep sequence-consuming statements in the same shape as Python, because `ON CONFLICT DO NOTHING` burns ids and the golden files hold exact ids. If ids drift, fix the statement, not the comparison.

### Pipeline path (`process_pipeline_report`)

Read lines 463–582 of `processing.py` and port them as they are: Pipeline upsert, Job upsert (with the `summary` flag change on conflict), Job executions deduplicated by `(job_id, ci_job_id)`, only brand-new executions count, `last_status` and `last_seen_at`, rescoring `jobs_with_new_exec | flag_changed`, `counts` including `duplicate`.

### Rescoring

- `rescore` and `rescore_jobs`: read the window from `attribution` with `include_default_branch=True`, compute in Rust, then write.
- **A bulk update must leave a column alone when Python's dict leaves it out.** `github_issue_fixed_noted_at` is set to NULL only when the streak is 0, and otherwise keeps its value.
- `_crossed`: only the flaky boundary counts, using the old score read **before** the update. A Test leaving the flaky tier while broken sends nothing.
- A missing Test or Job (deleted before the read) sends nothing.
- Explained failed Job executions become `skipped` in `_job_history`.
- `rescore_repo`: logs the count of dropped changes and returns none.
- Only history uses `day_counts`. Score history upserts replace score and Proven flake count, and **add** executions and failures.

### Attribution

Port `job_execution_explained`, the window functions and the rest of `attribution.py`. Keep the lateral query shape, so the indexes in `docs/perf/` are still used.

### Score history reads

`past_scores`, `newest_history_before`, `window_rows`, with `HISTORY_DAYS = 90` and the `<=` and `>` cutoffs as written. Dates are UTC.

### Clock

Inject the clock (a `Clock` trait or a `now` argument), so tests control days.

## Tests to port

`test_processing` (38), `test_pipeline_processing` (16), `test_attribution` (21), `test_score_history` (10), `test_report_retry_budget` (7). Backoff tests belong to task 08; change their manifest rows.

Add tests for:

- a column left alone by the bulk update
- each transient class and a permanent error (simulate with statements that raise the SQLSTATE)
- a 3,000-test report crossing chunk boundaries
- a retry-only report

Run the parity scenarios from a test helper that calls `process_next` directly for now, since the worker lands in task 08.

## Done when

- [ ] All verification in `00-shared-context.md` passes.
- [ ] `EXPLAIN` on the two rescore queries matches the plans in `docs/perf/` (same indexes, per-row lateral). Save the output as a test or note.
- [ ] Manifest rows for this task are not `pending`, and gaps are logged.
- [ ] `STATUS.md` is updated, and the branch is pushed.
