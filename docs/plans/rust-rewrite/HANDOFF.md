# Handoff: the Rust rewrite on `feat/rust-rewrite`

This file is for the reviewer of PR #135. It collects what landed, what each task's commits
are, everything the port could not verify, every checker finding and what happened to it, the
risks the workers listed, and where to start reading. `STATUS.md` is the row-by-row record;
this file is the summary you need before you read it.

Facts that can be re-derived from the repository were re-derived rather than copied from
`STATUS.md`, and where a `STATUS.md` claim could not be confirmed it says so. Verified here:
`git log` and `git rev-list --count` for every range, `cargo nextest list --all-features` for the
test count (1180, re-counted by the suite's own summary at this head), `cargo test --doc
--all-features` for the doctests (24), file counts under `tests/parity` for the goldens (382 JSON
files plus `schema.sql`), `git diff --shortstat` for the size, `gh run view` for the CI claims,
pydantic 2.14.2 and junitparser 5.0.3 for the two disproven findings, and `dups.py 8`,
`battery/run.sh all`, four coverage runs and the two Playwright projects for the measurements
tasks 32–44 rest on.
Not re-derivable, and quoted from the document named beside it: the coverage percentages
(`coverage-gaps.md`), the audit histories and older CI run ids (`STATUS.md`'s rows), and the
performance and reliability numbers (`docs/perf/22`).

The head this file describes is `bba16f2`, the last commit that changes code or a test; the
documentation commits that carry this file, the review page and the CI ids follow it. There have been two
reviews: `review-135/RE-REVIEW.md` is the page for the first review's fixes (tasks 16–31), and
`review-135b/RE-REVIEW.md` is the page to start from now, for the second review's fixes (tasks
32–44) and for the surface neither review reached. Section 2 covers tasks 01–15, section 2b tasks
16–31 and section 2c tasks 32–44.

| | |
| --- | --- |
| Branch | `feat/rust-rewrite`, head `bba16f2` (plus the documentation commits after it) |
| Python reference | `97b933d`, the last Python commit; `86c98804` for the review fixes, which is `97b933d` plus `main`'s #130 and #131 |
| Branch point | `33703f7` (the plan documents); task 01's work starts at `1179fe3` |
| Commits in `97b933d..HEAD` | 412, of which 2 (`bfd3fc4`, `86c9880`) are `main`'s #130 and #131 brought in by the merge `182ab4a` |
| Diff against `97b933d` | 920 files, +181492 / -24794; `backend/` alone is 96 files and 24535 deletions, and 54,309 of the insertions are `crates/flakeradar-core/tests/fixtures/encodings.tsv`, the reference's codec table |
| Reviewed heads | `f9330fd`, which `review-135/` was written against, and `ac5e6f3`, which `review-135b/` was written against |
| Diff of the first review's fixes | `f9330fd..ac5e6f3` is 129 commits over 261 files, +35480 / -16132 |
| Diff of the second review's fixes | `ac5e6f3..bba16f2` is 93 commits over 226 files, +72170 / -7863, with the codec fixture as most of the insertions |
| Pull request | #135, *feat: replace the Python backend with one Rust binary*, open, unmerged, untagged |

## 1. What landed

The Python service is gone: `backend/`, `tools/parity-record/`, `ruff.toml` and the manifest
checker are deleted, along with the CI jobs that drove them. One Rust binary now serves
everything the Python server did — the API, the dashboard embedded out of `rust-embed`, the MCP
server over stateless Streamable HTTP, and the report worker (plus the GitHub and webhook leader
tasks inside the same process) — and the workspace's gate is 1180 tests, 24 doctests, `cargo fmt`,
`clippy -D warnings`, `cargo doc` with warnings denied, and `cargo audit`. All 383 parity files
replay: 316 recorded scenario steps across 15 scenarios, 66 JUnit parse goldens, and `schema.sql`,
which `crates/flakeradar-db/tests/schema_parity.rs` compares against a freshly migrated database,
with `allowed-differences.json` as the only place a mismatch can be accepted. Every parity
scenario is enabled — `crates/flakeradar/tests/parity.rs` has no `ENABLED` list and no
`DEFERRED_STEPS` count, so a scenario cannot be switched off to make the replay pass.

## 2. Per-task commit ranges

The ranges below are contiguous and cover the whole branch: each one starts at the commit after
the previous task's last, and the 15 counts sum to 188, which with the plan commit `33703f7` is
the 189 in `97b933d..130691c`. They include each task's own audit follow-up and `docs(status)`
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

## 2b. The review fixes: per-task commit ranges for 16–31

The review of `f9330fd` found what CI could not see, and `docs/plans/rust-rewrite/00-fixes-context.md`
turns each finding into one task. The ranges are contiguous the same way: the 16 counts sum to
120, which with the review-fix plan commit `0e0fc57` and `f9330fd` itself are the 122 commits
after `130691c`. The "landed" column repeats `STATUS.md`'s Commit cell, and `review-135/RE-REVIEW.md`
is the page that summarizes what each one closed.

| Task | Title | Range (first..last) | n | Delivered | Landed |
| --- | --- | --- | --- | --- | --- |
| 16 | Leader task lifecycle | `4ec494e..79f5f5b` | 2 | `worker::LeaderRun` gives every run its own token, so a stopped task starts again; a call in flight ends at once except the filing `POST`; the GitHub queue is bounded at `QUEUE_MAX` | `4ec494e` |
| 17 | GitHub row locks and stop rules | `eb78420..4530dd5` | 2 | The sync holds no row lock across a GitHub call and writes each answered row guarded on the number it checked; a transport error ends the sync and the fixed-note batch; `Answer::classify` and `github::action` are the one policy table | `eb78420` |
| 18 | GitHub filing correctness | `e090919..451ce46` | 6 | `FiledLink::{Stored, RowGone, StoreFailed}` so only a gone row closes an issue; the reference's `Sample failure` block; the overall failure window; a client build that fails is an error at construction | `1588fd7` |
| 19 | Parser safety and parity | `b35b256..31fcbca` | 4 | An explicit suite walk with `SUITE_DEPTH_MAX` 990, a node budget and an identity-byte budget; non-finite `time` is a 400; comments and processing instructions no longer truncate text; encodings, namespaces, `DOCTYPE` and character references read as Python reads them | `a9ed51a` |
| 20 | Request validation parity | `0a3a6a3..2890c89` | 4 | The three JSON bodies read with Pydantic's lax rules and every issue collected; `page` bounded at `PAGE_MAX`; a NUL refused in every request text; the multipart garbage-header 400 restored | `0a3a6a3` |
| 21 | Secrets and settings | `b6645f0..201474c` | 4 | A malformed `.env` line names its number and never its text; the database URL is a `Secret` everywhere it is printed; a value outside its field's meaning refuses to start; every shared default is stated once | `b6645f0` |
| 22 | Parity evidence | `2699914..b05dbc3` | 11 | Two scenarios recorded from the reference (`github`, with a token, a Webhook and two filing gates; `failed-report`), a real listener for every step, the dump clock relations, field types and `required` lists in the OpenAPI comparison, allowances that name both values, and a real MCP client | `69a6e86` |
| 23 | Differential sweep | `2d150bf..64d42e9` | 6 | Both servers run side by side: 529 comparisons in the default profile and 31 in the retention profile, three differences fixed and six logged with their measurement | `d3f2f39` |
| 24 | Shared plumbing | `9fed875..38b1b31` | 11 | One `flakeradar_db::sql` for the row helpers, one `RouteError` with `Display` and `Error`, one validated JSON body extractor, the queries in the crate that decodes them, and `mcp/` with one file per tool | `d514d12` |
| 25 | Domain types | `ef69d84..2d99836` | 18 | `Fingerprint`, `Score` and the CI run id as newtypes; `ReportStatus`, `ReportKind`, `ExecutionStatus`, `FailureCategory` and `EntityKind` replace the string fields; the scoring row is built by field name; `Scope` cannot be built without its rule | `fc59d9b` |
| 26 | Split the GitHub module | `282e6c4..6d9c22f` | 5 | `github.rs` is `github/` with one file per responsibility, its tests follow, and both issue bodies are rendered by one `Body` builder | `929c08b` |
| 27 | Test and Job duplication | `92ec59f..21a766e` | 9 | The seven Test/Job pairs are one rule each over an `EntityKind` descriptor, and the entity-selected tests are one scenario per entity | `53cbd17` |
| 28 | Processing and efficiency | `003a8e8..b561943` | 9 | `process_report` is the two-kind dispatch, each large value is read once, the CPU work is off the async threads, and one `UPDATE` clears every due quarantine | `43b2f10` |
| 29 | Clock and test hygiene | `8b7de67..40ee134` | 12 | `AppState` carries the clock and `Utc::now` is gone from the routes and `github/`; both leader tasks drain one queue that counts finished batches; the test helpers are `flakeradar-test-support`'s; `close_issues_after_removal` answers a `RemovalReport` instead of a log line | `b6472fc` |
| 30 | Docs, lints and compliance markers | `2275fee..2fa6e6d` | 14 | Both lint opt-outs and the one `#[allow]` gone, one path per item across 102 files, every bound a named constant with a reason, every log event named and unformatted, `cargo doc` with warnings denied in CI, and two tests that read the source for the rules a compiler cannot see | `dd3924b` |
| 31 | Hand off for re-review | `512e466..` | 3+ | The ledger closed with every named test re-run, the proofs rewritten against this head, the differential battery re-run, three coverage runs, this file and `RE-REVIEW.md` corrected, and the PR body updated | this row |

## 2c. The second review's fixes: per-task commit ranges for 32–44

The review of `ac5e6f3` found four must-fix defects and a list of should-fix work, and
`docs/plans/rust-rewrite/00-fixes-2-context.md` turns each finding into one task. The ranges below
are contiguous the same way: the twelve counts sum to 93, which is the 93 commits in
`ac5e6f3..bba16f2`, and the documentation commits that carry this file and the CI ids follow them. The plan commit that added the task files, `1fbf1e5`, is
numbered inside task 32's range, as `0e0fc57` was inside task 16's. The "landed" column repeats
`STATUS.md`'s Commit cell. `review-135b/RE-REVIEW.md` is the page that summarizes what each one
closed.

| Task | Title | Range (first..last) | n | Delivered | Landed |
| --- | --- | --- | --- | --- | --- |
| 32 | Parser budget that holds for every shape | `1fbf1e5..8b626c7` | 4 | The pre-scan bounds entity expansion at 64 MiB and namespace declarations at 4,194,304 entries before `roxmltree` reads a byte, so a hostile body is a 422 instead of a dead server; ten shapes are measured under a counting allocator with a 1 GiB hard stop | `ff8ae02` |
| 33 | Parser parity: empty namespace, encodings, declarations | `986904e..0484a9e` | 4 | An explicit empty namespace is no namespace, so `<failure xmlns="">` is a failure; every encoding label decodes from the reference's own codecs (54,309-fixture rows over 421 labels and 66 code pages); and `DOCTYPE` and declaration handling is pinned case by case | `bc8f000` |
| 34 | Timestamps, settings and the database URL | `4cd7e06..b99959d` | 11 | A number past `2e10` is a millisecond count and the years a datetime cannot hold are refused, numeric strings are read as instants, the settings take the request body's spellings, a wait no `Duration` holds refuses by name, and a database URL's password never reaches the output | `080be67` |
| 35 | GitHub stop rules, off-thread JSON work, the SQL log | `23dcb25..400d0ee` | 7 | A store failure ends the GitHub outcome; a comment and its close each run through their answer; a JSON body's parse and a Pipeline body's serialisation leave the async thread; the query-name lookup is a map; and SQL statements log at `debug` | `8964291` |
| 36 | The leader loop and shutdown | `e949ca7..d5ff9c5` | 3 | The leader checks its lock session on every loop, a dead worker loop makes `serve` return an error and release the lock, a failed Ctrl-C listener no longer shuts the server down, the drain is bounded at 25 s, and a persistent `process_next` error backs off to the cap | `2d57ca9` |
| 37 | Playwright tests for the dashboard | `9c08e65..c872fde` | 6 | `frontend/e2e/` is two Playwright 1.65.0 projects — five mocked-UI tests and eight end-to-end tests against a mock install — and each is its own required CI job | `5d6eb3e` |
| 38 | Comments, compliance markers and the hygiene guard | `8480d6f..6a3b4dd` | 7 | 65 files lost a compliance marker a still-open finding contradicted, the comments describe the code as it stands, and both source-hygiene tests became rules that fail by file and line | `df679b2` |
| 39 | Log events and test shape | `8931cc6..645e4e4` | 10 | The last five log events carry their values as fields, `PruneResult::notifications_dropped` replaces a logged count, one `CapturedLog` replaces seven private sinks, the public-API tests leave `src/`, and Test and Job bodies split per entity | `5fc8585` |
| 40 | Finish the types and the plumbing | `6541866..ab611b0` | 16 | `Score`, `Fingerprint` and `CiRunId` travel through every read; every shipped number is a named constant; `policy::step` reads the action table's `Unreachable` column; one note and one filing entry point; `LinkedOut` is untagged; `TEST_UPDATES`/`JOB_UPDATES` are literal SQL; `ToolAnswer::shaped`; and a stored `NaN` score is refused — the one behaviour change, approved on 2026-11-07 | `a410e70` |
| 41 | Structure of the request and worker modules | `3aa7768..a44b0cf` | 12 | Nine structure rows: one match for the lock's three outcomes, one `BoxFuture` alias, `post` as attempt/match/wait, `lax::recorded`, `path_issue` deciding on the type, `Framing::next_part`, one split helper, `memmem::Finder` instead of the hand-written search, and `worker/{leader_task,lock}.rs` | `b0274bf` |
| 42 | Test helpers that remove the most lines | `96fc09b..e9fe17d` | 7 | A seeded row is one `create` call (733 of them against 45 `build` calls kept), a sync is `sync`/`sync_with`, and four local helpers carry the validation ceremony; one MCP client test became four | `c3c64c8` |
| 43 | The record and the missing controls | `3cee932..4b42e61` | 3 | The four self-approved rows cite the approved decisions, the OpenAPI comparison gained its four negative controls, `tests/parity/README.md` names the commit that recorded each scenario, and `review-135/ledger.md` names the rows the second review reopened | `0a3224f` |
| 44 | Hand off for the third review | `840c180..bba16f2` | 3 | The ledger closed by re-running every named test, the battery, the coverage runs and `dups.py` re-measured, the record corrected (`differences.md`, `coverage-gaps.md`, `tests/parity/README.md`, this file, `README.md`, `CONTRIBUTING.md`), the PR body rewritten and `review-135b/RE-REVIEW.md` written; and the one flaky test CI found — task 36's drain-bound test — fixed and re-measured (`bba16f2`) | `bba16f2` |

`e949ca7`, the first commit of task 36's range, is a `.fabro/test.toml` change made by the relay
runner between tasks, not a task deliverable: fabro-io rejects a timeout above 600 seconds, so the
900-second values in `targets.rust` and `checks.clippy` failed the file's own check. It is inside
the range because every commit on the branch belongs to one.

## 3. Every UNVERIFIED item

Collected from the `UNVERIFIED` notes in `STATUS.md`'s rows and from the workers' reports in
those rows. Nothing is softened. Items 1-4 and 6-10 are still open; 5 and 11-16 were closed
after they were written, and the paragraph after them names the CI run that closed three of
them.

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
5. **Task 09's Done-when browser check now runs in CI.** The condition asked for the frontend
   against the Rust server under Playwright. Tasks 09, 12, 22 and 25 each recorded it as not
   run; task 37 wrote the suites. `frontend/e2e/e2e.spec.ts` loads `frontend/dist` out of the
   real `flakeradar` binary against a seeded database (that is the check task 12 had run by
   hand, now repeated on every push), and `frontend/e2e/ui.spec.ts` covers the states a real
   server rarely produces. The two CI jobs are `Frontend UI (Playwright, mocked)` and
   `Dashboard e2e (Playwright, mock install)`.
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

### The review fixes' unverified items

Every `STATUS.md` row for tasks 16–31 carries its own `UNVERIFIED` note. These are the ones a
reviewer should carry forward, and `review-135/RE-REVIEW.md` lists them again with the rest of
what task 31 could not check:

- **The coverage figures come from a local `postgres:17-alpine`, not CI's service.** Every leg
  that measured coverage says so. CI measures on Linux against its own `postgres` service, so
  the percentages in section 7 are this host's.
- **Task 13's packaging legs were not re-run by the review fixes.** A Docker build, the 60 MB
  image size, the five Actions targets and a `workflow_dispatch` rehearsal are CI's (and the
  last cannot run before the workflow exists on the default branch).
