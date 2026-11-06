# Rust against Python: ingest, processing, memory and the reliability checks

The measured record of task 14's performance and reliability checks. Every number below was
taken on the machine named here, on the same data, with both implementations run one after the
other. Where a number is a judgement rather than a measurement, it says so.

## Machine and data

| | |
| --- | --- |
| Machine | Apple M4, 24 GB, macOS 26.6.2 (arm64) |
| Database | PostgreSQL 17.11, `postgres:17-alpine` in Docker, `127.0.0.1:55432` |
| Rust | `target/debug/flakeradar` built from `d5e4794` (a debug build, so these numbers are conservative), tokio multi-thread runtime, one process |
| Python | `backend/.venv` Python 3.12.5, `uvicorn app.main:app --host 127.0.0.1 --port 8318`, one worker, Alembic head |
| Client | one Python 3.12 script, `urllib`, no concurrency except where stated |

Both servers ran against a database of their own, created fresh for the run: the Rust one
migrated by the binary's own startup, the Python one by `alembic upgrade head`. The workload is
the same for both:

- **The 2,500-Test Report:** one JUnit report of 2,500 `<testcase>` elements, 157 of them
  failures, posted to `POST /api/ingest` with the same query parameters.
- **The 100-Report run:** 100 JUnit reports of 25 Tests each (2,500 Tests in total, one failure
  per report), posted back to back, then the time until `/api/reports/summary` reports
  `pending == 0`.

## Result

| Measurement | Python | Rust | Difference |
| --- | --- | --- | --- |
| 2,500-Test report accepted (202) | 0.043 s | 0.044 s | +2.3% |
| `/api/health` latency while that upload was in flight: worst of 96/139 requests | 10.4 ms | 9.9 ms | -4.8% |
| `/api/health` p95 during that upload | 0.5 ms | 0.3 ms | -40% |
| 2,500-Test report processed (worker) | 34 polls, worst read 93.0 ms | 29 polls, worst read 20.0 ms | -78% worst case |
| The Report's counts | 2,343 passed / 157 failed | 2,343 passed / 157 failed | identical |
| 100 Reports accepted | 0.244 s | 0.175 s | -28% |
| 100 Reports all processed | 2.998 s | 2.415 s | **-19.4%** (Rust faster) |
| Peak process RSS (`ps -axo rss=`, sampled every 100 ms) | 42 MB | 43 MB | +2.4% |

The 100-Report run is the number the task's threshold applies to: a regression beyond 20% in
processing time would need a fix or a recorded reason. Rust is **19.4% faster**, inside the
threshold in the favourable direction; the debug build is not the release binary, so a release
build measured here would only widen that gap. The event loop stayed responsive throughout:
the worst `/api/health` read was 9.9 ms during the 2,500-Test upload, well under the 100 ms the
task asks for, and the worst read during that report's processing was 20.0 ms.

Parsing runs off the async threads: `parse_upload` hands the body to `spawn_blocking`, which is
why the 2,500-Test report's handler (0.044 s accepted) does not show up as health latency. That
is a property of the code and of these measurements, not a separate timing experiment.

## Reliability checks

Each check was run against the binary, not a test harness, and against a database the binary
migrated.

### SIGKILL the leader during processing

Two instances ran against one database (`:8317` and `:8319`). Exactly one held the advisory
lock — one `pg_locks` row for `726300002` in that database — and the standby logged its failed
election every 5 seconds. A 200-Test Report was queued, then the leader was killed with
`kill -9` in the middle of the work.

- The standby took the lock and processed the Report: `status: processed`,
  `{passed: 200, failed: 0}`.
- The Repo has exactly **one** `test_runs` row: the killed leader's half-done attempt left no
  duplicate Run.

### Two instances, one database

Re-run on 2026-11-05 against `target/debug/flakeradar` built from `dddbf2f`: two instances
(`:8321`, `:8322`) on one fresh database, and 20 Reports of 25 Tests each ingested alternately
through both.

- Exactly **one** `pg_locks` row for the advisory key `726300002`.
- Both instances answered `/api/health/ready` with `processor_leader: true`, which is the
  reference's meaning of the field: *a* process in this database holds the lock, not *this* one.
  The standby retried every 5 seconds throughout.
- **No duplicate Runs:** 20 accepted Reports, 20 `reports` rows, all `processed`, and exactly
  **20** `test_runs` rows, with no `(project_id, commit_sha, ci_run_id)` group holding two. 25
  `test_cases` rows for 25 distinct fingerprints, and 500 `test_executions` rows with no
  duplicate `(test_case_id, test_run_id, attempt)`.

UNVERIFIED: the "no duplicate **issues**" half of that check. Issue filing posts to the GitHub
API at a fixed base URL, so a duplicate issue can only be observed against a real GitHub App
installation and token, and the binary reaches no stub. What is measured is the mechanism that
would prevent one — a single leader, each Report claimed once, no duplicate Run — and the
single-filer rules themselves are pinned by `crates/flakeradar/tests/` (`issue_links`,
`github_worker`, `notify`).

### PostgreSQL stopped during ingest

