# 01 — Parse the nextest flaky marker as a pass

## Tracer-Bullet Outcome
A nextest JUnit report with `flaky-fail-status = "failure"` parses to attempt 0 `failed` and attempt 1 `passed`. Processing that report gives the Test a Proven flake from one Run.

## User Story
As a maintainer of a Rust repo that runs nextest with `flaky-result = "fail"`, I want FlakeRadar to see that my test passed on retry, so the Test scores as flaky and the CI-run gate treats it as a known flake.

## Description
One parser change in `backend/app/parsing.py`, one fixture file, and tests in `test_parsing.py` and `test_processing.py`. No change to processing, scoring, attribution or queries.

## Context Pack
- Source decisions: `00-shared-context.md`, "Detection" and "What follows without code changes".
- Repo facts:
  - `parse_junit_xml` in `backend/app/parsing.py`, the loop body for one case today:
    ```python
            # Retries that ran before the final outcome, then the case's own
            # outcome, then retries that ran after it (document order).
            children = list(case._elem) if case._elem is not None else []
            for c in children:
                if c.tag in RETRY_BEFORE:
                    status, message, details, duration = _retry_outcome(c)
                    parsed.append(
                        ParsedCase(suite_name, classname, name, status, duration, message, details, file, line)
                    )
            status, message, details = _outcome(case)
            parsed.append(
                ParsedCase(suite_name, classname, name, status, float(case.time or 0.0), message, details, file, line)
            )
    ```
  - Module constants block (after `TRUNCATION_MARKER`):
    ```python
    # Retry elements Playwright (1.59+, includeRetries) and Surefire write as
    # children of a <testcase>, which junitparser does not model. `flaky*` ran
    # before the final outcome; `rerun*` ran after the first (failed) one.
    RETRY_BEFORE = ("flakyFailure", "flakyError")  # attempts before the final outcome
    RETRY_AFTER = ("rerunFailure", "rerunError")  # attempts after the first (failed) one
    ```
  - `backend/tests/test_parsing.py` has `test_flaky_failure_becomes_a_failed_attempt_before_the_pass` and `test_rerun_elements_follow_the_failure`. Tests there build XML inline and call `parse_junit_xml(xml.encode())`.
  - `backend/tests/test_processing.py` has `test_flaky_retry_is_proven_flake_from_one_report`, which uses the helper `_queue(db, body, project=None, **kw)` and `process_next(session_factory)`, then checks `outcome.counts`, `confirmed_flake_count` and the `(status, attempt)` rows. Copy that shape.
- Non-goals: reading the text of `<flakyFailure>` elements into Failure details (today `_retry_outcome` reads only `<stackTrace>`; nextest puts the error in the element text). Changing `_retry_outcome`. Any migration (task 02).

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `fix/nextest-flaky-fail` (create it from `main` in this task)

