# 27 — Test and Job duplication

Read `00-fixes-context.md` first. Behaviour stays fixed. Findings: `B1`, and `T2`'s string-selected tests.

## Goal

A rule about scoring, tiers or links changes in one place for both Tests and Jobs.

## Contracts

Work through the pairs `B1` and `J7` list, one pair per commit, running the parity replay on each: `rescore`/`rescore_jobs`, `test_tier_changes`/`job_tier_changes`, `old_test_scores`/`old_job_scores`, `write_test_updates`/`write_job_updates`, and the pairs in `reads.rs` and `retention.rs`.

- `Board` and `LinkTarget` are the patterns to follow: an abstraction over what differs, driven by `EntityKind` from task 25.
- Keep a pair apart when its difference is real, and say why in the ledger. Job scores count only unexplained failures, so that rule stays Job-only.
- Tests that switch on `if entity == "test"` become one test per concept, sharing helpers.

## Done when

- [ ] The duplicate-block measure from `review-135/repro/` or an equivalent shows the `B1` pairs gone, or each remaining pair has a stated reason.
- [ ] The `docs/perf/20` and `21` query plans still show the same index use.
- [ ] Goldens replay with `allowed-differences.json` untouched.
- [ ] The shared rules in `00-fixes-context.md` are met.
