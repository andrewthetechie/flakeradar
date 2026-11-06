# Code standards: Rust

The Rust workspace follows the Pragmatic Rust Guidelines as packaged in the `ms-rust` agent
skill (compliance date 2026-07-21). The skill's `SKILL.md` says which guideline file to load for
a given task — load them before writing Rust. This document records how those rules land in
_this_ repo. Where this document is stricter, this document wins.

## Compliance marker

A `.rs` file that fully complies carries the marker comment, per the skill:

```rust
// Rust guideline compliant 2026-07-21
```

Place it immediately before the first `use` or item, below any license header and any
module-level `//!` doc comment, separated from preamble and code by a blank line. When the file
has no preamble, it is line 1. Add it only to files you have actually checked — a stale marker
is worse than none.

## Static verification

- Lint levels live in `[workspace.lints]` at the repo root, and every member opts in with
  `lints.workspace = true`. The set follows M-STATIC-VERIFICATION and goes beyond it: all major
  clippy categories plus `nursery` at warn, the guideline's restriction lints, further
  restriction lints that catch panics or nondeterminism (`indexing_slicing`, `string_slice`,
  `iter_over_hash_type`, and more), and the allow-by-default rustc lints `rust_2018_idioms`,
  `unused_qualifications`, `unreachable_pub`, and `missing_docs`. Each choice and each lint
  deliberately left off carries its reason in the manifest. Do not weaken it. Case-by-case
  opt-outs happen in the config with a comment, never per-file.
- `multiple_crate_versions = "allow"` is ours, because the duplicates it reports live in the
  transitive tree and this workspace cannot pick a single version graph for them.
  `literal_string_with_formatting_args = "allow"` is not a local decision — it ships in the
  guideline's own snippet, because it fights structured logging.
- `clippy.toml` tunes lint behaviour only (levels stay in the manifest). It sets
  `allow-indexing-slicing-in-tests`, so `#[test]` functions and `#[cfg(test)]` modules may
  index, and `doc-valid-idents` so `FlakeRadar`, `SeaORM`, `PostgreSQL` and `JUnit` read as
  prose in doc comments.
- The set carries 20 of the 21 restriction lints M-STATIC-VERIFICATION lists. The absent one is
  `string_to_string`, which clippy has since removed in favour of `implicit_clone` — and that
  lint arrives through `pedantic`, which is already on. Do not "restore" it: it is a removed
  lint, so it trips `renamed_and_removed_lints` and fails `clippy -D warnings`.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo nextest run --all-features --profile ci` and `cargo audit` are the gates CI runs. Run
  them before every commit; the same commands are in `docs/plans/rust-rewrite/00-shared-context.md`.
- rustfmt runs with default settings. If we ever deviate, commit a `rustfmt.toml` so editors and
  CI agree.
- `#[expect]` with a reason for every override (M-LINT-OVERRIDE-EXPECT): a lint suppressed with
  `#[allow]` is never checked for staleness, and the set does not warn when the suppression
  stops being needed. Known lint debt is tracked, not hidden: a function that still violates a
  lint carries `#[expect(clippy::<lint>, reason = "<what is wrong>; tracked in #<issue>")]`, and
  fixing the code makes the attribute fail with `unfulfilled_lint_expectations`, so it goes in
  the same change. Do not add new `tracked in` sites for new code: fix new findings instead.

## Errors and panics

- The `flakeradar` binary crate uses `anyhow::Result<T>` (M-APP-ERROR). It is the process edge,
  where an error becomes a log line and an exit status.
- `flakeradar-core` and `flakeradar-db` carry canonical error structs, as `thiserror` enums
  (M-ERRORS-CANONICAL-STRUCTS). The threshold is "a second crate depends on it", and these two
  cross it: the server crate, the MCP server and the test-support crate all match on their
  failures. Add a variant only when a caller branches on it.
- A `thiserror` enum inside the binary crate is the exception, for a module whose callers branch
  on the failure kind.
