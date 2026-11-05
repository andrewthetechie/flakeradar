# 21 — Secrets and settings

Read `00-fixes-context.md` first. Findings: `H4`, `H5`, `J2`, `B11`, the `urls()` half of `H14`, and the repeated-defaults half of `H12`.

## Goal

No secret can reach a log or a `Debug` print, and bad configuration refuses to start instead of silently scoring wrong.

## Contracts

- **`.env` errors (`H4`).** The error names the path and line number and never the line's contents.
- **Database URL (`H5`).** `Settings.database_url` and `AdvisoryLock.url` hold `Secret`, so every derived `Debug` above them, including `AppState`, is redacted. The URL's password never appears in errors either, so check `connect` and the migration path.
- **Test hooks (`H14`).** `WebhookWorker::urls()` leaves the release binary. Move its test to what the behaviour shows.
- **Validation (`J2`).** Compare each field's constraints with `backend/app/config.py` at the reference, then refuse values outside them, and refuse NaN, negative durations and windows outside their meaning. A refused start exits 1 with the field name and never the value of a secret. `DbSettings::new` (`connect.rs:46`) is the pattern. `score.min(1.0)` no longer turns NaN into 1.0 (`scoring.rs:150`).
- **One default (`B11`, `H12`).** Each default lives once as a named, documented constant used by `Settings`, `ProcessingSettings`, `ReadSettings` and retention. Each doc says why that value and what breaks if it changes.

## Repro

`review-135/repro/proofs/settings_proof.rs`.

## Done when

- [ ] A test runs a malformed `.env` holding a token, through the path `main.rs` logs, and the token is absent.
- [ ] The redaction test covers the password in `Settings`, `AdvisoryLock` and `AppState`.
- [ ] `FLAKERADAR_SCORE_DECAY=NaN`, a negative threshold, a negative window and a negative poll are each refused in a test.
- [ ] A test asserts that the `Settings`, `ProcessingSettings` and `ReadSettings` defaults agree.
- [ ] `SECURITY.md` is true as written, or corrected.
- [ ] The shared rules in `00-fixes-context.md` are met.
