# 16 — Leader task lifecycle

Read `00-fixes-context.md` first. Findings: `S1`, `H1`, `S5`, `J3`.

## Goal

GitHub filing, sync, notes and Notifications survive any leadership change, and stopping them is prompt.

## Contracts

**Restart.** `GitHubWorker` and `WebhookWorker` work again after any `stop`. Each `start` creates a fresh `CancellationToken`, held with the task handle. Put the shared start and stop logic in one place, because both workers copy it today.

**Stop reaches in-flight work.** Audit every `.await` in both run loops, and classify each one in your status note:

- A sync GET, a note or webhook call, and any backoff sleep race the token and end at once.
- A `POST` that creates an issue runs through storing its link, then the batch stops. Cancelling between those two steps strands an issue and files a duplicate later. Stop latency is therefore at most one request timeout.

`ReportWorker::release` inherits this, so a dropped leader no longer waits for a whole sync.

**Queue.** Read `backend/app/worker.py` at the reference, including what happens to queued outcomes on a leadership change, and match it. Bound the GitHub queue as `WebhookShared` does (`QUEUE_MAX`), and log what it drops.

## Repro

`review-135/repro/proofs/worker_proof.rs`, `review-135/repro/differential/t9.py`.

## Done when

- [ ] For each real worker, a start, stop, start, enqueue test delivers the work.
- [ ] A test using a real `ReportWorker` drops leadership through a failing processor, regains it, and then files an issue.
- [ ] `stop` returns while a mock holds a sync GET unanswered forever, for both a sync and a webhook delivery.
- [ ] A test holds a filing `POST`, calls `stop`, releases the `POST`, and sees the link stored.
- [ ] The queue bound has a test that overfills it.
- [ ] `t9.py` semantics hold: after a Postgres restart, a new Proven flake gets its issue filed.
- [ ] The shared rules in `00-fixes-context.md` are met.
