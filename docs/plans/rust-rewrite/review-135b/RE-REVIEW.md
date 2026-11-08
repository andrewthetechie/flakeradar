# Third review handoff: PR #135 at `bba16f2`

This is the page to start from. It says what the second review's fixes (tasks 32–44) changed, the
numbers they were measured at, every intentional difference they added, what is still unverified,
and the surface neither review reached — which is where the third review starts. The second
review's findings are in `review-135b/`, the first review's in `review-135/`.

**Head: `bba16f2`, the last commit that changes code or a test, pushed to `feat/rust-rewrite`.**
The documentation commits that carry this page, `HANDOFF.md` and the CI ids follow it, and
`STATUS.md`'s task 44 row carries the run ids, so a later push cannot leave a stale run id here.

## 1. The tasks, one line each

| Task | Title | Range | n | What it changed |
| --- | --- | --- | --- | --- |
| 32 | Parser budget that holds for every shape | `1fbf1e5..8b626c7` | 4 | The pre-scan bounds entity expansion (64 MiB) and namespace declarations (4,194,304 entries) before `roxmltree` reads a byte; ten hostile shapes measured under a counting allocator |
| 33 | Parser parity: empty namespace, encodings, declarations | `986904e..0484a9e` | 4 | `<failure xmlns="">` is a failure again; every encoding label decodes from the reference's codecs (54,309 fixture rows); `DOCTYPE` and declarations pinned case by case |
| 34 | Timestamps, settings and the database URL | `4cd7e06..b99959d` | 11 | Epoch/millisecond rule, year range, numeric-string timestamps, settings spellings, waits no `Duration` holds, the password in a database URL |
| 35 | GitHub stop rules, off-thread JSON work, the SQL log | `23dcb25..400d0ee` | 7 | A store failure ends the outcome; a comment and its close run to their answer; JSON parse and Pipeline serialisation leave the async thread; a name map; SQL at `debug` |
| 36 | The leader loop and shutdown | `e949ca7..d5ff9c5` | 3 | The lock is checked every loop; a dead worker loop makes `serve` fail and release; a failed Ctrl-C listener does not shut down; the drain is bounded at 25 s; `process_next` errors back off |
| 37 | Playwright tests for the dashboard | `9c08e65..c872fde` | 6 | Two Playwright 1.65.0 projects — 5 mocked-UI and 8 end-to-end tests — each its own required CI job |
| 38 | Comments, compliance markers and the hygiene guard | `8480d6f..6a3b4dd` | 7 | 65 markers removed; comments describe the code as it stands; both hygiene tests became rules that fail by file and line |
| 39 | Log events and test shape | `8931cc6..645e4e4` | 10 | Five named log events with fields; `PruneResult::notifications_dropped`; one `CapturedLog`; public-API tests out of `src/`; Test/Job test split |
| 40 | Finish the types and the plumbing | `6541866..ab611b0` | 16 | Typed reads, named constants, a live policy table, one note and one filing entry point, literal SQL, shaped MCP answers, and the one approved behaviour change (a stored `NaN` score is refused) |
| 41 | Structure of the request and worker modules | `3aa7768..a44b0cf` | 12 | The lock's one match, one `BoxFuture`, `post` as attempt/match/wait, `lax::recorded`, `path_issue`, `Framing::next_part`, one split helper, `memmem::Finder`, `worker/{leader_task,lock}.rs` |
| 42 | Test helpers that remove the most lines | `96fc09b..e9fe17d` | 7 | Builder `create` (733 calls), `sync`/`sync_with`, validation and log-test helpers, four MCP tests instead of one |
| 43 | The record and the missing controls | `3cee932..4b42e61` | 3 | Four OpenAPI negative controls; the four self-approved `differences.md` rows cite their decisions; the parity README names each recording commit |
| 44 | Hand off for the third review | `840c180..bba16f2` | 3 | The ledger closed; the checks re-run; the record corrected; the PR body rewritten; this page; and task 36's flaky drain test fixed and re-measured after CI caught it |

## 2. The numbers at this head

