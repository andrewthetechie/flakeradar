# 30 — Docs, lints and compliance markers

Read `00-fixes-context.md` first. Behaviour stays fixed. Findings: `H9`, `H10`, `B7`, `H11`, `H12` (the rest), `H15`, `H16`, `H18` (the docs half), `J17` (`cargo doc`, `M-MSRV`).

This task runs last among the code tasks because it describes the final code.

## Contracts

- **Comments (`H10`, `B7`).** Rustdoc and comments describe end-state behaviour. Every mention of Python, FastAPI, Pydantic, Starlette, asyncpg, fastmcp, "the frozen server", `backend/`, task numbers and plan files leaves `crates/`. Keep a real reason as a statement about behaviour, and move parity notes to `docs/plans/rust-rewrite/`.
- **Lint overrides (`H9`).** The `mcp.rs` opt-out goes, with plain string literals. Add crate-level `cfg_attr(test, expect(...))` to `flakeradar-db` and `flakeradar-test-support`, and drop the module-level ones.
- **Magic values (`H12`).** Inline limits become named constants, and each constant's doc says why that value, what breaks if it changes and what it interacts with.
- **Crate shape (`H15`).** One path per item. Versions and sibling crates come from `[workspace.dependencies]`. `schemars` and `testcontainers` go if unused, and `CONTRIBUTING.md` says what really starts Postgres. Add `rust-version`.
- **Logs (`H16`).** Structured fields and named events, with no pre-formatted messages.
- **Wrong docs (`H18`).** Fix each one listed, including the `0.5` in `docs/agents/code-standards-rust.md` (the default is `0.30`). Add `cargo doc` with warnings denied to CI.
- **Markers (`H11`).** Remove all compliance markers, then check each file against the guidelines and add its marker back only if it passes.

## Done when

- [ ] `grep -rnEi 'python|fastapi|pydantic|starlette|asyncpg|fastmcp|backend/|frozen|task [0-9]+' crates --include='*.rs'` shows only names that describe real inputs, each justified in your status note.
- [ ] `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --all-features --document-private-items` passes, locally and in CI.
- [ ] Your status note lists every `.rs` file with no marker, and why.
- [ ] The shared rules in `00-fixes-context.md` are met.
