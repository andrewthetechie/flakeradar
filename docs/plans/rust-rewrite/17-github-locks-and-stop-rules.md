# 17 — GitHub row locks and stop rules

Read `00-fixes-context.md` first. Findings: `S2`, `J1`, `S4`, `A1`, `S6`, `B3`, and the sync-test half of `T2`.

## Goal

A slow or dead GitHub delays only the GitHub task, and every GitHub operation follows one stated policy.

## Contracts

**No lock across HTTP.** No database transaction is open during any GitHub call, anywhere in `github.rs`.

- Sync: read the rotation list, then for each row call GitHub, then write that row in its own short statement.
- That write applies only if the row still holds the number that was checked, so a link changed during the GET survives. Add this test.
- List every `begin()` in `github.rs` in your status note, with why no GitHub call sits inside it.

Stamping stays as in Python's `_clear_if_closed` docstring: every answered row is stamped, and a rate-limit stop or a transport error leaves the row unstamped.

**Stop rules, matching the reference.** A transport error stops the sync (`S4`), and a transport error on a fixed-note close stops the note batch (`S6`). A rate limit stops all of them.

**One policy (`B3`).**

- `Answer::classify()` returns one verdict type.
- One table states the per-operation differences, such as 404 meaning "re-arm" in a sync.
- `ControlFlow` replaces the three `bool` returns with mixed polarity.
- Batch operations return a `BatchReport`, logged once at the edge. "Never raises" stays true.

Compare each table row with the reference, including 401 on the note paths. Where `0a86d3a`'s 401 fix goes beyond the reference, apply it to every operation and add a `differences.md` row.

## Repro

`review-135/repro/differential/t6.py` (locks), `t10.py` (transport stop), and `review-135/repro/fake-github/`.

## Done when

- [ ] With a mock holding a sync GET on a channel, a second connection updates that row under a short `lock_timeout`. `POST /api/tests/{id}/quarantine` answers 200, and a Report touching that Test reaches `processed` with `retry_count` 0.
- [ ] With two rows and a counting server that drops connections, one sync makes exactly one request, and neither row is stamped.
- [ ] A transport error on a note close ends the note batch, with the request count asserted.
- [ ] Each verdict and operation pair in the table has a test asserting returned values.
- [ ] The test at `crates/flakeradar/tests/github.rs:3024` is split, one concept per test, and its comment matches what it asserts.
- [ ] The false "still stops" claim is corrected in `STATUS.md` with this task's commit.
- [ ] The shared rules in `00-fixes-context.md` are met.