| Check | Command | Result |
| --- | --- | --- |
| Gate | `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --doc --all-features` (24), `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --all-features --document-private-items --workspace`, `cargo nextest run --all-features --profile ci` | all pass; **1180 tests run, 1180 passed, 0 skipped** |
| Gate scripts | `node --test scripts/*.test.mjs` | **49 passed** |
| Differential battery | `battery/run.sh all` (reference at `86c98804` in `/tmp/python-base`, `/tmp/pyref`), `OUT=/tmp/battery44` | **555 comparisons in the default profile, 31 in the retention profile, 0 new differences**; every known difference is a `differences.md` row |
| Suite under load | the full suite three times with ten CPU spinners, no retries | **1180/1180 each** at load 13.79, 17.15 and 21.33; 0 retries |
| Coverage, CI's command | `cargo +nightly-2026-10-25 llvm-cov nextest --branch --all-features --profile ci --no-report` + `report --branch`, three times | lines **96.96%, 96.96%, 96.97%**; branches **93.80%, 93.80%, 93.87%**; regions 93.64/93.64/93.65%; functions 97.41% each. Floor `{96, 93}` clears every run |
| Browser | `CI=1 npx playwright test --project=ui` and `--project=e2e` | **5 passed**, **8 passed**, `retries: 0` |
| Duplication | `dups.py 8` over production and tests, same file set as the review's baseline | production **22 pairs / 203 lines** (baseline at `ac5e6f3`: 24 / 267); tests **366 / 3493** (baseline 652 / 6193) |
| Ledger | the 107 tests the rows name, one nextest expression | **107 run, 107 passed**; `declarations.rs`'s 25 cases for `SP4` also passed |

Two caveats on those numbers. The coverage totals are not byte-identical between runs: runs 1 and 2
miss one line in `github/sync.rs` and one region in `db/src/issue_links.rs` that runs 3 and 4 count
covered, and `docs/plans/rust-rewrite/coverage-gaps.md` records it (the line was not identified).
And every figure is this host's: a local `postgres:17-alpine`, not CI's service.

A fifth instrumented run, after the test fix below, reports the same 1180 tests and 96.97% of lines
and 93.87% of branches.

## 3. Differences the fixes added

Twenty rows, all in `differences.md` under "Added by the review fixes": two from task 32, twelve
from 33, four from 36, one from 40 and one from 43. No row waits for a decision: `SP5` gave each of
the four that did its approved decision's citation.

- **32** — a JUnit body whose internal entities expand past the pre-scan's 64 MiB budget, or whose
  namespace declarations would overflow its table, answers the **422** a bad report gets, before
  `roxmltree` reads a byte; and a `<testcase time>` or retry `time` beyond `TIME_CHARS_MAX` code
  points is refused the same way.
- **33** — the encoding labels the reference knows only through a stateful or EBCDIC codec stay
  **422** (one row lists all 49); and each of the eleven `DOCTYPE`/declaration shapes `roxmltree`
  cannot express keeps its answer with its own row: the three `ATTLIST` defaults and their
  normalization, the two parameter-entity shapes, and eight malformed forms it accepts.
- **36** — a leader with a backlog now checks its lock (so this is where the reference's own gap is
  closed rather than copied); a panic in the worker loop ends `serve` with an error and releases the
  lock, with no `panic = "abort"`; a graceful drain waits at most 25 s; and a persistent
  `process_next` error backs off to the cap instead of a flat poll.
- **40** — a stored `flakiness_score` that is not a score answers the plain-text **500** (and an MCP
  `isError` result) instead of serving `null`. *Approved by the user on 2026-11-07.* This is the only
  behaviour change in tasks 38–44.
- **43** — a multipart upload the reference's form parser refused, or one past its own limits,
  answers the status this port's reader gives; the row lists every measured shape.

One allowance value moved and no allowance entry was added or removed:
`tests/parity/allowed-differences.json` still holds 5 entries and 196 field allowances, and task 32
(`ff8ae02`) rewrote the two `bad-nested-entities` goldens' `rustValue` to the pre-scan's sentence.
`differences.md` recounted the file at this head: 71 data rows, 12 pre-approved, 14 from the port,
45 from the two reviews' fixes, with a per-task breakdown.

## 4. The ledger: every row is `fixed`, and what closed it

`review-135b/ledger.md` has 63 rows and every one is `fixed`; there is no `not reproduced`, no
`no change` and no row left open. Task 44 closed the ledger by re-running each row's named test
rather than by re-reading its evidence: the 107 tests the rows cite, in one nextest expression, and
`declarations.rs`'s 25 cases for `SP4` (whose row names a file). Each row's own text says what a
reader or user now sees; where a row names a rename, the row now names the current spelling (task
42 renamed `support::sync_batch` to `support::sync`).

