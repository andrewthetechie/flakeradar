# Handoff: the Rust rewrite on `feat/rust-rewrite`

This file is for the reviewer of PR #135. It collects what landed, what each task's commits
are, everything the port could not verify, every checker finding and what happened to it, the
risks the workers listed, and where to start reading. `STATUS.md` is the row-by-row record;
this file is the summary you need before you read it.

Facts that can be re-derived from the repository were re-derived rather than copied from
`STATUS.md`, and where a `STATUS.md` claim could not be confirmed it says so. Verified here:
`git log` and `git rev-list --count` for every range, `cargo nextest list --all-features` for the
test count (952), `cargo test --doc --all-features -- --list` for the doctests (23), file counts
under `tests/parity` for the goldens (366 JSON files plus `schema.sql`), `git diff --shortstat`
for the size, `gh run view` for the CI claims, pydantic 2.14.2 and junitparser 5.0.3 for the two
disproven findings, and two targeted `cargo nextest` runs for the tests those findings concern.
Not re-derivable, and quoted from the document named beside it: the coverage percentages
(`coverage-gaps.md`), the audit histories and older CI run ids (`STATUS.md`'s rows), and the
performance and reliability numbers (`docs/perf/22`).

| | |
| --- | --- |
| Branch | `feat/rust-rewrite`, head `130691c` |
| Python reference | `97b933d`, the last Python commit |
| Branch point | `33703f7` (the plan documents); task 01's work starts at `1179fe3` |
| Commits in `97b933d..HEAD` | 189, of which 2 (`bfd3fc4`, `86c9880`) are `main`'s #130 and #131 brought in by the merge `182ab4a` |
| Diff against `97b933d` | 692 files, +97391 / -24792; `backend/` alone is 96 files and 24535 deletions |
| Pull request | #135, *feat: replace the Python backend with one Rust binary*, open, unmerged, untagged |

## 1. What landed

The Python service is gone: `backend/`, `tools/parity-record/`, `ruff.toml` and the manifest
checker are deleted, along with the CI jobs that drove them. One Rust binary now serves
everything the Python server did — the API, the dashboard embedded out of `rust-embed`, the MCP
server over stateless Streamable HTTP, and the report worker (plus the GitHub and webhook leader
tasks inside the same process) — and the workspace's gate is 952 tests, 23 doctests, `cargo fmt`,
`clippy -D warnings` and `cargo audit`. All 367 parity files replay: 300 recorded scenario
steps across 13 scenarios, 66 JUnit parse goldens, and `schema.sql`, which
`crates/flakeradar-db/tests/schema_parity.rs` compares against a freshly migrated database, with
`allowed-differences.json` as the only place a mismatch can be accepted. Every parity scenario
is enabled — `crates/flakeradar/tests/parity.rs` no longer has an `ENABLED` list or a
`DEFERRED_STEPS` count, so a scenario cannot be switched off to make the replay pass.

## 2. Per-task commit ranges

The ranges below are contiguous and cover the whole branch: each one starts at the commit after
the previous task's last, and the 15 counts sum to 188, which with the plan commit `33703f7` is
the 189 in `97b933d..HEAD`. They include each task's own audit follow-up and `docs(status)`
commits, because that work belongs to the task that produced it. The "landed" column repeats
`STATUS.md`'s Commit cell, which is the commit the row marks Done at and which is sometimes not
the last commit of the range.

| Task | Title | Range (first..last) | n | Delivered | Landed |
| --- | --- | --- | --- | --- | --- |
| 01 | Workspace, lints, nextest, CI skeleton, code standards | `1179fe3..929f0a5` | 6 | The Cargo workspace, the toolchain pin, the lint set, the version and coverage-gate scripts, the first CI jobs, the Rust code standards, and the 2.0.0 version bump | `f95dbcf` |
| 02 | Golden recorder and pytest manifest | `8ab34ef..78f1087` | 4 | The pytest manifest checker (575 rows) and the recorder that produced all 367 goldens; separate corpus files and goldens for the three `<testcase time>` values Python accepts | `a305157` |
| 03 | Baseline schema, entities, test databases, SeaORM spike | `075a146..3d3925d` | 11 | The baseline migration, generated entities, `connect`, chunking, the test-support crate with per-test template databases, `crates/flakeradar-db/tests/schema_parity.rs`, and `spike-notes.md`'s eight query answers | `3184664` |
| 04 | Core logic | `9a31e71..61612e6` | 8 | `flakeradar-core`: the Python string/cap/round helpers, the JUnit parser, scoring, Failure categories, naming rules, webhook rendering and every DTO from `schemas.py`; all 66 JUnit goldens replay | `43e9f70` |
| 05 | Server shell | `724a6cb..c451d66` | 16 | `Settings` and `assert_secure_token`, the Axum app with its 422 extractors, the 307 fallback and the 404 body, `/api/health`, `/api/health/ready`, `/metrics`, `/docs` (Redoc) and `/openapi.json`, `Server::start`/`serve`, and the first parity harness with its `ENABLED`/`DEFERRED` lists | `293fc18` |
| 06 | Ingest and Report routes | `e5033de..939023c` | 10 | `POST /api/ingest` reading multipart or a raw body, `POST /api/ingest/pipeline`, the queue's summary/list/detail/retry/delete routes, and the two 400s this port decided (unparsable `time`, out-of-range `ci_run_attempt`) | `f144215` |
| 07 | Processing, attribution, Score history | `bd90d6c..967b53e` | 21 | `process_next` with its savepoint and retry budget, both processing paths statement for statement, `attribution`, `score_history` with `HISTORY_DAYS`, `job_history`, the injected `Clock`, and the parity harness driving the queue | `af5a9eb` |
| 08 | Worker and retention | `c511bd7..e86de42` | 19 | `ReportWorker` with `pg_try_advisory_lock(726300002)` leadership and a `pg_backend_pid()` liveness check, `retention.rs`, `run_server`'s SIGTERM/Ctrl-C path, and the parity harness driving the real worker | `e1a26bc` |
| 09 | Read APIs | `a3df71a..d0eb560` | 15 | `flakeradar-db/src/reads.rs`'s descriptor engine and every read route: repos, tests, summary, test history, quarantine (POST and list), jobs, job summary, job history and CI-run failures, each with its `#[utoipa::path]` | `628d091` |
| 10 | Issue links, GitHub, webhooks, repo admin | `373b88f..a95f38e` | 39 | The issue-link routes, the GitHub client with filing, sync, notes and close-on-delete, the Notification (webhook) task, the Repo and Project admin deletes, and 531 manifest rows | `1566540` |
| 11 | MCP | `c5e55d2..d5a4736` | 7 | The eleven tools over `rmcp` 3.5 with `NeverSessionManager`, the literal `tools/list` answer copied from the recorded wire, and all 46 `mcp` steps plus `trailing-slash` 7-8 replaying | `089b540` |
| 12 | Frontend embedding | `a5cad88..1574a80` | 6 | `rust-embed` plus a `build.rs` `cfg(has_frontend)`, the asset fallback with its ETag/304, cache policies, `HEAD`, the 405 and the traversal refusal, and the headless-browser check | `a5cad88` |
| 13 | CLI, Docker, release, five-target smoke test | `e8062f1..933b22e` | 9 | `flakeradar [serve\|healthcheck]` with `--version`, the 28.4 MB Dockerfile with its `HEALTHCHECK`, compose, `scripts/smoke.sh`, the five-target matrix, `release.yml` with provenance attestation, and the CI fixes that made them green | `0345928` |
| 14 | Convergence | `ce9f90b..57b22b6` | 8 | No `DEFERRED`/`DEFERRED_STEPS` left at all, the `openapi` contract for `trailing-slash` step 9, `crates/flakeradar/tests/consumers.rs`, the `report-action`/`summary-action`/`samples` jobs, `docs/perf/22` and the re-measured `20`/`21`, the reliability checks, and the coverage floor | `2ba24af` |
| 15 | Cutover and the PR | `fd259c1..130691c` | 9 | The Python deletion and the leftover sweep, the README/CONTRIBUTING/SECURITY/`.env.example` rewrite, `docs/cutover-v2.md`, the `.fabro` switch to Cargo, the 78 follow-on drafts, and `main`'s #130/#131 delta (the 401 Repo skip) ported before the merge | `92affdf` |

`bfd3fc4` (#130) and `86c9880` (#131) are numbered inside task 15's range because the merge
`182ab4a` brought them in; they were written on `main`, not here, and `git diff --shortstat
origin/main HEAD -- backend/` reports 96 files changed and 25179 deletions with no insertion, so
no conflict hunk kept a Python file.

## 3. Every UNVERIFIED item

Collected from the `UNVERIFIED` notes in `STATUS.md`'s rows and from the workers' reports in
those rows. Nothing is softened. Items 1-10 are still open; 11-16 were closed after they were
written, and the paragraph after them names the CI run that closed three of them.

1. **`x86_64-pc-windows-msvc` cannot be built on this macOS host.** There is no MSVC toolchain,
   no Windows SDK and no `link.exe` here, so the leg is declared in the matrix but only ever
   built on `windows-latest`. Nothing here ever ran it; task 13's row says so, and task 15's row
   repeats it. (The leg has since been *observed* green in CI — see 13.)
2. **`release.yml`'s `workflow_dispatch` rehearsal could not run.** GitHub only dispatches a
   workflow that exists on the default branch, and this one first lands on `feat/rust-rewrite`:
   `gh workflow run release.yml --ref feat/rust-rewrite` → `HTTP 404: workflow release.yml not
   found on the default branch`. Nothing has exercised the tag path: the checksums, the
   `gh attestation verify` instructions and the two publish workflows in the README are
   instructions, not observations.
3. **The duplicate-issues half of the two-instance reliability check is unverified.** Issue
   filing posts to GitHub at a fixed base URL and the binary reaches no stub, so a duplicate
   issue can only be observed against a real GitHub App installation and token. What was
   measured is the mechanism that would prevent one (one leader, each Report claimed once, no
   duplicate Run); `docs/perf/22` records the same gap.
4. **The follow-on coverage issues are drafts by design.** `coverage-follow-ups.md` holds 78
   drafted issues and the user files them after this PR merges, because they must not carry the
   `agent` label while the branch is off `main`. Nothing is filed yet, so the 78 gaps have no
   tracker entry.
5. **Task 09's Done-when browser check was not run.** The condition asked for the frontend
   against the Rust server under `npm run dev` plus Playwright. What was done instead is the
   ported API tests plus `fabro_reads_every_field_of_the_test_reads`. Task 12's headless-browser
   run loaded the dashboard from `target/debug/flakeradar`, which is a different check against a
   different server and does not discharge this one.
6. **Task 11's Done-when real-client check was not run.** A local Claude Code MCP config pointed
   at `http://localhost:8000/mcp/`, listing tools and calling `list_repos` and
   `top_flaky_tests`, was never tried: no such client is installed here. The transport is
   instead proved over a loopback socket and by the 46 golden replays, and the tests drive it
   with JSON-RPC documents over `reqwest`/`oneshot` rather than with `rmcp`'s own client type —
   the same wire, but not the same client code.
7. **The Docker path of `crates/flakeradar-test-support/src/postgres.rs` is not measured by
   CI.** CI sets `FLAKERADAR_TEST_DATABASE_URL`, so the container-start branch never runs there.
   It was exercised by hand: three cold-start runs (`docker rm -f flakeradar-test-postgres`
   first) ran the whole suite with no failure.
8. **Fabro, the external consumer, was never run against this server.** Its structs and its
   `normalise_ts` rule were reproduced in `crates/flakeradar/tests/consumers.rs`, including a
   negative control, so the payload shapes are pinned — but the real Fabro binary is not in this
   repo and was not pointed at the binary.
9. **`main`'s #130/#131 delta was ported but not reviewed by `main`'s authors.** Task 15's row
   says the 401 Repo skip was implemented in `file_batch` before the merge and that
   `a_bad_credentials_401_skips_that_repo_and_files_the_other_one` fails at its wiremock
   `expect(1)` without it. Reasonable, but it is this branch's judgement of `main`'s intent, and
   the diff the merge kept is a deletion of both Python files, so nothing on `main` re-tested
   it.
10. **The evidence behind the two disproven findings now lives outside the tree.** The auditor's
    `bool` suspicion and the comma claim were settled by running pydantic 2.14.2 and reading
    junitparser 5.0.3 — both pinned in `backend/requirements.txt` and installed in
    `backend/.venv`, which this branch deletes. Both were re-verified from the public package
    releases for this file (see section 4, rows 5 and 27), but a reviewer offline cannot repeat
    that from the repository. The same is true of every other reference measurement recorded in
    `STATUS.md`: the frozen Python service can be checked out at `97b933d`, not run in CI.

Closed after they were written:

11. **Task 14's new CI jobs and the five-target matrix were not observed green when the row was
    written** (`binary`, `report-action`, `summary-action`, `samples`, and the `parity-goldens`
    job that was deleted again in task 15). The `samples` job was not run locally either,
    because it needs the release binary plus `pytest`.
12. **Task 13's commit `0345928` was red when its row was written**: `Rust (fmt + clippy + test
    + audit)` failed on `flakeradar::github_worker a_lost_lock_connection_stops_the_github_task`
    at `crates/flakeradar/tests/github_worker.rs:397` with `the lock is held`, a real timing
    dependence in the test (it read `pg_locks` after the fact instead of staging the lock
    holder).
13. **Task 15's row could not observe the CI run on the PR head** — in particular the five
    target legs and the coverage job.
14. **The EXPLAIN figures in `spike-notes.md` came from a laptop seed** (500 Tests × 200 Runs),
    so only the plan shape and the index names were claimed against `docs/perf`. `docs/perf/20`
    and `21` later re-ran both reads on a 100,000-Execution seed and confirmed the same
    `Limit`-per-id plans over `ix_exec_case_id` / `ix_job_exec_job_id`.
15. **A worker's audit row claimed a check `docs/perf/22` did not perform** for the
    PostgreSQL-restart case; it was re-run on 2026-11-05 with its numbers recorded, and the
    two-instance section gained the evidence it used to assert.
16. **The `parity-goldens` reconciliation job was proved by the push after its row, not in the
    row.** It has since been deleted with `backend/` — see the risk list.

**The CI jobs added late in tasks 14 and 15 were observed green on PR #135.** `gh run view`
against the PR head `130691c`: CI run `42071653737` reports **15 jobs, every one `success`** —
`Rust coverage (lines + branches)`, `Rust (fmt + clippy + test + audit)`, `Binary (release)`,
`Frontend (typecheck + tests)`, `Frontend build`, `Changed paths`, `Release version
consistency`, `Report action (fixtures)`, `Samples (GitLab reporter + demo data)`, `Summary
action (fixture)`, and all five `Target (...)` legs including `x86_64-pc-windows-msvc`. Docker
run `42071653733` on the same head is also `success`, with `Build (linux/amd64)` and `Build
(linux/arm64)` both passing (`Merge the manifest list` is skipped on a PR, correctly). That
closes items 11, 12 and 13 above, and it means the Windows leg in item 1 has been built and
smoke-run somewhere even though it cannot be built on the machine this port was written on.

## 4. Checker findings and their resolution

One line each: what the independent checker found, and what fixed it or why it was accepted.
The severity is the checker's own label. Where a row below says "disproven", the auditor was
wrong and the code was right — please do not re-open those two on a later pass.

| # | Task | Found | Resolution |
| --- | --- | --- | --- |
| 1 | 03 | P2: `TestDb` published the template database before the migration had succeeded, so a killed setup could leave an empty template every later test clones | The template is built under a `<template>_building` staging name and published with `ALTER DATABASE ... RENAME` only after the migration succeeds; a stale staging database is swept first |
| 2 | 03 | P2: `crates/flakeradar-db/tests/schema_parity.rs` compared snapshots with no proof that a difference is reported by name | `snapshot_difference`, plus cases that drop a column, change a type and change nullability, each asserted to be named, and a guard that a column, constraint and index line keeps every field |
| 3 | 04 | `QuarantineIn.reason`, `QuarantineIn.expires_in_days` and `IssueLinkIn.url` lacked `#[serde(default)]`, so bodies Python accepts were refused | `#[serde(default)]` added; the bodies Python takes (`{"quarantined": false}`, `{"number": 12}`) are tested |
| 4 | 04 | The `flip_score` doctest asserted the wrong constant, and CI did not run doctests at all | It now says `1.0 / 6.0` (`scoring.py`'s value) and the Rust job runs `cargo test --doc --all-features` beside nextest |
| 5 | 04 | **Disproven by measurement.** The auditor said `python_float` was wrong to strip commas from a `<testcase time>`, on the reading that Python's `float()` would reject `1,5` | The auditor was wrong and the code was right: junitparser does `float(result.replace(",", ""))` at `junitparser.py:132` (5.0.3, the pinned version), so `<testcase time="1,5">` really is 1.5 in the reference. The testcase path keeps the stripping; `a_testcase_time_with_thousands_commas_is_read_as_junitparser_reads_it` passes. The auditor was right about the *other* path — `<flakyFailure time>` goes through raw `float()` — so the retry path no longer strips, and `a_retry_time_with_a_comma_is_not_a_number_so_it_is_zero` pins the 0.0 |
| 6 | 04 | The allowance for the unparsable-`time` golden named neither the golden's `body` nor its `content-type`, so the pre-approved 400 could not replay | Both fields added to that one golden's allowance, and `differences.md` says the 400 carries the usual `{"detail": …}` |
| 7 | 05 | P1: parity validation step 17 records **404**, not 422 — Pydantic's lax `bool` takes `{"quarantined": "yes"}` as true, so Python validated the body and answered `{"detail": "Test not found"}` | The extractor fixture no longer borrows the quarantine route's field names, and the obligation to take Pydantic's bool spellings and reproduce that 404 is written into the validation deferral and the task 09/10 rows |
| 8 | 05 | P2: Axum's empty 405 body where FastAPI answers `{"detail": "Method Not Allowed"}` | The body and an `Allow` header are produced, pinned by `tests/app.rs` |
| 9 | 05 | P2: the readiness probe's failed queue query answered a JSON `Database error`, where Python answers plain-text `Internal Server Error` | Reproduced Python's plain-text 500 |
| 10 | 05 | P2: `Settings` trimmed whitespace Python's `int()`/`float()` did not | Only what Python trimmed is trimmed; a padded bool or webhook format is refused |
| 11 | 05 | P2: `coverage-gaps.md` carried a false claim (that Axum's `ErrorKind::ParseError` arm was unreachable) and a stale `require_token` row, and missed several rows | The false row was deleted, the stale one removed, and `issue_for`'s `Seq`/`Enum`/unknown-`loc` arms, `python_float`'s guards and the renamed `ready`/`database_failure` row added |
| 12 | 06 | A `ci_run_attempt` past `int4` saturated instead of failing | On the user's decision it now answers **400** (`ci_run_attempt must be at most 2147483647`) and queues nothing; the value is read as text so a number past `i64` is told apart from a word; the saturating row left `differences.md` |
| 13 | 06 | `test_build_junit_report_defaults`'s manifest row was dishonest: `na` on a claim that other tests covered the defaults, which they did not | `ported`, with `ingest_defaults_branch_and_ci_run_id` asserting the stored and served `branch == "main"`, `ci_run_id == ""`, `status == "pending"` |
| 14 | 06 | The hand-written multipart reader diverged from `starlette.formparsers.MultiPartParser` 1.10.0 in three ways | Checked against the reference parser directly: a delimiter not followed by CRLF or `--` is data; a duplicated `report` part is the LAST one; a body with a preamble or bare-LF framing is no form. The refusal's detail is the one new differences row |
| 15 | 06 | The multipart scan ran `windows().position()` inline on the async worker | A KMP search with the failure table built once (at most two comparisons per body byte), run in `spawn_blocking`, proved by a unit test on a megabyte of near misses that asserts the comparison bound and no wall-clock |
| 16 | 06 | Ingest was not transactional, so a failed insert could leave a Repo and Project behind | One transaction for get-or-create, duplicate check and insert (both ingest routes); `a_failed_ingest_leaves_no_repo_or_project` and `a_duplicate_upload_creates_no_extra_rows` prove it |
| 17 | 07 | P1: `touched_test_ids` was collected in first-occurrence order, so a Report naming Tests out of id order broke Python's `sorted(latest)` | The ids are sorted; `touched_tests_are_rescored_in_id_order_not_report_order` and `tier_changes_are_emitted_in_id_order` fail with the sort removed |
| 18 | 07 | `a_downward_tier_crossing_is_reported`'s assertion could not catch a spurious `test.recovered` | It asserts the empty vector Python yields there |
| 19 | 07 | The parity harness decoded a hex `body` column anywhere, including inside HTTP bodies | Decoding is limited to a dumped table column (`names_a_dumped_column`), with `hex_is_decoded_in_a_dumped_table_column_and_nowhere_else` as the negative control |
| 20 | 07 | `flaky-tests` was deferred wholesale although its ingest steps needed nothing task 07 had not landed | Steps 1-12 and 48 replay with no allowance entry on that scenario; `DEFERRED_STEPS` 236 → 223 |
| 21 | 08 | The gap list was thin: two added lines were missing from it | The test-only `Sink::flush` in `settings.rs` and the shutdown warning in `server.rs` are logged; the fixture quarantine flag and the Job fixture's fixed-noted setter are covered by new tests instead of logged |
| 22 | 08 | A failing test (`run_prune_stops_at_time_budget`) was a test bug, not an implementation bug | It asserted a substring the log line does not contain; it now pins the whole `Retention prune hit its 60s budget; the rest waits for the next interval`. The row's claim that the line was Python's verbatim was corrected: Python prints `60.0s` |
| 23 | 08 | Task 08's Done was claimed at `f408d64` on a tree that was not green | The work lands at `ff83ba3`; the failure was the test refusing new connections before ending its own lock session, and both cases now order the statements correctly and prove the drop rather than assuming it |
| 24 | 08 | P1: the leader proved it still led with `SELECT 1` on a one-connection pool that replaces a connection the server closed, so a replacement holding no lock could answer | `AdvisoryLock` records `pg_backend_pid()` in the locking statement and `check()` compares it; a killed session, a statement answered by another backend and a never-elected session all drop leadership (`DbError::LockSessionLost`). `a_killed_lock_session_is_never_alive_again` fails against the old `SELECT 1` |
| 25 | 08 | `rescore_jobs`' id ordering was not covered | `job_tier_changes_are_emitted_in_id_order` fails with `[2, 1]` when the sort is removed |
| 26 | 09 | P1: `lax_bool` did not match Pydantic 2.14.2 — it refused the float `1.0`/`0.0`/`-0.0` that Pydantic accepts, and reported `bool_parsing` where Pydantic says `bool_type` | `interpret` was rewritten from a row-by-row run against the project's pinned Pydantic, `extract::kind_for` maps the two messages, and the tables are pinned by `a_lax_boolean_takes_exactly_the_values_pydantic_reads` and its neighbours |
| 27 | 09 | **Disproven by measurement.** The auditor suspected Pydantic's handling of an integer above `i64::MAX` was mis-ported | The auditor was wrong and the code was right: measured against pydantic 2.14.2, `9223372036854775807` (`i64::MAX`) is `bool_parsing` while `9223372036854775808` is `bool_type`, which is exactly what `dto::interpret` answers — see the note in `STATUS.md` under "Reference-behaviour notes" |
| 28 | 09 | Six parity steps were still deferred although task 09 lands every route they call (`auth` 6 and 13, `consumers` 6-9) | All six enabled; `DEFERRED_STEPS` 100 → 94, and `consumers` 6 was shown non-vacuous by clearing the list and watching the replay fail at that step |
| 29 | 09 | Three `coverage-gaps.md` rows were wrong and one was missing | `QuarantineBody::from_request`'s body-read arm, the `reports.failed` counter and the unknown-Failure-category path are covered by new tests and their rows are gone; `lax_bool::optional`'s `Value::Null` arm is reachable and covered, and the two genuinely unreachable arms are logged instead |
| 30 | 09 | Two comments claimed the `quarantine` dump was still deferred | Corrected; step 18 is enabled and proves the route clears a stale reason and expiry |
| 31 | 09 | The wip commit had flipped the manifest's line endings to CRLF | Back to LF, so the diff is the 90 rows |
| 32 | 10 | P1: the row claimed no gap was owed, on the ground that the misses were "reachable 500 and transport arms" — which contradicts the coverage goal | All 23 task-10 gap rows are logged, and almost everything else the report listed was covered by tests instead (client errors, failing streams, dropped tables, `is_deadlock`'s arms, redacted `Debug`, and more) |
| 33 | 10 | `deletes::commit_failure` mapped a 40P01/40001 from `commit` to nothing, only from the DELETE statement, so a deadlock at commit answered 500 where Python answered 503 | The conversion covers `commit`; the 503 path is tested |
| 34 | 10 | The Notification loop stopped taking batches when one delivery raised | `run_with` logs and keeps serving, with a raising delivery injected by a test |
| 35 | 10 | `Server::start` built `WebhookWorker` even with no URLs configured | Built only when `parse_urls` is non-empty (`webhook_task`), pinned by a unit test that also asserts the trimmed list |
| 36 | 10 | `test_an_unreachable_endpoint_never_raises` only asserted that `stop` returned, so a one-attempt `post` and a panic into the `JoinError` arm both passed | The test now captures logs, counts the six "Webhook endpoint unreachable" lines of two batches, asserts `stop` returned `Ok`, injects zero retry delays so nothing reads a clock, and fails at the first wait if the loop is cut to one attempt |
| 37 | 10 | `test_sync_failure_is_logged_and_retried`'s manifest row carried a claim the Rust test cannot make (`sync_closed_issues` never raises) | The row says so and names the retry and the standing link as what is asserted |
| 38 | 10 (second round) | The last four `github.rs` gap rows named reachable code | Covered with tests that stage the death on a loopback `TcpListener` (wiremock always finishes a response) and one that proves a token a header cannot carry goes out with no `authorization` header; `github.rs` 90.11% of lines and 93.29% of branches, and the rows are gone |
| 39 | 11 | The row said a raising tool answered a JSON-RPC `error`; the reference answers HTTP 200 with a JSON-RPC **result** carrying `isError: true` | Corrected in the row and in the code (`d5a4736`), and the same measurement closed three other transport divergences: unknown tool names and unreadable arguments are also 200 `isError` results, and a POST whose body is not JSON is the reference's 400 with `-32700` (`mcp::require_json_body`) where rmcp answered 415 |
| 40 | 12 | P2: `HEAD` carried `content-length: 0`, where Starlette's `FileResponse` announces the file's length | `asset_response` sets the header from the asset's own bytes; `a_head_request_carries_the_headers_and_no_body` asserts the page's length |
| 41 | 12 | P2: the traversal test did not cover a second decode | It now refuses `/assets/%252e%252e/index.html`, `/..%252findex.html` and `/%252e%252e/%252e%252e/etc/passwd`, so a future switch to repeated decoding cannot escape with no test to catch it |
| 42 | 12 | P2: `differences.md`'s 405 row omitted that the reference answered 404 for a trailing-slash API path when `dist` was mounted | The clause is added, together with the note that this port keeps the 307 |
| 43 | 13 | The parity replay raced `ReportWorker`'s first election, so `health`'s `/ready` and `/metrics` goldens compared `processor_leader: false` against `true` | The replay waits (bounded at 30 s) for its worker to hold the lock before any step (`69c0062`) |
| 44 | 13 | The `x86_64-unknown-linux-musl` static check only accepted `statically linked`, but that leg builds a `static-pie linked` binary | Both spellings accepted, and `ldd` must still report no dynamic dependency |
| 45 | 13 | The Windows leg passed its smoke test and then failed to upload, because the artifact path named `flakeradar` with no `.exe` | The matrix carries the built file's name for the smoke step and the upload alike |
| 46 | 13 | `release.yml`'s attestation steps ran under read-only workflow permissions | The build jobs ask for `id-token: write` and `attestations: write` |
| 47 | 13 | `a_lost_lock_connection_stops_the_github_task` read `pg_locks` after the fact and flaked on a loaded runner | `0345928` makes the test read the lock before disturbing it; the coordinator then looped that test 15 times and the full suite 3 times, all passing, and CI run `42044601945` is green |
| 48 | 14 | P1: `test_no_match_warns_without_failing_or_uploading` was `na` on a claim that the Report action covered it, and nothing exercised `actions/report/report.sh`'s no-match path | The job gained two steps: the warning and exit 0 on a pattern matching nothing, and an empty `report_ids` with the two fixture Reports still queued. The row stays `na` with a reason that names those steps |
| 49 | 14 | The `samples` job would have gone green on 13 skips, because the test module skips itself without `FLAKERADAR_URL`/`FLAKERADAR_TOKEN` | It captures `pytest -q -rs`, fails on a non-zero status or any skip, and requires the summary to read `13 passed` |
| 50 | 14 | `docs/perf/22` implied a check it had not performed for the PostgreSQL restart | The check was re-run on 2026-11-05 with its numbers (20.2 s outage, four 5.00 s 500s, `pending: 0` 2.0 s later, all 100 Reports processed, no duplicate Run group) and the two-instance section gained its own evidence |
| 51 | 14 | The row's `na` split and the README's pointer to the GitLab reporter were wrong | Corrected to 13/2, and the README now describes the file (POST the built payload to `/api/ingest/pipeline`, expect 202) |

## 5. Tasks that needed more than one fix attempt

Ten tasks needed more than one fix attempt after their first "Done" — task 10 needed two
rounds — and the list runs in task order. One line each on what the extra round caught:

- **04** — four defects: three input DTOs missing `#[serde(default)]`, the `flip_score` doctest
  and the absent doctest step in CI, the float/comma handling (half of that finding disproven,
  see #5 above), and an allowance that named neither `body` nor `content-type`.
- **05** — the P1 was a wrong expected status in a parity step (404, not 422, because Pydantic's
  lax bool accepted `"yes"`); the four P2s were the empty 405 body, the readiness probe's JSON
  error text, `Settings` trimming more than Python, and a gap list that was both wrong and
  incomplete.
- **06** — five: the saturating `ci_run_attempt`, a dishonest manifest row, three multipart
  divergences from Starlette, an O(n·m) scan inline on the async worker, and a non-transactional
  ingest.
- **07** — the P1 was rescores in report order instead of id order; the P2s were a tier-crossing
  test that could not fail, hex decoding applied too broadly in the harness, and a wholesale
  `flaky-tests` deferral that outlived the routes it waited for.
- **08** — four rounds' worth: the budget-log test asserting a substring the line never prints;
  a Done claimed on a red tree at `f408d64`; the P1 liveness check (`SELECT 1` answerable by a
  replacement connection); and a gap list that was missing two added lines while two reachable
  misses were logged instead of covered.
- **09** — the P1 was `lax_bool` disagreeing with Pydantic 2.14.2 on floats and on the
  `bool_type`/`bool_parsing` split; the rest were six steps deferred past their routes, four
  wrong gap rows, two stale comments and a CRLF manifest.
- **10, first round** — the P1 was the claim that no coverage gap was owed; alongside it, three
  production defects (deadlock at commit answering 500, the Notification loop dying on a raising
  delivery, `WebhookWorker` built with no URLs), a vacuous webhook test, and an overclaiming
  manifest row.
- **10, second round** — the last four `github.rs` gap rows named reachable code, so they became
  four tests staging connection and body deaths on a loopback listener.
- **11** — the row described a raising tool as answering a JSON-RPC `error`; the reference
  answers a 200 `isError` result, and the same measurement turned up three more transport
  divergences (unknown tool, unreadable argument, non-JSON body answering 415 instead of 400).
- **12** — three P2s: `HEAD`'s zero `content-length`, a traversal test that never tried a second
  decode, and a differences row missing the trailing-slash case.
- **14** — the P1 was the `na` manifest row that claimed coverage nothing provided; the P2s were
  the samples job passing on skips, `docs/perf/22` implying a check it had not run, and a wrong
  `na` split plus a wrong README pointer.

Two more tasks took follow-up commits without a fresh dispatch, and saying so is more useful than
a tidy list: **03** (two P2s — template publication and the schema-parity comparator) and **13**
(five CI failures — the parity election race, the musl static-link spelling, the Windows `.exe`
artifact path, the release attestation permissions, and the `github_worker` lock flake).

The coordinator's own run record says **sixteen worker runs hit the 30-minute timeout** and that
**two trees survived only because the coordinator committed them**. Neither figure is a
repository fact and this file cannot confirm them: the history carries seven `wip(...)` commits
(`bd90d6c`, `830af2d`, `82088e0`, `68711d3`, `f4d7365`, `ec11b64`, `a76bc8c`) and nothing marks
which of them the coordinator wrote to save a timed-out worker's tree. Treat both numbers as
run-record facts.

## 6. Risks workers listed

Collected from the `STATUS.md` rows and the perf record, in substance:

- **The duplicate-issue check is unproved.** Two instances on one database were measured to
  produce one leader and no duplicate Runs, but issue filing reaches no stub, so a duplicate
  issue is unobservable without a real GitHub App.
- **`release.yml` has never run.** The tag path, the checksums, the provenance attestation and
  the two publish workflows are unexercised, and `workflow_dispatch` cannot be tried before
  this workflow exists on the default branch.
- **The Windows leg is CI-only.** `x86_64-pc-windows-msvc` cannot be built on the macOS host
  this port was written on; its smoke test and its artifact have only ever run on
  `windows-latest`.
- **Some MCP states diverge on purpose.** A `POST /mcp/` body that is JSON but not JSON-RPC
  answers 415 with rmcp's plain text here and 400 `-32602` with Pydantic's 11-error union report
  in the reference. No golden pins either shape, and no client of this port sends such a body,
  but it is a recorded difference rather than a fixed one.
- **Some reference text cannot be reproduced.** The `Invalid multipart data.` detail, expat's
  JUnit parse messages, SQLAlchemy's and Pydantic's exception renderings in MCP failure results,
  and the Report `error` text all differ; the statuses, envelopes and field names match.
- **Tests that observe leadership after the fact are timing-sensitive.** The
  `github_worker` lock test flaked on a loaded runner; it now stages the lock instead. The class
  of risk (reading `pg_locks` after the fact) is worth watching in any new test.
- **The coverage floor follows the lower configuration.** CI measures with
  `FLAKERADAR_TEST_DATABASE_URL` set; with it unset the same tree reports 97.15% lines and
  94.91% branches because the test-support Docker path is covered too, so a local run can look
  better than CI's.
- **The Docker path of the test-support crate is unmeasured in CI** and was exercised by hand
  three times.
- **The perf numbers are a debug build on one machine over loopback.** They are conservative in
  the favourable direction for speed, they do not cover a release build, and nothing was
  measured across a network.
- **The reconciliation between `differences.md` and `allowed-differences.json` is now manual.**
  `record.py --verify` used to fail on an allowance that names no row, or a golden or field that
  does not exist; that check went with `backend/`, and re-measuring which allowances are
  load-bearing is now a hand-run replay. An allowance that covers nothing is no longer detected.
- **The `openapi` contract compares promises, not documents.** It asserts the same paths,
  methods, parameters, response field names and 2xx `$ref` schema, and reports anything else by
  path or schema — not that the two documents are equal, which they cannot be.
- **78 coverage gaps remain**, three of whose groups (process edges, test-only `flush` bodies,
  `llvm-cov` artifacts) are not real work; the rest are missing seams or arms that should be
  deleted.
- **The follow-on issues must be filed by the user after merge**, because they must not carry
  the `agent` label while the branch is off `main`.
- **The cutover is a fresh install.** The binary refuses an Alembic database, the database URL
  changes `?ssl=` to `?sslmode=`, and every id and score restarts; `docs/cutover-v2.md` is the
  only safe path, and it asks for the old deployment to be stopped and its open issues closed
  first.
- **The browser evidence lives outside the repo** (`/tmp/fr-browser/evidence.txt` and
  `dashboard.png`), and `frontend/dist` had to be rebuilt with `npm run build` before that run
  because it predated a later `frontend/src` commit.

## 7. Final coverage

Measured with `cargo +nightly-2026-10-25 llvm-cov nextest --branch --all-features --profile ci`
against a PostgreSQL service with `FLAKERADAR_TEST_DATABASE_URL` set — the configuration CI
measures in — and re-measured after the Python deletion with the same result, because deleting
Python changes no Rust line:

- **96.73% of lines** (13853/14321) and **94.21% of branches** (1074/1140); regions 92.53%
  (15278/16511), functions 96.03% (1644/1712). Three consecutive runs produced byte-identical
  summaries.
- The floor is `coverage-floor.json`'s `{"lines": 96, "branches": 94}`, both **rounded down** from
  those totals, so a run that reaches the measurement passes and a gate can never fail on a
  figure the same code did reach. Both are above the `{95, 80}` minimum the task set, and
  nothing was lowered. The floor sits below the measurement because a gate set at `{96.73,
  94.21}` could fail on a figure the same code did not reach on that run: some arms are
  state-dependent, and the gap list names them — `TestDb::new`'s template-build branch is
  reported as missed on a server that already holds the template, which is why that row is
  logged as state-dependent rather than unreachable.
- The floor follows the **lower** of the two honest configurations: with
  `FLAKERADAR_TEST_DATABASE_URL` unset the suite starts its own containers, that file's Docker
  path is covered, and the same tree reports 97.15% of lines and 94.91% of branches. CI takes
  the URL-set path, so that is the figure the gate is set from.
- The 468 missed lines and 66 missed branches are the **78 rows** in `coverage-gaps.md` and no
  others: `coverage(off)` process edges, arms PostgreSQL cannot produce (an aggregate with no
  `GROUP BY` answering no row, a foreign key making a row impossible, a schema change
  mid-query), `tracing` field-argument lines that `llvm-cov` counts zero although the event
  fires, test-only `flush` bodies, and the Docker path above.
- Each of the 78 rows is drafted as a follow-on issue in `coverage-follow-ups.md` — file,
  function, why it is not covered, what would cover it, and labels — for the user to file after
  this PR merges. Nothing is filed yet.

## 8. Rows added to `differences.md`

Fourteen rows were added during the port, on top of the twelve pre-approved ones (26 in total,
against 5 allowance entries holding 196 field allowances). In file order:

1. **`<testcase time>` that is not a number (02).** Python raises inside junitparser and
   answers 500 storing nothing; Rust validates and answers **400** with the usual
   `{"detail": …}` body. *Approved by the user on 2026-11-04.*
2. **The `line` attribute (04).** Unicode digits pass Python's `isdigit()` and some then crash
   `int()`; Rust takes ASCII digits only, so `line="١٢"` is no line.
3. **The suffix of a JUnit parse-error detail (02).** ElementTree/expat's message versus
   roxmltree's; the prefix and the 422 stay identical.
4. **The `detail` for an unreadable `multipart/form-data` body (06).** Starlette's `Invalid
   multipart data.` versus this port's `Empty report body`; both 400.
5. **A `ci_run_attempt` past the column's range (06).** asyncpg raises and Python answers 500;
   Rust answers **400** (`ci_run_attempt must be at most 2147483647`) and queues nothing.
   *Approved by the user on 2026-11-04.*
6. **The MCP result envelope (11).** fastmcp writes explicit `null`s, advertises `logging`, and
   reports its own version; rmcp omits absent fields, drops the deprecated capability, reports
   `2.0.0`, and adds `resultType`/`ttlMs`/`cacheScope` to `tools/list`.
7. **The text of two MCP failure results (11).** SQLAlchemy's exception class, rendered SQL,
   bind parameters and `sqlalche.me` link, and Pydantic's `[type=…, input_value=…]` taxonomy,
   versus this port's own messages; the status, envelope, header and field line match.
8. **A `POST /mcp/` body that is JSON but not JSON-RPC (11).** Python 400 `-32602` with
   Pydantic's union report; Rust 415 with rmcp's plain text.
9. **The headers and `ETag` of a frontend file (12).** A quoted SHA-256 instead of an
   mtime-size ETag, no `last-modified`, no `Range`, and `immutable`/`no-cache` cache policies.
10. **A static path that climbs out of the frontend's root (12).** `..`, a backslash or a NUL
    (encoded or not) answers the API's 404 instead of being normalised away.
11. **A method other than `GET`/`HEAD` on a path the frontend does not hold (12).** 405 only for
    a path the frontend serves, 404 otherwise — the reference's answer depended on whether
    `dist` happened to exist.
12. **The process's command line (13).** One binary, `flakeradar [serve|healthcheck]` plus
    `--version` and `--help`, with no flag for a setting; uvicorn's flags configured uvicorn.
13. **The exit status of a refused start-up (13).** 1, not uvicorn's `STARTUP_FAILURE` 3; 2 for
    a command line clap cannot parse; 0 for a clean SIGTERM or Ctrl-C.
14. **The `/openapi.json` document (14).** utoipa's document rather than FastAPI's; the replay
    compares the 23 paths, the methods, the parameters, the response field names and the 2xx
    `$ref` schema, not the prose or the operation ids.

## 9. How to review this PR

Read in this order; each one tells you what the others assume.

- **The parity corpus first: `tests/parity/README.md`, then `tests/parity/scenarios/`, then
  `tests/parity/golden/`.** 300 recorded steps across 13 scenarios plus 66 JUnit parse goldens
  from a 33-file corpus at two `line_base` values, plus `schema.sql`. This is the strongest
  evidence in the PR: the answers were recorded from the Python server at `97b933d`, and the
  replay compares them field by field. `allowed-differences.json` is the complete list of
  mismatches a replay may accept — 5 entries, 196 field allowances — and 183 of them are read
  by `crates/flakeradar/tests/parity.rs` on `every_scenario_replays_identically`.
- **`docs/plans/rust-rewrite/STATUS.md`.** The row per task: what it landed, what it measured,
  what its audit found, and what it could not verify. Section 3 of this file is the extracted
  version of the last part; do not read a "Done" row as "verified".
- **`docs/plans/rust-rewrite/differences.md`.** Every intentional difference with its reason,
  and a section reconciling the two lists that says which allowances are load-bearing, which
  rows no replay can reach, and what the deleted `record.py --verify` used to check.
- **`docs/plans/rust-rewrite/coverage-gaps.md`**, then `coverage-follow-ups.md`. 78 rows, each
  with a reason and a drafted issue; the second file is what the user files after merge.
- **`docs/cutover-v2.md`.** The runbook this branch is cut over with: tag `v1.0.0` at
  `97b933d`, stop the old deployment, close the open `flakeradar` issues, create an empty
  database, change `?ssl=` to `?sslmode=`, deploy with `FLAKERADAR_GITHUB_TOKEN` unset, verify,
  expect `ci_fix` instead of reruns and changed ids, set the token again once scores rebuild,
  add `Dockerfile.rust-node-pg` and the `PROFILES` entry, then tag `v2.0.0`.

The two decisions that need a user's word are already decided, and both replaced a 500 with a
400 where Python raised rather than answered:

- **An unparsable `<testcase time>` answers 400.** Decided 2026-11-04; row 1 of section 8. A
  missing, empty, negative or fractional `time` keeps Python's behaviour.
- **A `ci_run_attempt` outside the `int4` range answers 400.** Decided 2026-11-04; row 5 of
  section 8. The detail is `ci_run_attempt must be at most 2147483647`, nothing is queued and
  no Repo or Project row is created.

Both are places where Python 500s, so a reviewer who finds them surprising should read the two
rows and the `STATUS.md` row that records the decision, not the tests, which are downstream of
it.

To reproduce the gate: `cargo nextest run --all-features --profile ci` (952 tests),
`cargo test --doc --all-features` (23 doctests), `cargo fmt --all -- --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, then the coverage command in
section 7 against a PostgreSQL service. The test suite needs a database: set
`FLAKERADAR_TEST_DATABASE_URL` or let it start its own container. The CI evidence for the head
`130691c` is run `42071653737` (15 jobs, all green) and Docker run `42071653733`.
