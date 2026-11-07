# 41 — Structure of the request and worker modules

Read `00-fixes-2-context.md` first. Findings: `CC12`–`CC15`, `CC17`–`CC21`. Read their sections under "Second pass" in `review-135b/clean-code-findings.md`, where each has its evidence and size. Tasks 34 and 36 changed some of these lines, so read the code as it stands now. Structure only: goldens and allowances stay untouched, and a replay failure is a bug in your change. One commit per finding.

## Contracts

Work in this order. `CC20` moves code, so it goes last to keep the other diffs readable.

| Row | Change |
|---|---|
| `CC21` | `AdvisoryLock::check` (`worker.rs:408-436`) becomes one match with three arms. Keep the take and put-back. Its reason at `:339-341` is real |
| `CC19` | One public `BoxFuture<'a, T>`, promoted from `mcp/mod.rs:58`, replaces the seven future aliases in `worker.rs` |
| `CC18` | `post` (`webhooks.rs:164`) becomes a short loop over `attempt`, which returns `enum Attempt { Stopped, Finished, Retry { after: Option<f64> } }`. Name the status rule `is_permanent`. No log changes |
| `CC17` | `lax::recorded(result, issues) -> Option<T>` replaces the arms in `dto.rs` that record an issue. `dto.rs:1046` may stay as it is |
| `CC15` | `path_issue` matches `expected_type` directly, and `kind_for_type_name` goes. In `issue_for`, merge the two `missing` tests and drop `scope` |
| `CC12` | `Framing::next_part` or a `parts` iterator holds the walk, and `file_field` reads as boundary, framing and one `for`. The three dead `return Ok(found)` branches go |
| `CC13` | One private `split_once_byte` serves both splits. `trim_ascii` gives way to `<[u8]>::trim_ascii` |
| `CC14` | `memchr::memmem::Finder` replaces the hand-written search, per the proposed table. Keep the behaviour half of `a_crafted_body_is_scanned_in_one_pass`, and drop its counting half and the counter |
| `CC20` | Move only: `worker/leader_task.rs` and `worker/lock.rs`, with explicit `pub use` lines so no public path changes |

**Do not add abstraction beyond this table.** `CC8` is a descriptor that cost more than the duplication it removed. If a change here costs more lines than the finding estimates and leaves the reader no better off, close the row `no change` with the before and after line counts.

## Done when

- [ ] Each row is closed, with the reader's gain stated: for example, the retry rule in `post` is now readable in about 15 lines.
- [ ] `cargo tree -p flakeradar` shows `memchr` as a direct dependency, and `cargo nextest` runs the same test names, except the ones you name in `STATUS.md`.
- [ ] `dups.py 8` over production does not rise above its value at the start of this task.
- [ ] `battery/run.sh all` shows 0 new differences, with `OUT=` outside the repo, because `CC12`–`CC15` and `CC17` touch multipart and validation.
- [ ] The shared rules are met.