One row's test was flaky on CI, and task 44 fixed it. CI's coverage job failed
`a_stalled_upload_does_not_stop_serve_from_returning` once at `fa09e56` (run `42426433124`) —
`tests/server.rs:395:5`, `serve returned within the drain bound even though the upload stalled` —
because the stop could race the upload's arrival and 32 yields were not enough under
instrumentation. `bba16f2` is a test-only change the owner authorized: it waits on the arrival
`Notify` before stopping and advances the paused clock until the serving task ends, under a
10-second real hang guard. Red, measured by removing the drain bound: `tests/server.rs:399:9`,
`serve never returned past the drain bound: the stalled upload held it open`, after 12.2 s. After
the fix: 10/10 in `--profile ci`, 5/5 under ten spinners, 6/6 under `llvm-cov`, and the full suite
1180/1180. What this host cannot prove is that CI's 2-core instrumented job never sees another
timing failure; it is the entry to watch on the next runs.

Twelve rows close on evidence that is not a test, each saying why in its text: `ST30` (signal
wiring), `ST1` (a marker count), `ST13` (`cargo metadata`), `CC16` and `ST29` (doc text), `SP5`,
`SP7`, `SP13` and `SP15` (record rows), `ST8` (one `CapturedLog` the log tests assert), `ST9` (tests
that moved) and `CC6` (a helper name and three deleted fixtures). `R1`'s evidence is the two
Playwright projects, which ran.

Closing the ledger found four records the fixes had outlived, all corrected here: `differences.md`'s
multipart row said task 20 where the task that wrote it is 43, and its task-43 recount claimed
`allowed-differences.json` had not changed since task 22 (`ff8ae02` changed two values);
`coverage-gaps.md` listed five rows for conversions and arms task 40 deleted (the three files now
measure 100%, 100% and 99.16% of lines); `HANDOFF.md` section 4 row 22 quoted a retention sentence
the test no longer pins; and `tests/parity/README.md` named an older head. `README.md` and
`CONTRIBUTING.md` were read against the code and the fixes and needed no correction: the log-level
paragraph and both `RUST_LOG` spellings are task 35's, the gate block names every command this
handoff ran, and the Playwright commands are task 37's.

## 5. What is not verified

- **CI at this head is recorded in `STATUS.md`, not here.** This page was pushed first; task 44's
  row carries the run ids for `gh pr checks 135`, including the two Playwright jobs.
- **The Windows leg, the Docker image and the `workflow_dispatch` rehearsal are still CI's** (no
  packaging file changed after `ac5e6f3`).
- **Every coverage, load and perf figure is this host's**, and the coverage totals vary by one line
  and one branch between runs.
- **The battery needs the Python reference outside the tree**, so a reviewer offline can read its
  reports but cannot replay it.
- **The twelve rows above close without a test**, and one accepted item stays accepted: a leader
  task's panic surfaces only at `LeaderRun::stop`. It still looks right. `serve` supervises the
  worker loop, whose death is the one that processes nothing and can hold the lock; a leader task
  that panics stops its own loop, its token is noticed at the next stop, and the loop it leaves has
  no lock or queue of its own to strand. Supervising it as well would add a second failure path for
  a case whose damage is bounded by one stop.

## 6. What the third review starts on

The second review ran two passes and still did not reach these. They were not reviewed; treat them
as unread.

- **Spec.** MCP tool integer arguments beyond the two task 34 named (`limit`, `executions_limit`).
  Settings fields other than the sampled ones. A `.env` file on the reference side. A GitHub token
  in logs on a real failing call. A real, not staged, serialization conflict, and the retry path for
  Pipeline Reports. Uvicorn with `--workers 2`. The `HANDOFF.md` and PR bodies.
- **Standards.** `extract.rs` from line 420 onward; the unit tests in `multipart.rs` and
  `webhooks.rs`; `settings.rs`'s env parsing; most test bodies in `tests/worker.rs` and
  `tests/webhooks.rs`; `ms-rust`'s `11_universal_guidelines.md`, read as headings only; and what
  input, if any, can panic inside `process_next`.
- **Clean code.** `dto.rs` beyond the `CC17` arms, `settings.rs` outside lines 405-614, and test
  clusters past the top 12. The `martin-clean-code` skill is user-invocation-only and was not
  applied on either pass.
- **The one accepted deferral** is the leader panic in section 5.

## 7. Where to read next

`HANDOFF.md` (sections 2c, 3, 7 and 8 are this handoff's), `differences.md`'s final recount,
`coverage-gaps.md`'s task 44 section, `STATUS.md`'s task 44 row, then the code: the parser's
pre-scan, `worker/leader_task.rs` and `worker/lock.rs`, `github/policy.rs`, and
`crates/flakeradar/tests/parity.rs`.
