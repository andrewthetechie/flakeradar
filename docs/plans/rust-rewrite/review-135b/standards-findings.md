# PR #135 standards re-review: f9330fd..ac5e6f3

Head ac5e6f3. Read-only: the repo working tree is untouched and no worktree was created. Line numbers are at ac5e6f3.

## Captured checks

`ctx/checks.json`: 19 checks, no `bucket: fail`. 18 pass, 1 skipped ("Merge the manifest list"). fmt, clippy `-D warnings`, nextest, doctests, `cargo doc` and audit are green, so nothing below re-reports a lint that fires.

## Sources read

Read in full: `docs/agents/code-standards-rust.md`, `AGENTS.md`, `CONTRIBUTING.md`, `Cargo.toml`, `.config/nextest.toml`, `clippy.toml`, `smell-baseline.md`, ms-rust `SKILL.md` and guideline files 02, 03, 04, 06 (a header only, no rules), 08, 11, 14, 15.

Partly read: `docs/plans/rust-rewrite/00-fixes-context.md` (the "Tests that prove something" section). `review-135/ledger.md` was read only by row, as a claim to test.

Code coverage of this review, stated so nobody assumes more:
- Read in full: `worker.rs`, `queue.rs`, `webhooks.rs`, `github_worker.rs`, `github/{mod,client,sync,notes}.rs`, `error.rs`, `extract.rs`, `types.rs`, `sql.rs`, `limits.rs`, `defaults.rs`, `test-support/src/{wait,doubles,app,clock}.rs`, `tests/source_hygiene.rs`, all four `lib.rs`.
- Read by hunk or grep only: `dto.rs`, `parsing.rs`, `lax.rs`, `processing.rs`, `reads.rs`, `retention.rs`, `settings.rs`, `github/{filing,bodies,policy}.rs`, `mcp/*`, `routes/*`, and the files under `tests/`.

## Commands that settled findings

The hygiene test was compiled alone with `rustc` against a scratch tree, so no cargo target directory was used:

```
cd <scratch>/hyg
CARGO_MANIFEST_DIR=<scratch>/hyg/crates/fake rustc --edition 2024 --test source_hygiene.rs -o bin/hyg
./bin/hyg      # loophole samples: 2 passed. With a control line added: 2 failed, on the control only.
cargo metadata --no-deps --offline --locked --format-version 1   # rust_version is None for all four packages
cargo tree --locked --offline -p flakeradar -e normal | grep -c test-support      # 0
cargo tree --locked --offline -p flakeradar-core -e normal | grep -cE 'sea-orm|axum|reqwest|tokio v|hyper'   # 0
```

Not run: `cargo clippy` (no reason to doubt CI) and nextest (nothing here depends on a run). Findings are CONFIRMED by reading unless they say otherwise.

---

# HARD VIOLATIONS

## ST1. HARD, must-fix. The marker is on every file again (H11 falsely closed)

`code-standards-rust.md:18`: "Add it only to files you have actually checked — a stale marker is worse than none." Status: CONFIRMED.

150 of 150 non-generated `.rs` files carry `// Rust guideline compliant 2026-07-21`. Commit 515f0b9 and the ledger say each was read end to end against a checklist that includes "prose about this API rather than the deleted one" and "named and structured log events". Each of these fails that same checklist, and each carries the marker:

| File | Fails |
|---|---|
| `flakeradar/src/routes/reports.rs` | ST2 (`reports.py`) |
| `flakeradar/src/github/sync.rs` | ST2 (`_clear_if_closed` docstring) |
| `flakeradar/src/worker.rs` | ST2, ST4 |
| `flakeradar-db/src/retention.rs`, `processing.rs` | ST4 |
| `flakeradar-core/src/parsing.rs`, `lax.rs` | ST2 |
| `flakeradar/src/settings.rs`, `routes/reads.rs` | ST6 |
| `flakeradar-core/src/limits.rs` | ST6, ST9 |
| `flakeradar-core/src/types.rs`, `flakeradar-db/src/sql.rs`, `flakeradar/src/error.rs` | ST9 |
| `flakeradar-db/tests/retention.rs` | ST7, ST8 |

Fix: remove the marker from every file a finding here names, and add it back as each one is fixed.

## ST2. HARD, must-fix. Docs still describe the deleted implementation (H10 falsely closed)

`code-standards-rust.md:92-94` (M-NO-META-DESIGN-DOCUMENTATION): "rustdoc describes end-state behavior." H10 named "the reference" as one of the banned forms. Status: CONFIRMED.

21 lines in `crates/*/src` (17 rustdoc, 4 `//`), a further 49 in `crates/*/tests`. All 21 sit in files carrying the marker. The worst, which name a deleted file or function:

```rust
routes/reports.rs:320  /// Checks the range `reports.py` put on `limit`.
github/sync.rs:84      /// Stamping stays as the reference's `_clear_if_closed` docstring states: every row
worker.rs:477          /// ceiling the reference's own backoff used. Removing it would let a long
issue_links.rs:184     /// Applies the written link to the response shape, as the reference's ORM object did.
parsing.rs:550         /// The un-namespaced attribute `name` of `node`, as `ElementTree`'s `.get` reads it.
```

The rest: `lax.rs:434`; `parsing.rs:396,428,484,543,559`; `db/filing.rs:143,176`; `server.rs:41,116`; `app.rs:363`; `github/policy.rs:150`; `multipart.rs:81,143,290`; `github/bodies.rs:146`.

