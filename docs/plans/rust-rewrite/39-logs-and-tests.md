# 39 — Log events and test shape

Read `00-fixes-2-context.md` first. Findings: `ST4`, `ST7`, `ST8`, `ST9`, `ST32`, `ST33`, `CC5`, `CC6`, `CC7` (falsely closed `H16`, `T2`, `T3`, `T4`; partly closed `H14b`, `H20`, `T5`). Behaviour stays fixed, except that `PruneResult` gains a field.

## Contracts

**Tests that do not check their names (`ST32`).** `an_http_date_retry_after_uses_the_default_wait_and_still_retries` (`tests/webhooks.rs:600`) passes on a single attempt and really sleeps 1.5 s. Use `tokio::time::pause` and assert `recorded(&server).await == 3`. `stop_ends_the_task_without_starting_one_twice` (`:615`) cannot see a second loop, so count the closure's calls on `LeaderRun::start`. Show each one going red against a mutation, then revert it.

**A scheduling guess (`ST33`).** `tests/server.rs:296-300` yields four times in place of a signal. Add a small test seam on `Server` that fires a `Notify` when the request reaches the handler, and wait on it.

**Static messages (`ST4`).** The five events H16 listed still interpolate: `worker.rs:712`, `:770`, `processing.rs:1861`, `retention.rs:504`, `:525`. Their values are already named fields, so make the messages static, as `webhooks.rs:193-198` does. Then turn on the `{ident}` check from task 38.

**Log text is not an outcome (`ST7`).**

- Give `PruneResult` the dropped-Notification count, and assert that at `flakeradar-db/tests/retention.rs:761`, where the log line is the only assertion.
- Delete the redundant log assertions at `retention.rs:305`, `:1083` and `tests/github_worker.rs:1701`.
- Rewrite `tests/worker.rs:555`, `:658` to assert the event's fields.
- The sites `ST7` lists as "not flagged" stay as they are.

**One log capture (`ST8`, `CC7`).** Use `flakeradar_test_support::CapturedLog` (`doubles.rs:692`), adding `during(future)` for the `logged_while` shape. Replace the private rigs at `db/tests/retention.rs:1215`, `tests/app.rs:317`, `tests/ops.rs`, `tests/admin.rs`, `tests/github_worker.rs`, `tests/github/support.rs` and `tests/webhooks.rs`. The two `src/` sinks stay, for the two-instance reason `ST8` gives. Make `flaky_test(db)` call `flaky_test_in`.

**Tests in the right place (`ST9`).** Move the public-API-only `#[cfg(test)]` modules (`scoring.rs`, `types.rs`, `error.rs`, `sql.rs`, `migration.rs`, `test-support/junit.rs`) to `tests/`. Delete the tautology at `limits.rs:26-30`.

**One concept per test (`CC5`).** Split the six tests that run Test and Job in one body (`tests/github/filing.rs:721`, `:916`, `:965`, `:1006`, `:1105`, `:2014`) and `sync.rs:969-1045` into `scenario(kind)` helpers with thin per-entity tests. The 300-character log cap gets its own test. `a_gone_issue_rearms_its_link_but_any_other_answer_keeps_it` becomes two tests.

**No positional unpacking (`CC6`).** At the 20 sites like `let (number, filed, _, closed) = (link.number, ...)`, assert the fields directly. Delete the redundant `UPDATE`s (`sync.rs:135-142`, `:182-189`, `filing.rs:878-885`) and the `checked_at` helper.

**Names (`CC` sample).** Fix `a_candidate_read_that_fails_logs_and_reports_an_empty_batch`, `five_default_branch_failures_mark_a_test_broken`, and the vacuous "never raises" clauses, so each name states what its body asserts.

## Done when

- [ ] The test count does not fall except by the deleted tautology and redundant cases, and `STATUS.md` lists what went and why.
- [ ] `dups.py 8` over `crates/*/tests` is below its current 652 pairs / 6193 lines, with both numbers recorded. No specific target: do not abstract test bodies to move the number.
- [ ] A mutation in each of two split scenarios fails only that entity's test.
- [ ] Coverage floors hold. The shared rules are met.