- **Untrusted CI input never panics.** Uploads (`POST /api/ingest`, `POST /api/ingest/pipeline`)
  and API request bodies are attacker-controlled: a malformed JUnit file, a 10 MB timestamp or a
  NaN score produces a `Result`, a stored failure or a 4xx, never a panic and never an
  arithmetic overflow. Panics are for programming errors only (M-PANIC-IS-STOP, M-PANIC-ON-BUG).
- Intentional panics carry a message with the reason and the offending values (M-PANIC-MESSAGE).
- `unwrap`/`expect` on input or I/O paths is a review blocker. `expect` is for genuine
  invariants, with the invariant stated in the message. The lint set warns on `unwrap_used`,
  `expect_used`, and `panic`, and clippy runs with `-D warnings`, so each intentional site
  carries `#[expect(clippy::expect_used, reason = "...")]`, or the matching lint for `unwrap` or
  `panic!`.
- Unit tests are exempt through a crate-level `cfg_attr(test, expect(...))` in `lib.rs`. Where a
  crate's integration suite becomes one binary, its `#![expect(clippy::expect_used,
  clippy::unwrap_used, clippy::panic, reason = "...")]` header sits once at that crate root, and
  a new suite module inherits it. Production code gets no such exemption: Failure messages,
  Failure details and CI-supplied names are arbitrary UTF-8, so slice with `str::get` or on char
  boundaries.

## Documentation

- M-CANONICAL-DOCS: summary sentence, then extended docs, with `# Examples`, `# Errors`,
  `# Panics`, `# Safety` sections whenever they apply. The summary is one line of roughly 15
  words or fewer, because it is what the module listing shows (M-FIRST-DOC-SENTENCE). Public
  modules carry `//!` docs covering what the module holds and when to use it (M-MODULE-DOCS).
- Explain parameters in prose ("Copies a file from `src` to `dst`"), never `# Parameters`
  tables.
- No design-journey essays in doc comments (M-NO-META-DESIGN-DOCUMENTATION). Rationale lives in
  `docs/adr/`, `docs/plans/rust-rewrite/` and issue comments; rustdoc describes end-state
  behavior.
- `pub use` re-exports of crate items get `#[doc(inline)]` (M-DOC-INLINE). Re-exports of `std`
  or third-party types do not — leaving them un-inlined is what marks them as foreign.
- Re-export items by name, never `pub use foo::*` (M-NO-GLOB-REEXPORTS).
- Comments in American English.

## Workspace and types

- Crates are flat siblings under `crates/` (M-CRATES-FLAT-FOLDER) and every one is a workspace
  member (M-CRATES-IN-WORKSPACE). Versions live in `[workspace.dependencies]`, declared
  `default-features = false`, and each member enables the features it needs.
- `flakeradar-core` holds the DTOs (`serde` and `utoipa::ToSchema` derives) so `flakeradar-db`
  and `flakeradar` share them. It never depends on SeaORM, Axum or a network crate.
- Identifiers use `CONTEXT.md` vocabulary: `Repo`, `Project`, `Test`, `Report`, `Run`,
  `Execution`, `Job execution`, `Flaky test`, `Proven flake`, `Clean streak`, `Fail streak` —
  never the glossary's avoided synonyms. If a needed concept is not in the glossary, that is a
  signal to resolve it, not to invent a synonym.
- Strong types over primitive obsession (M-DESIGN-FOR-AI): a newtype for a fingerprint, a
  score, a CI run ID. A newtype that encodes an invariant enforces it with a fallible
  constructor (M-STRONG-TYPES-GUARD), and offers no infallible `From`.
- Public types implement `Debug` (M-PUBLIC-DEBUG; the lints enforce it) and, when meant to be
  read, `Display` (M-PUBLIC-DISPLAY). Types holding a secret — the API token, a Webhook URL —
  implement `Debug` manually with redaction, plus a unit test proving the secret does not appear
  in the output.
