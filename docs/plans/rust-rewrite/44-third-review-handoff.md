# 44 — Hand off for the third review

Read `00-fixes-2-context.md` first. This task changes no behaviour. If it finds a bug, stop with `BLOCKED` in `STATUS.md` and name the task that owns it.

## Deliverables

1. **Close `review-135b/ledger.md`.** For each `fixed` row, run its named test by name, and state what the reader or user now sees.
2. **Re-run the checks** at the final head and quote the numbers:
   - the gate, plus `node --test scripts/*.test.mjs`
   - `battery/run.sh all`, and the 3 stress runs (ten spinners; no retries used)
   - three coverage runs with CI's command
   - both Playwright projects
   - `dups.py 8` over production and tests
3. **Correct what these name:** `differences.md`, `coverage-gaps.md`, `tests/parity/README.md`, `HANDOFF.md` (add section `2c` for tasks 32–44), `README.md`, `CONTRIBUTING.md`.
4. **Update the PR body** with `gh pr edit 135 --body-file <file>`.
5. **Write `review-135b/RE-REVIEW.md`.** It is one page:
   - the head SHA, and one line per task with its range
   - every difference added, every row not `fixed` with its evidence, and what was not verified
   - **Head and CI:** push the page first, then record the green run in `STATUS.md` only, so the page is never stale. The last relay's page named a head two commits old and said 94 where the floor was 93.

## The surface the second review did not reach

The second review ran two passes and still left these. Say plainly in `RE-REVIEW.md` that they were not reviewed, so the third review starts there:

- **Spec:** MCP tool integer arguments beyond the two named in task 34. Settings fields other than the sampled ones. A `.env` file on the reference side. A GitHub token in logs on a real failing call. A real, not staged, serialization conflict, and the retry path for Pipeline Reports. Uvicorn with `--workers 2`. The `HANDOFF.md` and PR bodies.
- **Standards:** `extract.rs:420` onward, the unit tests in `multipart.rs` and `webhooks.rs`, `settings.rs`'s env parsing, most test bodies in `tests/worker.rs` and `tests/webhooks.rs`, and `ms-rust`'s `11_universal_guidelines.md`, which was read as headings only. What input, if any, can panic inside `process_next`.
- **Clean code:** `dto.rs` beyond the `CC17` arms, `settings.rs` outside lines 405-614, and test clusters past the top 12. The `martin-clean-code` skill is user-invocation-only and was not applied on either pass.
- **Accepted for now:** a leader task's panic surfaces only at `LeaderRun::stop` (task 36). Say whether that still seems right.

## Done when

- [ ] Every item above is done at one pushed head, with `gh pr checks 135` passing, including the two new Playwright jobs and the five targets.
- [ ] `STATUS.md` marks task 44 `Done` with that SHA and the CI run ids.
- [ ] You stop there. The owner runs the next review and decides on the merge.