Plan files are still pointed at from rustdoc, which H10 also named: `reads.rs:5` (`spike-notes.md`), `mcp/mod.rs:440` (`differences.md`), `core/lib.rs:8` (`wire-notes.md`), `main.rs:27,99`. A pointer to `docs/plans` is allowed for rationale by `code-standards-rust.md:93`, so treat these as the weaker half.

The ledger's H10 row reports its "Done-when grep" is clean. That grep's word list omits "reference", so it never looked for this. Fix: state the rule and drop the comparison (`reports.rs:320`: "Checks the range `limit` allows").

## ST3. HARD, should-fix. Both source-reading tests encode loopholes (task 30's claim)

`crates/flakeradar/tests/source_hygiene.rs`. Rule: M-TAUTOLOGICAL-TESTS in spirit, and the file's own header claims "that every log event carries a name" and "no comment or identifier in `crates/` points at the implementation this one replaced". Status: CONFIRMED BY RUNNING.

Every line below is in the scratch tree, and both tests pass:

```rust
//! Mirrors `reports.py` and the reference's `_clear_if_closed` docstring.
//! Set up in tasks 16-31, and see Task-12, plus task, then task 7.
use tracing::{info, warn};
    let message = format!("value {x}");
    tracing::error!(name: "a.b.c", "{message}");          // H16's exact shape
    info!("unnamed through an import {x}");
    warn!("unnamed through an import");
    tracing::event!(tracing::Level::INFO, "unnamed event macro");
    tracing::info!(name: "a.b.d", "inline format {x}");
    tracing::
        warn!("split across a line");
```

A control (`tracing::info!("…")`, `// task 12 and python here`) fails, so the harness is sound. The defects:

- `:193` matches only the literal `tracing::`. `retention.rs:26` has `use tracing::info;`, so its four events (`:428`, `:456`, `:501`, `:516`) are not checked at all. They do carry names, by reading, but the test does not know that.
- `:211` looks for `format!(` inside the call only, so `let message = format!(…); tracing::error!(name: …, "{message}")`, the deliberate dodge H16 quoted, passes. Inline interpolation passes too, which is ST4.
- `:50-59`: `REPLACED` omits "reference", `.py`, `ElementTree`, `sqlalchemy` and `alembic`, which is how ST2 stays green.
- `:176-186`: the doc says it matches `task 12` or `tasks 16–31`. `find("task")` looks at the first occurrence only and `rest.trim_start()` then starts with `s`, so `tasks 16` is never matched, and neither is a later `task 7` on a line with an earlier "task". The doc comment is false.
- `:67-123`: `ALLOWED` is per file and per word, so a comment may say "fastmcp" anywhere in `mcp/mod.rs`. Nothing checks that an entry is still needed.

Fix: add `the reference`, `.py`, `ElementTree` to the list; detect `info!`-family macros with or without the path; fail on any message holding a `{ident}`; fix the task-number scan. Or delete the tests, and let the review checklist carry the rule. A guard that passes on the violation it names is worse than none.

## ST4. HARD, should-fix. Five events still format their message (H16 falsely closed)

ms-rust 11 M-LOG-STRUCTURED, "Avoid String Formatting": "String formatting allocates memory at runtime. Message templates defer formatting until viewing time." Status: CONFIRMED.

All events are named, which is a real gain. Task 30 also claims "unformatted", and the ledger says the five sites are gone. They are the same five H16 listed, with the `format!` moved into the macro:

```rust
worker.rs:712      "the {task} leader task failed to take processed Report {report_id}: {error}",
worker.rs:770      "stopping the {task} leader task failed; releasing the lock anyway: {error}",
processing.rs:1861 "Rescore of Repo {repo_id} dropped {dropped} Notification(s)"
retention.rs:504   "Retention prune hit its {budget}s budget; the rest waits for the next interval",
retention.rs:525   "Pruned {reports} reports, {failed_reports} failed reports, {executions} executions, ...
```

The fields are already named properties on each call, so every value prints twice. The `tracing` macro formats only when the event is enabled, so the cost is small. These are not hot paths, and the failure here is the claim and the rule, not throughput.

Fix: static messages, as `webhooks.rs:193-198` does. Three tests pin the sentences (ST7) and must change with them.

## ST5. HARD, should-fix. The newtypes stop at the module edge (H6 partly closed)

`code-standards-rust.md:115-117`: "Strong types over primitive obsession (M-DESIGN-FOR-AI): a newtype for a fingerprint, a score, a CI run ID." Baseline Primitive Obsession. Status: CONFIRMED.

`types.rs` itself is good: fallible constructors, no infallible `From`, guarded `Deserialize`, and `from_digest` is `pub(crate)`. But the types barely travel. Outside `types.rs` the whole workspace has these uses:

- `Score`: only `scoring.rs`, the two functions that return it. The callers unwrap with `.as_f64()`.
- `Fingerprint`: `parsing.rs:174`, and `processing.rs:640`, which immediately takes `.as_str().to_owned()` (`:783`).
- `CiRunId`: `dto.rs:1081` on `PipelineReportIn`. `routes/reports.rs:137` and every other site keep `String`.

Every site H6 quoted is still a primitive:

```rust
dto.rs:1452,1910                     pub fingerprint: String,
dto.rs:1464,1597,1639,1964,2003      pub flakiness_score: f64,
dto.rs:1345,1562,1719,2022           pub ci_run_id: String,
notify.rs:46                         pub flakiness_score: f64,
db/filing.rs:78,80,93,360            pub fingerprint: String / pub flakiness_score: f64 / pub score: f64
db/reads.rs:1865,1916,1949,1988,2140 ci_run_id: &str,
db/reads.rs:506,550,662,1062,1389    let score: f64 = get(row, "flakiness_score")?;
```

