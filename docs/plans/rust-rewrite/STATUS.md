# Rust rewrite status

Branch: `feat/rust-rewrite`. Python reference: commit `97b933d`. Tasks run strictly in order.

Set `Done` and the last commit SHA when a task lands. If you are blocked, write `BLOCKED: <question>` in the Notes column and stop.

| Task | Title | State | Commit | Notes |
|---|---|---|---|---|
| 01 | Workspace, lints, nextest, CI skeleton, code standards | Done | f95dbcf | `cargo nextest` needs `--no-tests=pass` until task 03 adds the first tests. For the same reason the coverage job keeps its `has-coverage` guard (cargo-llvm-cov cannot export a report when no test has run) and prints a notice instead. Task 03 removes both. Rust coverage of added code: n/a, the crates hold doc comments only and `main` is `coverage(off)` (logged). |
| 02 | Golden recorder and pytest manifest | Done | a305157 | `test-manifest.csv` has exactly 575 rows (573 collected after de-duplication + the 2 tests `test_report_action.py` skips at module level), all `pending`; task 02 owns none of them, so the checker passes at `--upto 2`. `tools/parity-record/record.py --verify` reproduces all 367 goldens (366 JSON + `schema.sql`) with no diff on two consecutive runs. The three `time` cases Python accepts that used to share the aborting `junit/invalid-time.xml` (`""`, `-1.5`, `0.000123456789`) each have their own corpus file and golden now (`empty-time.xml`, `negative-time.xml`, `fractional-time.xml`), so the behaviour `differences.md` promises for them is recorded. 13 scenarios cover all 23 API paths and all 11 MCP tools. Task 02 adds **no Rust tests**, so `cargo nextest --no-tests=pass` and the coverage job's `has-coverage` guard stay until task 03, and Rust coverage for this task is n/a. Two intended differences are in differences.md with narrow allowances in `tests/parity/allowed-differences.json`: DECIDED 2026-11-04 (user): a non-numeric `<testcase time>` 500s in Python (unhandled junitparser ValueError) and Rust MUST validate it and answer **400** — missing/empty/negative/fractional `time` keep Python's behaviour. The second is the suffix of a JUnit parse-error detail (ElementTree vs roxmltree wording; the prefix and the 422 stay). UNVERIFIED: none. |
| 03 | Baseline schema, entities, test databases, SeaORM spike | Pending | | |
| 04 | Core logic | Pending | | |
| 05 | Server shell | Pending | | |
| 06 | Ingest and Report routes | Pending | | Ingest MUST validate a `<testcase time>` that is not a number and answer **400** (user decision 2026-11-04, see differences.md and `tests/parity/golden/ingest-junit/07.json`). Missing, empty, negative and fractional `time` keep Python's behaviour. |
| 07 | Processing, attribution, Score history | Pending | | |
| 08 | Worker and retention | Pending | | |
| 09 | Read APIs | Pending | | |
| 10 | Issue links, GitHub, webhooks, repo admin | Pending | | |
| 11 | MCP | Pending | | |
| 12 | Frontend embedding | Pending | | |
| 13 | CLI, Docker, release, five-target smoke test | Pending | | |
| 14 | Convergence | Pending | | |
| 15 | Cutover and the PR | Pending | | |
