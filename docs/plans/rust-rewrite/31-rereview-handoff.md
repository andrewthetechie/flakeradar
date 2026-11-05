# 31 — Hand off for re-review

Read `00-fixes-context.md` first. This task changes no behaviour. If it finds a bug, stop with `BLOCKED` in `STATUS.md` and name the task that owns it.

## Goal

The owner can review the final branch from one page that makes only true claims.

## Deliverables

1. **Close the ledger.** Every row in `review-135/ledger.md` is closed. For each `fixed` row, rerun its named test by name.
2. **Rerun the repros.** Run `repro/proofs/` and the task 23 battery against the final head. Each proof that asserted a defect must now fail or be updated to assert the fix.
3. **Run the full gate** from `00-shared-context.md`, plus `node --test scripts/*.test.mjs`, then measure coverage with CI's command and raise `coverage-floor.json`, rounded down. Run coverage three times and compare.
4. **Check CI on the pushed head.** `gh pr checks 135` shows every check passing, with the five targets green.
5. **Correct the record.** Recount and update what these name: `differences.md`, `coverage-gaps.md`, `coverage-follow-ups.md`, `tests/parity/README.md`, `HANDOFF.md`, `docs/cutover-v2.md`, `README.md`, `CONTRIBUTING.md` and `docs/agents/code-standards-rust.md`.
6. **Update the PR body** with `gh pr edit 135 --body-file <file>`: re-derived counts, a section on what tasks 16–31 changed, and an honest "Not verified here".
7. **Write `review-135/RE-REVIEW.md`:** the head SHA, one line per task with its commit range, every approved difference added, every `not reproduced` and `no change` row with its evidence, what was not verified, and where a reviewer should start. Quote numbers from commands you ran, and name the command.

## Done when

- [ ] Every item above is done at one pushed head, with CI passing on it.
- [ ] `STATUS.md` marks task 31 `Done` with that SHA.
- [ ] You stop there. The owner runs the next review and decides on the merge.
