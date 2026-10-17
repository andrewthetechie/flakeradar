# 03 — The flaky-fail Test explains its Job and is a known flake in its CI run

## Tracer-Bullet Outcome
One end-to-end test shows the issue #8 case after the fix. A nextest report with the flaky marker, plus a red Job in the same CI run, gives: the Job failure is **explained**, and `GET /api/ci-runs/{id}/failures` lists the Test with `status: "passed"`, `failed_attempts: 1` and a Proven flake.

## User Story
As the merge gate (fabro), I want a red CI run whose only failure is a nextest flaky-fail retry to show as a known flake, not as a real failure.

## Description
Tests only. No change to `attribution.py` or `queries.py`. If the test fails, the bug is in task 01's parser change or in this test, not in those modules. See "If the test fails" below.

## Context Pack
- Source decisions: `00-shared-context.md`, "What follows without code changes".
- Repo facts:
  - `backend/tests/test_ci_runs_api.py` has a `_url(ci_run_id, repo="acme/app", **params)` helper and imports `AUTH, make_junit` from `tests.conftest` and factories from `tests.factories` (including `make_pipeline_report`, `make_project`, `make_report`).
  - `backend/tests/test_attribution.py` builds a Pipeline report payload like this (copy it into the new test as a local dict; do not import from another test module):
    ```python
    {
        "repo": "acme/app",
        "provider": "github",
        "pipeline": ".github/workflows/ci.yml",
        "commit_sha": sha,
        "branch": "main",
        "default_branch": "main",
        "ci_run_id": ci_run_id,
        "ci_run_attempt": 1,
        "jobs": [{"ci_job_id": "J1", "name": "test", "status": "failed", "url": "", "runner_name": "", "runner_labels": []}],
    }
    ```
    and seeds it with `await make_pipeline_report(db, "acme/app", payload)`.
  - `process_next(session_factory)` (from `app.processing`) processes one pending Report and commits.
  - `app.attribution.explained_ci_job_ids(db, repo_id, ["J1"]) -> set[str]`.
  - Task 01 created `backend/tests/fixtures/nextest_flaky_fail.xml`. Its flaky Test is named `embedding::reindexer::worker::tests::invalidate_priority_readiness_drops_from_each_driver`, classname `writers-app`. Its attempt-0 message starts with `thread 'embedding::reindexer`.
- Non-goals: changing endpoint fields, MCP tool text or attribution rules.

## Delivery Strategy
- Shape: Normal tracer bullet (verification of existing behavior after task 01)
- Valid-state scope: `fix/nextest-flaky-fail`

## Implementation Contract
- Expected files: `backend/tests/test_ci_runs_api.py`.
- Interfaces and names: one new test, `test_nextest_flaky_fail_is_a_known_flake_in_its_ci_run(client, db, session_factory)`.
- Behavior rules (the test's steps):
  1. `proj = await make_project(db, "acme/app", "default")`.
  2. `await make_report(db, proj, fixture_bytes, commit_sha="777022bf", ci_run_id="38994487107", ci_job_id="J1")`. Read the fixture with `FIXTURES = Path(__file__).parent / "fixtures"`, as in task 01.
  3. `await make_pipeline_report(db, "acme/app", payload)` with the payload above, `sha="777022bf"`, `ci_run_id="38994487107"`, job `J1` `failed`.
  4. `await db.commit()`, then `await process_next(session_factory)` twice (one per Report).
  5. Assert `await explained_ci_job_ids(db, proj.repo_id, ["J1"]) == {"J1"}`.
  6. `body = (await client.get(_url("38994487107"))).json()`.
  7. Assert `body["reports"] == {"pending": 0, "processed": 2, "failed": 0}`.
  8. `(entry,) = [f for f in body["failures"] if f["name"].endswith("invalidate_priority_readiness_drops_from_each_driver")]`. Assert `entry["status"] == "passed"`, `entry["failed_attempts"] == 1`, `entry["message"].startswith("thread 'embedding::reindexer")`, `entry["confirmed_flake_count"] == 1`, `entry["ci_job_id"] == "J1"`.
  9. Assert `len(body["failures"]) == 1` (the passing Test is not listed).
  10. `(job,) = body["failed_jobs"]`. Assert `job["outcome"] == "explained"` and `job["ci_job_id"] == "J1"`.
- Error and security rules: None.

## If the test fails
- `failed_attempts == 2` or `status == "failed"`: task 01's parser change is not applied to this fixture. Fix `parsing.py`, not `queries.py`.
- `outcome == "unexplained"`: the attempt-0 Execution is not `failed`. Check that task 01 converts **only** the testcase's own marker and leaves the `<flakyFailure>` try as `failed`.
- `confirmed_flake_count == 0`: the Run has no pass. Same cause as the first point.
- A `reports` count that differs: check that each `make_*_report` call used `ci_run_id="38994487107"` and that both Reports were processed.

## Steps
1. Write the test.
2. Run it (`.venv/bin/python -m pytest -q tests/test_ci_runs_api.py -k nextest`), then run the Validator.

## Acceptance Criteria
- [ ] The new test passes.
- [ ] No file other than `backend/tests/test_ci_runs_api.py` changed.
- [ ] The whole backend suite and ruff pass.

## Test Expectations
- The single test above.

## Validator
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```

## Dependencies
- Blocked by: 01.
- Blocks: 04.

## Labels
`test`, `backend`, `priority:medium`
