# 29 — Clock and test hygiene

Read `00-fixes-context.md` first. Behaviour stays fixed. Findings: `H7`, `T1`, `H8`, `H14` (the rest), `H20`, `T3`, `T4`, `T5`, `J8` (`comparisons::record`).

## Goal

The suite tells the truth on a loaded runner: `code-standards-rust.md`'s "No test depends on real time" holds with no exceptions.

## Contracts

- **Clock seam (`H8`).** `AppState` carries the clock, so `Utc::now` leaves `routes/reports.rs` and `github/`. `settings.rs` stops building a clock per request.
- **Sleeps (`H7`, `T1`).** Every site listed moves to a seam signal, such as a drained-batch notification from the task, then asserts. A negative assertion must wait on proof that the task ran. Wall-clock polling goes too, or if a bound on a failure proves unavoidable, change the rule in the standards doc in this same commit and say why.
- **Factories (`H7`).** `factories.rs:26` stops stamping with `Utc::now`.
- **Test code placement (`H14`, `T5`).** `FixedClock`, `queued()`, `dropped()`, `with_api_token`, `tests/common/mod.rs` and the per-file fakes move to `flakeradar-test-support`, and its `#![allow(dead_code)]` goes.
- **Log scraping (`T3`).** The 27 `logged_from` assertions use task 17's `BatchReport`, or another returned value, where the outcome is not itself a log requirement.
- **Test language (`T4`).** Named accessors replace positional reads such as `.await.2`.
- **Placement (`H20`).** Tests that touch only public items move to `tests/`. Tautological tests are rewritten to assert behaviour or deleted, with coverage holding.
- **Production hooks (`J8`).** `comparisons::record` leaves `Needle::find`, with the linear-scan test kept.

## Done when

- [ ] `grep -rn 'time::sleep\|Instant::now\|Utc::now' crates/*/tests crates/flakeradar-test-support` shows only `tokio::time::pause` tests or listed exceptions.
- [ ] The suite passes three times running under CPU stress (`stress-ng` or equivalent), with identical coverage summaries.
- [ ] The shared rules in `00-fixes-context.md` are met.
