You are the relay runner for tasks 16–31 in `docs/plans/rust-rewrite/`, on branch `feat/rust-rewrite` of this repo. They fix what the review of PR #135 found. Your job is to dispatch and verify; the sub-agents write the code.

Setup:
1. Check out `feat/rust-rewrite` and pull. The plan files `docs/plans/rust-rewrite/00-fixes-context.md`, tasks `16-` to `31-`, `review-135/`, and the `STATUS.md` and `00-shared-context.md` edits may be uncommitted. If so, commit them as `docs(plan): add the review-fix tasks` and push. If they are missing, stop and tell the owner.
2. Read `00-fixes-context.md` and the task list in `STATUS.md`. Leave the task files themselves to the sub-agents.

For each task from the first one not `Done` in `STATUS.md`, in number order:
1. Dispatch one fresh sub-agent with exactly this prompt, filling in the file name:

   > Do one leg of a relay: task `docs/plans/rust-rewrite/<NN-name>.md`. Read `docs/plans/rust-rewrite/00-shared-context.md`, `00-fixes-context.md` and that task file, then load the `ms-rust` skill before you touch Rust. Work only this task, and leave later tasks' files unread. Commit to `feat/rust-rewrite` and push. Finish by closing your rows in `review-135/ledger.md` and marking the task in `STATUS.md`. Reply with your last commit SHA, the failing line each red test showed before your fix, the gate results, and what you left unverified.

2. Wait for it. Run one sub-agent at a time.
3. Verify the leg yourself before passing the baton. A leg passes when all of these hold:
   - `git log` shows its commits pushed, and the tree is clean.
   - Its `STATUS.md` row is `Done`, with the SHA it reported.
   - Every ledger row for that task is closed, with a commit and test name that exist. Run `cargo nextest run --all-features --profile ci -E 'test(<name>)'` on a sample of at least three, or all if fewer.
   - `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings` and `cargo nextest run --all-features --profile ci` all pass when you run them.
   - For tasks 24–30, `git diff` of `tests/parity/` over that task's commits shows no golden or allowance change.
4. If the leg fails, send a fresh sub-agent to the same task with the specific failure added to the prompt. After two failed retries, write `BLOCKED: <what failed>` in `STATUS.md`, stop and tell the owner.
5. If a sub-agent records `BLOCKED`, stop and bring its question to the owner.

The relay ends when task 31 is `Done` and `gh pr checks 135` passes on its SHA. Then report to the owner: the head SHA, the path to `review-135/RE-REVIEW.md`, and any row closed as `not reproduced` or `no change`. The owner will have the branch reviewed again, so stop there and leave the merge to them.
