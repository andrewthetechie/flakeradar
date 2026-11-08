# Clean code re-review: PR #135, f9330fd..ac5e6f3

Scope cut by the coordinator's budget message: must-fix and should-fix only, nits omitted. The investigation stopped early, so this list is not exhaustive.

## Verdict

- **No must-fix.** I found no behaviour change from a refactor in the code I compared against f9330fd or 6d9c22f: `rescore`/`rescore_jobs`, tier changes, `TierFilter`, `process_report`, `build_test_executions`, `expire_executions`, `clear_expired_quarantines`.
- **Not compared:** `multipart.rs`, `extract.rs`, and the bodies byte for byte.
- **The refactors helped.** Production duplication fell from 639 to 267 lines, and `process_report` and the MCP layout are much better.
- **The main problem is how rows were closed.** Several rows closed on a grep the code was then edited to satisfy, while the reader's cost stayed. See CC1, CC5 and CC6.

**How this was checked:** by reading code, plus scripts and `dups.py`. I ran no cargo and no mutation, and created no worktree or target dir. All findings are CONFIRMED by reading unless marked as judgement.

**CI (`checks.json`):** no `bucket: fail`. 18 pass, 1 skipping (`Merge the manifest list`).

## Measurements

### dups.py, MIN_LINES 8

| Set | f9330fd | ac5e6f3 |
|---|---|---|
| Production src (no test-support) | 59 pairs / 639 lines | **24 / 267** |
| Production src + test-support | not measured | 32 / 368 |
| test-support alone | not measured | 8 / 101 |
| All `crates/*/tests` | 391 / 3833 (33,402 LOC) | **652 / 6193** (40,324 LOC) |
| Task 27's four files | ledger says 18 / 223 | 7 / 77 **reproduced at 53cbd17**; 5 / 58 at head |

Test duplication grew 62% while test LOC grew 21%.

### The remaining pairs in task 27's file set

The ledger's 7 pairs are now 5.

| Pair | Reading |
|---|---|
| `routes/issues.rs:80-111` / `:161-192` | Real, see CC4 |
| `routes/issues.rs:57-67` / `:138-148` | `utoipa` attributes, legitimate |
| `retention.rs:216-225` / `:243-252` | Query shell with different filters, leave |
| `processing.rs:1279-1286` / `:1314-1324` | Signature and chunk loop around different SQL, leave |
| `processing.rs:853-862` / `:1146-1155` | The `execute_raw(Statement::from_sql_and_values(...))` tail. `sql.rs` has `rows` and `one` but no `execute`, and this tail appears at 11 sites. Small |

Largest pairs outside that file set: `notes.rs` 22, `github/filing.rs` 18 and 11, `attribution.rs` 15 and 13 (literal SQL measured in `docs/perf`, leave), `db/filing.rs` 14, 9 and 9.

### Size

- **Largest non-test functions:**
  - `from_map` 100 (flat table, fine)
  - `runner_stats` 97
  - `note_batch` 94, 7 params
  - `file_batch` 89, 6 params
  - `upsert_pipeline_jobs` 87
  - `get_test` 85
- **Worst file:** `reads.rs`, 2,202 production lines.
- **Size that is legitimate content:**
  - `dto.rs`: 2,115 lines, 594 of them comments, DTOs and 422 readers
  - `settings.rs`: 874 production lines, 590 test lines
- **Real seams, moves only, optional (judgement):**
  - `processing.rs:1175-1867`, the rescore cluster, to `processing/rescore.rs`
  - `reads.rs:1846-2200`, the CI run reads, to `reads/ci_run.rs`
- **`tests/parity.rs`, 3,025 lines:** 24 tests and 79 helpers, mixing replay, comparison library, OpenAPI contract and self-tests. The seam is real, as `tests/github/` showed. The cost is navigation only.
- **`tests/github/filing.rs`, 2,062 lines:** 41 tests and 2 helpers. Readable, and helpers do not hide assertions. Its size is mostly call boilerplate: the batch entry points are called 120 times across `tests/github/` (37 sync, 40 file, 21 job file, 22 note), each about 7 lines. A fixture with `.sync()` and `.file(&ids)` would remove roughly 600 lines. Worth doing only if these files are touched again.

### Dead code

- 735 `pub` items were scanned. 67 have no use outside their own file; most are route signature types and documented constants, which is harmless.
- No production use:
  - `SortKey::SPELLINGS` (`reads.rs:211`): no use at all, and `routes/reads.rs:158` hard-codes the same list
  - `is_default_token` and `REDOC_SCRIPT_PATH`: their own unit tests only
  - `processing::job_history` and `JobHistory`: `tests/attribution.rs` only
  - `with_processor` and `with_waiter`: legitimate test seams
- test-support: 170 `pub fn`, 3 with no external use. `Stamp::number_filed_elsewhere` and `clear_note` are dead; `ensure_postgres` is internal. The D8 concession is mild in practice.

### Test name sample

15 tests, read against their assertions (`the_tier_filter_table_names_every_state_the_options_can_be_in` read in part):

- **8 truthful.**
- **4 carry a vacuous "never raises" clause**, which a value-returning Rust function cannot fail.
- **3 outrun their body:**
  - `a_candidate_read_that_fails_logs_and_reports_an_empty_batch` has no log assertion.
  - `five_default_branch_failures_mark_a_test_broken` never reads `broken`.
  - `every_operation_judges_every_verdict_as_the_reference_does` compares against its own copy of the table.

Classed as nits and not ranked further.

## Findings, ranked by reader cost

### CC1. should-fix. B7 was closed by swapping a word, and the guard cannot see the swap.

