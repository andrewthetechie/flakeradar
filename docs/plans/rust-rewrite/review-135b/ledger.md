# Finding ledger: second review, at `ac5e6f3`

One row per finding. The owning task closes its rows. **State** is `open`, `fixed`, `not reproduced` or `no change`. A `fixed` row needs a commit, a test name, and what a reader or user now sees. A `not reproduced` row gives its evidence in the last column, and a `no change` row gives its before and after line counts. See `00-fixes-2-context.md`. **Reopens** names the first-review row this finding shows was not truly closed.

| Id | Must | Finding | Reopens | Task | State | Commit | Test or evidence |
|---|---|---|---|---|---|---|---|
| SP1 | 1 | Entity references and namespace declarations pass the parser budget: 1 MiB body past 3 GiB of heap | H3 | 32 | open | | |
| SP2 | 2 | `xmlns=""` stores a failing test as passed (reproduced on a real server by the reviewer) | S10 | 33 | open | | |
| SP3 | | 258 of 496 encoding labels disagree with Python; 23 change fingerprints | S10 | 33 | open | | |
| SP4 | | DOCTYPE and declaration handling diverges, including fingerprints, with no row | S10 | 33 | open | | |
| SP8 | 3 | A number past 2e10 is read as seconds; `Date.now()` values answer 202, then the Report fails |  | 34 | open | | |
| SP9 | 4 | Numeric-string timestamps the reference accepts answer 422 |  | 34 | open | | |
| SP11 |  | A database password reaches the output when it holds `/` or sits in `?password=` | H5 | 34 | open | | |
| SP12 |  | Settings spellings the reference starts with (`5.0`, `1_000`, `t`, `Y`) refuse to start, with no row |  | 34 | open | | |
| ST26 |  | `DB_COMMAND_TIMEOUT_SECONDS=1e30` panics at start-up |  | 34 | open | | |
| ST27 |  | An interval a `Duration` cannot hold silently becomes a zero wait |  | 34 | open | | |
| SP6 | | A store failure stops only the Test batch; the reference ends the outcome | A3 / S13c | 35 | open | | |
| ST16 | | A cancelled comment `POST` can repeat; a cancelled close is never retried | S5 | 35 | open | | |
| ST17 | | `from_object` and `pipeline_body` do 20 MiB-scale work on the async thread | J4 | 35 | open | | |
| R2 | | Default `info` log is 96% `sqlx::query` lines (1546 of 1605 in the demo seed run) | | 35 | open | | |
| ST28 |  | The query's name lookup is quadratic on the async thread |  | 35 | open | | |
| ST24 |  | A leader with a backlog never checks its lock, so two processors can run (reference has the same gap) |  | 36 | open | | |
| ST25 |  | A worker-loop panic holds the lock and processes nothing, with no health signal |  | 36 | open | | |
| ST30 |  | A failure to listen for Ctrl-C shuts the server down at once |  | 36 | open | | |
| ST31 |  | PLAUSIBLE: the graceful drain has no bound; reproduce first |  | 36 | open | | |
| ST34 |  | PLAUSIBLE: a persistent `process_next` error retries forever and restarts leader tasks |  | 36 | open | | |
| R1 | | No browser test exists: Playwright with mocks, and e2e against a mock install (owner's request) | | 37 | open | | |
| ST1 | | The compliance marker is on all 150 files, including ones failing its checklist | H11 | 38 | open | | |
| ST2 / CC1 | | 69 comment lines still describe the deleted implementation | H10, B7, C4 | 38 | open | | |
| ST3 | | Both source-hygiene tests pass on the violations they claim to catch | | 38 | open | | |
| ST13 | | `rust-version` is not inherited by any member | J17 | 38 | open | | |
| ST29 |  | Stop and retry docs say the opposite of the code |  | 38 | open | | |
| CC16 |  | `extract.rs`'s module doc describes a JSON path that no longer exists |  | 38 | open | | |
| ST4 | | Five log events still interpolate their messages | H16 | 39 | open | | |
| ST7 | | Tests use log text as the outcome; `retention.rs:761` has no other assertion | T3 | 39 | open | | |
| ST8 / CC7 | | Seven to nine private log-capture rigs outside test-support | H14b, T5 | 39 | open | | |
| ST9 | | Public-API tests in `src/`; a tautology at `limits.rs:26` | H20 | 39 | open | | |
| CC5 | | Six tests still run Test and Job in one body | T2 | 39 | open | | |
| CC6 | | Positional unpacking came back at 20 sites; redundant `UPDATE`s | T4 | 39 | open | | |
| ST32 |  | Two Webhook tests do not check what they are named for; one sleeps 1.5 s |  | 39 | open | | |
| ST33 |  | PLAUSIBLE race: a yield loop guesses when the upload is being read |  | 39 | open | | |
| ST5 | | `Score`, `Fingerprint`, `CiRunId` never reach the read paths | H6 | 40 | open | | |
| ST6 | | Inline defaults and bounds remain | H12 | 40 | open | | |
| CC2 | | The `action` table's Unreachable row is dead; `already_gone` is a second list; loops did not shrink | B3 | 40 | open | | |
| CC3 | | Note and filing entry points are still paired | B1 | 40 | open | | |
| CC4 | | Four impossible `LinkedOut` arms, three names for `identity_error`, 10 issue-list sites | B6 | 40 | open | | |
| CC8 | | The bulk-write descriptor costs more than it removed; SQL cannot be grepped | | 40 | open | | |
| CC9 | | The `LISTS` comment in 11 files claims one source of truth that does not exist | B10 | 40 | open | | |
| CC10 | | Job status and provider stay strings; `CiRunAttempt` clump | B4, C2 | 40 | open | | |
| CC11 | | `runner_stats` is 97 lines; B9's named split was dropped | B9 | 40 | open | | |
| CC12 |  | `file_field` walks the framing and decides the field in one loop |  | 41 | open | | |
| CC13 |  | `disposition` splits by hand on a copy of a standard method |  | 41 | open | | |
| CC14 |  | A hand-written KMP with a test counter in the production search |  | 41 | open | | |
| CC15 |  | An error kind passes through a string to reach a constructor |  | 41 | open | | |
| CC17 |  | Every `lax` caller repeats the same four lines |  | 41 | open | | |
| CC18 |  | `post` is 69 lines of request, classification, logging and waiting |  | 41 | open | | |
| CC19 |  | Seven aliases name one future shape |  | 41 | open | | |
| CC20 |  | `worker.rs` holds three modules, and the loop starts at line 513 |  | 41 | open | | |
| CC21 |  | `AdvisoryLock::check` asks the same question of `answer` twice |  | 41 | open | | |
| CC22 |  | In `validation.rs` the assertion ceremony outweighs the cases |  | 42 | open | | |
| CC23 |  | One 77-line MCP test checks four things |  | 42 | open | | |
| CC24 |  | 773 seeded rows each cost a `build`, an `await` and an `expect` |  | 42 | open | | |
| CC25 |  | 37 sync calls differ in one argument |  | 42 | open | | |
| SP5 | | Four `differences.md` rows were self-approved or left awaiting a decision | | 43 | open | | |
| SP7 | | The record is false or stale: floor 94, reconciliation contradiction | | 43 | open | | |
| E5 | | The OpenAPI type and `required` comparison has no failing control | E5 | 43 | open | | |
| SP10 |  | A query reports one bad value, chosen by URL order; row needed |  | 43 | open | | |
| SP13 |  | Row 115's "same 400" is false, and multipart's limits have no row | S10 | 43 | open | | |
| SP14 |  | A refused timestamp's `msg` differs, and row 124 says it matches |  | 43 | open | | |
| SP15 |  | The parity README names the wrong reference commit for two scenarios |  | 43 | open | | |

**Left as nits, with no task:** `ST10`–`ST12`, `ST14`, `ST15`, `ST18`–`ST23`, the dead items in `clean-code-findings.md`'s "Dead code" list, and the "Nits" lists in both second-pass sections. One of those needs a check, not a row: `ReportWorker` holds one `CancellationToken`, so a `start` after `stop` exits at once. Fix one only when your task already touches that line.
