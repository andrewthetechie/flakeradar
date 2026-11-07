# 40 — Finish the types and the plumbing

Read `00-fixes-2-context.md` first. Findings: `ST5`, `ST6`, `CC2`, `CC3`, `CC4`, `CC8`, `CC9`, `CC10`, `CC11` (partly closed `H6`, `H12`, `B1`, `B3`, `B4`, `B6`, `B9`, `B10`, `C2`). Structure only: goldens and allowances stay untouched, and a replay failure is a bug in your change. One commit per bullet.

## Contracts

- **`ST5`: newtypes travel.** `Score`, `Fingerprint` and `CiRunId` reach the read DTOs and read structs named in `ST5`. A row read goes through the fallible constructor, and a bad stored value becomes a `DbError`. Wire shape is unchanged because `Serialize` is transparent. Change `tier_for` so its two floats cannot be swapped.
- **`CC10`: Job status and provider.**
  - Type `JobOut.last_status` and the like with `JobStatus`. First check whether stored values can fall outside it (`reads.rs:1468`'s else arm). If they can, say so and leave that field, because changing it would be a behaviour change.
  - Make `job_issue_url` the one spelling.
  - Add `struct CiRunAttempt` for the `ci_run_*` clump.
- **`ST6`: constants.** Name the defaults and bounds `ST6` lists, with the three-part doc: why this value, what a change costs, what depends on it. Use one `DEFAULT_PAGE_SIZE`. `github_issue_min_score`'s default needs a name of its own or a stated reason it equals the threshold.
- **`CC4`: routes.** Remove the four impossible `LinkedOut` arms by making the shape `Serialize` (untagged); `utoipa`'s `body =` stays. Use one `identity_error`. Use one `Issues::finish()` for the 10 `issues.is_empty()` sites.
- **`CC2`: one GitHub sequence.**
  - One step owns call, transport error, classify, `action` and log, and a transport error goes through `action(op, Unreachable)`, so that table row is live.
  - `already_gone` becomes a `Verdict` match.
  - Remove output parameters where the step makes that natural. `note_batch` and `file_batch` should end shorter, with fewer parameters.
- **`CC3`: Test and Job entry points.** One `note_fixed_for(kind, …)` and one filing entry point. Drop the `entity` parameter that `row.target` already holds. Keep two candidate statements behind one mapper. Task 35 touches these paths first, so read its result.
- **`CC8`: trim the descriptor.** Keep `rescore_kind`, `Window`, `Update::of`, `old_scores`. Replace `Column`/`UpdateTable` with literal SQL heads and tails so `UPDATE test_cases AS t SET flakiness` can be grepped. Simplify `crossed`, drop `scored_table`, and use one `pub fn rescore(db, kind, …)`.
- **`CC9`: list-ness once.** `ToolAnswer::shaped(LISTS, &value)` replaces both constructors, so the comment in the 11 tool files becomes true. Or delete the comment and state what is really so.
- **`CC11`: `runner_stats`.** Split into tally, rank and map, with one `#[expect]`.

## Out of scope

Splitting `reads.rs`, `processing.rs` or `tests/parity.rs`, and a batch-call fixture for `tests/github/`. These cost only navigation, so leave them for the follow-on issues.

## Done when

- [ ] `git diff` of `tests/parity/` is empty and the replay passes.
- [ ] `docs/perf/20` and `21` plans still hold for any statement whose text changed.
- [ ] `dups.py 8` over production `src` is below 267 lines. Record the number.
- [ ] Each reopened row says what a reader now sees. The ledger rule on greps applies.
- [ ] The shared rules are met.