**Where:** `github/sync.rs:84`, `db/filing.rs:176-177`, `routes/reports.rs:320`, `:405-406`, `parsing.rs:150`, `:396`, `scoring.rs:144`, `processing.rs:1799`, `tests/source_hygiene.rs:51-60`.

```rust
/// Stamping stays as the reference's `_clear_if_closed` docstring states: every row   // sync.rs:84
/// The window is the overall one, with no Default-branch arm: the reference's          // db/filing.rs:176
/// `_window_failures` passes no `include_default_branch`, so older Default-branch
/// Checks the range `reports.py` put on `limit`.                                        // routes/reports.rs:320
```

- **Counts:** "the reference" appears on 22 production comment lines and 47 test or test-support comment lines, excluding XML "character reference". CONTEXT.md does not define the term. Several test names also carry it, such as `the_body_matches_the_reference_byte_for_byte`.
- **Guard:** `no_source_line_points_at_the_replaced_implementation` bans "python" but not "reference", `.py` or `_private_name`, so its name overclaims.
- **Journal voice:** "used to" at `parsing.rs:150` and `scoring.rs:144`; "has always carried" at `bodies.rs:5`; "`ElementTree` parses" at `parsing.rs:396`.
- **Named in B7 and still present:** "Rescales every Job" (`processing.rs:1799`).
- **C4's false comment survives, reworded and now worse.** `routes/reports.rs:405-407` says "Checked here, where the insert fails on the same value", but `accepted_ci_run_attempt` checks nothing: it only extracts the number that `validate` already accepted.
- **Reader cost:** a newcomer needs the deleted tree to follow these comments, which was B7's whole complaint.
- **Better shape:** state the end-state rule. For example: "Every answered row is stamped; a rate-limit stop or transport error leaves it unstamped, so it is retried first." Move rationale to `differences.md`, and add `the reference`, `.py` and `docstring` to the guard.
- **Churn:** about 70 comment lines and 6 renames, text only.

### CC2. should-fix. The "one status policy" is not the only policy, and the loops did not shrink.

**Where:** `github/policy.rs:134-198`, `notes.rs:93-186`, `:257-333`, `filing.rs:189-277`, `sync.rs:87-151`, `:232-312`, `:350`.

```rust
Err(error) => {                                        // notes.rs:127, same shape at filing.rs:221, notes.rs:279, sync.rs:105
    report.stopped = Some(batch_stop(Verdict::Unreachable));
    break;
}
report.refused.insert(*number, Verdict::Unreachable);  // sync.rs:248, :279
fn already_gone(answer: &Answer) -> bool { matches!(answer.status(), 404 | 410) }  // sync.rs:350
```

- `action(_, Unreachable)` is called only by `tests/github/policy.rs`. Every transport arm hard-codes its result, so changing that table row changes nothing in production. The doc's "the module's one status policy" overclaims.
- `already_gone` is a second status list, although `note_refused` already holds the `verdict`.
- B3 predicted these functions would shrink. They did not:

| Function | f9330fd | ac5e6f3 |
|---|---|---|
| `note_batch` | 92 lines, 5 params | 94 lines, 7 params |
| `file_batch` | 90 lines, 5 params | 89 lines, 6 params |
| `clear_if_closed` | 99 lines, 7 params | 65 lines, 7 params |

- Output parameters remain: `&mut BatchReport` in `clear_if_closed` and `record_filed`, and `&mut report.skipped` in `close_note`.
- The same sequence (call, transport error, classify, `action`, log per arm) is written five times.
- **Reader cost:** a change to stop behaviour means reading five loops and one table that is only partly authoritative.
- **Better shape:** one step that owns the sequence, so each loop handles only `Use` and `Rearm`:

```rust
fn judged(op: Operation, answered: Result<Answer, TransportError>, success: Success, repo: &str,
          skipped: &mut BTreeSet<String>) -> ControlFlow<BatchStop, (Action, Option<Answer>)>
```

  This logs the stop and the Repo skip once, with `operation = ?op`, and sends a transport error through `action(op, Unreachable)`. `already_gone` becomes `matches!(verdict, NotFound | Gone)`. `batch_stop` can disappear.
- **Trade-off:** per-operation event names merge into one name plus a field. T3 says logs are no longer a compatibility surface, but `logged_call` assertions need checking.
- **Churn:** about 150 lines, net about -100.

### CC3. should-fix. B1 is closed with five of its own 13 table rows unmerged, and those rows are outside the measure.

**Where:** `notes.rs:28-87`, `filing.rs:41-173`, `db/filing.rs:370-433`.

`note_fixed_issues_for` and `note_fixed_job_issues_for` differ in one function name and one `EntityKind`:

```rust
match filing::note_test_candidates(db, test_ids, settings.github_fixed_streak).await {   // notes.rs:39
    Ok(candidates) if !candidates.is_empty() => {
        let report = note_batch(db, candidates, EntityKind::Test, settings, api, clock, stop).await;
```

- B1's rows for filing candidates, note candidates, the file entry point, the note entry point and link routes remain. Task 27's file does not mention `github`, `notes`, `filing` or `candidates`, and its measure covered only four files, so these rows left the scoreboard without a recorded reason. The ledger gives a reason only for link routes.
- B4's "sharpest instance" survives in typed form: `note_batch(.., entity, ..)` receives the kind that every `row.target` already holds, and likewise `file_batch` and `FileRequest.target`.
- **Reader cost:** a rule change to notes or filing is still made twice, which is B1's original complaint.
- **Better shape:**
  - One `note_fixed_for(kind, ids, ...)`, with the candidate read dispatching on `kind`.
  - Drop the `entity` parameter in favour of `row.target`; this also cuts `note_batch` and `file_batch` by one parameter.
  - Keep two candidate SQL statements behind one row mapper.
  - The `github/filing.rs` Test and Job body-building loops can stay apart, because their reads differ.
