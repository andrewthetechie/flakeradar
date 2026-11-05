# Finding ledger

One row per finding. The owning task closes its rows. **State** is `open`, `fixed`, `not reproduced` or `no change`. A `fixed` row needs a commit and a test name. A `not reproduced` row gives its evidence in the last column. `no change` applies only to rows marked accepted here.

`Must` marks the owner's must-fix list from the review.

| Id | Must | Finding | Task | State | Commit | Test or evidence |
|---|---|---|---|---|---|---|
| S1 / H1 | 1 | Leader tasks die after a leadership drop | 16 | fixed | 4ec494e | `worker::LeaderRun` makes a `CancellationToken` per run and holds it beside the handle, so `start` after any `stop` runs again; both workers use it. `a_stopped_github_task_files_again_after_a_restart`, `a_stopped_webhook_task_delivers_again_after_a_restart`, `leadership_a_failing_processor_drops_files_after_it_returns`, `a_database_that_drops_the_leader_files_the_next_proven_flake` |
| S5 | 5 | SIGTERM and release wait for the whole sync | 16 | fixed | 4ec494e | `worker::until_stopped` races each call and wait against the run's token; the sync's `GET`, the fixed note's comment, its close, a delivery and a retry wait use it, and `ReportWorker::release` stops the same tasks. `stopping_ends_an_unanswered_sync_get_at_once`, `stopping_ends_an_unanswered_note_post_at_once`, `stopping_ends_an_unanswered_close_at_once`, `stopping_ends_an_unanswered_delivery_at_once`, `stopping_ends_the_wait_before_a_retry_at_once`; the issue-creating `POST` is the one call left to finish, pinned by `stopping_waits_for_a_filing_post_and_keeps_its_link` |
| J3 | | Unbounded GitHub queue | 16 | fixed | 4ec494e | The GitHub queue is bounded at `github_worker::QUEUE_MAX` (1000, the Notification queue's bound) and drops the oldest outcome with a `tracing::warn!` carrying the running count. `a_full_queue_drops_the_oldest_outcome_and_says_so`. Queued outcomes survive a leadership change, as the reference's do, which `a_database_that_drops_the_leader_files_the_next_proven_flake` also pins |
| S2 / J1 | 2 | Sync holds row locks across HTTP | 17 | fixed | eb78420 | `sync_closed_issues` opens no transaction: it reads the rotation list, calls GitHub for one row, then writes that row with one statement guarded on the number it checked (`issue_links::record_check`). `a_sync_holds_no_row_lock_across_its_get` holds a sync `GET` open, then updates the row the sync already wrote under `lock_timeout = '250ms'` and asserts a 200 from `POST /api/tests/{id}/quarantine` and a Report that reaches `processed` with `retry_count` 0; with one transaction held across the `GET`s (the old shape, restored as a mutation) it fails with SQLSTATE 55P03. `a_link_replaced_during_the_sync_get_survives` pins that the guarded write leaves a link re-read or replaced mid-`GET` alone. The only `begin()` left in `github.rs` is `commit_filed_link`'s savepoint, which opens after a filing `POST` has answered |
| S4 / A1 | 4 | Sync continues when GitHub is unreachable | 17 | fixed | eb78420 | `clear_if_closed` now breaks the batch on a transport error, as the reference's `network` arm does. `a_sync_stops_at_a_transport_error_with_one_request` uses two rows against a counting endpoint that hangs up on every connection: one request, neither row stamped, `stopped == Some(BatchStop::Unreachable)`. It failed with `left: 2, right: 1` before the fix. The misleading test comment the finding names is gone with the test it was in (see `T2 (sync)`) |
| S6 | | Fixed-note close does not stop on a transport error | 17 | fixed | eb78420 | `close_note` returns `ControlFlow::Break(BatchStop::Unreachable)` when the close gets no answer, and `note_batch` ends on it. `a_transport_error_on_a_fixed_note_close_ends_the_batch` has two noteable rows and a counting endpoint that answers the first comment and hangs up after: exactly 2 connections, the first row noted, the second never posted. It failed with `left: 3, right: 2` before the fix |
| B3 | | Status policy scattered, `bool` polarity, `()` returns | 17 | fixed | eb78420 | `Answer::classify(Success) -> Verdict` is the one classifier; `github::action(Operation, Verdict) -> Action` is the one table, with the per-operation differences in its doc comment (`Sync`/`NotFound` re-arms, the note paths treat a 401 per item, the post-delete close has no Repo skip); `ControlFlow<BatchStop>` replaced the mixed-polarity `bool`s of the sync and the close; and `file_issues_for`, `file_job_issues_for`, `note_fixed_issues_for`, `note_fixed_job_issues_for`, `sync_closed_issues` and `close_issues_for_deleted_repo` return `BatchReport { linked, skipped, stopped }`, logged once at the edge and still never raising. `every_operation_judges_every_verdict_as_the_reference_does` asserts all 40 verdict/operation pairs, `classify_names_every_answer_as_one_verdict` pins the classifier, and the 401 question is answered there: the reference's `_NOTE_SKIP_REPO_KINDS` excludes bad credentials deliberately, so `0a86d3a` does not go beyond it and no `differences.md` row was owed. `commit_filed_link`'s `bool` is A3, task 18's |
| T2 (sync) | | The sync test that hid A1 | 17 | fixed | eb78420 | `a_sync_rate_limit_stops_the_batch_and_keeps_the_stamps_it_made` covered a transport failure and a rate limit and its comment claimed "nothing is stamped, and the batch ends" without asserting either. It is three tests now, one concept each: `a_sync_stops_at_a_transport_error_with_one_request`, `a_sync_rate_limit_stops_the_batch_and_keeps_the_stamp_it_made` and `a_sync_continues_from_the_stamp_a_stopped_run_left`. The Test/Job string-selector half of T2 stays with task 27 |
| A3 / S13c | 6 | A database error closes a filed issue | 18 | fixed | ae87301 | `commit_filed_link` answers `FiledLink::{Stored, RowGone, StoreFailed}`, and `record_filed` acts on which it got: only `RowGone` runs the comment and close. `a_store_failure_leaves_the_issue_open_and_stops_the_batch` raises from a `BEFORE UPDATE` trigger on `test_cases`, files two Tests against a counting mock and asserts one `POST` and no comment or close, both links `NULL`, `stopped == Some(BatchStop::StoreFailed)`, and one error line naming `acme/app` and the number; before the fix it failed at `left: ["/repos/acme/app/issues", "/repos/acme/app/issues/7/comments", "/repos/acme/app/issues", "/repos/acme/app/issues/7/comments"], right: ["/repos/acme/app/issues"]`. `a_row_deleted_while_filing_closes_the_issue_it_just_created` still pins the row-gone half |
| A2 | 7 | Re-link reports the wrong `github_issue_closed` | 18 | fixed | ae87301 | `patched_out` takes the `LinkState` the call wrote instead of a `closed` argument, so a re-link of the same number reports what the row holds. `re_linking_a_declined_issue_reports_the_stored_closed_flag` exists twice, once through `PUT /api/tests/{id}/issue` (`tests/issue_links.rs`) and once through the `link_issue` tool (`tests/mcp.rs`); both failed with `left: Bool(false), right: Bool(true)` before the fix |
| S7 | | Issue body drops `Sample failure` | 18 | fixed | ae87301 | `newest_failure` takes the commit from the newest failing row and the sample from the first failing row with any text, as the reference's loop does. `the_body_matches_the_reference_byte_for_byte` compares the whole body to `S7_BODY`, generated by running the frozen `_issue_body` (86c98804) over S7's three rows; before the fix the Rust body lacked the `### Sample failure` block and `older details` that the expected value has |
| S8 | | `min_failures` uses the wrong window | 18 | fixed | ae87301 | `window_failures` uses the new `test_window_ids_overall_sql`, the first arm of the union, which is what `_window_failures` builds with no `include_default_branch`. `the_failure_gate_counts_the_overall_window_only` gives one Test five older Default-branch failures and a newest five with one failure on a feature branch, and asserts nothing is filed; before the fix it failed at `one failure in the overall window is under a gate of two`. The Job path was checked and already used `job_window_ids_overall_sql`; `the_job_failure_gate_counts_the_overall_window_only` pins it (it fails at its first assertion when `unexplained_failure_counts` is switched to the union), and `only_unexplained_job_failures_meet_the_min_failures_gate` keeps pinning the unexplained-only filter |
| J11 | | HTTP client build failure falls back silently | 18 | fixed | ae87301 | `Api::new`, `Api::production`, `AppState::new` and `WebhookWorker::{new, with_retry_delays}` answer `Result` and carry the builder's error with `.context`, and every caller propagates it: `Server::start` returns it, and the test suites call `.expect` at construction. No test can make a healthy TLS backend fail, so this row's evidence is the signature change plus the five `coverage-gaps.md` rows that record the unstageable arms |
| J13 / C6 | | Swallowed errors | 18 | fixed | ae87301 | The two dropped errors are logged with their context. `a_job_history_that_cannot_be_read_is_logged_and_the_issue_still_files` drops `job_executions`, proves the Job is still filed and asserts one `could not read a Job's Executions` line naming the Repo and `entity_id` (it failed with `left: 0, right: 1` before); `metrics_logs_the_error_it_answers_503_for` drops `reports`, keeps the empty 503 and asserts one `metrics scrape failed` line naming the table (also `left: 0, right: 1`). The client-build error C6 also lists is J11's row |
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
