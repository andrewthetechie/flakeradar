# 00 — Shared context for the second review's fixes (tasks 32–44)

**Read this file, then `00-fixes-context.md`, then `00-shared-context.md`.** Where they disagree, this file wins. `00-fixes-context.md`'s process still applies: check the claim, go red, fix, close the ledger row, run the gate, pass the baton.

## Why these tasks exist

The second review of PR #135, at `ac5e6f3`, found four must-fix defects, two in the parser and two in timestamp reading, plus should-fix behaviour, record and clean-up work. It ran as two passes: the first stopped early on budget, and the second reached most of what it left. The relay's own numbers held up. The reviewer re-ran them with these results:

- `dups.py`: production duplication fell from 639 to 267 lines.
- Differential battery: 529 + 31 comparisons, 0 new differences.
- Suite under ten CPU spinners: 1103/1103 on three runs, with no retries.
- Browser check by hand: passed.

Findings have ids and rows in `review-135b/ledger.md`. Read the sections your task names in `review-135b/`: `spec-findings.md` (`SP`), `standards-findings.md` (`ST`) and `clean-code-findings.md` (`CC`). `R` rows are the reviewer's own.

## The lesson from the last relay

Several rows closed on a grep that the code was then edited to pass, while the reader's cost stayed. `T4`'s tuple came back as `let (number, filed, _, closed) = (...)`, and `B7` swapped "Python" for "the reference". So:

- **A grep is never a row's only evidence.** Say what a reader or user now sees that they did not see before.
- **`no change` needs numbers.** A structure row may close `no change` only with before and after line counts showing the change would cost more than it removes.
- **Do not close a row as `fixed` while its text lists open items.** Close it `fixed` only when nothing is left, or split the rest into a new row.
- **Do not self-approve a difference.** A status or value change outside the table below is `BLOCKED`. Writing "needs the owner's decision" in a row and marking the task `Done` does not satisfy this rule.

## Overrides

- **Tasks 32–36 may change behaviour** only as their contracts say.
- **Task 37 adds tests and CI only.**
- **Tasks 38–42 change structure and text, not behaviour.** The replay, goldens and allowances stay untouched.
- **Task 43 fixes the record. Task 44 hands off.**
- **Goldens:** the recorder is gone. A reference value goes in a test as a literal, with the command that measured it in the `STATUS.md` note. Measure it with `/tmp/pyref` and `/tmp/python-base` (see `review-135/README.md`).
- **Coverage floors:** `{"lines": 96, "branches": 93}` is the owner's decision. Do not lower them.
- **Do not reopen** `HANDOFF.md` section 4 rows 5 and 27, `J16`, `J17c` or `J8c`.

## Proposed decisions (approved when the owner commissions this plan)

The owner is the only operator and accepts changes that make sense. Each row becomes a `differences.md` row citing this file. Anything else is `BLOCKED`.

| Case | Decision |
|---|---|
| A body past 20 MiB on a JSON route (`differences.md:123`) | Keep the 413 |
| A serialization failure, SQLSTATE 40001 (`:122`) | Keep the retry. A transient failure should not fail a Report |
| A stored `status` or `kind` this version never writes (`:128`) | Keep the plain-text 500. Only a hand-written row reaches it |
| A stored `failure_category` outside the five (`:129`) | Keep `null` |
| A naive pipeline timestamp read as UTC; a name-less multipart part's `detail` | Keep both |
| An entity-expansion or namespace-declaration body past the budget (`SP1`) | 422, the answer a bad report gets, even where the reference would have parsed it |
| What roxmltree cannot express: `ATTLIST` defaults and attribute normalization, and the malformed forms `SP4` lists that Rust accepts | Keep roxmltree's answer, with one `differences.md` row per case. No CI runner emits these. Everything `SP4` calls cheap (the `<?xml` match, BOM plus declaration, parameter entities if roxmltree allows, UTF-16 declared mismatches) still gets fixed |
| An encoding label Python knows only through a stateful or EBCDIC codec (`iso-2022-*`, `hz`, `utf-7`, `cp037` family) | 422 with one `differences.md` row listing them. No CI runner emits these |
| A query with several bad values (`SP10`) | Keep one issue per parse, and widen row `:124` to say so. The status is the same 422 |
| Multipart's extra parts and limits (`SP13`) | Keep Rust's answers. Correct row `:115`, and add one row covering the cases in `SP13`'s table |
| A refused timestamp's `msg` suffix (`SP14`) | Keep Rust's two sentences, with the exception stated in row `:124` |
| A settings interval a `Duration` cannot hold (`ST26`, `ST27`) | Refuse it at start-up, by name |
| A leader with a backlog (`ST24`) | Check the lock once per loop. The reference checks it only on an empty queue |
| A panic in the worker loop (`ST25`) | `serve` returns an error and the process exits. No `panic = "abort"`, so a parser panic stays a 500 |
| A stalled client during a stop (`ST31`, if reproduced) | Bound the drain with a documented constant |
| A persistent `process_next` error (`ST34`) | Back off with `backoff()`. The Report stays where it is |
| A direct `memchr` dependency (`CC14`) | Accept it, and delete the hand-written search. The crate is already in `Cargo.lock` |

## Running it on this machine

The last relay lost hours to the environment, and the disk has only about 13 GB free:

- Set `CARGO_INCREMENTAL=0`.
- Keep one shared `target/`. If `target/` passes 30 GB, run `cargo clean`.
- Warm test binaries before timing anything.
- A local `frontend/dist` changes `cfg(has_frontend)`. CI's Rust jobs build without it, so run clippy once with `dist` moved aside before pushing a change near `tests/frontend.rs`.
- `battery/run.sh` kills any process matching `target/debug/flakeradar`. Pass `OUT=` outside the repo.
