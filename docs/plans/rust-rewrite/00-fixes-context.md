# 00 — Shared context for the review fixes (tasks 16–31)

**Read this file and `00-shared-context.md` before your task.** `00-shared-context.md` still governs: the compatibility rule, the consumers, the Python trap table, verification and coverage. Where the two disagree, this file wins.

## Why these tasks exist

A review of PR #135 at `f9330fd` found defects CI cannot see: no parity scenario sets a GitHub token, restarts the database or runs a sync. These tasks fix every finding on the same branch, then the owner reviews again.

Each finding has an id and a row in `review-135/ledger.md`:

| Id | Source file in `review-135/` |
|---|---|
| `S1`–`S13` | `spec-findings.md`, numbered sections |
| `H0`–`H20`, `J1`–`J18` | `standards-findings.md` |
| `A1`–`A4`, `B1`–`B12`, `C1`–`C6`, `T1`–`T5` | `clean-code-findings.md` |

Read the sections your task names, then the code. `review-135/README.md` explains the repro scripts.

## Overrides to `00-shared-context.md`

- **Order:** tasks 16–31 run strictly in number order, one at a time.
- **PR:** #135 is open. Push to `feat/rust-rewrite`, which updates it. Task 31 updates the PR body. Merging belongs to the owner.
- **Python reference:** `backend/` is deleted. The reference is commit `86c98804`, which is `97b933d` plus main's #130 and #131. Extract it outside the repo:
  `mkdir -p "$TMP/python-base" && git archive 86c98804 backend | tar -x -C "$TMP/python-base"`
- **Test manifest:** finished. New tests need no manifest row.
- **Docs:** a task updates any doc its change makes stale. `CONTEXT.md` and existing ADRs stay as they are.

## How every task works

1. **Check the claim.** A finding is a reviewer's claim, with line numbers from `f9330fd`. Reread the code and the reference. If a claim is wrong, record your evidence in the ledger as `not reproduced` and move on.
2. **Go red.** Write a test that fails on the current code for the reason the finding gives. Save the failure line for your `STATUS.md` note.
3. **Fix, then go green.** Make the smallest change that satisfies the task's contract.
4. **Close the ledger rows** your task owns: `fixed` with the commit and test name, or `not reproduced` with evidence. Use `no change` only on rows the ledger already marks as accepted.
5. **Run the gate** in `00-shared-context.md`, read the coverage report for the files you touched, and commit.
6. **Pass the baton.** Mark the task `Done` in `STATUS.md` with its last commit SHA and push. The note says what you proved, how, and what you left unverified.

A task is done when every ledger row it owns is closed, the gate passes and its "Done when" list is checked.

### Tests that prove something

- Assert the outcome a user or the database sees: request counts on a counting mock, row state, return values. Log text is not an outcome.
- Stay deterministic. Hold a mock GitHub response on a channel or `Notify` to create "during the call" states, with no sleeps. A test that would hang on the old code is a valid red, because nextest's timeout ends it.
- Use the real worker in lifecycle tests, not `RecordingTask`.
- Commit messages and status notes claim only what a test proves. Commit `0a86d3a` said "a transport failure still stops a … sync … batch", which was false, and that is how `S4` shipped.

### What stays fixed

- Goldens are immutable. New goldens are recorded only from the Python reference.
- Tasks 24–30 change structure, not behaviour: no new entries in `tests/parity/allowed-differences.json` and no golden changes. A replay failure there is a bug in your change.
- Tasks 16–23 may change behaviour only to match the reference or as approved below, and log each change in `differences.md`.
- The lint set in `Cargo.toml` stays as strong. Coverage floors only rise.

## Approved behaviour decisions

The owner approved this table by commissioning this plan. Any other status-code change follows "When you are blocked" in `00-shared-context.md`. Each row becomes a `differences.md` row citing this file.

| Case | Decision |
|---|---|
| A crash in Python (500) on hostile input: nesting past the limit, identity strings past the budget, `page` overflow, NUL in text | Answer the 4xx the route already uses for that kind of bad input. A crash is not a contract |
| Pipeline `ci_run_attempt` past `int4` | 400, the same as ingest. Never saturate |
| `time="NaN"` or infinite in JUnit | 400, the same as non-numeric `time`. Negative values follow the recorded `negative-time` golden |
| `DOCTYPE` in JUnit | Restore Python: accepted. External entities stay unfetched, and an entity bomb gets a 4xx, never an abort |
| Pydantic lax coercion, multiple validation issues, query-int forms, multipart garbage header (`S11`, `S12`) | Restore Python |
| Log only, no code change: `HEAD` and `OPTIONS` answering 200, disallowed preflight 200 and missing `max-age`, `GET /api/` 307, `/api/tests/1.0/history` 422, ids past `int4` 404 or 422 | Add `differences.md` rows, and correct the `PUT /mcp/` row after measuring it with and without a bearer token |

Python's own answer wins wherever a row says "restore". Measure it from the reference and do not infer it from the finding.

## When a task is too big or wrong

Keep each commit small. If a contract proves impossible or a required change is outside this table, write `BLOCKED: <question>` in `STATUS.md` and stop.
