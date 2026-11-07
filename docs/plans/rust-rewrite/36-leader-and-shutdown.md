# 36 — The leader loop and shutdown

Read `00-fixes-2-context.md` first. Findings: `ST24`, `ST25`, `ST30`, `ST31`, `ST34`. Read their sections under "Second pass" in `review-135b/standards-findings.md`. No wire answer changes.

## Goal

Exactly one processor runs, a stalled loop ends the process rather than holding the lock, and a stop finishes in a stated time.

## Contracts

**`ST24`: check the lock while there is a backlog.** `worker.rs:661-672` runs `self.lock.check()` only when the queue is empty. If the lock's connection dies while the main pool stays healthy, a standby takes the lock, and the old leader keeps claiming until its queue drains. `FOR UPDATE SKIP LOCKED` lets both work side by side. Check once per loop, at the top of the leader branch before `maybe_prune`, and delete the check in the `Ok(None)` arm. The reference has the same gap (`app/worker.py:380-383`), so add a `differences.md` row citing the proposed table.

**`ST25`: a dead loop ends the process.** A panic in `ReportWorker::run` leaves the lock held and `is_leader` true, with nothing processing, and no health answer shows it. Have `Server::serve` select on the worker loop ending as well as on the shutdown signal. A loop that ended before a stop releases the lock and makes `serve` return an error, so the process exits and the orchestrator restarts it. Log the `JoinError` at `worker.rs:634`. Do not set `panic = "abort"`, because a parser panic in `spawn_blocking` is a 500 today. Leader tasks stay as they are: a leader task's panic surfaces at `LeaderRun::stop`, and `tests/worker.rs:872` pins that. State this in `differences.md`'s row and in `STATUS.md`, so the third review can judge it.

**`ST30`.** `main.rs:103-104` discards a failure to listen for Ctrl-C, and the block's finish then shuts the server down. Copy the SIGTERM arm at `:112-118`: log it and await `pending`.

**`ST31`: reproduce it first.** It is PLAUSIBLE, not run. Hold a client mid-upload, stop the server, and see whether `serve` returns. If it does not, bound the drain with `tokio::time::timeout` and a documented constant below the orchestrator's grace period. Look for that period in the repo's deployment files. If the repo sets none, use Kubernetes's default of 30 s as the grace period. If `serve` returns, close the row `not reproduced` with the test as evidence, and keep the test.

**`ST34`: back off on repeated errors.** A persistent `Err` from `process_next` drops leadership, waits one poll, re-elects and claims the same Report forever, restarting the leader tasks each time. Trace whether a stored value can reach this path, and put the answer in `STATUS.md`. Count consecutive `Err`s and wait with the existing `backoff()`. Keep the Report where it is: do not mark it failed. Correct the claim "one bad Report can never block the queue" on `process_next` if your trace shows it is false.

## Done when

- [ ] `ST24`: `a_leader_with_a_backlog_checks_its_lock_between_reports` uses a processor that always answers `Some` and a `LockSession` whose `check` fails, and it asserts that leadership drops and claims stop.
- [ ] `ST25`: a processor that panics on its first claim makes `serve` return an error, and `wait_until` sees the lock free.
- [ ] `ST31`: a test with a stalled upload, under `tokio::time::pause` if you add the bound.
- [ ] `ST34`: a processor stub that always answers `Err`, with `RecordingWaiter`, sees the wait grow up to `max_backoff`.
- [ ] `tests/worker.rs` still passes, and any test whose meaning changed is renamed.
- [ ] The full suite under ten CPU spinners passes with no retries, because this task changes the election loop.
- [ ] The shared rules are met.
