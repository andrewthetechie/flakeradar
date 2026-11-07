# 06 — Ingest and Report routes

Read `00-shared-context.md` first. Tasks 01–05 have landed.

Python reference: `backend/app/routers/reports.py`, `report_intake.py`, `identity.py` (get-or-create).

## Goal

CI can upload Reports and read their status. Reports stay `pending` until tasks 07 and 08 process them.

## Contracts

### `POST /api/ingest` (token required, 202)

Query parameters, with the same limits as `reports.py`: `repo`, `project`, `root`, `line_base` (0 or 1), `commit_sha`, `branch` (default `main`), `ci_run_id`, `default_branch`, `ci_job_id`, `ci_run_attempt` (at least 1), `pipeline`.

Read the body by this rule, whatever else the request says:

- A `Content-Type` starting with `multipart/form-data`: use the file field named `report`. A missing field, or a non-file field, counts as empty.
- Anything else: the raw body. That includes `application/xml` and `application/x-www-form-urlencoded`. **Never parse a form.** curl's `--data-binary` defaults to the urlencoded type.

Check in this order, with these statuses and texts:

1. Empty body: 400 `Empty report body`
2. Over `MAX_REPORT_BYTES` (20 MiB): 413 `Report exceeds 20 MB limit`
3. Name normalization errors: 422 with the error text
4. Parse errors: 422 with the error text. Run parsing in `spawn_blocking`.

Disable the default body limit on this route and enforce your own limit while reading, so a huge upload is cut off rather than buffered in full. Allow a small allowance above 20 MiB for multipart framing, and still return 413.

Blank `default_branch`, `ci_job_id` and `pipeline` become NULL. Create the Repo and Project with the race-safe get-or-create (SELECT first, then `ON CONFLICT DO NOTHING`, then SELECT).

**Duplicates:** use the identity from `find_junit_duplicate`: `project_id`, `commit_sha`, `ci_run_id`, `ci_run_attempt`, `ci_job_id`, `COALESCE(line_base, 1)` and `body_sha256`, newest first. **NULLs match NULLs here:** SQLAlchemy renders `column == None` as `IS NULL`, so an upload with no `ci_job_id` or `ci_run_attempt` is a duplicate of a stored row that also has none. Use `IS NOT DISTINCT FROM` for `ci_run_id`, `ci_run_attempt` and `ci_job_id`, or plain `=` would never match and CI retries would queue twice. A duplicate returns the stored Report's id and status with `duplicate: true`.

### `POST /api/ingest/pipeline` (token required, 202)

`PipelineReportIn` validated by `validate()` from task 04. Store its canonical JSON as the body, with no digest and no Project. Pipeline reports are never treated as duplicates here. `counts` come from Job execution uniqueness in task 07.

### Other routes

Keep `reports.py`'s statuses and texts:

- `GET /api/reports/summary`: register before the `{report_id}` route
- `GET /api/reports?status=&limit=` (1–200, default 50), with the `status must be one of ...` error
- `GET /api/reports/{id}`: 404 `Report not found`
- `POST /api/reports/{id}/retry` (token): 409 `Only failed reports can be retried`. Resets `retry_count`, `status`, `error` and `processed_at`
- `DELETE /api/reports/{id}` (token): 204. 409 `Only failed reports can be deleted`

### Queue transitions

Port `complete_report`, `fail_report`, `requeue_report` and `retry_failed_report` as field-only functions that never commit. A stored NULL `line_base` reads as 1 in `stored_junit_body`.

## Tests to port

`test_report_intake` (18), `test_ingest_api` (20), `test_pipeline_ingest_api` (6), `test_reports_api` (6).

Add tests for:

- exactly 20 MiB accepted, one byte over rejected
- a body far past the limit stops being read
- the urlencoded content type carrying raw XML
- writers-app's exact request shape

Add ingest and reports scenarios to `ENABLED` in `parity.rs` where they need no processing. Any step after `wait_processed` waits for task 08.

## Done when

- [ ] All verification in `00-shared-context.md` passes.
- [ ] Enabled parity scenarios pass.
- [ ] Manifest rows for this task are not `pending`, and gaps are logged.
- [ ] `STATUS.md` is updated, and the branch is pushed.