- **The reconciliation runs in the suite.** `tests/common/allowances.rs`'s `reconcile`, through
  `every_allowance_names_its_values_and_cites_a_recorded_difference`, refuses an entry whose
  `differenceRow` is not text in `differences.md`, whose id repeats, that gives no reason, that
  names a golden or field that is not there, whose `pythonValue` is not what the golden holds,
  and an entry that names neither values nor a contract. What nothing checks: no test re-derives
  a golden from the reference (`record.py --verify` re-recorded and byte-compared them, and went
  with `backend/`), no test proves that a row changing a recorded answer has an allowance, and an
  allowance that covers nothing is still undetected — re-measuring which ones are load-bearing is
  a hand-run replay of `every_scenario_replays_identically` with the harness logging each match.
- **`E5`'s negative controls exist as of task 43.** `a_retyped_field_fails_the_openapi_contract`,
  `a_dropped_required_entry_fails_the_openapi_contract`,
  `a_required_entry_only_the_port_has_fails_the_openapi_contract` and
  `a_field_widened_to_nullable_fails_the_openapi_contract` each feed one mutated copy of the
  served document through `compare_openapi`, and each was measured against a comparator with its
  check removed before it was committed.
- **Task 29's `T3` row names the pre-rename test.** It explains the rename from
  `a_note_network_error_is_logged_and_never_raises` to
  `a_note_network_error_ends_the_batch_and_never_raises`; the current name exists and passes.
