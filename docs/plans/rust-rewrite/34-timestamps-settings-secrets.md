# 34 — Timestamps, settings and the database URL

Read `00-fixes-2-context.md` first. Findings: `SP8` and `SP9` (must-fix), `SP11`, `SP12`, `ST26`, `ST27`. Read their sections under "Second pass" in `review-135b/spec-findings.md` and `review-135b/standards-findings.md`. The scripts behind the `SP` rows are in `review-135b/repro/spec/second-pass/`.

## Goal

A client's timestamp names the same instant it named on the reference. A settings value either starts a working server or is refused by name. No password reaches the process output.

## Contracts

**`SP8`, first and in its own commit.** `epoch_instant` (`lax.rs:361`) reads every number as seconds. Pydantic reads a value with `abs(value) > 2e10` as milliseconds. The reviewer confirmed this directly against the reference venv: `20000000000` is `2603-10-11T11:33:20`, and `20000000001` is `1970-08-20T11:33:20.001`.

- `1767225600000`, the shape `Date.now()` gives, answers 202 here and the Report then fails, with the year past 9999 in its stored body. It must store `2026-01-01T00:00:00`.
- Past the year-9999 limit the answer is `datetime_parsing`. The limit is known only to lie between `1e14` and `1e15`, so measure the exact bound with `/tmp/pyref` before writing it as a literal.
- `completed_at` goes through the same reader. Test both fields.
- Correct `INTEGER_RANGE` (`lax.rs:22`) in the same commit. Its comment says `2^63`, and its 31-digit literal is about 9.22e30, so `1e19` up to that bound saturate to `i64::MAX`. `ci_run_attempt` then answers 400 where the reference answers 422, and `number` and `expires_in_days` answer `less_than_equal` where it answers `int_parsing_size`. Measure the MCP callers at `mcp/mod.rs:706` and `:715` as well.
- Correct `differences.md:113`.

**`SP9`.** In `parse_timestamp` (`lax.rs:267`), a string holding a plain decimal number goes through the rule from `SP8`. `"1767225600"`, `"1767225600.5"`, `"1767225600000"`, `"0"` and `"-5"` are accepted. `"1e3"`, `"1_000"` and `" 1767225600"` stay refused. Correct the comment at `validation.rs:331`.

**`SP11`.** `redact` (`connect.rs:116`) cuts the authority at the first `/`, so it misses the `@` after a password holding `/`. Take the userinfo up to the last `@` in the whole string, and print nothing of the query. Correct `differences.md:120`.

**`SP12`.** Read whole numbers with `lax::text_int`, and booleans with the body's `lax_bool`. `SCORE_WINDOW=5.0`, `SCORE_WINDOW=1_000`, `GITHUB_CLOSE_FIXED=t` and `GITHUB_CLOSE_FIXED=Y` all start. Read both helpers first, and measure any spelling you add with `/tmp/pyref`.

**`ST26` and `ST27`.** A settings interval a `Duration` cannot hold is refused at start-up by name, whichever way it fails.

- `interval` (`settings.rs:159`) also requires `Duration::try_from_secs_f64(value).is_ok_and(|wait| !wait.is_zero())`.
- `connect.rs:47-54` uses `try_from_secs_f64` and answers `DbError::InvalidTimeout`.
- The two `unwrap_or_default` calls (`worker.rs:509-511`, `server.rs:122-123`) become stored `Duration`s, so nothing can turn a bad value into a zero wait.

## Done when

- [ ] `SP8` and `SP9`: `the_pipeline_body_reads_a_timestamp_the_way_the_wire_does` holds every row of both findings' tables, with the reference's measured answer as a literal, and the commands are in `STATUS.md`.
- [ ] A test through `POST /api/ingest/pipeline` sends `1767225600000` and reads the stored Job's `started_at`.
- [ ] `SP11`: the four inputs in the finding are in `a_database_url_that_cannot_be_parsed_never_names_its_password`, plus a test that runs the binary's error path and sees no password.
- [ ] `SP12`: `a_boolean_takes_every_spelling_it_documents` and a whole-number case hold the new spellings.
- [ ] `ST26` and `ST27`: `1e30` and `1e-12` are refused for `WORKER_POLL_SECONDS`, `PRUNE_INTERVAL_SECONDS` and `DB_COMMAND_TIMEOUT_SECONDS`. `rejects_a_timeout_postgres_cannot_take` holds `1e30`.
- [ ] `battery/run.sh all` shows 0 new differences, with `OUT=` outside the repo. Add `SP8`'s and `SP9`'s bodies to it.
- [ ] The shared rules are met.
