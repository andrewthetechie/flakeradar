# 02 — Golden recorder and pytest manifest

Read `00-shared-context.md` first. Task 01 has landed.

## Goal

Capture what the frozen Python server does, once, as committed files. Rust tests replay those files, so Python never has to run in CI. Also list every pytest function so nothing is forgotten.

This is the only task that writes new Python. The recorder is deleted in task 15. The golden files stay as regression tests.

Set up with `cd backend && python3.12 -m venv .venv && .venv/bin/pip install -r requirements-dev.txt`.

## Deliverables

### `docs/plans/rust-rewrite/test-manifest.csv`

Generate it with `pytest --collect-only -q` from `backend/`. One row per test **function**: strip the `[...]` parameter suffix and de-duplicate (649 collected nodes come down to 573 functions). `test_report_action.py` skips itself at module level when a tool it needs is missing, so collection can leave out its 2 tests. Add them by hand if they are absent. There must be exactly 575 rows. Columns:

`pytest_id,task,status,rust_test,reason`

Set `status` to `pending`, and set `task` by test file:

| Task | Test files |
|---|---|
| 03 | `test_db` |
| 04 | `test_parsing`, `test_scoring`, `test_classify`, `test_identity` |
| 05 | `test_app_startup`, `test_ops_api` |
| 06 | `test_report_intake`, `test_ingest_api`, `test_pipeline_ingest_api`, `test_reports_api` |
| 07 | `test_processing`, `test_pipeline_processing`, `test_attribution`, `test_score_history`, `test_report_retry_budget` |
| 08 | `test_worker`, `test_retention` |
| 09 | `test_tests_api`, `test_test_detail_api`, `test_jobs_api`, `test_ci_runs_api` |
| 10 | `test_github`, `test_github_worker`, `test_issue_links_api`, `test_issue_links_lifecycle`, `test_webhook`, `test_webhook_worker`, `test_repos_api` |
| 11 | `test_mcp` |
| 14 | `test_gitlab_reporter`, `test_report_action` |

A later task may hand a single test to the task that owns its code. Change the `task` column and say so in the commit message.

### `scripts/check-test-manifest.mjs`

`node scripts/check-test-manifest.mjs [--upto NN]` fails if:

- any row with `task <= NN` is still `pending`,
- a `ported` row's `rust_test` is not in `cargo nextest list --message-format json`, or
- an `na` row has no reason.

Add it to the `rust` CI job with `--upto` set to the last finished task, which it reads from `STATUS.md`. Add tests for it.

### JUnit corpus: `tests/parity/junit/`

XML files, plus `tests/parity/golden/junit/<name>.json` holding what Python's `parse_junit_xml` returned, or the error. Each case records `suite`, `classname`, `name`, `status`, `duration`, `message`, `details`, `file`, `line`, the `fingerprint`, and the `category` from `classify_failure` for failing cases.

Cover at least the following, reusing fixtures from `backend/tests/` and `actions/report/test/`:

- real output shapes: pytest (xunit1 and xunit2), Jest, Vitest, Playwright with `flakyFailure`, Surefire with `rerunFailure` and `rerunError`, and cargo-nextest with the flaky marker (`backend/tests/fixtures/nextest_flaky_fail.xml`)
- structure: a `<testsuites>` wrapper, a bare `<testsuite>`, nested suites, a case with no `name`, several results in one case
- encoding: declared ISO-8859-1, UTF-8 with a BOM, UTF-16, invalid bytes in captured output
- content: CDATA, entities, `file` starting with `./`, non-numeric `line`, `line_base` 0 and 1, missing or invalid `time` values
- caps: a message and details past the limits, with multi-byte characters across the cut
- whitespace: messages and output with the characters from the Python trap table in `00-shared-context.md`
- bad input: empty body, not XML, valid XML with no test cases, a DOCTYPE with an external entity, nested entity expansion

Record each file with both `line_base` values where it matters.

### Scenarios: `tests/parity/scenarios/*.json`

Each scenario is a declarative list of steps, run on an empty database, with the real worker running:

```json
{"name": "flaky-test-lifecycle", "settings": {}, "steps": [
  {"http": {"method": "POST", "path": "/api/ingest", "query": {"repo": "o/r", "commit_sha": "a1"}, "headers": {"X-API-Key": "$TOKEN", "Content-Type": "application/xml"}, "body_file": "junit/x.xml"}},
  {"wait_processed": true},
  {"http": {"method": "GET", "path": "/api/tests", "query": {"repo": "o/r"}}},
  {"dump_db": ["test_cases", "test_executions"]}
]}
```

Seed only through the public API, so both servers run the same steps. Cover:

- every route in `backend/app/routers/` and `main.py`, on success and on every hand-written error
- auth failure on every token-protected route
- validation failures (bad query types, out-of-range values, oversized fields)
- an upload past 20 MB, a multipart upload, an empty body, a duplicate upload
- pipeline reports, including explained and unexplained failures, and a summary Job
- Default branch changes
- quarantine with reason and expiry, Issue links (`replace`, conflicts, wrong repo)
- every leaderboard filter and sort key, pagination
- repo and project delete
- `/metrics`, `/api/health`, `/api/health/ready`
- trailing-slash requests
- the request shapes in the "Consumers" table, including writers-app's

Do not use GitHub in scenarios. Task 10 covers it with `wiremock`. Time-dependent behaviour (trend, retention, `new_within_days`) stays in the ported unit tests, because it cannot be reached through the API.

### MCP scenarios

Add an `mcp` step type recorded through the real FastMCP client: `initialize`, `tools/list` and `tools/call` for all 11 tools with success and error arguments, plus unauthenticated requests to `/mcp/` and `/mcp`. Record the `instructions` text, each tool's description, input schema and annotations.

### Recorder: `tools/parity-record/record.py`

Starts `backend` against a throwaway Postgres 17 for each scenario, with fixed settings and a known token, runs the steps, and writes `tests/parity/golden/<scenario>/<nn>.json` (status, relevant headers, parsed JSON or text body). Sequences start at 1 on the fresh database, so ids are deterministic.

**Normalization at record time:** replace values the server generates from the clock with `"<timestamp>"`, after checking they parse. Keep timestamps that came from the scenario. Keep ids exact. Running the recorder twice must produce no diff, so sort anything Python returns in an unspecified order, and add a CI-free check for this in the script's own `--verify` mode.

Also write `tests/parity/schema.sql`: `pg_dump --schema-only --no-owner --no-privileges` of a database at Alembic `0018`, with `alembic_version` removed. Task 03 uses it.

### Replay contract for later tasks

Document in `tests/parity/README.md` how scenarios, steps and golden files work, and how comparison works:

- JSON is compared by value, ignoring key order.
- `"<timestamp>"` matches any value in the format from the timestamp rule.
- Timestamps from the scenario compare as instants.
- Numbers compare numerically.
- Framework 422 `msg` text is ignored, while `type`, `loc` and the shape are compared.
- Any other mismatch fails, unless a machine-readable entry in `tests/parity/allowed-differences.json` covers it. Every entry there must match a row in `differences.md`.

## Done when

- [ ] The manifest has exactly 575 rows.
- [ ] `record.py --verify` reproduces all golden files with no diff.
- [ ] Every route in `backend/app/routers/` appears in at least one scenario (the script checks this against FastAPI's route table).
- [ ] `STATUS.md` is updated, and the branch is pushed.