- **The perf figures are one host and a debug build** (`docs/perf/22`), and task 28's back-to-back
  table is the comparison that matters.
- **Task 26's "moved, not changed" is a comparison of code lines**, not `git diff -M`: a file
  split has no rename to show.
- **Task 27's Job window plan was measured on a fresh fixture**, not the recorded seed (which is
  not committed); the statement is byte-identical to the pre-task one.
- **Task 22's two new scenario directories were recorded and replayed on this host only.** CI's
  `postgres` service is the first other place they run.
- **Task 23's sweep did not stage a deadlock or a literal PostgreSQL restart**, and it did not
  re-record any golden; both are pinned by existing tests instead.
- **The differential battery needs the Python reference outside the tree.** Task 31 ran it from
  `/tmp/python-base` and `/tmp/pyref`; a reviewer offline can check out `86c98804` but cannot
  replay it from the repository alone.

Closed since task 31, so it is no longer on this list: **the frontend browser check** that
tasks 09, 12, 22 and 25 each recorded as unverified. Task 37 added `frontend/e2e/` — the two
Playwright projects the new `Frontend UI (Playwright, mocked)` and `Dashboard e2e (Playwright,
mock install)` CI jobs run, the second of which drives the dashboard out of the binary against
a seeded database (see section 3 item 5).

