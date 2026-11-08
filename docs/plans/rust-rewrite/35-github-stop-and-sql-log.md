# 35 — GitHub stop rules, off-thread JSON work, and the SQL log

Read `00-fixes-2-context.md` first. Findings: `SP6`, `ST16`, `ST17`, `ST28`, `R2`.

## Contracts

**`SP6`: a store failure ends the outcome.** Task 18's contract says that on "store failed", as in Python, "the issue stays open, the batch stops". In `github/mod.rs`, `on_report_processed` ignores `filed.stopped`, so `FiledLink::StoreFailed` stops only `file_batch`, and the Job filing and both note batches still run. In the reference the exception propagates to `GitHubWorker._file`, so none of them run. Check this against `_file_batch` at `86c98804`, then make the Rust match. A rate-limit or transport stop must keep its current, separately measured behaviour.

**`ST16`: do not repeat a comment.** `until_stopped` wraps the fixed-note comment (`github/notes.rs:115`, `:264`). Dropping that future does not un-send the request. If GitHub received it, the comment lands, no stamp is written, and the next Report comments again. `select!` can also discard a completed `201`. The note is stamped before the cancellable close, so a cancelled close is never retried and the issue stays open.

- Treat a comment like the filing `POST`: it runs through its stamp, then the batch stops.
- Make a close that a stop interrupted retried on a later Report, or run it through as well. Measure what the reference does across a shutdown, and follow it.
- Update `differences.md` row 16 and the doc on `until_stopped`, and state the longest stop this now allows.

**`ST17`: the rest of the CPU work leaves the async thread.** `extract.rs:157` (`T::from_object`) and `routes/reports.rs:590` (`pipeline_body`, inside an open transaction) stay on the async worker for a 20 MiB body, although `:146-157` moved the parse off for that reason. Do both in `spawn_blocking`, before the transaction opens.

**`ST28`: the query's name lookup is linear.** `last_value_per_name` (`extract.rs:213-222`) scans `pairs` for each name, on the async thread, for every `ApiQuery` route. A copy of the loop took 383 ms on 16,384 names, which fits in a URI `http` allows. Use a `HashMap<String, usize>` from name to index, and keep the order and last-value-wins behaviour. No timing test: pin the behaviour with a repeated-name case.

**`R2`: the default log is not SQL.** At the default `info` filter, `sqlx::query` wrote 1546 of 1605 log lines in the reviewer's demo seed run, one per statement. No setting asks for it, and the reference's engine does not echo. `apply_shared` in `connect.rs` never calls `sqlx_logging`. Turn statement logging off, or move it to `debug`, for both pools so `RUST_LOG` can still bring it back. Say which in `README.md`'s logging or settings section.

## Done when

- [ ] `SP6`: a test with a failing link store and Job and note candidates present, on a counting mock, sees no further GitHub call, and the issue stays open.
- [ ] `ST16`: a test holds a comment `POST`, calls `stop`, releases it, and sees the stamp stored, with no second comment on the next Report. A test does the same for the close.
- [ ] `ST17`: a test or a compile-level shape proves the work runs off the request task. Do not use a wall-clock assertion.
- [ ] `R2`: a test proves `RUST_LOG=info` writes no `sqlx::query` line during a read, and the `debug` path is stated.
- [ ] `stopping_waits_for_a_filing_post_and_keeps_its_link` and the other `stopping_*` tests still pass, and any that changed meaning are renamed.
- [ ] The shared rules are met.