- **Churn:** about 120 lines, net about -70.

### CC4. should-fix. B6 is marked `fixed` while its own row lists open items.

**Where:** `routes/issues.rs:90-95`, `:123-128`, `:171-176`, `:203-208`; `routes/admin.rs:61`, `routes/reads.rs:67`, `routes/reports.rs:543`; 10 `if issues.is_empty()` sites.

```rust
match out {
    LinkedOut::Test(test) => Ok(Json(test)),
    LinkedOut::Job(_) => Err(RouteError::Refused(IssueLinkError::NotFound("Test not found".to_owned()))),
}
```

- The four impossible arms invent a 404 that cannot happen, and they are the bulk of the largest remaining pair.
- `identity_error` (twice) and `named_error` are one function under two names.
- The `Rules` collector was never built: the issue-list boilerplate stands at 10 sites (reports 1, reads 6, dto 3).
- **Reader cost:** a reader must work out that the arm is dead. The `fixed` state also tells the owner this is done.
- **Better shape:**
  - `LinkedOut: Serialize` (untagged), with handlers returning `Json<LinkedOut>`. The `utoipa` `body = TestOut` stays, and the parity replay proves the wire is unchanged.
  - One `identity_error` in `error.rs`.
  - `Issues::finish() -> ApiResult<()>`.
  - Reopen the row until this is done.
- **Churn:** about 60 lines, net negative.

### CC5. should-fix. T2 was closed by grep: the first review's named examples are unchanged in shape.

**Where:** `tests/github/filing.rs:1006`, `:1105`, `:721`, `:916`, `:965`, `:2014`; `tests/github/sync.rs:969-1045`.

- Six tests still run Test and Job scenarios in one body. `a_permission_403_costs_only_the_denied_repo` (97 lines) and `a_repo_whose_issues_are_disabled_is_skipped_for_the_rest_of_the_batch` (100 lines) are the two T2 named. The first also still asserts the 300-character log cap, a third concept.
- The ledger's evidence is `grep 'for entity in\|entity == "test"'`, which those tests never matched.
- `a_gone_issue_rearms_its_link_but_any_other_answer_keeps_it` was split by entity but not by concept; the "but" is still there. It also carries a pointless match:

```rust
match kind {
    EntityKind::Test => stamp_link(&db, EntityKind::Test, id, stamp().number(11)).await,
    EntityKind::Job => stamp_link(&db, EntityKind::Job, id, stamp().number(5)).await,
}
```

- **Reader cost:** a failure does not name the entity or the rule, and a bug in one half can hide behind the other. That is how A1 slipped by.
- **Better shape:** `denied_repo_scenario(kind)` with thin per-entity tests, a separate log-cap test, and `stamp_link(&db, kind, id, stamp().number(expected))`.
- **Churn:** about 250 lines in tests.

### CC6. should-fix. T4 was closed by rewriting the tuple rather than removing it.

**Where:** 20 sites, for example `tests/github/sync.rs:116`, `:160`, `:207`, `:230`, `:267`.

```rust
let link = link_of(&db, EntityKind::Test, test).await;
let (number, filed, _, closed) = (link.number, link.filed, link.checked(), link.closed);
```

- `grep '\.await\.[0-9]'` now answers nothing, but positional `_` slots came back. A struct that exists to name fields is immediately un-named.
- Translation debris:
  - `stamp().number(12)` already sets `filed = Some(true)`, yet `sync.rs:135-142`, `:182-189` and `filing.rs:878-885` then run `UPDATE ... SET github_issue_filed = TRUE`. That falsely implies the stamp did not set it.
  - `checked_at` (`sync.rs:29-42`) re-reads what `IssueLink.checked_at` already carries.
- **Better shape:** assert `link.number`, `link.closed` and so on directly, and delete the three updates and the helper.
- **Churn:** about 60 lines, mechanical.

### CC7. should-fix. Nine hand-written log captures, and one shared helper most of them ignore.

**Where:** `tests/github/support.rs:293`, `tests/github_worker.rs:830`, `tests/ops.rs:481`, `tests/admin.rs:576`, `tests/webhooks.rs:98`, `src/webhooks.rs:429`, `src/settings.rs:899`, plus same-named `struct CapturedLog` redefinitions at `tests/app.rs:317` and `db/tests/retention.rs:1215`.

- T5 is `fixed` and lists `CapturedLog` as moved to `flakeradar_test_support::doubles`, but the row does not mention these copies.
- These are dups.py's 19-line and 10-line pairs.
- `flaky_test` and `flaky_test_in` are still the 21-line pair T5 named (`support.rs:86-106`, `:221-241`).
- **Reader cost:** nine `Sink` impls to recognise as the same thing, and a local type shadowing the shared name.
- **Better shape:** `CapturedLog` plus `logged_call` in test-support, with `flaky_test(db)` calling `flaky_test_in`. The two `src/` unit-test copies need checking, since they test private items.
- **Churn:** about -200 lines.

### CC8. should-fix (judgement). The bulk-write descriptor costs more than the duplication it removed.

**Where:** `processing.rs:1606-1743`, also `:1420`, `:1452`, `:1567-1571`, `:1517`.