### The second review's fixes' unverified items

Task 44 re-ran the checks at the head this file describes and closed `review-135b/ledger.md`; that
file's closing section and `review-135b/RE-REVIEW.md` say what each row was proved with. What is
still not verified, and matters to a reviewer:

- **Every performance, coverage and load figure is this host's.** The four coverage runs in section
  7 used a local `postgres:17-alpine`, not CI's service, and two of the four differ from the other
  two by one line and one branch in `github/sync.rs` and `db/src/issue_links.rs`. The line was not
  identified. The floors are unaffected in every run.
- **The Windows leg, the Docker image and the `workflow_dispatch` rehearsal are still CI's.**
  Nothing changed here: section 3's items 1 and 2 stand, and `ac5e6f3..HEAD` touches no packaging
  file except `.fabro/test.toml`.
- **The differential battery needs the Python reference outside the tree.** Task 44 ran
  `battery/run.sh all` from `/tmp/python-base` and `/tmp/pyref` with `OUT` outside the repository;
  a reviewer offline can check out `86c98804` but cannot replay it from the repository alone.
- **Twelve rows closed on evidence that is not a test**, each for a reason the row itself gives:
  `ST30` (a failed Ctrl-C listener is process-level wiring a test cannot raise against itself),
  `ST1` (the marker set is text, held by a count and by `ST3`'s guard), `ST13` (`cargo metadata`
  reports the inherited `rust-version`), `CC16` and `ST29` (doc text, held by `cargo doc -D
  warnings` and by the code the doc describes), `SP5`, `SP7`, `SP13` and `SP15` (record rows in
  `differences.md`, `tests/parity/README.md` and `review-135/ledger.md`), `ST8` (one
  `CapturedLog` whose lines the log tests assert), `ST9` (tests that moved) and `CC6` (a helper
  name and three deleted fixtures). `SP4` names a file, whose 25 cases ran; `CC19` and `CC20` name
  their seams' tests in a second evidence cell, and those ran; `R1`'s evidence is the two
  Playwright projects, which ran.
