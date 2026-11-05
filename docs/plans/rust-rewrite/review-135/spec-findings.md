# PR #135 spec review: partial findings

Head f9330fd, base 86c98804. **The review was cut short by the coordinator (token quota).** See "Not finished" at the end. Nothing below was edited in the repo, and nothing was posted to GitHub.

CI at capture (checks.json): 18 pass, 1 skipped (`Merge the manifest list`), **no `bucket: fail` check**. The CI suite does not reach findings 1 to 3.

## How this was tested

I ran the real Python server (base 86c98804, one uvicorn worker) and the Rust debug binary side by side, each on its own Postgres 17 database, and sent both the same requests. The scratch scripts are in `S/dt`, `S/jd` and `S/run`.

For GitHub runs, a scratch copy of the Rust source (`S/rs-patched`) read the API base from an env var, and Python's `API_BASE` was patched at start-up. Both pointed at a local fake GitHub. The only source change was that API base line. No release build was run, except the parser harness in `S/jd`.

"Proved by running" means I saw it in these runs. "By reading" means I did not run it. Python paths below are relative to base 86c98804.

---

## (c) Requirements whose implementation looks wrong

### 1. CRITICAL (proved by running): GitHub filing and sync stop for good after any leadership drop

Spec, task 08: "**Any other error:** log it, release leadership, sleep `poll`, retry." `LeaderTask` has "`start`, `stop`", and the leader tasks start with leadership. Task 14: "Restart Postgres during ingest. The server recovers with no restart."

- Rust: `crates/flakeradar/src/github_worker.rs:119,155,182,189` holds one `CancellationToken` for the worker's whole life. `stop()` cancels it and `start()` clones the same cancelled token, so `run` (`:99`) exits at once. `crates/flakeradar/src/webhooks.rs:207,249,285,292` has the same pattern.
- Python: `backend/app/worker.py` `GitHubWorker.start` and `WebhookWorker.start` create a new task on every start.

Run (`S/dt/t9.py`): I filed 30 links, ran `docker restart` on Postgres, and both servers reported `processor_leader: true` again. I then uploaded a new proven flake.

| | Python | Rust |
|---|---|---|
| POST /issues total | 31 (filed `new1`) | 30 (nothing filed) |
| Issue number on `new1` | 131 | empty |
| GET sync requests | 91, still running | 30, frozen |

Failure scenario: one Postgres restart or network blip means no issue is filed, no link is re-armed and no fixed note is posted until the process restarts. `/api/health/ready` still looks healthy. By reading, the Webhook task dies the same way, and the unbounded GitHub queue keeps growing. Task 14's reliability check did not catch this.

### 2. CRITICAL (proved by running): the sync holds row locks across GitHub HTTP calls and makes Reports fail

Spec, task 10: "Port these with the same behaviour". The Rust module's own header says "a slow or hanging GitHub API cannot delay Report processing" (`github_worker.rs:3-4`), which also matches the docstring at the top of Python's `worker.py`.

- Rust: `crates/flakeradar/src/github.rs:1273` opens one transaction, and `:1341` (`stamp_checked`) sends an `UPDATE` for each row straight away, so its row lock stays held through every later GET until `:1311`.
- Python: `github_integration.py:770-776` only sets ORM attributes and flushes them in the final `db.commit()`, so no row lock is held during HTTP.

Run (`S/dt/t6.py`): 150 links, fake GitHub at 1 s per GET, default 30 s `statement_timeout`.

| During the sync | Python | Rust |
|---|---|---|
| `POST /api/tests/1/quarantine` | 200 in 0.0 s | **500 after 30.0 s** |
| Report touching those Tests | `processed`, retry_count 0 | **`failed`, retry_count 3**, `canceling statement due to statement timeout` |

`pg_stat_activity` showed the sync session `idle in transaction` and the Report's `INSERT INTO test_cases` waiting on a Lock for about 30 s.

Failure scenario: any sync longer than 30 s (about 120 links at 250 ms each, assuming that latency; the batch cap is 2000 per table) stalls the whole queue, because processing is serial. A sync that outlasts the retries (about 2 minutes in this run) turns live Reports into permanent `failed` rows. Quarantine and link routes also return 500.

### 3. HIGH (proved by running): one small upload kills the Rust process

Spec, task 04: "Parsing must never panic on any input." Task 01: "untrusted CI input (uploads, API bodies) is never allowed to panic."