- `Column`, `UpdateTable`, two constants and `write_updates` take 138 lines. The two functions they replaced took about 100.
- The statement can no longer be grepped: `UPDATE test_cases AS t SET flakiness` now has zero hits, and the template is `"UPDATE {} AS {} SET {} FROM (VALUES {rows}) AS v({}) WHERE {}.id = v.id"`.
- `Column::name`'s doc, "The stored name of the column", is false for `reset_note`.
- Leftover plumbing from the two-copy era:
  - `crossed<T: Copy>(&[(i32, f64, T)], ...)` has one caller, which builds `(update.id, update.flakiness_score, *update)` from a struct that already holds those fields.
  - `scored_table(kind)` duplicates `table_of(kind).table`.
  - `rescore` and `rescore_jobs` only forward to `rescore_kind`.
- **Keep:** `rescore_kind` itself, `Window`, `Update::of` and `old_scores` read well. This is a trim, not a revert.
- **Better shape:**

```rust
struct BulkUpdate { head: &'static str, tail: &'static str, width: usize, bind: fn(&Update) -> Vec<Value> }
```

  `head` and `tail` hold literal SQL. `crossed` becomes `fn crossed(update: &Update, old, threshold) -> bool`. `pub fn rescore(db, kind, ids, ...)` replaces the two wrappers.
- **Churn:** about 100 lines, net about -50.

### CC9. should-fix. A comment repeated in 11 files claims one source of truth that does not exist.

**Where:** `mcp/tools/*.rs` (for example `top_flaky_tests.rs:34-38`, `:118`), `mcp/tools/mod.rs:92-107`.

```rust
/// The definition's output schema and the answer's `_meta` are both derived from
/// this one statement.
pub(super) const LISTS: bool = true;
...
ToolAnswer::list(&page.items)
```

- `LISTS` feeds only the output schema. `_meta` comes from the `ToolAnswer` variant the handler picks. A tool can set `LISTS = false`, call `ToolAnswer::list`, and still compile. Only the parity replay would notice.
- B10's "list-ness stated once" is really twice. That is better than the earlier three.
- **Better shape:** `ToolAnswer::shaped(LISTS, &value)` replaces both constructors, so the sentence becomes true.
- **Churn:** about 25 lines.

### CC10. should-fix. Job status and provider stayed stringly while Test status moved to enums.

**Where:** `core/dto.rs:1650`, `:1711`, `reads.rs:1459`, `:1468`, `:1639`, `processing.rs:1061`, `issue_links.rs:198`, `:272`, `reads.rs:578`.

```rust
pub last_status: String,          // JobOut; TestOut.last_status is ExecutionStatus
let unexplained = i64::from(status == "failed" && !explained);   // reads.rs:1639
job.github_issue_url = if job.provider == "github" {             // issue_links.rs:198
```

- `JobStatus` exists, and `processing.rs:1167` uses it, so B4's `fixed` is partial.
- `issue_links.rs:198-202` still duplicates `job_issue_url` (`reads.rs:577`), which B4 named.
- `ci_run_*` still pass `repo_id, repo_name, ci_run_id, attempt` together at 6 to 7 params (`reads.rs:1862`, `:1984`, `:2136`), and `CiRunId` stops at the pipeline DTO. That is C2's `CiRunAttempt` clump, which the ledger concedes it left.
- **Better shape:** type the two DTO fields with `JobStatus` and check that the wire stays lowercase. Make `job_issue_url` `pub(crate)` and call it. Add `struct CiRunAttempt { repo_id, repo_name, ci_run_id, attempt }`.
- **Unverified:** whether stored Job statuses can fall outside `JobStatus`. `reads.rs:1468`'s else arm tolerates others, so check this before tightening.
- **Churn:** about 80 lines.

### CC11. should-fix. `runner_stats` is the largest real function, unchanged, and B9 named its split.

**Where:** `reads.rs:1612-1708`, 97 lines both before and after.

- B9 said it "also splits into aggregate, rank and map". The ledger's B9 row covers only the tuples.
- The comparator carries two identical `#[expect]` blocks inside a `sort_by` closure.
- **Better shape:** `fn tally_runners(rows)`, `RunnerStat::unexplained_rate()` holding one `#[expect]`, and `fn rank`.
- **Churn:** about 40 lines moved.

## Earlier rows

**Truly closed, as far as I read:**

| Row | Evidence |
|---|---|
| B2 | Split with real seams; candidate SQL moved to `flakeradar_db::filing`. HeaderMap item not checked |
| B5 | `sql.rs` is clean, apart from the missing `execute` |
| B8 | `process_junit_report` narrates well |
| B12 | `TierFilter` is good; behaviour equals the old folded `q` filter |
| C1 | `Body` reads clearly and is not over-built |
| C3 | Except the public `py` module, and `kind` (31), `target` (19), `entity` (7) as three names for `EntityKind` |
| C5 | The items I read |
| T1 | Only helper and parity sleeps remain; `until_finished` makes negative assertions real |
| T3 | Sampled: assertions read report values; remaining log uses look justified |

**Falsely or partly closed:**

| Row | Why | Finding |
|---|---|---|
| B7 | Word swap | CC1 |
| T2 | Closed by grep | CC5 |
| T4 | Tuple rewritten | CC6 |
| B6 | Open items listed inside a `fixed` row | CC4 |
| B1 | Five table rows unmerged | CC3 |
| B3 | Transport arms bypass the table; functions did not shrink | CC2 |
| B4 | Job status and provider | CC10 |
| B9 | `runner_stats` split dropped | CC11 |
| B10 | List-ness stated twice | CC9 |
| T5 | Log sinks, `flaky_test` pair | CC7 |
| C2 | `CiRunAttempt` clump | CC10 |
| C4 | Parsing fixed; false comment survives | CC1 |
| B11 | Only the defaults half done. `github_fixed_streak <= 0` is still repeated, 18 `unwrap_or(i32::MAX)` remain, and no `GitHubSettings` view exists. Low cost, no separate finding | None |