- **The surface neither review reached is named, not reviewed.** `review-135b/RE-REVIEW.md` lists
  it under "What the third review starts on": the untested spec edges (MCP integer arguments
  beyond task 34's two, most settings fields, a real token in logs, a real serialization conflict,
  `uvicorn --workers 2`), the unread code (`extract.rs` from line 420, the unit tests in
  `multipart.rs` and `webhooks.rs`, `settings.rs`'s env parsing, most of `tests/worker.rs` and
  `tests/webhooks.rs`, `ms-rust`'s `11_universal_guidelines.md` other than its headings, `dto.rs`
  beyond the `CC17` arms, `settings.rs` outside lines 405-614, test clusters past the top 12), and
  the `martin-clean-code` skill, which is user-invocation-only and was never applied.
- **CI at the head is recorded in `STATUS.md` only.** Task 44 pushed the page first and wrote the
  run id afterwards, so the page cannot name a run that a later commit invalidates.

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
| 22 | 08 | A failing test (`run_prune_stops_at_time_budget`) was a test bug, not an implementation bug | It asserted a substring the log line does not contain. Since task 39 the line is a static message carrying `budget_seconds` as a field, and the test asserts the outcome instead — `result.executions == 2` against four surviving rows — so no wording is pinned. The claim that the line was Python's verbatim is withdrawn rather than corrected: Python prints the budget as a float, `60.0s`, and neither spelling exists here now |
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

