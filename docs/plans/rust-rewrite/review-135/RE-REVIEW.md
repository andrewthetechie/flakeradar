# Re-review of PR #135: the fixes for the first review

This page is for the owner's second review of `feat/rust-rewrite`. The first review is
`review-135/` — `spec-findings.md`, `standards-findings.md`, `clean-code-findings.md` and the
`ledger.md` that tracks every finding — and it was written against `f9330fd`. The fixes are tasks
16–31 of `docs/plans/rust-rewrite/`, whose shared contract is `00-fixes-context.md`.

**Head: `150bcd4`** for the code, the tests and `coverage-floor.json`. The commits after it are
documentation only — this file, `HANDOFF.md` and `STATUS.md`'s row for task 31 — so
`git diff 150bcd4..HEAD --stat` names only those files. The path from the reviewed head is
`f9330fd` → `e8563b1` (the parity test's race) → `04bc0ac` (the conditional import) → `46c244d`
(the merge of `main`'s #129) → `150bcd4` (the branch floor), with documentation commits between
them. **CI is green on the pushed head `bcabc16`** (documentation only): CI run `42276295751` and
Docker run `42276295719` are both `success`, and `gh pr checks 135` lists 18 `pass` and one
`skipping` (`Merge the manifest list`, correct on a PR), `Rust coverage (lines + branches)` among
the passes at 93.95% of branches and 96.48% of lines. The run before it, on `04bc0ac`, passed 17
of its 19 checks and failed only the coverage floor this head changes.

**The coverage floor is 93 branches, by the owner's decision of 2026-11-06.** The first CI run
after the fixes failed `Rust coverage (lines + branches)` on `branches 1274 / 1356 = 93.95%` against
a floor of `94%`, with `lines 15369 / 15929 = 96.48%` clearing `96%`. The floor was task 15's,
taken from a local measurement; CI's configuration (Linux, no `frontend/dist`,
`FLAKERADAR_TEST_DATABASE_URL` set) measures 9 branches fewer than any run on a machine that takes
the test-support Docker path, and all nine are in `flakeradar-test-support/src/postgres.rs` and one
`db.rs` branch — the path `coverage-gaps.md` already logs as unreachable in CI. Measured for
context, with `FLAKERADAR_TEST_DATABASE_URL` unset for the coverage command only so that the
container path runs, the same tree measures **1283 / 1356 = 94.62%** of branches and 96.87% of
lines. The owner chose to lower the floor rather than move the coverage job onto the Docker path:

> **Owner decision 2026-11-06:** the branch floor is 93, not 94, because CI's coverage
> configuration measures 93.95% (1274/1356) on the merged tree after tasks 16–30; the 9-branch gap
> against a local run is `flakeradar-test-support/src/postgres.rs`'s testcontainers path plus one
> `db.rs` branch, both already logged in `coverage-gaps.md` as unreachable when CI sets
> `FLAKERADAR_TEST_DATABASE_URL`. Lines stay at 96.

`coverage-floor.json` is therefore `{"lines": 96, "branches": 93}`, nothing else in it changed, and
`.github/workflows/ci.yml`'s coverage job is untouched. `differences.md` gains no row: this is a
measurement, not a wire or behaviour difference.

## 1. The ledger is closed

`review-135/ledger.md` holds **90 rows: 84 `fixed`, 3 `not reproduced`, 3 `no change`**, and no
row is open. Task 31 re-ran every named test:

- `cargo nextest run --all-features --profile ci -E 'test(<name>) | …'` with the 170 distinct test
  names the 84 `fixed` rows cite: **171 tests run, 171 passed, 932 skipped** across 55 binaries.
- For the rows whose evidence names a file or a binary instead of a test, the same command over
  `binary(parity) | binary(validation) | binary(lax)`: **56 tests run, 56 passed**.
- For the rows whose evidence is a grep or a command rather than a test (structure, docs and
  compile-error rows), each command was re-run and its answer recorded in section 5 below.

Two rows cite a name that is not a test and one cites a pre-rename test; they are the two honest
caveats at the end of this section, not silent failures.

## 2. One line per task

The ranges are contiguous and their counts sum to 120 before this leg's own documentation commits,
which with the review-fix plan commit `0e0fc57` and `f9330fd` are the 122 commits after
`130691c`. The SHA is the commit `STATUS.md` marks the task Done at.

| Task | Range | n | What it closed | Landed |
| --- | --- | --- | --- | --- |
| 16 | `4ec494e..79f5f5b` | 2 | `S1`/`H1` (a stopped leader task never ran again), `S5` (a stop waited for the whole sync), `J3` (an unbounded GitHub queue) | `4ec494e` |
| 17 | `eb78420..4530dd5` | 2 | `S2`/`J1` (row locks held across GitHub calls), `S4`/`A1` (the sync continued when GitHub was unreachable), `S6`, `B3`, `T2 (sync)` | `eb78420` |
| 18 | `e090919..451ce46` | 6 | `A3`/`S13c` (a store failure closed a filed issue), `A2`, `S7`, `S8`, `J11`, `J13`/`C6` | `1588fd7` |
| 19 | `b35b256..31fcbca` | 4 | `H0`/`S3` (nested suites aborted the server), `H3`, `H19b`, `S9`, `S10`, `S13b`, `J18` | `a9ed51a` |
| 20 | `0a3a6a3..2890c89` | 4 | `A4`/`H2`, `H19a`, `S11`, `S12`, `S13a`/`J12`/`C4`, `H18a` | `0a3a6a3` |
| 21 | `b6645f0..201474c` | 4 | `H4` (a malformed `.env` logged the token), `H5`, `H14a`, `J2`, `B11`/`H12a` | `b6645f0` |
| 22 | `2699914..b05dbc3` | 11 | `E1`–`E7`: a scenario that sets a token, a failed-Report scenario, a real listener, the dump clock relations, field types and `required`, allowances that name both values, a real MCP client | `69a6e86` |
| 23 | `2d150bf..64d42e9` | 6 | `D1`–`D8`: both servers side by side, 529 + 31 comparisons, three differences fixed and six logged | `d3f2f39` |
| 24 | `9fed875..38b1b31` | 11 | `B5`, `B6`, `B10`, `J7`, `J9`, `H13`, `J17a` | `d514d12` |
| 25 | `ef69d84..2d99836` | 18 | `H6`, `B4`, `B9`/`J15`, `B12`, `J10`, `H17`/`C3` | `fc59d9b` |
| 26 | `282e6c4..6d9c22f` | 5 | `B2`, `C1`; `J14a` not reproduced | `929c08b` |
| 27 | `92ec59f..21a766e` | 9 | `B1`, `T2 (rest)` | `53cbd17` |
| 28 | `003a8e8..b561943` | 9 | `B8`, `C2`, `C5`/`J5`, `J4`, `J6`, `J8a`, `J14b` | `43b2f10` |
| 29 | `8b7de67..40ee134` | 12 | `H7`/`T1`, `H8`, `H14b`/`T5`, `H20`, `T3`, `T4`, `J8b` | `b6472fc` |
| 30 | `2275fee..2fa6e6d` | 14 | `H9`, `H10`/`B7`, `H11`, `H12b`, `H15`, `H16`, `H18b`, `J17b` | `dd3924b` |
| 31 | `512e466..` | 4+ | This page, the ledger re-run, the proofs, the battery, three coverage runs, `HANDOFF.md`, the other corrected docs and the PR body | this row |

## 3. Every approved difference the fixes added

`differences.md` grew from 26 rows to **52**: 12 pre-approved, 14 added during the port and 26
added by these tasks. The 26 are:

- **16** — the GitHub queue holds at most 1000 outcomes and drops the oldest with a log line; a
  leadership change ends every GitHub call at once except the filing `POST`, which runs on through
  storing its link.
- **17** — a link re-read or replaced while the sync's `GET` is in flight survives, because each
  answered row is written guarded on the number that was checked.
- **19** — a `<testsuite>` nested past 990 answers 422 (the deepest the reference could parse); a
  non-finite `<testcase time>` or retry `time` answers 400; a report past 1,000,000 nodes or 64
  MiB of copied identities answers 422; a byte a declared single-byte code page cannot decode is
  decoded as the code page maps it.
- **20** — a `page` past `i64::MAX / 100` answers 422 and the offset is computed with
  `checked_mul`; a NUL in any request text answers 422; a naive or epoch pipeline timestamp is
  read as UTC; a multipart part whose header block is no header block answers the reference's
  400 `Invalid multipart data.`; a part with no field name answers `Empty report body`;
  `HEAD`/`OPTIONS` answer 200 with an `Allow` header where the reference answered 405, and a
  disallowed preflight 200 with no `max-age`; `GET /api/` answers 307 where the reference
  answered 404; a float path id answers 422 and one past `int4` 404.
- **21** — a setting whose value its field could not mean refuses to start, naming the variable; a
  `.env` line that does not parse names the file and the line and never the line's text; a
  database URL the driver cannot parse has its password replaced by `***`.
- **23** — a serialization failure (SQLSTATE 40001) is retried; a JSON body past 20 MiB answers
  413; the `input`/`ctx` of a framework 422 differ beyond `type`/`loc`/`msg`; a `json_invalid`
  offset and sentence differ; a racing delete's `deleted` counts are racy on both sides; a 204
  carries no `Content-Type`.
- **25** — a `reports` row whose stored `status` or `kind` this version never writes answers the
  plain-text 500; a `failure_category` the five categories do not name reads as `null`.

**All 26 are decided, none awaits a decision.** A JSON body past 20 MiB answers **413** here where
the reference validated it and answered 422; the owner kept the 413, because the plan's
explicit-limits rule guards a route without a token and keeping the reference's answer means
accepting an unbounded body there. `differences.md`'s row 23 cites the decision, and the four rows
this review's first pass could not settle are decided the same way in `00-fixes-2-context.md`.

No allowance was added, removed or widened by tasks 24–30: `git diff 64d42e9..e8563b1 --
tests/parity/` is empty. Over the whole leg, `git diff f9330fd..e8563b1 -- tests/parity/` names
the two scenario directories task 22 recorded from the reference with their 16 goldens,
`tests/parity/README.md`, `tests/allowances.rs`, and `allowed-differences.json` — the last by task
22's `E6`, which rewrote each entry to name both values and removed the `prefix` match rule. It
holds **5 entries and 196 field allowances before and after**, so nothing was added or removed;
the 9 entries that used `prefix` now pin roxmltree's exact message.

## 4. The rows that are not `fixed`

Three findings were not reproduced and three are accepted with no change. Each is the ledger's
evidence, repeated here because a reviewer should not have to trust the ledger's summary.

| Row | State | Evidence |
| --- | --- | --- |
| `D7` (task 23) | not reproduced | The criteria of tasks 01, 02, 03, 12 and 13 were checked one by one: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo nextest run --all-features --profile ci` (1067 tests at the time) and `cargo test --doc` are green from this tree, `node --test scripts/*.test.mjs` was 49 pass / 0 fail, `ci.yml` holds no `--no-tests=pass` and no `has-coverage` guard, `test-manifest.csv` has 575 rows and no `pending`, and every crate module is named by a task file. No defect was found. Re-run at this head in section 5: the same gate, 1103 tests and 24 doctests. |
| `D8` (task 23) | not reproduced | No module without a task: every file under `crates/*/src` — `app`, `assets`, `cli`, `docs`, `error`, `extract`, `github`, `github_worker`, `lib`, `main`, `mcp`, `middleware`, `multipart`, `queue`, `routes`, `server`, `settings`, `telemetry`, `webhooks`, `worker`, and the core and db modules — is named by at least two task files or fix records. Checked file by file; nothing was added to the branch that no task asked for. |
| `J14a` (task 26) | not reproduced | The three `let _ = settings;` / `let _ = (db, settings);` sites the finding names were already removed by task 17's `eb78420`: `clear_if_closed`, `close_issues_after_removal` and `close_note` no longer take those parameters, `grep -rn 'let _ = ' crates/flakeradar/src/github/` answers three lines and none of them is a parameter, and `unused_variables` under the gate's `-D warnings` makes a dead parameter a build failure. Re-run at this head: the grep answers the same three non-parameter lines and the gate is green. |
| `J16` | no change | Accepted in the ledger before the relay began: the `dyn` seams serve tests the standard asks for. |
| `J17c` | no change | Accepted in the ledger before the relay began: `M-MIMALLOC-APPS` for musl only, `M-TARGET-CPU` and the absent `cargo-hack`/Miri are defensible per `standards-findings.md`. |
| `J8c` | no change | Accepted in the ledger before the relay began: the Pydantic error classification by serde message, and the hand-written multipart reader, are the wire contract and are pinned by tests. |

## 5. What this leg ran, with its numbers

Every command below was run at `e8563b1` on the machine that wrote the branch.

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --all-targets --all-features -- -D warnings` | exit 0 |
| `cargo nextest run --all-features --profile ci` | **1103 tests run, 1103 passed, 0 skipped** |
| `cargo test --doc --all-features` | **24 doctests passed** (22 + 2) |
| `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --all-features --document-private-items --workspace` | exit 0 |
| `cargo audit` | exit 0, 1422 advisories loaded, no vulnerability in 403 dependencies |
| `node --test scripts/*.test.mjs` | **49 tests, 49 pass, 0 fail** |
| `cargo nextest … -E 'test(<the 170 names the ledger cites>)'` | **171 tests run, 171 passed** |
| `cargo nextest … -E 'binary(parity) \| binary(validation) \| binary(lax)'` | **56 tests run, 56 passed** |
| `review-135/repro/proofs` (the scratch crate, six test files) | **13 tests, 13 passed**; each now asserts the answer the fix gives |
| `./target/relproof/release_depth_main suites 1000 \| 5000 \| 200000` | each answers the 422 detail and exits 0; no abort |
| `./target/relproof/release_depth_main elements 800000` | parses, 1 case, exit 0 |
| `review-135/repro/differential/battery/run.sh all` | **529 comparisons** in the default profile and **31** in the retention profile, **0 new differences**; the known ones are the recorded rows |
| `cargo +nightly-2026-10-25 llvm-cov nextest --branch --all-features --profile ci`, three times | **96.87% of lines** (15445/15944, 499 missed) and **94.62% of branches** (1283/1356, 73 missed) each time; regions 93.38%, functions 97.46% |

The coverage floor is `{"lines": 96, "branches": 93}`: the branch floor is the owner's decision
of 2026-11-06 recorded at the top of this page, and 94.62% locally (93.95% on CI) clears it, as
96.87% of lines clears 96. The measurement raises nothing and lowers nothing.

The proofs are the six files in `review-135/repro/proofs/`. They were scratch code that asserted
each defect at `f9330fd`; they now assert the fix, so a pass means "the defect is gone" rather
than "the defect is present". `Cargo.toml` there carries a `<repo>` placeholder and a `[[bin]]`
for the release-profile harness, and `review-135/README.md` says how to run them.

## 6. What this leg found and fixed

One defect, and it is a test's race rather than a behaviour bug.
`a_scenario_with_a_github_token_webhooks_and_gates_files_only_the_test_that_qualifies` read the
Test's stored Issue number the moment the GitHub stand-in recorded the `POST`. A stand-in records
a request when it arrives, before the answer the filing reads and before the link it stores, so
the read could see `None`: it did once in the three instrumented coverage runs, at
`crates/flakeradar/tests/parity.rs:2530` with `left: None, right: Some(100)`. The read is now
`flakeradar_test_support::wait_until` — the bounded wait the code standard names for a row the
code under test writes — which also makes the negative assertion beside it a claim about a batch
that finished (`e8563b1`).

Two CI failures were fixed at this leg, both found only because CI had not run since the reviewed
head. `tests/frontend.rs` imported `flakeradar_test_support::settings` at the top of the file and
used it inside a `#[cfg(has_frontend)]` test, so `cargo clippy -D warnings` failed on CI's build,
which has no `frontend/dist`; the import moved into that test (`04bc0ac`). And the PR had been
CONFLICTING with `main` since `c911fdb` (#129 changes `backend/app/processing.py`, which this
branch deletes), which is why GitHub created no `pull_request` runs for any push after
2026-11-05T18:21; merging `main` in with the deletion winning (`46c244d`) made the PR mergeable and
started the runs, and `git diff` against the previous head is empty, so the tree is unchanged.

One number in the record was wrong and is corrected: the `E6` row of the ledger said twelve
allowances used the `prefix` match rule, and the file held nine before `E6` removed the rule (196
field allowances before and after, 9 `prefix` entries before and 0 after). The row says so now.

## 7. What was not verified

- **CI itself.** The gate above is this machine's, against a local `postgres:17-alpine`, not
  CI's `postgres` service on Linux. The pushed head's run is the evidence for CI, and the
  coverage percentages are this host's.
- **The packaging legs.** A Docker build, the 60 MB image size, the five Actions targets and a
  `release.yml` `workflow_dispatch` were not re-run; the last cannot run before the workflow
  exists on the default branch.
- **The differential battery needs the Python reference outside the tree.** It ran from
  `/tmp/python-base` and `/tmp/pyref`; a reviewer offline can check out `86c98804` but cannot
  replay it from the repository alone.
- **`E5`'s four negative controls were never written.** The ledger row cites `binary(parity)` and
  the `trailing-slash` step 9 replay instead of a function name, and the controls the task wanted
  — a retyped field, a dropped `required` entry, a port-only `required` entry, a field widened to
  nullable — do not exist. The red was real (the comparison found two document defects), and the
  task's Done-when does not require the controls; they are recorded here rather than added,
  because the check belongs to task 22 and task 31 changes no behaviour.
- **Task 29's `T3` row names the pre-rename test.** It explains the rename from
  `a_note_network_error_is_logged_and_never_raises` to
  `a_note_network_error_ends_the_batch_and_never_raises`; the current name exists and passes, and
  the old name is the one the finding used.
- **`record.py --verify` is gone with `backend/`, and a narrower check replaced it.** The
  reconciliation between `differences.md` and `allowed-differences.json` is
  `tests/common/allowances.rs`'s `reconcile`, run by the suite; it refuses an allowance naming no
  row, a golden or field that is not there, or a `pythonValue` the golden does not hold, but it
  cannot re-record a golden from the reference, cannot tell that a row has no allowance, and an
  allowance that covers nothing is still undetected. `differences.md`'s recount (52 rows, 196
  field allowances, 183 + 12 + 1 by reader) is a count of the two files.