**Not reviewed because of the budget cut:** `multipart.rs`, `extract.rs`, `lax.rs`, `webhooks.rs`, `worker.rs`, `queue.rs`, and the new test files `validation.rs` and `mcp_client.rs`.

# Second pass

Head ac5e6f3. This pass covers only the surface listed above, plus `settings.rs` and the test duplication. It does not repeat CC1-CC11, ST1-ST34 or SP1-SP15.

**How this was checked:** by reading every file in full, except `settings.rs` (lines 405-614). I ran `dups.py 8` once over the 60 files under `crates/*/tests/`, and it gave the same 652 pairs and 6193 lines as the README. I ran no cargo and changed no source. Function lengths and site counts are measured. Net line changes are estimates, because no fix was built.

**Verdict:** no must-fix, because no structural problem here hides a defect. `queue.rs` is clean. The most expensive items are in the tests: CC24 and CC25 remove about 1000 lines between them. The new ids have rows in `ledger.md`, added after this pass.

## Findings

### CC12. should-fix. CONFIRMED by reading. `file_field` walks the framing and decides the field in one loop

Where: `multipart.rs:54-100`, 47 lines. `:68` `delimiter_at.checked_add(framing.delimiter.len())`, `:79` `data_at + CRLF.len()`, `:85` `body.get(part_start..part_end)`, `:95` `part_end.checked_add(CRLF.len())`.

Cost: the file's one rule, "the last part that carries the field decides", takes 4 lines (`:91-93`), inside a loop with five `return Ok(found)` exits. Three of those exits cannot run: offsets inside a buffered slice do not overflow, and `next_end` found `part_end` at or after `part_start`. `:79` and `:151` use plain `+` on the same offsets, so a reader cannot tell which guard matters.

Fix: add `Framing::next_part(body, at) -> Option<(part, next_at)>`, or a `parts` iterator, and move the walk there with plain arithmetic. `file_field` then reads as boundary, framing, and a `for` over parts with the `match`. The reader gets the rule in about 8 lines and the framing separately.

Size: about 35 lines moved, about 8 fewer net, and three dead branches removed.

### CC13. should-fix. CONFIRMED by reading. `disposition` splits on `=` one byte at a time, on top of a copy of a standard method

Where: `multipart.rs:203-214` (two `Vec`s and a `valued` flag per parameter), `:233-243` `fn trim_ascii`, `:162-166` (the same split on `:`, written another way).

Cost: 12 lines of state machine do what `position` and `split_at` do in 3, and `part_field` already does that at `:162-166`. `trim_ascii` repeats `<[u8]>::trim_ascii`, stable since 1.80, under the workspace's `rust-version = "1.99"`. The comment at `:201` describes a check that sits at `:215`.

Fix: one private `split_once_byte(slice, byte) -> Option<(&[u8], &[u8])>` used at both places, and delete `trim_ascii` for the standard method, which trims the same `is_ascii_whitespace` set.

Size: about 20 lines fewer. No new type.

### CC14. should-fix, judgement call. CONFIRMED by reading. A hand-written KMP, with a test counter in the production search

Where: `multipart.rs:325-396`, 72 lines. `:372` `fn search(&self, haystack: &[u8], from: usize) -> (Option<usize>, usize)`, and the comment at `:369-371` defending the counter.

Cost: a reader must verify a failure table and a resume rule to trust a file about multipart framing. The return tuple and two `comparisons += 1` lines exist only so `a_crafted_body_is_scanned_in_one_pass` can count. `memchr` 2.8.3 is already in `Cargo.lock`, and `memchr-2.8.3/src/memmem/mod.rs:94-96` guarantees worst-case `O(needle.len() + haystack.len())`.

Fix: use `memchr::memmem::Finder` as a direct dependency. Keep the behaviour half of the test, and drop the counting half, because the crate's guarantee replaces it. `next_end`'s restart keeps the bound `standards-findings.md:556` reasoned out: occurrences cannot overlap, and each restart scans only to the next one. The owner decides whether a direct dependency beats 72 tested lines.

Size: about 72 production lines and 10 test lines fewer, 2 lines added, 1 direct dependency.

### CC15. should-fix. CONFIRMED by reading. An error kind passes through a string to reach a constructor

Where: `extract.rs:417-424` `other if is_integer_failure(&format!("expected {other}")) => "int_parsing"`, `:512-523` `match kind_for_type_name(expected_type) { "int_parsing" => ...`, `:365` and `:370` `kind == "missing"` twice, `:357` `issue_for(scope, ...)`, whose only caller passes `"query"`.

Cost: to learn whether a path parameter's type is an integer, a reader goes through `path_issue`, `kind_for_type_name`, a `format!` that builds a fake serde sentence, `is_integer_failure`, and `INTEGER_TYPES`. Then the answer is matched back from a string into a constructor.

Fix: `path_issue` matches `expected_type` directly: `"bool"`, `"f32" | "f64"`, and a guard over `INTEGER_TYPES` with `strip_prefix("expected ")`. Delete `kind_for_type_name`; `a_path_issue_names_the_type_it_could_not_read` already covers the same branches. In `issue_for`, merge the two `missing` tests into one branch and drop `scope`.

Size: about 15 lines fewer, one unit test folded.

### CC16. should-fix. CONFIRMED by reading. The module doc describes a JSON path that no longer exists

