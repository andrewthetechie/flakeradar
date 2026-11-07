# 38 — Comments, compliance markers and the hygiene guard

Read `00-fixes-2-context.md` first. Findings: `ST1`, `ST2`, `ST3`, `ST29`, `CC1`, `CC16`, `ST13` (falsely closed `H10`, `H11`, `B7`, `C4`, `J17` M-MSRV). Text and tests only.

## Contracts

**Docs that contradict the code (`ST29`, `CC16`).** Correct `worker.rs:626-628` (a claim in flight runs on to its commit; it is not rolled back), `webhooks.rs:46-47` (the wait is capped at ten seconds and held, up to 35 s per endpoint), and `extract.rs:6-9` (only the query goes through `serde`; the body goes through the model's `from_object`). If tasks 34–36 changed these paths, describe the code as it stands now.

**Comments describe this program (`ST2`, `CC1`).** Rewrite the 69 lines that say "the reference" (22 production, 47 test) and every line naming deleted code (`reports.py`, `_clear_if_closed`, `ElementTree`, `_window_failures`) as an end-state rule. For example, `sync.rs:84` becomes "Every answered row is stamped; a rate-limit stop or transport error leaves it unstamped, so it is retried first." Rationale moves to `differences.md`.

- Remove journal voice ("used to", "has always carried").
- Fix `routes/reports.rs:405-407`, whose false claim survives from `C4`: `accepted_ci_run_attempt` checks nothing.
- Rename the six test names carrying "reference".
- `lax.rs` and `parsing.rs` may name Pydantic or Python where the behaviour's definition needs it, and each such line says why.

**The guard catches what it claims (`ST3`).** `tests/source_hygiene.rs` passes on the violations it names. The reviewer proved that with a scratch tree: `use tracing::info`, a pre-built `format!`, inline `{x}`, "the reference" and `tasks 16` all pass. Either fix all of these or delete the tests:

- Detect the `info!` family with or without the path, and `event!`.
- Fail on a message holding `{ident}`. Task 39 removes the five live ones, so land the check there if you need to.
- Add `the reference`, `.py` and `docstring` to the banned list.
- Fix the task-number scan so every occurrence on a line is checked.
- Fail an `ALLOWED` entry that no longer matches anything.

Commit a negative-control test that feeds each violation through the checker function.

**Markers mean something (`ST1`).** `code-standards-rust.md:18`: "a stale marker is worse than none." All 150 files carry one. Remove the marker from every file a `review-135b` finding names, and add it back only in the commit that fixes that file. At the handoff, the list of unmarked files is in `STATUS.md`, with a reason per file.

**`ST13`.** Members inherit `rust-version` (`rust-version.workspace = true`), so `cargo metadata` reports it.

## Done when

- [ ] A reader can follow every comment in `crates/*/src` with `backend/` absent. Say how you checked this: read the files the findings name, not only a grep.
- [ ] The guard's negative controls pass, and each one failed before its fix.
- [ ] `cargo metadata --no-deps --format-version 1` shows `rust_version` for all four packages.
- [ ] `git diff` of `crates/*/src` for this task changes comments and doc comments only. `tests/` may also change names and the guard.
- [ ] The shared rules are met.
