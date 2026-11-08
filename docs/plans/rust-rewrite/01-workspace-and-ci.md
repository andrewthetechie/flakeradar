# 01 — Workspace, lints, nextest, CI skeleton, code standards

Read `00-shared-context.md` first. This task needs no earlier task.

## Goal

An empty Cargo workspace that passes a strict CI, built from writers-app's setup at `/Users/andrew/Documents/code/writers-app`. Later tasks only add code to it.

## Deliverables

### Workspace

- Root `Cargo.toml`: virtual manifest, `resolver = "3"`, members for the four crates in the layout in `00-shared-context.md`, `[workspace.package]` with `version = "2.0.0"`, `edition = "2024"` and `license = "Apache-2.0"`, `[workspace.dependencies]` declared with `default-features = false` and features enabled per member.
- Each crate has `lints.workspace = true` and a minimal documented `lib.rs`. `crates/flakeradar` also has a `main.rs` that exits successfully.
- Copy the profiles from writers-app's root `Cargo.toml`: release `opt-level = 3`, `lto = true`, `codegen-units = 1`, `strip = "symbols"`; dev `debug = "line-tables-only"`; dependencies `debug = false`.
- `rust-toolchain.toml` pinned to `1.99.0` with `rustfmt` and `clippy`.
- Commit `Cargo.lock`. Add `target/` to `.gitignore`.

### Lints and standards

- Copy the `[lints.rust]` and `[lints.clippy]` sets from `writers-app/src-tauri/Cargo.toml` into `[workspace.lints]`, with their comments. Reword the `multiple_crate_versions` comment, since Tauri is not the reason here.
- Copy `src-tauri/clippy.toml`, keeping `allow-indexing-slicing-in-tests`.
- Write `docs/agents/code-standards-rust.md` from `writers-app/docs/agents/code-standards-rust.md`. Remove the Tauri, frontmatter, manuscript and provider rules. Keep the sections on the compliance marker, static verification, `#[expect]` with reasons, errors and panics, documentation and testing. State the error rule for this repo: `anyhow` in the `flakeradar` binary crate, canonical error structs in `flakeradar-core` and `flakeradar-db` because a second crate depends on them, and untrusted CI input (uploads, API bodies) is never allowed to panic.
- Add a "Code standards" section to `AGENTS.md` pointing to that file and the `ms-rust` skill. Load the `writing-for-agents` skill before editing `AGENTS.md`.

### Test runner and audit

- `.config/nextest.toml`: copy from writers-app (default and `ci` profiles, JUnit, `retries = 1`, `flaky-result = "fail"`).
- `.cargo/audit.toml`: empty config with a header comment saying every ignore must carry its reason.

### CI (`.github/workflows/ci.yml`)

Keep the existing `backend`, `frontend` and `summary-action` jobs running until task 15. Add:

- **Trigger:** add `feat/rust-rewrite` to `push.branches`. Task 15 removes it. This is how the branch gets CI with no PR open.
- **Concurrency:** copy the block from writers-app that cancels superseded PR runs and never cancels `main`. Do not cancel `feat/rust-rewrite` pushes either, so every task gets a full result.
- **`rust` job** on `ubuntu-24.04`: Postgres 17 service, `FLAKERADAR_TEST_DATABASE_URL` set, `RUSTFLAGS=-D warnings`, `Swatinem/rust-cache`, `cargo fmt --check`, clippy, `cargo nextest run --all-features --profile ci`, upload `target/nextest/ci/junit.xml` as an artifact (`if-no-files-found: error`), then `cargo audit` with the pinned, cached binary and the daily advisory-db cache from writers-app. Do not upload to FlakeRadar: GitHub-hosted runners cannot reach the internal instance.
- **`coverage` job:** copy writers-app's job. Pinned `nightly-2026-10-25`, `cargo-llvm-cov@0.9.1`, `cargo +nightly llvm-cov nextest --branch`, reports exported from one collection, `RUSTFLAGS=""`, its own cache prefix, Postgres service. Port `scripts/coverage-gate.mjs` and its test `scripts/coverage-gate.test.mjs`, and read floors from `coverage-floor.json` at the repo root. Set the floors to `{"lines": 0, "branches": 0}`. Task 15 sets the real ones.
- **No-decrease check in the coverage job:** add a `--baseline <file>` option to `coverage-gate.mjs`, and fail if lines or branches fall below the baseline at two decimals, with no tolerance. Upload `coverage-summary.json` as the `rust-coverage` artifact on pushes to `main`. On PRs, download the newest `rust-coverage` from the base branch using the logic in the current `backend` job. With no baseline, only the floors apply.
- **Version check:** port `scripts/version.mjs` from writers-app so `node scripts/version.mjs check [version]` compares the workspace version, `frontend/package.json` and `frontend/package-lock.json`. Add its tests. Add a `version-consistency` job. Set `frontend/package.json` and its lock to `2.0.0`.
- Pin third-party actions the same way as the rest of the file.

### Do not copy

`setup-apt`, pandoc, epubcheck, pilot, Tauri, signing, R2, `check-glibc-floor.sh`, self-hosted runner labels, or the `ingest-flakeradar` step.

## Done when

- [ ] `cargo fmt`, clippy with `-D warnings` and `cargo nextest run --profile ci` pass on the empty workspace (use `--no-tests=pass` if nextest needs it until tests exist, and remove it in task 03).
- [ ] `node --test scripts/*.test.mjs` passes, including new no-decrease cases.
- [ ] The branch is pushed and the new jobs are green on GitHub.
- [ ] `STATUS.md` is updated.