Where: `extract.rs:6-9`, "[`ApiQuery`] and [`ApiJson`] parse with the same `serde` machinery Axum uses, keeping the path to the failing field".

Cost: `ApiJson` (`:137-158`) parses to a `Map` and calls `T::from_object`, where `lax::Fields` builds each `loc`. `serde_path_to_error` runs only for the query, at `:189`. A reader who follows the doc looks for a body's field path in the wrong place. ST2's list does not name these lines.

Fix: say that the query goes through `serde`, the body goes through the model's `from_object`, and the path comes from Axum's rejection. It belongs with task 38.

Size: 4 lines of doc.

### CC17. should-fix. CONFIRMED by reading the two arms at `dto.rs:802-808` and `:822-828`, and the count by grep. Every `lax` reader's caller repeats the same four lines

Where: `lax.rs:92`, `:111`, `:226`, `:248`, `:393`, `:406` all return `Result<_, Box<ValidationIssue>>`. `dto.rs` has 16 `Err(issue) =>` arms and 15 `issues.push(*issue)`.

Cost: the arm `Err(issue) => { issues.push(*issue); None }` takes more lines in each field reader than the field's rule. `lax.rs` already has the accumulating style at `required` and `refuse_nul`, but offers no way to bring a `Result` into it. I found no such helper in `lax.rs` or `dto.rs`.

Fix: add `lax::recorded<T>(result, issues: &mut Vec<ValidationIssue>) -> Option<T>`. A reader becomes `let text = lax::recorded(lax::string(value, fields.loc(field)), issues)?;`, followed by its rule.

Size: 8 lines added, about 60 fewer in `dto.rs`. The arm at `dto.rs:1046` has no `fields` and may stay as it is.

### CC18. should-fix. CONFIRMED by reading. `post` is 69 lines of request, classification, logging and waiting

Where: `webhooks.rs:164-232`, four levels of nesting. `answer` is bound three times (`:169`, `:170`, `:178`). `let mut delay` (`:168`) is overwritten deep in a branch (`:219`). The retry rule is inline at `:201`: `!RETRYABLE_STATUSES.contains(&status) && status < 500`.

Cost: to answer "when does this retry, and how long does it wait?", a reader must trace one mutable and five exits across 69 lines. This is the policy ADR 0009 describes.

Fix: add `async fn attempt(&self, ...) -> Attempt`, with `enum Attempt { Stopped, Finished, Retry { after: Option<f64> } }`, holding the POST and its logs. `post` becomes a loop of about 15 lines: attempt, match, wait. Name the status rule `is_permanent(status)`.

Size: about 40 lines moved, about 10 added for the enum and its signature. No log changes.

### CC19. should-fix. CONFIRMED by reading. Seven aliases name one future shape, and the crate already has the generic one

Where: `worker.rs:58` `StartFuture`, `:152` `StopFuture`, `:183` `ProcessFuture`, `:214` `PruneFuture`, `:252` `WaitFuture`, `:271` `LockFuture`, `:274` `SessionFuture`. `mcp/mod.rs:58` has `pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;`.

Cost: `fn check(&self) -> SessionFuture<'_>` hides the one fact a reader wants, the output, behind a jump. `StartFuture` and `WaitFuture` are the same type under two names. `ProcessFuture` and `PruneFuture` take a lifetime that every signature fills with `'static`.

Fix: one public `BoxFuture<'a, T>`, promoted from `mcp`, so the signature reads `-> BoxFuture<'_, Result<(), DbError>>`. Delete the seven.

Size: mechanical. 33 uses outside `worker.rs` (18 in `doubles.rs`, 6 in `src/`, 9 in `tests/`) and about 16 inside it. Removes about 20 lines of alias and doc.

### CC20. should-fix. CONFIRMED by reading. `worker.rs` holds three modules, and the loop starts at line 513

Where: `worker.rs:57-180` (`LeaderRun`, `until_stopped`, `LeaderTask`), `:270-460` (`LockSession`, `AdvisoryLock`), and the rest (seams, settings, `ReportWorker`). `webhooks.rs:26` imports `LeaderRun`, `LeaderTask` and `until_stopped` from it.

Cost: a reader of the election loop scrolls past 512 lines first. The Notification task depends on a file that holds the advisory lock SQL. Aliases sit between unrelated items, for example `StopFuture` at `:152`, just after `until_stopped`.

Fix: a move only. `worker/leader_task.rs` takes about 124 lines, `worker/lock.rs` takes about 191, and `worker.rs` keeps about 484 with explicit `pub use` lines, so no path changes.

Size: about 315 lines moved, no logic changed. Do it after CC19 and CC21 to keep their diffs readable.

### CC21. should-fix. CONFIRMED by reading. `AdvisoryLock::check` asks the same question of `answer` twice

Where: `worker.rs:408-436`. `:408` reduces `answer` to `same_session`, `:415` branches on it, then `:419` matches `answer` again. `:366-371` is `match ... { Ok(row) => row, Err(error) => return Err(DbError::Database(error)) }`.

Cost: this code decides whether the process still owns the lock. A reader must hold two passes in mind to see there are only three outcomes.

Fix: one match, with the comments moved onto their arms: `Ok(Some(row)) if row.try_get::<bool>("", "same_session").is_ok_and(|same| same)` keeps the session, `Err(error)` keeps it and answers `Database`, and `Ok(_)` drops it and answers `LockSessionLost`. At `:366-371`, use `.map_err(DbError::Database)?` under the same comment. Keep the take and put-back, because their reason at `:339-341` is real.

Size: about 15 lines fewer, 50 lines down to about 35.

