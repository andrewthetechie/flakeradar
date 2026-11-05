# 22 — Parity evidence

Read `00-fixes-context.md` first. Findings: the "Weaknesses" list in `spec-findings.md`'s parity harness audit, and section (a) on the real MCP client, ledger ids `E1`–`E7`.

## Goal

The harness can now see the classes of bug tasks 16–21 fixed, and its allowances hide nothing.

## Contracts

- **Settings in scenarios (`E1`).** Scenarios can set settings, and at least one replay runs with a GitHub token, webhooks and non-default gates against a mock. Record new goldens only from the reference, using `review-135/repro/fake-github/`.
- **Failure paths (`E2`).** Scenarios reach a failed Report, a sync and a leadership drop. Add a Postgres restart test to the reliability suite, running in CI, with its assertion on a real filed issue.
- **Real listener (`E3`).** At least one replay runs over a real HTTP listener rather than `app.oneshot`, covering the writers-app `curl`.
- **Timestamps (`E4`).** `<timestamp>` placeholders check ordering and equality between clock values, where the golden shows them.
- **OpenAPI (`E5`).** `compare_openapi` checks types and required lists, not only names. Log any real difference.
- **Allowances (`E6`).** `allowed()` with no `pythonValue` or `rustValue` accepts any mismatch (`parity.rs:878-901`). Audit all 196 allowances, including the 179 MCP ones. Each one names its expected values, or the entry is narrowed or removed. Add a test that fails when an allowance and a `differences.md` row do not match, restoring the reconciliation the recorder used to do.
- **Real MCP client (`E7`).** Task 11's unmet criterion: drive `/mcp/` with an actual MCP client over a real listener through `initialize`, `tools/list`, a read tool, `link_issue` and `unlink_issue`. Use the official SDK or Inspector CLI in a CI-runnable test where you can, and record what you ran.

## Done when

- [ ] Reverting task 16's, 17's or 19's fix turns a harness or reliability test red. Show this for each by a local revert, and paste the failing line in your status note.
- [ ] No allowance lacks expected values.
- [ ] The reconciliation test passes, and fails when you remove a row in a local trial.
- [ ] `tests/parity/README.md` is true as written, including what the corpus still cannot reach.
- [ ] The shared rules in `00-fixes-context.md` are met.
