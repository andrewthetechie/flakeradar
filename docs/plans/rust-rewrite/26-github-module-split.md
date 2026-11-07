# 26 — Split the GitHub module

Read `00-fixes-context.md` first. Behaviour stays fixed. Findings: `B2`, `C1`, and `J14` for `github.rs`.

## Goal

`github.rs` has one reason to change per file.

## Contracts

Split it along the five responsibilities `B2` names, under `crates/flakeradar/src/github/`: the HTTP client, the policy table from task 17, issue rendering, filing, and sync and notes.

- Render bodies with one builder (`C1`). Bodies stay byte-identical, and task 18's test proves it.
- Remove dead parameters and `let _ = settings;` (`J14`).
- Keep public paths stable or update every caller in the same commit, with one path per item (`M-SINGLE-ITEM-PATH`).
- Split `tests/github.rs` (5,154 lines) along the same lines.
- Move code in commits of its own, separate from edits, so a reviewer can see that moved code did not change.

## Done when

- [ ] No file in `github/` mixes HTTP, rendering and database work.
- [ ] Goldens and GitHub tests pass unchanged, except for moved paths.
- [ ] The shared rules in `00-fixes-context.md` are met.