The 51 rows above are the checker findings for tasks 01–15 and they are unchanged. The review of
`f9330fd` found a different set: 90 findings, each with an id and a row in
`review-135/ledger.md`, closed by tasks 16–30 and summarized one line per task in
`review-135/RE-REVIEW.md`. Their state, their commit and the test or evidence that closed each
one are in the ledger, which is the record for them; this file does not repeat it.

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

### The review-fix legs that needed more than one round

Several of tasks 16–31 also took more than one pass, and their ranges in section 2b show it:
**18** closed its rows over three commits, **19**, **20** and **21** each added a commit naming
the leg's last SHA, **22** recorded partial progress on `E5`/`E6` before its close, **24** needed
two status commits (the first five findings, then the rest), **25** took two legs, and **30**
corrected numbers its own row had claimed in two later `status:` commits. **31** is the leg that
found one more defect while measuring: the `github` scenario's test read the stored Issue number
the moment the GitHub stand-in recorded the `POST`, before the filing had stored the link, and it
failed once in three instrumented runs; the read is now a bounded `wait_until` (`e8563b1`).

### The second review's fix legs that needed more than one round

- **33** rewrote its own record once: `0484a9e`'s commit message says the namespace budget test
  pointed at the wrong owner, and the message was corrected in a commit of its own.
- **36**'s range opens with the relay runner's `.fabro/test.toml` timeout cap (`e949ca7`), which
  is a build fix rather than a task deliverable.
- **40** stopped mid-verification on its first pass; a second leg re-read every row's code itself,
  found the last untyped stored read (`a410e70`) and closed the nine rows.
- **41** took three record commits after its nine code commits, because two of its own measures
  made three `coverage-gaps.md` rows stale and the record had to name the tests it removed.
- **42** had to land the builders' `create` (`96fc09b`) before the four refactors that use it, so
  its six commits are one feature, two crates' sweeps, two helper refactors and its record.
- **43** was measured against the reviewer's own controls: the four OpenAPI negative controls
  (`3cee932`) came first, then the record (`0a3224f`).
- **44** found a flake on CI at a documentation-only head and fixed it:
  `a_stalled_upload_does_not_stop_serve_from_returning` failed the coverage job at `fa09e56` (run
  `42426433124`) because the stop could race the upload's arrival and 32 yields were not enough
  under instrumentation. The fix
  is `bba16f2`, a test-only change the owner authorized; the red and the repeat runs are in
  `STATUS.md`'s row 44 and in `ST31`'s ledger evidence.

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
- **The reconciliation between `differences.md` and `allowed-differences.json` is a test**, and
  narrower than `record.py --verify` was: it refuses an allowance that names no row, a golden or
  field that is not there, or a `pythonValue` the golden does not hold, but it cannot re-record a
  golden from the reference, cannot tell that a row has no allowance, and cannot tell that an
  allowance covers nothing — re-measuring which ones are load-bearing is a hand-run replay.
- **The `openapi` contract compares promises, not documents.** It asserts the same paths,
  methods, parameters, response field names and 2xx `$ref` schema, and reports anything else by
  path or schema — not that the two documents are equal, which they cannot be.
- **82 coverage gaps remain**, three of whose groups (process edges, test-only `flush` bodies,
  `llvm-cov` artifacts) are not real work; the rest are missing seams or arms that should be
  deleted. 73 have a drafted follow-on issue and nine do not (section 7).
- **The coverage totals are not byte-identical run to run.** One line in `github/sync.rs` and one
  `issue_links.rs` region were counted covered in two of four otherwise identical runs; the line
  was not identified. The floors clear by about a point, so the gate cannot flip, but a reviewer
  comparing two local reports will see 96.96% and 96.97%.
- **A leader task's panic surfaces only at `LeaderRun::stop`.** `serve` supervises the worker loop,
  not the two leader tasks, so a panic inside `GitHubWorker` or `WebhookWorker` stops that task and
  is reported when the run is stopped. Task 36 left it accepted; task 44 says in
  `review-135b/RE-REVIEW.md` that it still looks right, because the loop that panics loses its
  processing and the token it leaves is noticed at the next stop.