`types.rs:97-98` says "A Test's or Job's score carries this". On every read path, it does not. `helpers::tier_for(score: f64, threshold: f64)` still takes two swappable floats.

Fix: since `Serialize` is transparent, move the DTOs and read structs to the newtypes. Reading from a row goes through the fallible constructor, and a bad stored value becomes a `DbError`, as `UnknownExecutionStatus` already does.

## ST6. HARD, should-fix. Inline defaults and bounds remain (H12 partly closed)

`code-standards-rust.md:122-124` and ms-rust 11 M-DOCUMENTED-MAGIC. Task 30 claims every bound is a named constant with a reason. `defaults.rs` and most constants are real improvements; these were missed. Status: CONFIRMED.

```rust
settings.rs:360   github_issue_min_score: 0.30,         // not defaults::FLAKE_THRESHOLD, and no name of its own
settings.rs:363   github_sync_batch: 2000,
settings.rs:371   worker_poll_seconds: 1.0,
settings.rs:378   prune_interval_seconds: 3600.0,
routes/reads.rs:304,666  page_size: …unwrap_or(50),     // the default page, written twice
processing.rs:1075,1078,1079  chunk.len() * 13 / index * 13 / value_row(base + 1, 13)
identity.rs:83    value.chars().count() > 255
github/filing.rs:138  i64::try_from(BODY_EXECUTIONS).unwrap_or(10),   // restates the constant's value
```

`settings.rs:309,325,339` document the field and give no reason for the number. `limits.rs:10-12` names `LIST_LIMIT_MAX` and states no why, no cost of change and no external system, which is exactly H12's "named but saying only what".

Fix: named constants in `defaults.rs` with the three-part doc, one `DEFAULT_PAGE_SIZE`, a named column count at `processing.rs`.

## ST7. HARD, should-fix. Log text used as the outcome (T3 falsely closed)

`00-fixes-context.md:41`: "Assert the outcome a user or the database sees ... Log text is not an outcome." The ledger: "CLOSED: no test asserts a `tracing` line where the outcome has a carrier." Status: CONFIRMED.

- `flakeradar-db/tests/retention.rs:761`: the **only** assertion in the test. Nothing else proves two Notifications were dropped, since `prune` does not return the count.
  ```rust
  assert!(log.lines().contains("dropped=2"), "the prune counts both dropped Notifications: {}", log.lines());
  ```
- `retention.rs:305`: the carrier exists and is asserted four lines above (`assert_eq!((result.reports, result.failed_reports), (1, 2));`), then `log.lines().contains("2 failed reports")` pins the sentence. This one directly contradicts the ledger.
- `retention.rs:1083`: pins the exact sentence "with the recorded wording", while the comment above it concedes "log format may differ".
- `tests/worker.rs:555`, `:658`: pin full interpolated sentences, which is what makes ST4 expensive to fix.
- `tests/github_worker.rs:1701`: redundant after `dropped(worker.queue())` was asserted just above.

Not flagged: `github/filing.rs:1053,1251,1315,1635`, whose comments state why no returned value carries the text. Those are honest. Neither are the secret-absence checks (`admin.rs:621`, `webhooks.rs:409`) or `tests/app.rs:229`, where the log line is the subject.

Fix: return the dropped count on `PruneResult`, and delete the redundant log assertions.

## ST8. HARD, should-fix. Log-capture rigs outside test-support (H14b partly closed)

`code-standards-rust.md:156`: "Test-only helpers ... live in `flakeradar-test-support`." Baseline Duplicated Code. Status: CONFIRMED.

`flakeradar_test_support::CapturedLog` (`doubles.rs:692`) exists, and `tests/worker.rs` uses it. Seven private rigs remain:

- `flakeradar-db/tests/retention.rs:1215-1250`: `struct CapturedLog` and `struct SharedWriter`, the same bodies. That file already imports test-support at `:24`.
- `flakeradar/tests/app.rs:325-350`: the same again.
- `tests/ops.rs:499`, `tests/admin.rs:604`, `tests/github_worker.rs:848` (`logged_while`), `tests/github/support.rs:311` (`logged_while`, a second copy), `tests/webhooks.rs:115` (`captured`).

`coverage-gaps.md` carries three separate rows just for these writers' `flush`.

Not flagged: the two unit-test sinks at `webhooks.rs:460` and `settings.rs:917`. Test-support depends on `flakeradar`, so a `#[cfg(test)]` module in that crate would see two instances of its types. That is a real constraint.

Fix: use `CapturedLog`, adding a `during(future)` method if the `logged_while` shape is wanted.

## ST9. HARD, should-fix. Public-API tests in `src/`, and one tautology (H20 partly closed)

`code-standards-rust.md:130-135`: "Tests that only touch a public API are integration tests and live in `tests/`" and "never restate ground truth". Status: CONFIRMED, by checking every name each module imports from `super` against a `pub` item.

The ledger's closure moved the six modules H20 named. These `#[cfg(test)]` modules import only public items, and several are new in this diff:

| Site | Lines | New here |
|---|---|---|
| `core/src/scoring.rs:309` | 293 | no |
| `core/src/types.rs:233` | 134 | yes |
| `flakeradar/src/error.rs:238` | 126 | largely |
| `db/src/sql.rs:207` | 44 | yes |
| `db/src/migration.rs:217` | 43 | no |
| `test-support/src/junit.rs:36` | 34 | no |
| `core/src/limits.rs:20` | 12 | yes |

