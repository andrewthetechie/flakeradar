# 14 — Convergence

Read `00-shared-context.md` first. Tasks 01–13 have landed.

## Goal

Prove the Rust server is complete before Python is deleted, and get coverage as high as this project can reach. **If this task finds a bug, fix it here, and add a test that would have caught it.**

## Deliverables

### Completeness

- `node scripts/check-test-manifest.mjs --upto 14`: no `pending` rows. Review every `na` reason, and port anything that still applies.
- Every scenario in `tests/parity/scenarios/` is in `ENABLED`. Remove the `ENABLED` list so new scenarios run by default.
- Every entry in `tests/parity/allowed-differences.json` has a row in `differences.md`, and the reverse. Reread `differences.md`: any entry that removes or renames a field, or changes a status code, must have the user's approval on record.
- Compare `/openapi.json` with the FastAPI spec from task 02's recording: same paths and methods, same parameters, and same response field names. Save a short result in `differences.md`.

### Consumer checks

Add `crates/flakeradar/tests/consumers.rs`, using real payloads:

- **writers-app:** the `curl` request from `ingest-flakeradar/action.yml`, and the JSON from `flakeradar-jobs.yml`.
- **Fabro:** the struct and timestamp checks from task 09, including `normalise_ts`.

### Actions and samples in CI

Python tests for these move out of `backend/tests` now.

- **Report action job:** replaces `test_report_action.py`. A CI job starts the real binary against a Postgres service, runs `./actions/report` on `actions/report/test/fixture-*.xml`, and asserts that the `report_ids` output has one id per file and every Report becomes `processed`.
- **Summary action job:** rewrite the `summary-action` job to use the real binary. **Delete `actions/summary/test/seed_fixture.py`.** Seed through `actions/report` instead: upload pass-and-fail for `test_known_flaky` on three commits, so it has three Proven flakes and is `flaky`, then a failing report for `test_new_failure` in the fixture CI run. Update the `grep` assertions to the values the real pipeline produces, keeping the same intent ("1 of 2 failures are known flaky tests", one `flaky` row with 3 proven, one `stable` row).
- **Samples job:** move `backend/tests/test_gitlab_reporter.py` to `samples/gitlab/test_report_pipeline.py`. Replace its `PipelineReportIn` validation by POSTing the report it builds to the real binary's `/api/ingest/pipeline` and expecting 202. Run it in a small Python job that needs only `pytest`, with the binary's URL in an env var. Also run `samples/simulate_ci.py` against the binary, and check the demo data appears. Set the manifest rows for these two files.

### Performance

- Rerun the queries in `docs/perf/20-test-rescore-explain.md` and `21-job-rescore-explain.md` with `EXPLAIN (ANALYZE, BUFFERS)` on a realistic data set. Confirm the same index use, and add a "Rust" section to each doc.
- Ingest a 2,500-test report with concurrent reads running, and confirm the event loop stays responsive (health responds under 100 ms) and parsing runs off the async threads.
- Compare against Python on the same data set: time to process 100 reports, and peak memory. Record the numbers in `docs/perf/`. A regression beyond 20% in processing time needs a fix or a recorded reason.

### Reliability checks

- Kill the leader with SIGKILL during processing. The standby takes over, and the Report still reaches a final state.
- Restart Postgres during ingest. The server recovers with no restart, and reads return errors rather than hanging.
- Run two instances against one database. There is one leader, and no duplicate Runs or issues.

### Coverage push

**Goal: as close to 100% as possible.**

1. Run the coverage command from `00-shared-context.md` and sort by missed lines and branches.
2. Work through `coverage-gaps.md` and the report. Add tests, or restructure code to make it testable with injected seams. Do not delete behaviour to gain coverage.
3. Run the suite three times, and diff the coverage summaries. They must be identical. Fix any line that varies by adding a test that reaches it every time. Do not add a tolerance.
4. Update `coverage-gaps.md` with what remains and why. Each remaining entry must be specific enough to become an issue.

## Done when

- [ ] All verification in `00-shared-context.md` passes, and all CI jobs are green.
- [ ] The manifest has no `pending` rows.
- [ ] Coverage is at least 95% lines and 80% branches, and identical across three runs. Report the figures in the commit message.
- [ ] Consumer, performance and reliability results are recorded.
- [ ] `STATUS.md` is updated, and the branch is pushed.