- **The review fixes' own risks**, in substance: a leadership change lets the filing `POST`
  finish, so a stop can take up to one GitHub timeout (`S5`'s one exception); the GitHub queue
  drops its oldest outcome past 1000 with a log line, so a long outage loses notifications
  (they were never guaranteed); a parser report past the node or identity budget is refused
  with the 422 a bad report gets, so a pathological upload a CI runner produced would be
  refused rather than parsed; the `github` and `failed-report` scenarios were recorded and
  replayed on one host. `E5`'s four OpenAPI controls, which task 43 added, are in section 3.
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

Measured at the head this file describes with CI's own command — `cargo +nightly-2026-10-25
llvm-cov nextest --branch --all-features --profile ci --no-report`, then `llvm-cov report
--branch` — against a PostgreSQL service with `FLAKERADAR_TEST_DATABASE_URL` set, which is the
configuration CI measures in. Three runs are the record, and a fourth confirmed the third; every
one executed 1180 tests with no failure and no retry:

| Run | Regions | Functions | Lines | Branches |
|---|---|---|---|---|
| 1 | 93.64% (19,222; 1,222 missed) | 97.41% (2,125; 55) | 96.96% (16,671; 506) | 93.80% (1,452; 90) |
| 2 | 93.64% (19,222; 1,222) | 97.41% (2,125; 55) | 96.96% (16,671; 506) | 93.80% (1,452; 90) |
| 3 | 93.65% (19,222; 1,220) | 97.41% (2,125; 55) | 96.97% (16,671; 505) | 93.87% (1,452; 89) |

- The floor is `coverage-floor.json`'s `{"lines": 96, "branches": 93}` — the owner's decision of
  2026-11-06, unchanged. Every run clears both by about one point of lines and nearly one of
  branches, and no path was dropped to reach them.
- Runs 1 and 2 differ from 3 and 4 by one line and one branch, both export attribution of a
  lazily evaluated `tracing` argument: `github/sync.rs` misses 49 of 226 lines in the first two and
  48 in the last two, and `db/src/issue_links.rs` loses one region in the first two only. The line
  was not identified; the one candidate that could make it timing-dependent is
  `a_database_restart_under_the_leader_files_a_real_issue_and_syncs_again`, whose sync statement
  races the database it kills. `coverage-gaps.md`'s task 44 section records this.
- The 505–506 missed lines and 89–90 missed branches are the **82 rows** in `coverage-gaps.md` and
  no others: `coverage(off)` process edges, arms PostgreSQL cannot produce, `tracing`
  field-argument lines `llvm-cov` counts zero although the event fires, test-only `flush` bodies,
  and the Docker path the CI configuration never takes. Five rows left the file in task 44 because
  they described conversions and arms task 40 deleted.
- 73 of the 82 rows have a drafted follow-on issue in `coverage-follow-ups.md` — file, function,
  why it is not covered, what would cover it, and labels — for the user to file after this PR
  merges. The nine without a draft are the five task 18 added (the client-build arms) and the four
  task 20 added (the lax and multipart guards): task 15 drafted only the list it left. The five
  rows task 44 deleted were not among the drafted ones. Nothing is filed yet.

## 8. Rows added to `differences.md`

Seventy-one rows in total: twelve pre-approved, fourteen added during the port, and forty-five
added by the two reviews' fixes (tasks 16–25, 32, 33, 36, 40 and 43), against 5 allowance entries
holding 196 field allowances. The recount at this head is in `differences.md` itself, with the
per-task breakdown. In file order, the fourteen from the port:

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

The twenty-six the review fixes added, by the task that logged each one:

- **16** — the GitHub queue's bound, and what a leadership change does to a GitHub call in
  flight (the filing `POST` runs on; every other call ends at once).
- **17** — a link re-read or replaced while the sync's `GET` is in flight survives, because each
  answered row is written guarded on the number that was checked.
- **19** — four: a `<testsuite>` nested past the reference's recursion limit answers 422; a
  non-finite `time` answers 400; a report past the node or identity budget answers 422; and a
  byte a declared single-byte code page cannot decode is decoded as the code page maps it.
- **20** — seven: a `page` whose offset does not fit is a 422; a NUL in any request text is a
  422; a naive or epoch pipeline timestamp is read as UTC; the multipart garbage-header 400 is
  restored, a part with no field name answers `Empty report body`; `HEAD`/`OPTIONS` answer 200
  where the reference answered 405, and a disallowed preflight 200 with no `max-age`; `GET
  /api/` answers 307 where the reference answered 404; a float path id is a 422 and one past
  `int4` a 404.
- **21** — three: a setting whose value its field could not mean refuses to start; a `.env` line
  that does not parse names its number and never its text; a database URL the driver cannot
  parse has its password replaced.
- **23** — six: a serialization failure is retried; a JSON body past 20 MiB answers 413; the
  fields of a framework 422 beyond `type`/`loc`/`msg` differ; a `json_invalid` offset and
  sentence differ; a racing delete's `deleted` counts are racy on both sides; a 204 carries no
  `Content-Type`.
- **25** — two: a `reports` row whose stored `status` or `kind` this version never writes is a
  plain-text 500, and a `failure_category` the five categories do not name reads as `null`.

The nineteen the second review's fixes added, with the commit that landed each:

- **32** — two: the parser's entity-expansion budget is a 422 rather than a dead server, and its
  namespace-declaration budget is the same; both refuse a body before `roxmltree` reads a byte.
- **33** — twelve: one for the stateful and EBCDIC codec labels this port keeps at 422, and one
  for each shape of the `SP4` table `roxmltree` cannot express (the three `ATTLIST` defaults and
  their normalization, the two parameter-entity shapes, and eight malformed forms it accepts).
- **36** — four: a dead worker loop ends `serve` with an error and releases the lock; a leader
  checks its lock on a backlog as well as an empty queue; a failed Ctrl-C listener no longer shuts
  the server down; and the graceful drain is bounded at 25 seconds.
- **40** — one: a stored `flakiness_score` that is not a score answers the plain-text 500 (and an
  MCP `isError` result) instead of serving `null`. *Approved by the user on 2026-11-07.*
- **43** — one: a multipart body the reference's form parser refused, or one past its own limits,
  answers the status this port's own reader gives (the row lists every measured shape).

No row in this file waits for a decision any more: `SP5` gave each of the four that did its
approved decision's citation, including the 20 MiB row, which keeps the **413** because the
plan's explicit-limits rule guards an unauthenticated route.

## 9. How to review this PR

Read in this order; each one tells you what the others assume.

- **`review-135b/RE-REVIEW.md` first, then `review-135/RE-REVIEW.md`.** The first is the page for
  the second review's fixes: the head, one line per task with its commit range, every difference
  the fixes added, what each ledger row now shows a reader or user, what was not verified, and
  the surface neither review reached, which is where the third review starts. The second is the
  same page for the first review's fixes (tasks 16–31).
- **The parity corpus: `tests/parity/README.md`, then `tests/parity/scenarios/`, then
  `tests/parity/golden/`.** 316 recorded steps across 15 scenarios plus 66 JUnit parse goldens
  from a 33-file corpus at two `line_base` values, plus `schema.sql`. This is the strongest
  evidence in the PR: the answers were recorded from the Python server at `97b933d` (`github`
  and `failed-report` from `86c98804`), and the replay compares them field by field.
  `allowed-differences.json` is the complete list of mismatches a replay may accept — 5 entries,
  196 field allowances — and 183 of them are read by `crates/flakeradar/tests/parity.rs` on
  `every_scenario_replays_identically`.
- **`docs/plans/rust-rewrite/STATUS.md`.** The row per task: what it landed, what it measured,
  what its audit found, and what it could not verify. Section 3 of this file is the extracted
  version of the last part; do not read a "Done" row as "verified".
- **`docs/plans/rust-rewrite/differences.md`.** Every intentional difference with its reason,
  and a section reconciling the two lists that says which allowances are load-bearing, which
  rows no replay can reach, and what the deleted `record.py --verify` used to check.
- **`docs/plans/rust-rewrite/coverage-gaps.md`**, then `coverage-follow-ups.md`. 82 rows, each
  with a reason; 73 of them have a drafted issue in the second file, which is what the user
  files after merge.
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

To reproduce the gate: `cargo nextest run --all-features --profile ci` (1180 tests),
`cargo test --doc --all-features` (24 doctests), `cargo fmt --all -- --check`,
`cargo clippy --all-targets --all-features -- -D warnings`,
`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --document-private-items
--workspace`, `cargo audit`, `node --test scripts/*.test.mjs` (49 tests), then the coverage
command in section 7 against a PostgreSQL service, and the two Playwright projects in
`CONTRIBUTING.md` (13 tests, `retries: 0`). The test suite needs a database: set
`FLAKERADAR_TEST_DATABASE_URL` or let it start its own container. The CI evidence for the
reviewed head `130691c` is run `42071653737` (15 jobs, all green) and Docker run `42071653733`;
`STATUS.md`'s rows for tasks 31, 37, 39 and 43 name the runs at their heads, and task 44's row
names the run at this one.