`limits.rs:26-30` is a test that cannot fail for any reason but a deliberate edit of the same number:

```rust
fn the_shared_bounds_are_the_documented_numbers() {
    assert_eq!(LIST_LIMIT_MAX, 100);
    assert_eq!(NEW_WITHIN_DAYS_MAX, 90);
}
```

Fix: move them to `tests/`, and delete the `limits.rs` test. The 422s at the routes already pin the bounds.

# JUDGEMENT CALLS

## ST16. JUDGEMENT, should-fix. Cancelling a comment `POST` can repeat it

Cancellation safety of `until_stopped`. `github/notes.rs:115`, `:264`; `worker.rs:141-149`. Status: CONFIRMED by reading, not run. `differences.md` row 16 records the cancel as intended; it does not, in the part I read, weigh this cost.

```rust
let Some(answered) = until_stopped(stop, api.comment(&row.repo, row.number, &body)).await
```

Dropping this future does not un-send the request. If GitHub received it, the comment lands, no stamp is written, and the next Report comments again. `tokio::select!` is unbiased too, so a stop arriving with the answer can discard a completed `201`. This is the same failure `github/mod.rs:55-58` refuses to accept for the issue `POST`: "cancelled between creating the issue and storing its number strands an issue". Two docs state otherwise: `notes.rs:91-92` ("a comment that never posted stamps nothing") is true only of one that never reached GitHub, and `github_worker.rs:14-15` is garbled: "end at once, because neither can be interrupted without costing more than a retry".

The close has a related gap: the note is stamped at `:141` before the cancellable close at `:264`, so a cancelled close is never retried and the issue stays open.

A sync `GET` and a Notification are fine: the first is read-only, the second best-effort by ADR 0009.

Fix: treat the comment as the filing `POST` is treated, run it through to the stamp, then stop. If the duplicate is accepted, say so in the doc.

## ST17. JUDGEMENT, should-fix. Half of the 20 MiB JSON work stays on the async thread (J4 partly closed)

M-YIELD-POINTS. Status: code CONFIRMED, cost PLAUSIBLE, not measured.

`extract.rs:146-157` moves the parse off the thread, and its comment gives the reason: "A Pipeline body can be 20 MiB, and reading it as JSON is CPU work". The two steps after it, proportional to the same input, stay on the async worker:

```rust
extract.rs:157         Ok(Self(T::from_object(&object)?))        // validates every Job in the body
routes/reports.rs:590  body: pipeline_body(&payload),            // serde_json::to_vec of the whole Report
```

The second runs inside an open transaction. Dropping the parsed `Map` is also on that thread. Fix: run `from_object` inside the same `spawn_blocking`, and serialize there too.

# NITS NOT WRITTEN UP

Left out at the coordinator's request, to save budget; ids are kept so references below resolve. Each was verified, except ST22 and ST23 (PLAUSIBLE).

ST10 real-clock test reads (`tests/reads.rs:1180`, `db/tests/spike.rs:42`). ST11 three per-file `#![expect]` in test-support (`app.rs:11`, `doubles.rs:11`, `links.rs:5`). ST12 `#[expect(unused_imports)]` hiding a dead alias (`tests/github/support.rs:13`). ST13 `rust-version` is not inherited, so `cargo metadata` shows `None`. ST14 `DbError` variants no caller branches on. ST15 `ApiError` has no `Display`. ST18 duplicated code. ST19 Middle Man and test-only items. ST20 `CiRunId` accepts NUL. ST21 small residue, including stale `clippy.toml` idents. ST22 `within` overclaims. ST23 coverage counts test-support.

# TASK 30 CLAIMS, ONE BY ONE

| Claim | Verdict |
|---|---|
| Both lint opt-outs and the one `#[allow]` gone | TRUE for those named. Three new per-file ones elsewhere (ST11) |
| Compliance markers mean something | FALSE (ST1) |
| One path per item | TRUE. No public module's item is also re-exported; `flakeradar-db`, `github/` and `test-support` re-export private modules only |
| Every bound a named constant with a reason | FALSE at the sites in ST6 |
| Every log event named and unformatted | Named: TRUE. Unformatted: FALSE (ST4) |
| Two source-reading tests | They run, and encode loopholes (ST3) |

CI changes: the `cargo doc -D warnings` step is correct and matches `code-standards-rust.md:45`. `.fabro/test.toml` is a comment change. No `cfg_attr`, `coverage(off)` or `#[ignore]` was added as a dodge.

# EARLIER ROWS

**Spot-checked and truly closed:**

- H2: `checked_mul` at `reads.rs:434`.
- H4: `settings.rs:1354-1366` asserts the value is absent.
- H5: `Settings.database_url` and `AdvisoryLock.url` are `Secret`.
- H8: `Utc::now` in `src` is `clock.rs` only.
- H9: the two sites it named.
- H13: `ProcessError` is `pub(crate)`, no `Result<_, String>`.
- H15: all workspace-declared, `schemars` and `testcontainers` gone.
- H17: the Rust names are renamed. `confirmed` survives only as a SQL alias and a local in `reads.rs:842,854,1729,1741`, which is borderline.
- J1: no transaction across a `GET` in `sync.rs`.
- J3: `BatchQueue` is bounded.
- J6: one `UPDATE … RETURNING`.
- J7a: one `get` and `rows`, in `sql.rs`.
- J9: only the advisory-lock SQL stays in the server crate, and `IssueLinkError::status` is gone.
- J11: constructors return the error.
- J13: both silent drops are gone.
- J14: the `let _ = settings` sites are gone.
- J18: `position % 10` at `parsing.rs:920`.