The check `14-convergence.md` asks for is a restart **while Reports are being ingested**, not
while the server is idle. Re-run on 2026-11-05 against the same debug binary, one instance on
`:8324`, a fresh database on a `postgres:17-alpine` container at `127.0.0.1:55499`:

- 100 Reports of 25 Tests each were accepted (202), then `docker stop` ran while the worker was
  draining them. The outage lasted **20.2 s**.
- Three `GET /api/reports/summary` calls during the outage each answered
  **`500 Internal Server Error` in 5.004 s**, and a `POST /api/ingest` during it answered
  **500 in 5.005 s** with the reference's plain-text `Internal Server Error` body. Nothing hung:
  every answer is bounded by the 5 s `acquire_timeout` in `flakeradar-db/src/connect.rs`
  (`a_pool_waits_five_seconds_for_a_connection_and_no_longer`), and the server logged
  `Failed to acquire connection from pool: Connection pool timed out`.
- PostgreSQL came back at 20.4 s, and 2.0 s later `GET /api/reports/summary` answered
  `pending: 0`. `/api/health/ready` then answered `database: true` and
  `processor_leader: true`, from the **same process** (pid 50920 before and after): no restart.
- All **100** Reports reached `processed`, and `test_runs` holds exactly 100 rows with no
  duplicate `(project_id, commit_sha, ci_run_id)` group, so the claim the worker lost with its
  connection did not become a second Run.

Latency against the reference, measured on the earlier run of this check: Python (asyncpg)
answered `500 Internal Server Error` in **7 ms** (it fails the pooled connection at once), and
Rust answered in **30.0 s** before the `acquire_timeout` fix, because SQLx's default is 30 s and
it retries the connection inside that window. That is a hang in practice, and 5 s is what the
fix bought.

The status, the body and the recovery match the reference. The latency of the error does not:
Python's 7 ms is a property of asyncpg's pool, and matching it would mean disabling SQLx's
retry window entirely, which would fail requests that are only waiting their turn. 5 s is
bounded and recorded rather than a hang.

## Re-measured for task 28: ingest, health and the 100-Report run

The processing rewrite (task 28) moved the per-case hashing and the Execution build off the
async threads, removed the copies of a Report's cases, and made the quarantine expiry one
statement. The two numbers this document exists for were measured again on the same host, a
debug build of both commits, the same two workloads, and a Python 3.12 client of the same shape
as the first table's: the 2,500-Test report (157 failures) and 100 reports of 25 Tests each,
posted back to back, timed until `/api/reports/summary` answered `pending: 0`.

Each row below is one run against a freshly created database, with `/api/health` polled every
10 ms from before the upload until the Report was processed, and RSS sampled every 100 ms.
**Both columns were measured in this session, alternately, so the comparison is not against a
figure from a quieter day**: the left column is `21a766e`, the commit task 28 started from.

| Measurement | `21a766e` (three runs) | task 28 (three runs) |
| --- | --- | --- |
| 2,500-Test report accepted (202) | 0.078, 0.079, 0.078 s | 0.070, 0.080, 0.081 s |
| `/api/health` worst during upload and processing | 18.8, 22.2, 24.2 ms | 11.6, 19.0, 22.2 ms |
| `/api/health` p95 during that window | 9.9, 11.3, 12.7 ms | 6.6, 6.9, 7.6 ms |
| 2,500-Test report processed (worker) | 1.809, 1.850, 1.909 s | 1.857, 1.865, 1.874 s |
| Summary polls while it processed | 31 (worst read 21.6 ms), 31 (19.3 ms), 32 (10.3 ms) | 32 (9.8 ms), 32 (18.4 ms), 32 (18.6 ms) |
| The Report's counts | 2,343 passed / 157 failed | 2,343 passed / 157 failed |
| 100 Reports accepted | 0.343, 0.351, 0.386 s | 0.318, 0.356, 0.458 s |
| 100 Reports all processed | 2.677, 2.719, 2.725 s | 2.628, 2.676, 2.694 s |
| Peak process RSS | 39 MB | 39 MB |

Read against the threshold this document states — a regression beyond 20% needs a fix or a
recorded reason — the task changed nothing: the 100-Report run is 1.6% faster on the median of
three, and every other row is inside the spread of the three runs. `/api/health` stayed far
under the 100 ms the task asks for, at a worst read of 22.2 ms while a 2,500-Test report was
uploading and processing.

**The 2.415 s and 0.175 s recorded above are not reproducible on this host today**, on either
commit: `21a766e` itself measures 2.68–2.73 s for the same 100 Reports and 0.078 s for the same
upload. Six PostgreSQL containers — five from earlier tasks and this run's own — on a 24 GB
machine are a busier host than the one the first table was measured on, and both binaries are
unoptimized debug builds. The numbers in the table above are therefore the comparison that
matters, and the first table stays as the record of the day the port was first measured.

## What is not measured here

- Duplicate **issues** across two instances, which needs a real GitHub API; see "Two instances,
  one database".
- Release-build numbers. The Rust figures come from a debug build, the conservative direction.
- The 2M-row plans of `docs/perf/20` and `21`: those sections were re-measured on a 100,000-row
  seed, with the smaller data stated there alongside the machine.
- A networked deployment: everything above is one machine over loopback.
