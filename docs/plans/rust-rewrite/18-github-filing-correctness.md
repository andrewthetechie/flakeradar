# 18 — GitHub filing correctness

Read `00-fixes-context.md` first. Findings: `A3`, `S13` (third bullet), `A2`, `S7`, `S8`, `J11`, `J13`, `C6`.

## Goal

What FlakeRadar files, closes and reports to its callers matches the reference, and every failure is told truthfully.

## Contracts

- **Filed link (`A3`).** `commit_filed_link` returns one of three outcomes: stored, row gone, or store failed with its error. Only "row gone" closes the issue and posts `CLOSE_COMMENT`. On "store failed", as in Python, the issue stays open, the batch stops, and an error log names the Repo and number.
- **Re-link (`A2`).** A link response reports the stored `github_issue_closed`. Remove the hard-coded parameter from `patched_out`.
- **Issue body (`S7`).** The sample search walks all failing rows, as Python's `_issue_body` does. Bodies match byte for byte.
- **Failure gate (`S8`).** `github_issue_min_failures` counts Python's overall window for Tests, not the union with the Default-branch window. Check the Job path the same way against `file_job_issues_for`.
- **HTTP client (`J11`).** A failed client build is an error at construction, for both GitHub and webhooks, never a client without a timeout.
- **Silent failures (`J13`, `C6`).** Every dropped error named in those findings is handled or logged with context.

## Repro

`S7`'s input is in `spec-findings.md`. Generate each expected body from the reference, not from Rust's output.

## Done when

- [ ] A forced store failure leaves the issue open, posts no comment, and stops the batch. A deleted row still closes the issue. Both are asserted on the mock.
- [ ] Re-linking a closed-as-not-planned issue answers `github_issue_closed: true` through REST and MCP.
- [ ] A byte-for-byte body test uses `S7`'s input with the reference's output as the expected value.
- [ ] A Test with feature-branch runs and older Default-branch failures follows Python's gate decision in a test.
- [ ] The shared rules in `00-fixes-context.md` are met.