## Implementation Contract
- Expected files: `backend/app/parsing.py`, `backend/tests/fixtures/nextest_flaky_fail.xml` (new), `backend/tests/test_parsing.py`, `backend/tests/test_processing.py`.
- Interfaces and names:
  - `parsing.py`: add `import re` (stdlib imports stay sorted: `hashlib`, `re`, `xml.etree.ElementTree as ET`). After `RETRY_AFTER` add:
    ```python
    # cargo-nextest with `flaky-result = "fail"` writes the final try of a flaky
    # test, which passed, as the testcase's own <failure> with this message. The
    # failed tries before it are <flakyFailure>/<flakyError> children.
    NEXTEST_FLAKY_MARKER = re.compile(r"test passed on attempt \d+/\d+ but is configured to fail when flaky")
    ```
  - In `parse_junit_xml`, right after `status, message, details = _outcome(case)`:
    ```python
            if (
                status in ("failed", "error")
                and NEXTEST_FLAKY_MARKER.fullmatch(message)
                and any(c.tag in RETRY_BEFORE for c in children)
            ):
                status, message, details = "passed", "", ""  # the nextest flaky marker: that try passed
    ```
    Use `fullmatch` (anchored at both ends). Do not import `scoring` into `parsing.py`: it stays free of app imports.
  - Fixture `backend/tests/fixtures/nextest_flaky_fail.xml`, exactly this content (the shape nextest writes; the failing test is the one from issue #8):
    ```xml
    <?xml version="1.0" encoding="UTF-8"?>
    <testsuites name="nextest-run" tests="2" failures="1" errors="0" timestamp="2026-10-15T18:02:11.104+00:00" time="3.517">
        <testsuite name="writers-app" tests="2" disabled="0" errors="0" failures="1">
            <testcase name="embedding::reindexer::worker::tests::invalidate_priority_readiness_drops_from_each_driver" classname="writers-app" timestamp="2026-10-15T18:02:12.950+00:00" time="0.412">
                <failure type="flaky failure" message="test passed on attempt 2/2 but is configured to fail when flaky"/>
                <flakyFailure timestamp="2026-10-15T18:02:12.530+00:00" time="0.398" type="test failure" message="thread &apos;embedding::reindexer::worker::tests::invalidate_priority_readiness_drops_from_each_driver&apos; (7136) panicked at src-tauri/src/embedding/reindexer/worker.rs:2706:17">thread &apos;embedding::reindexer::worker::tests::invalidate_priority_readiness_drops_from_each_driver&apos; (7136) panicked at src-tauri/src/embedding/reindexer/worker.rs:2706:17:
    assertion failed: readiness.is_empty()
                    <system-out>
    running 1 test
    test embedding::reindexer::worker::tests::invalidate_priority_readiness_drops_from_each_driver ... FAILED
    </system-out>
                    <system-err>thread &apos;embedding::reindexer::worker::tests::invalidate_priority_readiness_drops_from_each_driver&apos; (7136) panicked at src-tauri/src/embedding/reindexer/worker.rs:2706:17:
    assertion failed: readiness.is_empty()
    note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
    </system-err>
                </flakyFailure>
            </testcase>
            <testcase name="embedding::chunker::tests::splits_on_headings" classname="writers-app" timestamp="2026-10-15T18:02:11.230+00:00" time="0.003">
            </testcase>
        </testsuite>
    </testsuites>
    ```
    The `<?xml` line must be the first line of the file (no leading blank line or indent).
  - Test helper, in **both** test files: `FIXTURES = Path(__file__).parent / "fixtures"` and read the file with `(FIXTURES / "nextest_flaky_fail.xml").read_bytes()`.
- Behavior rules: as in `00-shared-context.md`, "Detection".
- Error and security rules: None. The regex runs on the message, which is already capped at `MESSAGE_MAX`.

## Steps
1. `git switch -c fix/nextest-flaky-fail main`.
2. Write the failing tests below, and the fixture file. Run them and see them fail.
3. Make the `parsing.py` change.
4. Run the Validator.

## Acceptance Criteria
- [ ] Parsing the fixture gives, for the flaky test, `[("failed", ...), ("passed", ...)]` in that order, and one `passed` case for `splits_on_headings`.
- [ ] Processing the fixture gives the flaky Test `confirmed_flake_count == 1`, Executions `[("failed", 0), ("passed", 1)]`, `last_status == "passed"`.
- [ ] A marker without `flaky*` children stays failed. A message that only starts like the marker stays failed.
- [ ] Every existing test still passes. The whole backend suite and ruff pass.

## Test Expectations
`backend/tests/test_parsing.py` (new tests; the names are part of the contract):
- `test_nextest_flaky_marker_is_a_passed_attempt`: parse the fixture. Keep only the cases whose `name` ends with `invalidate_priority_readiness_drops_from_each_driver`. Assert `[c.status for c in cases] == ["failed", "passed"]`. `cases[0].message` starts with `"thread 'embedding::reindexer"` and ends with `"worker.rs:2706:17"`. `cases[0].duration == 0.398`. `cases[1].message == ""`, `cases[1].details == ""`, `cases[1].duration == 0.412`. Also assert the other test parses to one case with status `"passed"`.
- `test_nextest_flaky_marker_in_error_is_a_passed_attempt`: inline XML, a testcase with `<error message="test passed on attempt 3/3 but is configured to fail when flaky"/>` and two `<flakyError message="boom"/>` children → statuses `["error", "error", "passed"]`.
- `test_nextest_flaky_marker_without_earlier_tries_stays_failed`: inline XML, a testcase with only `<failure message="test passed on attempt 2/2 but is configured to fail when flaky"/>` → statuses `["failed"]`, and the message is kept.
- `test_message_that_only_starts_like_the_marker_stays_failed`: inline XML, a `<flakyFailure message="x"/>` child plus `<failure message="test passed on attempt 2/2 but is configured to fail when flaky, and more"/>` → statuses `["failed", "failed"]`.

`backend/tests/test_processing.py` (new test):
- `test_nextest_flaky_fail_is_proven_flake_from_one_report`: `await _queue(db, fixture_bytes, commit_sha="777022bf")`, then `outcome = await process_next(session_factory)`. Assert `outcome.counts == {"passed": 2, "failed": 1, "error": 0, "skipped": 0, "retried": 1}`. Load the flaky `TestCase` by its `name`. Assert `confirmed_flake_count == 1`, `flakiness_score >= 0.6` (the same bound as `test_flaky_retry_is_proven_flake_from_one_report`) and `last_status == "passed"`. Select `(status, attempt, failure_category)` of its Executions ordered by `id`. Assert the rows are `("failed", 0, <any>)` then `("passed", 1, None)`.

## Validator
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```

## Dependencies
- Blocked by: None.
- Blocks: 02, 03, 04.

## Labels
`bug`, `backend`, `priority:high`