**Falsely closed:** H10 (ST2), H11 (ST1), H16 (ST4), T3 (ST7), J17 M-MSRV (ST13).

**Partly closed:** H6 (ST5), H12 (ST6), H14b (ST8), H18 (ST21), H20 (ST9), J4 (ST17), J17 M-PUBLIC-DISPLAY (ST15).

**Not re-checked here:** H0, H1, H3 and H19 behaviour (spec axis). `Scope` (J10), J12, J15.

# CHECKED AND CLEAN

- Production panics: the sites are the same shape as before, on literals or impossible serialization, each with `#[expect(..., reason)]`. `scoring.rs:190`'s `Score::new(...).expect(...)` relies on a non-negative decay, which `settings.rs:152` enforces for anything from configuration. No `unwrap` or `expect` on input or I/O.
- `as` casts: `scoring.rs:55`, `lax.rs:164,376,380,385`, `dto.rs:969`, `reads.rs:1679,1684`, each bounded with a reason. I checked `lax.rs:372-385` for overflow: `whole as i64 - 1` needs a fraction, and a float at `i64::MIN`'s magnitude has none.
- Locks: no `std` guard across an `.await`. `LeaderRun::stop` drops its guard before awaiting the handle.
- Channels: the only unbounded one is in `Unanswered`, a test helper.
- Secrets: `Api`, `AdvisoryLock`, `WebhookShared` and `GitHubShared` print no secret through `Debug`.
- `flakeradar-test-support` is in no normal dependency tree, and `flakeradar-core` still has no SeaORM, Axum, Tokio, Hyper or Reqwest (`cargo tree`).
- `RouteError`, `ApiJson`, `LeaderRun`, `sql`, `github/` and `mcp/`: public docs present and `# Errors` / `# Panics` where they apply, with `cargo doc -D warnings` green in CI.

# OUTSIDE THIS DIFF, FOR THE RECORD

`crates/flakeradar/tests/server.rs:296`: `for _ in 0..4 { tokio::task::yield_now().await; }` stands in for "the upload is still being read", which is a scheduling guess rather than a signal, against `code-standards-rust.md:140`. It predates this range (line 287 at f9330fd), so it is not a finding against tasks 16-31. PLAUSIBLE race, never seen failing.

# Second pass

Head `ac5e6f3`. This pass covers only what the first pass did not reach. No must-fix: nothing here has a known trigger that loses data, hangs or crashes in normal use. ST24 and ST25 are the two to read first.