- **Task 23's sweep did not stage a deadlock or a literal PostgreSQL restart**, and it did not
  re-record a golden; both are pinned by existing tests instead.
- **`E7`'s real client is `rmcp`'s own client**, not a desktop MCP client or the Inspector CLI:
  none is installed here.

Closed since this page was written, so it is no longer a gap: **the frontend in a** browser,
which tasks 09, 12, 22 and 25 each recorded. Task 37 added `frontend/e2e/` — the `ui` project
mocks every read from fixtures, and the `e2e` project drives the dashboard out of the real
binary against a seeded `postgres:17-alpine`. CI runs both as `Frontend UI (Playwright,
mocked)` and `Dashboard e2e (Playwright, mock install)`.

## 8. Where to start reading

1. **`review-135/ledger.md`** — the 90 rows, each with its commit and the test or evidence that
   closed it. The ids are the review's.
2. **`docs/plans/rust-rewrite/differences.md`** — the 52 behaviour rows, in particular the 26 this
   work added, every one of them citing an approved decision or a finding.
3. **`tests/parity/README.md`, then `tests/parity/scenarios/`** — 15 scenarios, 316 steps, 383
   golden files; `github` and `failed-report` are the two the fixes added, and
   `a_scenario_with_a_github_token_webhooks_and_gates_files_only_the_test_that_qualifies` is the
   one test that reads what a scenario cannot record.
4. **`crates/flakeradar/tests/`** — one file per area, `github/` and `mcp/` following their source
   modules. The tests the ledger names are the shortest path into each fix.
5. **`docs/plans/rust-rewrite/STATUS.md`** — the row per task, with what each measured, what its
   audit found and what it left unverified. Do not read a `Done` row as "verified"; sections 3
   and 7 above are the extracted version of the last part.
6. **`HANDOFF.md`** — the same ground for tasks 01–15, which the first review already covered.