### CC22. should-fix. CONFIRMED by reading. In `validation.rs` the assertion ceremony outweighs the cases

Where: `tests/validation.rs`. 13 uses of `issues(&reply)`, 12 of them paired with a status assert, for example `:295-305`, 11 lines. 4 `ACCEPTED` blocks. 7 copies of `{"ci_job_id": "1", "name": "n", "status": "passed",`. 10 literal `("x-api-key", TOKEN)` header lists. `:773-781` and `:787-798` build the same `PUT` twice.

Cost: the header says each test "pins one recorded answer", but the answers are scattered among 800 lines. The rows of the tables are what a reader came for, and they are the minority.

Fix: four local helpers in this file: `assert_refused(&reply, kind, loc, label)` and `assert_accepted(&reply, label)` with `#[track_caller]`, `one_job(field, value)`, and `send_json(app, method, uri, &body)`. I checked that test-support has `send`, `get`, `post` and `request_with_headers`, and no JSON helper or refusal assert.

Size: about 100 lines fewer of 826. No test changes what it checks.

### CC23. should-fix. CONFIRMED by reading. One 77-line test checks four things, with three `and`s in its name

Where: `tests/mcp_client.rs:100-176`, `a_real_mcp_client_initializes_lists_its_tools_and_reads_a_test`: the handshake (`:105-115`), the tool list (`:117-135`), a read (`:138-156`), and a tool error (`:161-172`).

Cost: a failed handshake assert hides the other three, and the name cannot say what broke. The 4 `call_tool(CallToolRequestParams::new(..).with_arguments(arguments(&json!(..))))` sites span 6 to 10 lines each.

Fix: four tests over the existing `seed` and `connect`, and a `call(&client, name, json!(..))` helper. Each test gets 3 lines of setup and 2 of teardown.

Size: about 15 lines added by the split, about 15 removed by `call`. Net zero, four named failures.

### CC24. should-fix. CONFIRMED by reading the factories and the sites. Every seeded row costs a `build`, an `await` and a hand-written `expect`

Where: `.build(db.connection())` at 790 sites in 27 files, and 773 of them have an `.expect(` within two lines. All 10 builders in `flakeradar-test-support/src/factories.rs` return `Result<_, TestError>`. No builder has a `create` method.

Cost: this is the shape behind four of the seven rows in the table below. Most of those are two seeded rows that run to 8 lines, for example `flakeradar-db/tests/retention.rs:74` and `flakeradar/tests/ci_runs.rs:323`. A reader skims them to find what the test set up differently. The `expect` texts add nothing: "a Project", "the project" and "the Project inserts" mean the same.

Fix: add `pub async fn create(self, db: &TestDb) -> Model` to each of the 10 builders, panicking with the builder's name, and sweep the sites mechanically. Keep `build`, because `flakeradar-test-support/tests/factories.rs` (45 sites) and any transaction site need it. Separately, 145 test lines say `.repo("acme/app")`, which `make_project` already defaults to (`factories.rs:62-67`); dropping it where the repo is not the point saves another line each.

What this costs: about 60 added lines, and a panic location inside `factories.rs`, because an `async fn` cannot be `#[track_caller]`. The test name still appears. Unlike CC8, the addition is one small method per builder, and a reader never has to learn a descriptor.

Size: about 60 lines added, at least 773 removed, 2-3 lines per site where the chain then fits on one line.

### CC25. should-fix. CONFIRMED by reading. 37 calls to one function differ in one argument

Where: `github::sync_closed_issues(` at 31 sites in `tests/github/sync.rs` and 6 in `tests/github/notes.rs`, for example `sync.rs:107-114`. 26 pass `&gh_settings(&[])`, 10 pass `&settings`, 35 pass `&system_clock()`, 36 pass `&unstopped()`. One site (a stoppable call with its own clock) differs in more.

Cost: this is the largest cluster. A test that says "sync, then check" spends 7 lines on the verb, and the one argument that varies is hard to spot.

Fix: in `tests/github/support.rs`, which already holds `gh_settings`, `api` and `unstopped`, add `sync(db, &server)` and `sync_with(db, &settings, &server)`, both returning `BatchReport`. Neither name exists in `tests/github/` or test-support. Sites that pass a chosen clock or stop token keep the full call, so those two arguments stand out where they matter. `github::file_*_issues_for` has the same shape at 21 sites and can take the same treatment later.

Size: about 12 lines added, about 200 removed.

## Test duplication

`dups.py` reports pairs, so the count is quadratic in copies. A block with 15 copies is 105 pairs, or at least 840 of the 6193 lines. So 6193 overstates what can be removed, and part of the rise from 3833 likely reflects more copies of the same few blocks; I did not measure the old tree. To see real savings I grouped its windows with a throwaway copy of its normalization (`repro/clean-code/clusters.py`). The 12 largest clusters save 622 lines.

| Rank | Copies x lines | Saves | Where | What | Helper |
|---|---|---|---|---|---|
| 1 | 15 x 8 | 112 | `github/sync.rs` (13), `github/notes.rs` (2) | `sync_closed_issues` with default settings | `sync`, CC25 |
| 2 | 10 x 8 | 72 | `db/tests/spike.rs` (4), `test-support/tests/factories.rs` (3), `db/tests/score_history.rs` (2), `db/tests/relations.rs` (1) | a Project and a Test | `create`, CC24 |
| 3 | 9 x 8 | 64 | `db/tests/retention.rs` | a `backend` Project and a Test | `create`, CC24 |
| 4 | 8 x 9 | 63 | `flakeradar/tests/ci_runs.rs` | a `backend` Project and a Test | `create`, CC24 |
| 5 (tie) | 7 x 8 | 48 | `db` tests and `test-support/tests/factories.rs` | a Pipeline and a Job | `create`, CC24 |
| 5 (tie) | 7 x 8 | 48 | `github/sync.rs` | `sync_closed_issues` with chosen settings | `sync_with`, CC25 |
| 5 (tie) | 7 x 8 | 48 | `flakeradar/tests/issue_links.rs` | set-up, then a first `put` link | a local `link` over `put` and `one_test`, optional, about 30 lines |