- Rust: `crates/flakeradar-core/src/parsing.rs:274-287` (`collect_cases_named`) recurses per nesting level with no depth limit.
- Result: a 115 KB report with 5,000 nested `<testsuite>` elements gave `fatal runtime error: stack overflow, aborting`, and the server was gone (debug binary). The release-built parser harness also overflowed at depth 5,000 under a 2 MiB stack (`ulimit -s 2048`, main thread). I did not run the release server binary.
- Python: a 19.7 MB report of the same shape answered 500 and the server stayed up. The parser harness shows `RecursionError` for depth 2,000.

An abort is not a Rust panic, so `parser_robustness.rs`'s byte mutations cannot catch it. The upload needs the API token. It crashes at the ingest parse, before the Report is stored, so it does not poison the queue. The in-flight requests are lost.

### 4. HIGH (proved by running): a sync with GitHub unreachable no longer stops

Spec, task 10: "Port these with the same behaviour". Python `github_integration.py:816-820` stops the run on `network` (and at 97b933d, `except httpx.HTTPError` wrapped the whole loop).

- Rust: `github.rs:1330-1336` returns `true` (carry on) on a transport error.
- Run (`S/dt/t10.py`): one sync with connections dropped made **1 GET in Python and 30 in Rust**.
- Failure scenario: at the 15 s timeout, up to 4000 rows could block this task for about 16 hours, assuming every call times out. No filing happens meanwhile, because the task is sequential.
- Commit 0a86d3a says "a transport failure still stops a filing/sync/note batch". **That is false for the sync.** The test comment at `crates/flakeradar/tests/github.rs:3024` says "the batch ends", but the test never asserts it.

### 5. HIGH (proved by running): SIGTERM and leadership release wait for the whole sync

Spec, task 08: "On SIGTERM... cancel the worker with `CancellationToken` and wait for it".

- Rust: `github_worker.rs:99-101` does not check the token during `sync()`, and `stop()` waits for it (`:190-193`).
- Run: SIGTERM during a slow sync took **52.9 s in Rust and 0.2 s in Python** (30 links at 2 s each). Python cancels the task.
- Failure scenario: a large sync outlasts the container's stop grace period and ends in SIGKILL. `release()` blocks the same way, so a leader that drops on a DB error stays stuck there until the sync ends (by reading).

### 6. MEDIUM (reading plus a Python test name): a transport error on the fixed-note close does not stop the batch

Python `github_integration.py:1124-1126` returns, and base has `test_fixed_note_network_error_on_the_close_stops_the_batch`. Rust `github.rs:1771-1774` returns `false`, which means carry on. This is a #130/#131-era behaviour test. Commit 0a86d3a does not treat it as a port item and claims the note path is preserved. Not run.

### 7. MEDIUM (proved by running): issue body differs from Python

Spec, task 10: "Issue bodies and comments must match byte for byte".

- Python `github_integration.py:396-401` keeps looking for a sample through all failing rows. Rust `github.rs:587-600` reads only the newest failing row.
- Input: a `<failure message="boom older">` report, then `<failure/>` on sha s2, then a pass on s2.
- Python's body has `### Sample failure` with `older details`. **Rust leaves that section out.**

### 8. MEDIUM (by reading, not run): `github_issue_min_failures` counts the wrong window for Tests

- Python `_window_failures` (`github_integration.py:451-462`) calls `test_execution_window_ids(chunk, window)` with no `include_default_branch`, so it counts the overall window only.
- Rust `github.rs:854-858` uses `test_window_ids_sql()`, which is the UNION with the Default-branch window.
- Failure scenario: a Test with many recent feature-branch runs and older Default-branch failures passes the gate in Rust but not in Python, and gets an issue filed.

### 9. MEDIUM (proved by running): XML comments and processing instructions truncate details

Spec, task 04: "identical results to `parsing.py`".

- Rust: roxmltree's `.text()` at `parsing.rs:399,452` stops at the first non-text node. ElementTree merges the text around a comment.
- `<failure message="m">a<!-- c -->b</failure>`: details are `ab` in Python, `a` in Rust.
- `<failure message="m"><!-- c -->body</failure>`: `body` in Python, empty in Rust.
- A comment first in `system-out` drops the stdout section. A comment first in `stackTrace` loses both message and details.
- By reading, a body-only failure message then falls back to empty, which can change the category.

### 10. MEDIUM (proved by running): fingerprints and statuses differ on some inputs

The ADR says "Test fingerprints stay byte-identical". The cases are rare, but the rule is absolute. None of them is in differences.md.