- Magic values are named constants with a doc comment saying why the value, what breaks if it
  changes, and what external system it interacts with (M-DOCUMENTED-MAGIC). The `0.5` default
  Flake threshold and the 2000-character `MESSAGE_MAX` are examples.

## Testing

- `cargo nextest run --all-features --profile ci` is the suite. Nextest runs each test in its
  own process and writes `target/nextest/ci/junit.xml`, which CI uploads.
- Tests assert behavior and properties, never restate ground truth from the implementation
  (M-TAUTOLOGICAL-TESTS).
- Tests that only touch a public API are integration tests and live in `tests/`, not
  `mod tests {}`. Where a test could be written either way, the integration test wins
  (M-INTEGRATION-TESTS); `#[cfg(test)]` modules are for what genuinely needs to see private
  internals.
- **No test depends on real time.** The clock, poll intervals, HTTP base URLs and settings are
  injected, and a wait uses `tokio::time::pause` and `tokio::time::advance`, never
  `tokio::time::sleep` against the real clock. A path reached only by timing makes CI fail at
  random, and the coverage gate has no tolerance.
  - A wait for work a background task does is a signal, not a sleep: a worker's queue counts
    the batches it finished (`flakeradar_test_support::until_finished`), a stub records a call,
    a `Notify` marks the moment. A negative assertion waits for that signal first, so
    "nothing was sent" is a claim about a task that ran.
  - Three facts have no signal, and each is one documented helper in
    `flakeradar-test-support` rather than a sleep at the call site:
    `wait_until` polls a row, a counter or a socket answer and fails on a 30-second deadline
    (a bound on a failure, never on a duration); `within` asserts that a cancellation did not
    sit through the seconds the cancelled work asked for, which is the only observable a
    cancel has; and `tests/parity.rs` bounds its own waits so a stuck replay reports a
    mismatch in its answer instead of panicking. Every other sleeping wait is a bug.
  - Two more reads of the real clock are listed here because the clock is the subject.
    A retention window and a "new within N days" filter are about the present day, so the
    tests that age rows and the `days_ago` helpers count back from `Utc::now()`; and
    `postgres.rs` polls a container that is really starting. Every fixture timestamp that
    is not about the present is `flakeradar_test_support::FIXTURE_INSTANT`.
- Test-only helpers — test databases, factories, mocks — live in `flakeradar-test-support`, which
  only a `[dev-dependencies]` entry can name, so no test helper reaches a release binary
  (M-TEST-UTIL).
- Golden fixtures for the JUnit corpus live in `tests/parity/golden/`. A test never edits one;
  an intended behaviour change goes in `docs/plans/rust-rewrite/differences.md` and
  `tests/parity/allowed-differences.json`.
- CI gates line and branch coverage against `coverage-floor.json` at the repo root. A change
  that adds tests raises floors in the same PR; task 15 sets the final ones.
  `#[cfg_attr(coverage_nightly, coverage(off))]` is limited to code a test process cannot run,
  and every use is logged in `docs/plans/rust-rewrite/coverage-gaps.md`.
- `node --test scripts/*.test.mjs` covers the two Node scripts CI depends on
  (`coverage-gate.mjs`, `version.mjs`).

## Review checklist for Rust changes

- [ ] `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
      `cargo nextest run --all-features --profile ci` clean.
- [ ] No new `unwrap`/`expect` on input or I/O paths; untrusted CI input returns a `Result`.
- [ ] Public items documented per M-CANONICAL-DOCS; fully compliant files carry the marker.
- [ ] No secrets or Failure details in logs; no `println!`/`dbg!`.
- [ ] New I/O reached through an injected seam, not a real clock or a hard-coded URL.
- [ ] New invariant-bearing newtypes have a fallible constructor and no infallible `From`.
- [ ] Glossary vocabulary in identifiers; scores, fingerprints and categories match Python.
- [ ] Lint set unchanged; any override is `#[expect(..., reason = "...")]`.
