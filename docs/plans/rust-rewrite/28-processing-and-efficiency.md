# 28 — Processing and efficiency

Read `00-fixes-context.md` first. Behaviour stays fixed. Findings: `B8`, `C2`, `C5`, `J4`, `J5`, `J6`, `J8` (exception-name prefixes), `J14` (the rest).

## Goal

`process_report` reads at one level, and processing no longer hogs an async thread or copies large data for nothing.

## Contracts

- **Structure (`B8`, `J14`).** `process_report` delegates to named steps, the `.map_err(ProcessError::from)?` noise goes, and `rescore_repo`'s unused results and parameters go.
- **Flags (`C2`).** Apply the verdicts that finding lists per flag argument.
- **Async (`J4`).** The hashing and `BTreeMap` work over up to a million cases runs off the async threads or yields. Inline parses of 20 MiB of JSON move to `spawn_blocking`. Avoid the double parse at `mcp.rs:1452`.
- **Copies (`J5`, `C5`).** Remove the copies those findings list. Hashing each case three times becomes once.
- **N+1 (`J6`).** `clear_expired_quarantines` becomes one `UPDATE … RETURNING`, keeping any per-row logging.
- **Stored error prefixes (`J8`).** Check whether a golden pins Python exception names such as `"OperationalError"`. Keep and document those that are pinned, and replace the rest with a real error description only if no consumer reads them.

## Done when

- [ ] `/api/health` stays under 100 ms while a 2,500-Test Report processes, as `docs/perf/22` measured, and 100 Reports process no slower than recorded there. Update that doc with the new figures.
- [ ] Goldens replay with `allowed-differences.json` untouched.
- [ ] The shared rules in `00-fixes-context.md` are met.
