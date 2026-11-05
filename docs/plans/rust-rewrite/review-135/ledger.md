# Finding ledger

One row per finding. The owning task closes its rows. **State** is `open`, `fixed`, `not reproduced` or `no change`. A `fixed` row needs a commit and a test name. A `not reproduced` row gives its evidence in the last column. `no change` applies only to rows marked accepted here.

`Must` marks the owner's must-fix list from the review.

| Id | Must | Finding | Task | State | Commit | Test or evidence |
|---|---|---|---|---|---|---|
| S1 / H1 | 1 | Leader tasks die after a leadership drop | 16 | fixed | 4ec494e | `worker::LeaderRun` makes a `CancellationToken` per run and holds it beside the handle, so `start` after any `stop` runs again; both workers use it. `a_stopped_github_task_files_again_after_a_restart`, `a_stopped_webhook_task_delivers_again_after_a_restart`, `leadership_a_failing_processor_drops_files_after_it_returns`, `a_database_that_drops_the_leader_files_the_next_proven_flake` |
| S5 | 5 | SIGTERM and release wait for the whole sync | 16 | fixed | 4ec494e | `worker::until_stopped` races each call and wait against the run's token; the sync's `GET`, the fixed note's comment, its close, a delivery and a retry wait use it, and `ReportWorker::release` stops the same tasks. `stopping_ends_an_unanswered_sync_get_at_once`, `stopping_ends_an_unanswered_note_post_at_once`, `stopping_ends_an_unanswered_close_at_once`, `stopping_ends_an_unanswered_delivery_at_once`, `stopping_ends_the_wait_before_a_retry_at_once`; the issue-creating `POST` is the one call left to finish, pinned by `stopping_waits_for_a_filing_post_and_keeps_its_link` |
| J3 | | Unbounded GitHub queue | 16 | fixed | 4ec494e | The GitHub queue is bounded at `github_worker::QUEUE_MAX` (1000, the Notification queue's bound) and drops the oldest outcome with a `tracing::warn!` carrying the running count. `a_full_queue_drops_the_oldest_outcome_and_says_so`. Queued outcomes survive a leadership change, as the reference's do, which `a_database_that_drops_the_leader_files_the_next_proven_flake` also pins |
| S2 / J1 | 2 | Sync holds row locks across HTTP | 17 | open | | |
| S4 / A1 | 4 | Sync continues when GitHub is unreachable | 17 | open | | |
| S6 | | Fixed-note close does not stop on a transport error | 17 | open | | |
| B3 | | Status policy scattered, `bool` polarity, `()` returns | 17 | open | | |
| T2 (sync) | | The sync test that hid A1 | 17 | open | | |
| A3 / S13c | 6 | A database error closes a filed issue | 18 | open | | |
| A2 | 7 | Re-link reports the wrong `github_issue_closed` | 18 | open | | |
| S7 | | Issue body drops `Sample failure` | 18 | open | | |
| S8 | | `min_failures` uses the wrong window | 18 | open | | |
| J11 | | HTTP client build failure falls back silently | 18 | open | | |
| J13 / C6 | | Swallowed errors | 18 | open | | |
| H0 / S3 | 3 | Nested suites abort the server | 19 | open | | |
| H3 | 8 | Quadratic allocation from one upload | 19 | open | | |
| H19b | | Non-finite `time` accepted | 19 | open | | |
| S9 | | Comments and processing instructions truncate text | 19 | open | | |
| S10 | | Fingerprint and status differences: encodings, namespaces, DOCTYPE, char refs | 19 | open | | |
| S13b | | `line` past range with no `file` fails a Report | 19 | open | | |
| J18 | | `Nd` digit block assumption | 19 | open | | |
| A4 / H2 | 9 | `?page=` overflow | 20 | open | | |
| H19a | | NUL gives 500 | 20 | open | | |
| S11 | | Lax coercion and multi-issue validation gone | 20 | open | | |
| S12 | | Unlogged status changes | 20 | open | | |
| S13a / J12 / C4 | | Pipeline attempt saturated; parsed twice | 20 | open | | |
| H18a | | Wrong `loc` for `runner_labels` | 20 | open | | |
| H4 | 10 | Malformed `.env` logs the token | 21 | open | | |
| H5 | | Database password prints through `Debug` | 21 | open | | |
| H14a | | `urls()` in the release binary | 21 | open | | |
| J2 | | Settings ranges unchecked; NaN scores 1.0 | 21 | open | | |
| B11 / H12a | | Defaults written up to four times | 21 | open | | |
| E1 | | Scenarios cannot set settings | 22 | open | | |
| E2 | | No failed-Report, sync, leadership or DB-restart scenario | 22 | open | | |
| E3 | | Replay never uses a real listener | 22 | open | | |
| E4 | | `<timestamp>` checks no ordering or equality | 22 | open | | |
| E5 | | `compare_openapi` checks names only | 22 | open | | |
| E6 | | Open-ended allowances; no reconciliation with `differences.md` | 22 | open | | |
| E7 | | Task 11: no real MCP client | 22 | open | | |
| D1 | | MCP tools differential | 23 | open | | |
| D2 | | Read routes with real parameters; `queries.py` vs `reads.rs` | 23 | open | | |
| D3 | | Failed-Report paths | 23 | open | | |
| D4 | | Concurrency | 23 | open | | |
| D5 | | Ingest limits and `t8` | 23 | open | | |
| D6 | | Retention and webhooks, run | 23 | open | | |
| D7 | | Tasks 01, 02, 03, 12, 13 criteria | 23 | open | | |
| D8 | | Scope check | 23 | open | | |
| B5 | | SQL builders and row helpers | 24 | open | | |
| B6 | | Route plumbing | 24 | open | | |
| B10 | | MCP tool in five places | 24 | open | | |
| J7 | | Duplicated helpers | 24 | open | | |
| J9 | | SQL and HTTP across layers | 24 | open | | |
| H13 | | `Result<_, String>`; `ProcessError` | 24 | open | | |
| J17a | | `M-PUBLIC-DISPLAY` on route errors | 24 | open | | |
| H6 | | No newtypes | 25 | open | | |
| B4 | | Stringly typed domain values | 25 | open | | |
| B9 / J15 | | Positional tuples, argument order | 25 | open | | |
| B12 | | Tier booleans; `Board` copies `ListOptions` | 25 | open | | |
| J10 | | `Scope` invariant unenforced | 25 | open | | |
| H17 / C3 | | Glossary and misleading names | 25 | open | | |
| B2 | | `github.rs` has five reasons to change | 26 | open | | |
| C1 | | Issue bodies built by pushing lines | 26 | open | | |
| J14a | | Dead parameters in `github.rs` | 26 | open | | |
| B1 | | Parallel Test and Job flows | 27 | open | | |
| T2 (rest) | | Tests selected by entity string | 27 | open | | |
| B8 | | `process_report` mixes levels | 28 | open | | |
| C2 | | Flag arguments | 28 | open | | |
| C5 / J5 | | Needless copies | 28 | open | | |
| J4 | | CPU work on async threads | 28 | open | | |
| J6 | | N+1 update | 28 | open | | |
| J8a | | Python exception names as stored prefixes | 28 | open | | |
| J14b | | Remaining dead parameters | 28 | open | | |
| H7 / T1 | | Real-clock tests | 29 | open | | |
| H8 | | Production bypasses the clock seam | 29 | open | | |
| H14b / T5 | | Test helpers outside test-support | 29 | open | | |
| H20 | | Test placement, tautological tests | 29 | open | | |
| T3 | | Log scraping | 29 | open | | |
| T4 | | Positional test tuples | 29 | open | | |
| J8b | | `comparisons::record` in production | 29 | open | | |
| H9 | | Lint opt-outs | 30 | open | | |
| H10 / B7 | | Rustdoc about the deleted codebase | 30 | open | | |
| H11 | | Stale compliance markers | 30 | open | | |
| H12b | | Remaining magic values | 30 | open | | |
| H15 | | Two paths, versions outside the workspace, unused dependencies | 30 | open | | |
| H16 | | Unstructured logs | 30 | open | | |
| H18b | | Wrong docs and doc links | 30 | open | | |
| J17b | | `cargo doc` gate, `rust-version` | 30 | open | | |
| J16 | | `dyn` seams | accepted | no change | | The seams serve tests the standard asks for |
| J17c | | `M-MIMALLOC-APPS` musl only, `M-TARGET-CPU`, no `cargo-hack` or Miri | accepted | no change | | Defensible per `standards-findings.md` |
| J8c | | Pydantic error classification by serde message; hand-written multipart | accepted | no change | | Wire contract, pinned by tests |
