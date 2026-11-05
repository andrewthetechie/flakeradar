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

Both instances answered `/api/health/ready` with `processor_leader: true`, which is the
reference's meaning of the field: *a* process in this database holds the lock, not *this* one.
The lock itself was held once (one row in `pg_locks`, the standby retrying), and the SIGKILL
above shows the hand-over keeps exactly one leader. That is the same meaning the parity
`health` goldens record.

### PostgreSQL stopped during reads

PostgreSQL was stopped with `docker stop` while the server kept running, a read was issued, then
PostgreSQL was started again.

- Python answered `500 Internal Server Error` in **7 ms** (asyncpg fails the pooled connection at
  once).
- Rust answered `500 Internal Server Error` in **30.0 s** before this task's fix, because SQLx's
  default `acquire_timeout` is 30 s and it retries the connection inside that window. That is a
  hang in practice.
- The fix: `flakeradar-db/src/connect.rs` now sets `acquire_timeout` to **5 s**
  (`a_pool_waits_five_seconds_for_a_connection_and_no_longer`). Measured after the fix: the same
  read answered `500 Internal Server Error` in **5.0 s**.
- Neither implementation needed a restart: both answered `/api/health/ready` with
  `database: true` once PostgreSQL was back, and the Rust one processed the next Report.

The status, the body and the recovery match the reference. The latency of the error does not:
Python's 7 ms is a property of asyncpg's pool, and matching it would mean disabling SQLx's
retry window entirely, which would fail requests that are only waiting their turn. 5 s is
bounded and recorded rather than a hang.

## What is not measured here

- Release-build numbers. The Rust figures come from a debug build, the conservative direction.
- The 2M-row plans of `docs/perf/20` and `21`: those sections were re-measured on a 100,000-row
  seed, with the smaller data stated there alongside the machine.
- A networked deployment: everything above is one machine over loopback.
