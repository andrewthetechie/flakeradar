# 00 — Shared context for the nextest-flaky-fail plan

**Read this whole file before you start any task in `docs/plans/nextest-flaky-fail/`.** Every task file assumes you know what is here. Each task file gives the exact contracts for its own step. This file gives the ground rules, the vocabulary, the decisions and the current code that all the tasks share.

## What we are fixing

GitHub issue #8: "Parsing: nextest flaky-fail retries are recorded as two failures".

cargo-nextest can retry a test, and with `flaky-result = "fail"` it fails a test that passed only on retry. In its JUnit report, nextest then writes:

- each earlier, failed try as a `<flakyFailure>` (or `<flakyError>`) child of the `<testcase>`;
- the final try, **which passed**, as the testcase's own `<failure type="flaky failure" message="test passed on attempt 2/2 but is configured to fail when flaky"/>`.

FlakeRadar reads the testcase's own `<failure>` as the outcome of the last try. So it stores two failed Executions and no pass. Consequences today:

- No pass and fail on the same SHA, so the Test is never a **Proven flake** and scores `0.0` (`stable`).
- `GET /api/ci-runs/{id}/failures` reports the Test as `status: "failed"`, `failed_attempts: 2`. A merge gate reads that as a real failure, not a known flake.

The fix: when the testcase's own `<failure>`/`<error>` is nextest's flaky marker **and** the testcase has at least one `<flakyFailure>`/`<flakyError>` child, store that last try as `passed`. Then the existing same-SHA rule records a Proven flake. A data migration corrects Executions already stored.

**Verified nextest facts** (source: `nextest-rs/nextest`, `nextest-runner/src/reporter/aggregator/junit.rs` and `nextest-runner/src/config/elements/retry_policy.rs`, checked 2026-10-16):

- The marker message is built as `format!("test passed on attempt {attempt}/{total_attempts} but is configured to fail when flaky")`.
- nextest writes the marker as `<failure>` with `type="flaky failure"`. It never writes it as `<error>`. We still accept `<error>` (cheap, and safe).
- `flaky-fail-status = "failure"` is nextest's **default**. With `flaky-fail-status = "success"`, nextest writes a plain pass and FlakeRadar already handles it.
- The earlier tries are written as `<flakyFailure>`/`<flakyError>` with a `message` attribute (the short panic line), the full error as the element's **text**, and `<system-out>`/`<system-err>` children.
- The testcase's own `time` is the time of the last (passing) try.

## Delivery rules (read carefully)

- **Branch:** all tasks land in order on `fix/nextest-flaky-fail`. Create it from `main` if it does not exist yet (`git switch -c fix/nextest-flaky-fail main`), otherwise `git switch fix/nextest-flaky-fail`. Commit each task as one or more commits on that branch. Do not merge to `main`. The whole series ships as **one PR**, and task 04 opens it.
- **Order:** tasks run **one after another** in number order (01 … 04). You can assume every task with a lower number has landed.
- **Keep the suite green.** Every task must leave these passing:
  ```bash
  cd backend && .venv/bin/python -m pytest -q          # needs Docker (testcontainers Postgres 17)
  cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
  ```
  Use `uvx ruff@0.16.9` (or `pip install ruff==0.16.9`). An older system `ruff` fails on `ruff.toml` with `unknown field 'lint'`. No task touches `frontend/`.
- **Do not edit** `CONTEXT.md`. Its **Execution** entry already says the outcome is what the try did. Task 04 edits `docs/adr/0005-retry-attempts-from-junit.md`. No other ADR changes, and no new ADR.
- **Commit messages** end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. If you are a different model, use your own name in the same format.
- **If a contract in a task file is wrong** (for example, a library behaves differently), do the smallest correct thing, keep the public names, and say what you changed in the commit message.
- **Match the surrounding code.** Comment density, naming and idiom like `backend/app/parsing.py` and the existing tests. No ORM relationships: every read is an explicit `select()`/join.

## Domain vocabulary (from `CONTEXT.md`: use these words in code, docs and error text)

| Term | Meaning | Avoid |
|---|---|---|
| **Test** | One logical test case in one Repo and Project. Table `test_cases`, model `TestCase`. | test case (in user-facing text) |
| **Report** | One upload from CI. A JUnit report becomes a Run. Table `reports`. | upload |
| **Run** | The processed result of **one JUnit Report**. Table `test_runs`, model `TestRun`. | build |
| **Execution** | One try of a Test inside one Run, with `attempt` numbered **from 0**. Its outcome is what that try did. Table `test_executions`, model `TestExecution`. | result |
| **Proven flake** | A commit SHA on which the same Test both passed and failed. Column `test_cases.confirmed_flake_count`. | confirmed flake (in user-facing text) |
| **Explained failure** | A failed Job execution where a Run with the same `ci_job_id` in the same Repo has a failed or errored Execution. Rule in `backend/app/attribution.py`. | |
| **CI run / CI run attempt** | One run of a Pipeline in the CI provider (`ci_run_id`), and one try of it (`ci_run_attempt`, from 1). | "run" or "attempt" alone |

Use "nextest flaky marker" (or "the marker") in comments for the message `test passed on attempt N/M but is configured to fail when flaky`. Write "cargo-nextest" or "nextest" in prose.

## Decisions (the ledger): treat as requirements