So two helpers cover six of these seven rows. The three copies in `test-support/tests/factories.rs` test the builders, so they keep `build`.

Two smaller clusters are worth a local helper when those files are next touched. `tests/webhooks.rs` has 5 copies of 10 lines at `:216`, `:290`, `:319`, `:626` and `:644`: a 200 mock, then start, enqueue and wait. `tests/github/bodies.rs` has 3 copies of 13 lines at `:500`, `:563` and `:603`: create mock, file, then the existing `only_issue_body` at `:454`. I would not unify `flaky_test` and `flaky_test_in`, since their fixtures differ in root, suite and classname, though see the nit below.

## Possible defects, not clean-code

- `worker.rs:530`, `:631`: `ReportWorker` holds one `CancellationToken` for life, so a `start` after `stop` exits at once, which is what `:62-63` warns about for `LeaderRun`.
- `spec-findings.md:476` gives `INTEGER_RANGE` as about 9.22e27. The literal at `lax.rs:22` has 31 digits, so it is about 9.22e30. The defect is already recorded; the magnitude is not right.

## Nits

- `multipart.rs:145`: `Option<FieldPart>`'s `None` and `Other` mean the same at the only call site (`:91`); return `FieldPart`.
- `multipart.rs:452`: the test extends `body` and nothing reads it afterwards.
- `extract.rs:33-42`: `From<BodyRefusal> for ApiError` belongs in `error.rs`.
- `extract.rs`: `ApiError::unprocessable(vec![...])` nine times; `path_error`'s three parse arms differ only in `tail` (`:465-486`).
- `lax.rs:123-156`: `integer_from_number` takes `number` and `value`, two views of one value, and writes `Err(Box::new(...))` five times.
- `lax.rs:321-339`: `normalised_timestamp` nests three matches and calls `sign.unwrap_or_default()` on a sign it just matched.
- `dto.rs:114`: `lax_bool` lives in `dto` while `lax::boolean` lives in `lax`, and each module imports the other.
- `webhooks.rs:164`: `post` takes `self: &Arc<Self>` and uses no `Arc`.
- `webhooks.rs:35`, `:80`, `:225`: waits stay `f64` until the last line; `[Duration; 2]` removes the conversion, but keep `unwrap_or_default` so a NaN `retry-after` still means no wait.
- `webhooks.rs:278-326`: three constructors repeat `urls, format, public_url`, and `with_` means a constructor here but a consuming setter on `ReportWorker`.
- `webhooks.rs:426`, `:533`: the `Write` import and the `stdout().flush()` do nothing.
- `worker.rs:702-719`: `deliver` also sleeps out the backoff, which its name does not say; put the sleep in `run`'s arm.
- `worker.rs:99-104`, `:127-131`, `:618`: the "still running" test is written three times.
- `queue.rs:58`, `:107`: both `push` callers use only `.is_some()`, and no caller reads `finish`'s count; `:56`'s "the caller logs what it dropped" is not what they do.
- `validation.rs:809-810`: the comment names a non-object body, and the test sends only `{}`.
- `validation.rs:645-646`: the third repeated-parameter case asserts only the status, so it cannot see which value won.
- `validation.rs:110-122`: repeats `stored_pipeline`'s query; share a `newest_report(db)`.
- `validation.rs:546-550`: the `if` can go if `authorised_get` is used for every URI, PLAUSIBLE, not run.
- `github/support.rs:93-105`, `:228-240`: `flaky_test` and `flaky_test_in` share a Run and a failing Execution; extract only that part.

## Checked and clean

- `queue.rs`: 119 lines, one concept, and the longest function is 16 lines. Only the nit above.
- `settings.rs::from_map`, 100 lines at `:438-537`, and `validate`, 44 lines at `:566-609`: long but flat, one field per statement. A split would divide a table, and a macro or descriptor would repeat CC8. Length is not a cost here.
- `Waiter` (`worker.rs:255`): I expected `tokio::time::pause` to replace it. `RecordingWaiter` records delays (`doubles.rs:570`) and `GateWaiter` holds the loop at a wait (`doubles.rs:613`), and pausing the clock does neither. The seam earns its place.
- `WebhookShared::run_with` and `with_client_build`: each seam names the one failure a test could not stage otherwise (`webhooks.rs:118-119`, `:299-302`).
- `Fields::required` and `lax::refuse_nul` take `issues: &mut Vec<_>`, an output parameter that is correct for a validator that reports every failure.
- `ReportWorker::run`: 37 lines at one level of abstraction.
- `extract.rs::last_value_per_name`, `serialised`, `json_error_position`: small and single-purpose. ST28 owns the first one's cost.
- `lax.rs::text_int`, `digits_of`, `parse_timestamp`: read cleanly.

## Not reached

- `settings.rs` outside lines 405-614, including its tests and `dotenv_values`.
- `dto.rs` beyond the arms CC17 samples.
- Test clusters past the top 12, and the 40-pair listing beyond its first 11 rows.
- The first pass's "not compared" item stays open. I read `multipart.rs` and `extract.rs` for structure and did not compare them byte for byte against f9330fd.