| Input | Python | Rust | Cause |
|---|---|---|---|
| `encoding="ISO-8859-1"`, byte 0x80 in name | U+0080 | `€` (different fingerprint) | WHATWG label maps to windows-1252 (`parsing.rs:179`) |
| `encoding="US-ASCII"`, UTF-8 `é` in name | `té` | `tÃ©` (different fingerprint) | same |
| `<testcase j:name="x" name="u">` | name `u` | name `x` (different fingerprint) | roxmltree `attribute()` ignores the namespace |
| `<j:failure>` under a testcase | passed | **failed** | `tag_name().name()` ignores the namespace |
| `<j:testcase>`, `<j:flakyFailure>` | not seen | counted | same |
| `<testsuite xmlns="urn:x">` | 422 | accepted | same |
| `<!DOCTYPE ...>` (benign, SYSTEM or internal entity) | accepted | **422** `XML with DTD detected` | `ParsingOptions::default()` at `parsing.rs:219` |
| Declared UTF-16 with no BOM | accepted | 422 | |
| `&#xD800;`, `&#x110000;` | 422 | accepted as U+FFFD | |

The DOCTYPE case is a status-code change. The spec says to stop and ask before changing a status code. Neither the DOCTYPE case nor the namespace cases are logged.

### 11. MEDIUM (proved by running): Pydantic's lax coercion is gone, and it is not in differences.md

Spec, compatibility rule: statuses and types stay the same unless logged. README: "Any CI can send the same JSON".

| `POST /api/ingest/pipeline` input | Python | Rust |
|---|---|---|
| `ci_run_attempt: "2"`, `2.0` or `true` | 202 | 422 |
| `summary: "true"`, `"yes"` or `1` | 202 | 422 |
| `started_at` naive, `2026-01-01`, a space separator, no seconds, or an epoch | 202 | 422 |
| `{}` | 6 `missing` issues | 1 issue (serde stops at the first) |
| Several job errors | all listed | first only |
| Long runner label `loc` | `[..., "runner_labels", 0]` | no index |

Many type-mismatch issues also report `value_error` where Python reports `string_type`, `list_type` and similar. The spec says `type` must stay.

Same class on query, path and body:
- `?limit=1&limit=2`: Python last wins, Rust 422.
- `limit= 5`, `10.0`, `1_0`: Python 200, Rust 422.
- `quarantine {"expires_in_days":"5"}`: Python accepts, Rust 422.
- `{`: `loc` is `["body",1]` in Python, `["body",0]` in Rust.

writers-app (`run.run_attempt` is a number) and `samples/gitlab` still work. Third-party shell reporters that send strings would lose uploads.

### 12. LOW (proved by running): status codes changed without a differences.md row

- HEAD on GET routes: Python 405, Rust 200.
- OPTIONS without CORS headers: Python 405, Rust 200.
- Preflight from a disallowed origin: Python 400, Rust 200. No browser-visible harm, because there is no allow-origin header. `max-age` is gone too.
- `PUT /mcp/` without a bearer token: Python 405, Rust 401. differences.md says it "answers the same 405 in both". I did not test it with a bearer token.
- `GET /api/`: 404 becomes 307. The spec allows this.
- `/api/tests/1.0/history`: Python 404, Rust 422.
- Ids past int4: Python 500, Rust 404 or 422 (an improvement, but unlogged).
- Multipart part with a garbage header line before Content-Disposition: **Python 400, Rust 202** (only status difference seen in the multipart cases I reviewed).

### 13. LOW (one case run, one by reading)

- Pipeline `ci_run_attempt: 3000000000`: Python 500, Rust 202. By reading, `stored_attempt` saturates it to 2147483647 (`report_intake.rs`, used by `routes/reports.rs:695`). differences.md rejects exactly that for ingest ("must not be silently rewritten") and has no pipeline row.
- By reading: `line="3000000000"` with no `file` fails the whole Report in Rust (`processing.rs:582-589`), while Python only writes `line` when `file` is set. Not run through the server.
- By reading: in `commit_filed_link` (`github.rs:1241-1250`), any DB error is treated as "row vanished", so the new issue is closed with the "admin delete" comment. Python lets a non-Stale error propagate.

---

## (a) Missing or partial requirements

- **Task 11 Done-when, "A real client works: point a local Claude Code MCP config at..."**: not done. The PR body admits it ("A real MCP client... was never pointed at `/mcp/`"). I did not verify whether STATUS.md marks task 11 done anyway.
- **Task 14 "Restart Postgres during ingest. The server recovers with no restart"**: finding 1 shows the recovery is partial.
- **Task 04 "never panic on any input"**: finding 3.
- **Task 10 "byte for byte" and error policy**: findings 4, 6 and 7.

## (b) Scope creep