**Detection (parse time, `backend/app/parsing.py`)**
- A testcase's own outcome is the nextest flaky marker when its status is `failed` or `error` **and** its message (after strip, as `_outcome` returns it) fully matches `^test passed on attempt \d+/\d+ but is configured to fail when flaky$`. The `type` attribute is ignored (older nextest versions may not write it).
- The marker is converted **only** when the same `<testcase>` has at least one `<flakyFailure>` or `<flakyError>` child (`RETRY_BEFORE`). A marker without them stays `failed`/`error`, so a Run never loses its only failure. (A red Job with no failing Test would count as unexplained.)
- The converted try becomes a `ParsedCase` with `status="passed"`, `message=""`, `details=""`, and `duration` = the testcase `time` (as today). Its `file`/`line` are unchanged. The marker text is not stored anywhere.
- `RETRY_BEFORE` and `RETRY_AFTER` handling is unchanged.

**What follows without code changes** (checked in code; tasks 01 and 03 add tests for it)
- `processing._parse_and_classify` classifies only `FAILING` cases, so the converted try gets `failure_category = None`.
- `processing.process_report` sets `TestCase.last_status` from the last parsed case of the Test, so it becomes `passed`. Report `counts` get one `passed` in place of one `failed`.
- `scoring` sees a fail (attempt 0) and a pass (attempt 1) on one SHA, so the Test gets a Proven flake.
- `attribution.explained_clause` / `explained_ci_job_ids` look for **any** failing Execution in the Run. Attempt 0 is still `failed`, so the Job failure stays **explained**. No change to `attribution.py`.
- `queries._ci_run_test_failures` lists the (Test, Run) pair because attempt 0 fails. `status` = last Execution → `"passed"`. `failed_attempts` = `1`. `message` / `failure_category` come from attempt 0. No change to `queries.py`.

**Existing data (migration `0009`)**
- A data-only Alembic migration rewrites Executions already stored with the marker: `status IN ('failed','error')`, `attempt > 0`, message matches the marker regex, **and** an Execution of the same Test in the same Run with a lower `attempt` has `status IN ('failed','error')`. Set them to `status='passed'`, `message=''`, `details=''`, `failure_category=NULL`.
- No rescore in the migration. `processing.rescore` recomputes a Test from its stored Executions, so each affected Test's score, Proven flake count, Clean streak and Failure category fix themselves the next time a Report includes that Test.
- Not corrected (accepted): past `test_score_history` rows, old `reports.counts`, and `test_cases.last_status` until the Test's next Report.
- `downgrade()` does nothing: the old values cannot be restored, and the new ones are correct.

**Docs**
- ADR 0005 gains nextest in its context and one Consequences bullet about the marker.
- README `## Retries` gains a cargo-nextest row and one paragraph about `flaky-result = "fail"`.

## Current code you will touch (facts, checked 2026-10-16 on `main` @ `c9b4a12`)

- `backend/app/parsing.py`: pure functions, no database. Imports `hashlib`, `xml.etree.ElementTree as ET`, `dataclass`, and `from junitparser import Error, Failure, JUnitXml, Skipped, TestSuite`. **No `re` import yet.** Module constants `MESSAGE_MAX`, `DETAILS_MAX`, `TRUNCATION_MARKER`, `RETRY_BEFORE = ("flakyFailure", "flakyError")`, `RETRY_AFTER = ("rerunFailure", "rerunError")`. Functions `_outcome(case) -> (status, message, details)`, `_retry_outcome(elem)`, `parse_junit_xml(content) -> list[ParsedCase]`. In `parse_junit_xml`, per case: `children = list(case._elem) ...`, then `RETRY_BEFORE` cases, then `status, message, details = _outcome(case)` and the case's own `ParsedCase`, then `RETRY_AFTER` cases.
- `backend/app/processing.py`: `_parse_and_classify`, `process_report` (assigns `attempt` per Test in parse order, from 0), `rescore`.
- `backend/app/scoring.py`: `FAILING = {"failed", "error"}`.
- `backend/migrations/versions/`: latest is `0008_ci_run_indexes.py` (`revision = "0008"`, `down_revision = "0007"`). `0002_ci_jobs.py` shows the data-migration style: `op.execute("UPDATE ...")`.
- `backend/tests/test_db.py::test_migrations_are_idempotent` ends with `assert version == "0008"`. `test_upgrade_backfills_report_repo_id` shows how a data migration's SQL is tested against seeded rows.
- `backend/tests/conftest.py`: fixtures `engine`, `session_factory`, `db` (an `AsyncSession`), `client` (httpx against the app; **no worker runs**). `client` and `db` share `session_factory`, so a test can process Reports with `process_next(session_factory)` and then call `client`.
- `backend/tests/factories.py`: `make_project(db, repo, project)`, `make_test_case(db, project, name=..., ...)`, `make_run(db, project, commit_sha=..., ci_run_id=..., ci_job_id=..., ci_run_attempt=...)`, `make_execution(db, tc, run, status=..., message=..., details=..., attempt=..., failure_category=...)`, `make_report(db, project, body, commit_sha=..., branch=..., ci_run_id=..., ci_job_id=..., ci_run_attempt=..., default_branch=...)`, `make_pipeline_report(db, repo, report_json)`. Factories flush but do not commit: call `await db.commit()` before `process_next` or `client`.
- There is **no** `backend/tests/fixtures/` directory yet. Task 01 creates it.
