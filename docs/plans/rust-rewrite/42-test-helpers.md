# 42 — Test helpers that remove the most lines

Read `00-fixes-2-context.md` first. Findings: `CC22`–`CC25`. Read their sections and the "Test duplication" section under "Second pass" in `review-135b/clean-code-findings.md`. Tests and test-support only. No test changes what it checks.

## Why

Test duplication rose from 3833 to 6193 lines in `dups.py 8`, partly because the tool counts pairs. The 12 largest clusters would save 622 lines, and two helpers cover six of the seven largest rows.

## Contracts

**`CC25`, first.** In `tests/github/support.rs`, add `sync(db, &server)` and `sync_with(db, &settings, &server)`, both returning `BatchReport`. Sweep the sites in `tests/github/sync.rs` and `notes.rs`. A site that passes a chosen clock or stop token keeps the full call, so those arguments stand out where they matter.

**`CC24`.** Add `pub async fn create(self, db: &TestDb) -> Model` to each of the 10 builders in `flakeradar-test-support/src/factories.rs`, panicking with the builder's name. Sweep the sites mechanically, one commit per crate. Keep `build`, because `flakeradar-test-support/tests/factories.rs` and any transaction site need it. An `async fn` cannot be `#[track_caller]`, so the panic location is in `factories.rs`. The panic message and the test name identify the failure.

**`CC22`.** Add `assert_refused`, `assert_accepted`, `one_job` and `send_json` as local helpers in `tests/validation.rs`, using `#[track_caller]` on the two asserts. Test-support has no JSON helper or refusal assert to reuse. Check this again before you write one.

**`CC23`.** Split `a_real_mcp_client_initializes_lists_its_tools_and_reads_a_test` into four tests over the existing `seed` and `connect`, plus a `call(&client, name, json!(..))` helper.

**Leave these alone:** `flaky_test` and `flaky_test_in`, because their fixtures differ. The smaller clusters in `tests/webhooks.rs` and `tests/github/bodies.rs` get a local helper only if you already touch those lines.

## Done when

- [ ] `cargo nextest list` shows the same test names before and after, except `CC23`'s split. Put the diff in `STATUS.md`.
- [ ] `dups.py 8` over tests is quoted before and after. The net line change across `crates/*/tests/` and `flakeradar-test-support` is also quoted.
- [ ] Line coverage and branch coverage are not below the floors, because the sweep must not drop a path.
- [ ] Each row says what a reader now sees, such as a two-row seed that fits on two lines.
- [ ] The shared rules are met.