Nothing major confirmed. Small item: the liveness check uses `pg_backend_pid()` instead of the spec's `SELECT 1` (`worker.rs:53`). It is a justified fix, because sqlx replaces a dead connection. Not reviewed further.

## Parity harness audit (part C), what I got to

Good:
- Goldens have two commits only: a305157 (recording) and 78f1087, whose stat lists only new `empty-time`, `negative-time` and `fractional-time` goldens. No edits by the Rust side.
- `dump_db` is `SELECT *`, so all columns compare, and all 11 data tables get dumped at some step.
- Floats compare exactly, which is stricter than the README's wording.

Weaknesses that let this PR pass:
- **Every scenario has `settings: null`.** No GitHub token, no webhooks, no non-default gate.
- **No scenario restarts the DB, reaches a failed Report, or runs a sync.** So findings 1, 2, 4, 5 and 7 are invisible to the "367 goldens".
- Replay uses `app.oneshot`, not a real HTTP listener.
- `<timestamp>` accepts any valid timestamp, so ordering and equality between clock values are not checked.
- `compare_openapi` checks names only, not types or required lists.
- `allowed()` with no `pythonValue`/`rustValue` accepts any mismatch on that field (`parity.rs:878-901`). I did not check whether the 179 MCP allowances set those values.
- After the recorder's deletion, nothing reconciles allowances with differences.md. differences.md says so itself.

## Faithful: compared and matched

- **Schema (mechanical, proved):** `pg_dump --schema-only` of Alembic head 0018 and the Rust baseline differ only in `alembic_version` and `seaql_migrations`. `tests/parity/schema.sql` matches Alembic head.
- **Processing and scoring (proved on a rich dataset, `S/dt/t3.py`):** 81 Reports with retries, same-SHA flips, branch changes, summary Jobs and duplicates. Row-by-row dumps of all 11 tables matched, ids included, once timestamps were normalized.
- **Scoring (read side by side):** formula, decay, `round4`, `proof_floor`, streaks. Classify rules and order. Marker logic, including the migration 0009 rule, on 9 edge cases.
- **Reads (partly):** 483 of 486 requests matched. The 3 differences were `/api/ci-runs//failures` validation order and the metrics clock value. **Caveat:** that battery used `limit` and `offset` for lists, which are not real params, so `page`, `page_size`, `file` and several filter combinations were not exercised. ORDER BY sites line up between `queries.py` and `reads.rs` by grep.
- **Report intake:** duplicate SQL, transitions and queue status text.
- **GitHub (proved):** 11 filings, fixed-note, close and re-arm requests and final DB state matched, apart from finding 7. Headers matched except User-Agent, which the spec requires.
- **Attribution, score history, retention SQL, admin delete, notify rendering, webhook retry logic:** read, matched. Retention and webhooks not run.
- **Settings:** all 28 Python fields exist with the same defaults. Token text matches.
- **PR-body counts:** 26 differences rows, 5 entries with 196 allowances, floors 96/94, 78 gap rows and 78 draft headings all check out. All 6 `coverage(off)` attributes are in `main.rs` and `telemetry.rs`, and coverage-gaps.md names sites in both. `backend/` is gone, `.fabro` is switched, and stale references left in non-plan docs look intentional. Dangling `backend/...` mentions remain in Rust comments and tests, plus a comment at `frontend/src/api.ts:1`.

## Not finished

- **A:** I read all 15 task files but did not check each acceptance criterion. Tasks 01, 02, 03, 12 and 13 (workflows, Dockerfile, release, smoke, manifest) are essentially unchecked. HANDOFF.md, coverage-gaps details and most of STATUS.md were not read.
- **B:** not run or not line-compared:
  - MCP tools (Python read only; `mcp.rs` not read; no differential, including string-typed args)
  - read-query params noted above
  - `queries.py` versus `reads.rs` line by line
  - failed-Report paths
  - concurrency and deadlock behaviour
  - the writers-app curl over a real listener (the plain and multipart ingest battery did run)
  - ingest cases after `lb0` are in `S/dt/t8.out` and were **not reviewed**, including size limits. That run's ids had drifted, so many of its "diffs" are noise.
- **C:** the 179 MCP allowances and `compare_openapi` details were not audited. The goldens were not re-recorded from Python.
- **D:** partial, as listed above.
- **E:** done for GitHub by reading (the only code on main after 97b933d per `git log --stat`). Findings 4 and 6 are the result: 0a86d3a's "behaviour-preserving" claim is wrong on both. Frontend and actions had no commits in that range.
- **F:** not done systematically.