**Hard violations:** ST25, ST26, ST27, ST29, ST30, ST32, ST33. **Judgement calls:** ST24 (no rule names it, but `worker.rs`'s own docs state the invariant it breaks), ST28, ST31, ST34.

### ST24. should-fix. CONFIRMED by reading: a leader with a backlog never checks its lock, so two processors can run

Where: `crates/flakeradar/src/worker.rs:661-672`.

```rust
Ok(Some(outcome)) => self.deliver(&outcome).await,   // no check on this arm
Ok(None) => {
    if let Err(error) = self.lock.check().await {
```

`check` runs only when the queue is empty. If the lock connection dies while the main pool stays healthy, the server frees the lock and a standby takes it within `STANDBY_WAIT`. The old leader keeps claiming, pruning and filing until its queue drains, which under steady ingest may be never. `FOR UPDATE SKIP LOCKED` (`processing.rs:237`) lets both processes work side by side on different Reports.

Rule: ADR 0002 as quoted at `worker.rs:4-5` ("exactly one processor runs"), and `worker.rs:758-760` ("would let a replacement leader take the lock and file the same issue twice"). The existing tests kill the leader with an empty queue only (`tests/worker.rs:790`, named `..._when_the_queue_is_empty`). I did not check whether the Python reference has the same gap, and the invariant does not depend on that.

Not established: how badly the scores suffer. A later Report may recompute them, so I rate this should-fix rather than must-fix.

Fix: call `self.lock.check()` once per loop, at the top of the leader branch, before `maybe_prune`. That costs one short statement per Report. Delete the check in the `Ok(None)` arm.

Red test: `a_leader_with_a_backlog_checks_its_lock_between_reports`. Use `with_processor` with a processor that always answers `Some`, and a `LockSession` whose `check` fails. Assert leadership drops and claims stop. It fails today because `check` is never called.

### ST25. should-fix. CONFIRMED by reading: a panic in the worker loop leaves a process that holds the lock and processes nothing

Where: `worker.rs:621` and `worker.rs:634`.

```rust
*task = Some(tokio::spawn(async move { worker.run().await }));
...
let _ = handle.await;
```

Tokio contains the panic to that task. Nothing watches the handle until shutdown, and at shutdown its result is discarded. The session stays alive inside `Arc<ReportWorker>`, so the lock stays held and `is_leader` stays true.

- Standbys get `Ok(false)` forever, and the queue stalls until someone restarts the process.
- HTTP keeps answering. `grep is_leader` in `routes/` and `app.rs` finds nothing, so no health answer shows the stall.
- The release profile sets no `panic` (`Cargo.toml:68-72`), and `src` has no `catch_unwind` or panic hook.
- A leader task's loop has the same gap: its panic surfaces only at `LeaderRun::stop`, which is what `tests/worker.rs:872` pins.

No known input triggers a panic here, so this is should-fix. With a trigger, it is a must-fix hang.

Rule: M-PANIC-IS-STOP ("invoking a panic means _this program should stop now_"; "It is not valid to ... assume panics will be caught"). M-PANIC-CONTINUATION ("should still promote an application restart").

Fix: have `Server::serve` select on the worker loop ending as well as on the shutdown signal. A loop that ended before a stop releases the lock and makes `serve` return an error, so the process exits and restarts. `panic = "abort"` is the shorter fix, but it turns a parser panic in `spawn_blocking`, today a 500, into a process exit. That choice is the owner's. Either way, log the `JoinError` at line 634.

Red test: a processor that panics on its first claim. Assert with `wait_until` that the lock becomes free. Quoted signature: `pub async fn wait_until<F, Fut>(what: &str, mut condition: F)`. It reaches its deadline today.

### ST26. should-fix. CONFIRMED by running: `DB_COMMAND_TIMEOUT_SECONDS=1e30` panics at start-up, past both of its checks

Where: `crates/flakeradar-db/src/connect.rs:47-54`, and `settings.rs:159-160`, which has the same hole.

```rust
if !command_timeout_seconds.is_finite() || command_timeout_seconds <= 0.0 {
...
command_timeout: Duration::from_secs_f64(command_timeout_seconds),
```

Command, using the `target/debug/flakeradar` built after the head commit, without rebuilding:

```
FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS=1e30 FLAKERADAR_DATABASE_URL='postgresql://u:p@127.0.0.1:1/d' FLAKERADAR_API_TOKEN=x target/debug/flakeradar
panicked at .../library/core/src/time.rs:962:23:
cannot convert float seconds to Duration: value is either too big or NaN
```

Control: the same command with `=0` answers `FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS: expected a number of seconds more than 0, got '0'`.

Rule: `code-standards-rust.md:70` (panics are for programming errors) and M-PANIC-IS-STOP. The doc at `connect.rs:43-45` claims the check covers what `from_secs_f64` "cannot represent". It does not.

Fix: `Duration::try_from_secs_f64(value)`, mapping the error to `DbError::InvalidTimeout`.

Red test: add `1e30` to the list in `rejects_a_timeout_postgres_cannot_take` (`connect.rs:260`). It panics today.

### ST27. should-fix. CONFIRMED by running for the conversion and the validation; the loop itself CONFIRMED by reading only

Where: `worker.rs:509-511` and `server.rs:122-123`.

```rust
fn seconds(value: f64) -> Duration {
    Duration::try_from_secs_f64(value).unwrap_or_default()
```

`interval` (`settings.rs:159`) accepts any finite value above 0, but a value `Duration` cannot hold silently becomes zero. So `WORKER_POLL_SECONDS=1e30` or `1e-12` gives a poll of 0, which is the case `settings.rs:558-559` says stops the process at start-up ("a poll of 0 made the worker query with no wait"). `PRUNE_INTERVAL_SECONDS=1e30` gives a prune on every loop and a zero GitHub sync interval.

Measured:

- With `rustc -O`, `a throwaway `q.rs`, not kept,` prints `try_from 1e30 = Err(TryFromFloatSecsError { kind: OverflowOrNan })` and `try_from 1e-12 = Ok(0ns)`.
- `FLAKERADAR_WORKER_POLL_SECONDS=1e30` with the same URL and token passes validation and stops only at `pool timed out`.

I did not run the hot loop against a database.

Rule: `code-standards-rust.md:122` for a bound with a reason, and a swallowed failure on a validation path.

Fix: `interval` also requires `Duration::try_from_secs_f64(value).is_ok_and(|wait| !wait.is_zero())`. The two `unwrap_or_default` calls can then become a stored `Duration`. This one change also closes the settings half of ST26.

Red test: `Settings::from_map` with each of `1e30` and `1e-12` for `WORKER_POLL_SECONDS` and `PRUNE_INTERVAL_SECONDS` is refused. It is accepted today.

### ST28. should-fix. CONFIRMED by reading (quadratic); the cost was measured on a copy, not on a server

Where: `crates/flakeradar/src/extract.rs:79` and `:213-222`.

```rust
for (name, value) in form_urlencoded::parse(query.as_bytes()) {
    match pairs.iter_mut().find(|(seen, _)| *seen == name) {
```

This runs on the async thread for every `ApiQuery` route. The `http` crate allows a URI of `MAX_LEN = u16::MAX - 1` (`http-1.5.0/src/uri/mod.rs:145`), which fits about 16,000 distinct three-letter names. A release-mode copy of this loop in that `q.rs` took 383 ms for 16,384 names. That is one runtime thread stalled per request. The copy uses owned strings, and I did not send the request to a real server.

Rule: M-YIELD-POINTS. `multipart.rs:9-12` already treats the same shape as "a quadratic attack on the thread that serves the upload". The deployment is internal (see the auth note in project memory), which is why this is should-fix.

Fix: a `HashMap<String, usize>` from name to index in `pairs`. It is only looked up, so `iter_over_hash_type` is not involved.

Test: none goes red without timing, which the standard forbids.

### ST29. should-fix. CONFIRMED by reading: docs on the shutdown and retry paths say the opposite of the code

Rule: `code-standards-rust.md:92-94` (rustdoc describes end-state behavior). These are not in ST2's list, which covers deleted implementations.

- `worker.rs:626-628`: "a Report transaction in flight is rolled back ... so the Report simply stays pending". `run` has no `select` around `process_next`, and `stop` only cancels the token, so the claim runs on to its commit.
- `webhooks.rs:46-47`: "Ten seconds is the longest a Notification can be held back; past it the batch is retried on the fixed schedule instead". `webhooks.rs:241` clamps to 10 and waits, and a unit test pins that ("the wait is capped at ten seconds"). One endpoint can hold a batch for 3 × 5 s + 2 × 10 s = 35 s.

Fix: correct the text. Test: none.

### ST30. should-fix. CONFIRMED by reading: failing to listen for Ctrl-C shuts the server down at once

Where: `crates/flakeradar/src/main.rs:103-104`.

```rust
// A failure to listen leaves the other arm to stop the process.
let _ = tokio::signal::ctrl_c().await;
```

If this returns `Err`, the block finishes and `select!` at `:124` resolves, so `shutdown_signal` returns. The SIGTERM arm at `:112-118` does this correctly: it logs and then awaits `pending`. Rare in practice, and excluded from coverage by `:100`.

Rule: a swallowed failure, and M-LOG-STRUCTURED for the missing log event.

Fix: copy the SIGTERM arm's shape. Test: none; a test cannot run this code, per `:97-99`.

### ST31. should-fix, judgement call. PLAUSIBLE: the graceful drain has no bound

Where: `server.rs:166-170`.

```rust
axum::serve(self.listener, self.app)
    .with_graceful_shutdown(shutdown)
    .await
    .context("serving requests");
self.worker.stop().await;
```

`with_graceful_shutdown` waits for open requests. `grep 'TimeoutLayer\|timeout('` in `routes/` and `app.rs` finds nothing. So a client stalled mid-upload should keep the process from reaching `worker.stop`, and the orchestrator's kill then replaces the ordered release at `worker.rs:758`. Not run. The session lock does die with the process, so the cost is a lost clean stop rather than two leaders.

Fix: `tokio::time::timeout` around the drain, with a documented constant below the orchestrator's grace period.

Red test: a request that stalls mid-body must not stop `serve` from returning, under `tokio::time::pause`.

### ST32. should-fix. CONFIRMED by reading: two Webhook tests do not check what they are named for, and one spends real time

Where: `crates/flakeradar/tests/webhooks.rs:600` and `:615`. Rule: `code-standards-rust.md:130` and `:136` ("No test depends on real time").

- `an_http_date_retry_after_uses_the_default_wait_and_still_retries` ends at `until_finished(worker.queue(), 1)`, which a single attempt also satisfies. The mocks use `.up_to_n_times(2)` without `.expect(n)`. It stays green if a 429 with this header stops retrying.
- The same test uses `worker()` (`:85`, the production `RETRY_DELAYS`), so it really sleeps 1.5 s, and no paused clock is in the file. "Uses the default wait" is never observed.
- `stop_ends_the_task_without_starting_one_twice` starts twice and asserts only that `stop` returns `Ok`. It could not see a second loop. The double start is pinned for `ReportWorker` at `tests/worker.rs:762`, not for `LeaderRun`.

Fix:

- Use `tokio::time::pause`, and add `assert_eq!(recorded(&server).await, 3)`, with the helper `async fn recorded(server: &MockServer) -> usize` (`:90`).
- For the double start, count the closure's calls on `LeaderRun::start` directly.

Red check: make `retry_after_seconds` or `RETRYABLE_STATUSES` stop the retry on this path. The new assertion goes red, and the current test stays green.

### ST33. should-fix. PLAUSIBLE race, not run: the yield loop guesses when the upload is being read

Where: `crates/flakeradar/tests/server.rs:296-300`, noted but given no id in the first pass.

```rust
for _ in 0..4 {
    tokio::task::yield_now().await;
}
let _ = stop.send(());
```

The comment at `:280-281` promises "the upload is still being read when the shutdown starts". Nothing checks that. If the accept loop has not taken the connection when `stop` fires, the listener closes and the test should fail on the write or the response. So the failure mode is a random red, the one `code-standards-rust.md:138-139` names. I did not stress it, because that needs a database run and could not prove the absence of a race.

Rule: `code-standards-rust.md:140` (a wait for background work is a signal).

Fix: no signal exists today. `Server::start` builds its own app, so `wait_until` has nothing to poll until the whole body arrives. Either add a test seam on `Server` that fires a `Notify` when the request reaches the handler, or list this as a fourth documented exception at `code-standards-rust.md:144`. I prefer the seam.

### ST34. should-fix, judgement call. PLAUSIBLE: an `Err` from `process_next` retries for ever and restarts the leader tasks each time

Where: `worker.rs:673`, against the doc at `processing.rs` on `process_next`: "one bad Report can never block the queue".

A failed claim-row decode or a failed terminal `write_report` returns `Err` without moving the Report. The loop drops leadership, waits one poll, re-elects and claims the same Report. There is no limit, and each cycle stops the GitHub and Notification tasks, which also feeds ST16. A database outage should behave this way. A persistent failure on one row should not. I found no input that reaches it: I did not trace whether a stored value can fail those two steps.

Fix: count consecutive `Err`s for a claim and back off with `backoff()`. Then decide with the owner whether the row should be marked failed. Test: a processor stub that always answers `Err`, asserting the wait grows.

## Earlier rows

- **J10 (`Scope`): truly closed.** The fields are private (`reads.rs:157-160`), and `grep 'Scope {'` finds no struct literal outside `reads.rs`. The `Default` derive gives `None, None`, which meets the rule. The MCP helper goes through `Scope::new` (`mcp/mod.rs:507`).
- **J12: truly closed, with a residue.** One parse: `read_ci_run_attempt` (`dto.rs:1034`). `AttemptValue` and `attempt_value` are gone. `a_pipeline_attempt_past_the_column_is_a_bad_request` exists (`tests/validation.rs:310`). `stored_ci_run_attempt` still saturates (`report_intake.rs:166-167`), for stored bodies, and says why. The ledger's "no longer parses a second time" is accurate. The route's call at `routes/reports.rs:594` follows the 400, so saturation should not be reachable there.
- **J15: truly closed.** `ci_run_failed_jobs` and `ci_run_test_failures` take the same order, and the two calls sit side by side at `reads.rs:1896-1898`. The ledger's `reads.rs:1813` is a stale line number. The remaining `(&str, &str, ExecutionStatus)` at `scoring.rs:329` is a test helper's parameter (location inferred from its name and line, not read), and the ledger names the tests that use it.

## CI and packaging since f9330fd

Clean, with evidence from the full diff:

- The new `cargo doc` step has no `if`, and `ci.yml` has no `continue-on-error`. It runs what it names, and it matches `code-standards-rust.md:45`.
- It adds no action or image. `actions/cache@v4` and `postgres:17-alpine` are floating tags that predate this range.
- `.fabro/test.toml` changes a comment only.
- The floor change from 94 to 93 is the owner's recorded decision (`00-fixes-2-context.md:31`).
- No `Dockerfile` changed.

## Nits

- `lax.rs:186-195`: `text_int("-9223372036854775808")` is refused, because the digits accumulate as a positive number, though `i64` holds it and the doc refuses only runs past the range.
- `webhooks.rs:227-229`: a stop during a retry wait returns with no log, while the stop during a POST at `:171` logs one.
- `webhooks.rs:141` and `:211`: one event name, `webhook.delivery.failed`, for a task panic at error level and a status failure at warn.
- `extract.rs:154`: a parser `JoinError` goes to the client as text and is not logged.
- `extract.rs:397-413`: `kind_for` reads serde's wording, so text in a refused value can pick the `type`.
- `multipart.rs:35`: `MalformedHeaders` is public with no `Display` (same class as ST15).
- `tests/mcp_client.rs:3-4`: garbled sentence; `:11-15` is a note about this run ("No desktop client ... was available here") in a `//!` doc.
- `report_intake.rs:161`: "a `2.0` build" is a history note in rustdoc.

## Checked and clean

- `queue.rs`: bounded, with counts. `notify_one` stores a permit, so the push-before-await window cannot lose a wake.
- `webhooks.rs`: no URL in a log (`endpoint`, `without_url`). A NaN or `inf` `retry-after` is harmless: `clamp` then `delay.max(0.0)`. Attempts are bounded by `ATTEMPTS`. A stop ends both the POST and the wait.
- `worker.rs`: `backoff` cannot panic. `0 * inf` gives NaN, and `NaN.min(x)` is `x`, so the value is always within `max_backoff`. No `select` wraps a lock statement, so a cancel cannot cut one. `sleep`'s `select` is safe to drop.
- `connect.rs`: both reapers are off and `test_before_acquire` is false on the lock pool, so the pool does not retire the lock's connection behind it. `check` handles the replaced connection.
- `multipart.rs`: runs in `spawn_blocking` (`routes/reports.rs:476`). `Needle` is linear. `next_end` restarts one byte past an occurrence, and a boundary cannot hold the CR that would let occurrences overlap, so the total stays linear. No slicing that can panic.
- `lax.rs` to line 371: `normalised_timestamp`'s `split_at(2)` runs only on four ASCII digits. `-value` cannot overflow, since `value` is non-negative. `byte - b'0'` runs after the digit check. A long string costs linear work.
- `settings.rs` error paths read (`at_least`, `fraction`, `interval`, `parsed`, `validate`, `check_database_url` start): refusals name the variable, and only `interval` has a hole.
- `extract.rs:228`'s `let _ =` discards a builder reference, not an error. `settings.rs:768`'s discards a map's old value.
- Sampled tests: `tests/validation.rs` and `tests/mcp_client.rs` have no sleep, no `yield_now` and no real-clock read. Per-file `#![expect]` headers are allowed here, because each file in `crates/flakeradar/tests/` is its own binary (`code-standards-rust.md:78`). A scan of all four files found no test with no check at all. Three expect-only tests assert through `until_finished` or `wait_until`: `tests/worker.rs:525` (read), `tests/webhooks.rs:642` (grep only, `until_finished` at `:653,661`) and `tests/worker.rs:790` (grep only, condition at `:800`). `within` at `webhooks.rs:695,717` is the documented use, each after a real signal.

## Not reached

- `extract.rs:420-681`, including `path_error` and its unit tests.
- `multipart.rs:390-477` and `webhooks.rs:409-535` (unit tests).
- `lax.rs:372-440`, beyond the first pass's check of its casts.
- `settings.rs` outside the listed error paths, including env-file parsing at about `:760-870`.
- Most test bodies in `tests/worker.rs` and `tests/webhooks.rs`, and `tests/validation.rs` past line 150 (scanned by grep and script, not read).
- `11_universal_guidelines.md`: headings only. `08_performance_guidelines.md`: M-YIELD-POINTS and headings only.
- Whether any stored value can reach ST34's `Err` path, and whether any input can panic inside `process_next` (ST25).
