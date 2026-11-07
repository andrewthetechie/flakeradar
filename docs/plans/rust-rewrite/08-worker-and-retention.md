# 08 — Worker and retention

Read `00-shared-context.md` first. Tasks 01–07 have landed.

Python reference: `backend/app/worker.py` (`ReportWorker`), `retention.py`, `main.py` (lifespan). ADR 0002.

## Goal

The single elected processor runs, with failover, retention and clean shutdown. The GitHub and webhook tasks arrive in task 10. This task defines the hooks they plug into.

## Contracts

### `ReportWorker`

Every server process runs one. Only the holder of the advisory lock leads.

- **Leadership:** `pg_try_advisory_lock(726300002)` on one dedicated, autocommit connection, held for as long as it leads and never returned to a pool. `is_leader` is true while it holds that connection.
- **Standby:** retry every 5 seconds (injectable).
- **Loop**, in this order: `maybe_prune`, `process_next`. If the queue is empty, run `SELECT 1` on the lock connection, then sleep `worker_poll_seconds`.
- **After an outcome:** call the `on_processed` hook, then the webhook hook. Hook errors are logged and never stop the loop.
- **Backoff:** when an outcome's status is `pending`, sleep `min(poll * 2^retry_count, 30)` seconds before the next claim.
- **Any other error:** log it, release leadership, sleep `poll`, retry.
- **Prune:** at most once per `prune_interval_seconds`, leader only. The first prune runs on the first leading loop. A failed prune is logged and retried next interval.

### Hooks

Define a `LeaderTask` trait with `start`, `stop`, and a non-blocking `enqueue(outcome)`. Task 10 implements it for GitHub and webhooks.

### Release order

**Stop leader tasks first, then unlock.** A replacement leader must never see the lock free while this process still has an issue POST in flight, or both could file the same issue. A failed stop is logged, and the lock is released anyway. If the lock connection is broken, drop it, because the server has already released the lock.

### Shutdown and startup

`run_server(settings)`:

1. Check the token.
2. Connect and migrate.
3. Build the app and start the worker.
4. Serve with graceful shutdown.
5. On SIGTERM or Ctrl-C (Ctrl-C and Ctrl-Close on Windows), stop accepting requests, cancel the worker with `CancellationToken` and wait for it, then close the pool.

If a Report transaction is in flight at cancellation, rolling it back is fine, since the Report stays `pending`.

Keep signal wiring in one small function. It may be `coverage(off)`. Everything after the signal must be testable by triggering the token.

### Retention

Port `retention.py`. Each `prune` round is one committed transaction that deletes at most `prune_batch` rows per table, oldest id first, and rescores the Tests and Jobs whose Executions it deleted, in that transaction, passing no Tier changes on (log the count). `run_prune` repeats until a round deletes fewer than `prune_batch` from every table, or `prune_time_budget_seconds` runs out, which is a normal return. Include:

- processed Reports (`report_retention_days`)
- failed Reports only when `failed_report_retention_days > 0`
- Executions and Runs left empty (`execution_retention_days`)
- Job executions
- both history tables (`score_history_retention_days`)
- expired quarantines cleared (reason and expiry too)

Inject the monotonic clock, as Python's `_monotonic` does.

## Tests to port

`test_worker` (14), `test_retention` (19), the lifespan tests from `test_app_startup`, and the backoff tests from `test_report_retry_budget`.

Add tests for:

- failover: two workers on one database, stop the leader, the standby takes over
- the leader running `SELECT 1`, and leadership dropped when the lock connection breaks
- tasks stopping before unlock, proven by order of events
- graceful shutdown finishing an in-flight HTTP request

Use `tokio::time::pause` and injected intervals. No test may depend on real timing.

Enable every parity scenario that needs only processing and these routes, using the real worker. Remove the direct-call helper from task 07.

## Done when

- [ ] All verification in `00-shared-context.md` passes.
- [ ] `/api/health/ready` reports `processor_leader: true` with the worker running, and `false` after it stops.
- [ ] Manifest rows for this task are not `pending`, and gaps are logged.
- [ ] `STATUS.md` is updated, and the branch is pushed.
