# 20 — Request validation parity

Read `00-fixes-context.md` first. Findings: `A4`, `H2`, `H19` (NUL), `S11`, `S12`, `S13` (first bullet), `J12`, `C4`, and the `loc` half of `H18`.

## Goal

Every request that Python accepted is accepted, every bad value gets a truthful 4xx, and no request overflows or reaches the database with bytes it cannot store.

## Contracts

- **Paging (`A4`, `H2`).** Bound `page` so the offset fits, in validation, and use `checked_mul` in `Board::run`. Cover REST and MCP.
- **NUL (`H19`).** Refuse NUL in every text input, body and query, in the shared validators.
- **Attempt (`S13`, `J12`, `C4`).** One parse and one rule for `ci_run_attempt` across both ingest routes.
- **Coercion (`S11`).** Restore Pydantic's lax rules on `POST /api/ingest/pipeline` and the other JSON bodies, and on query ints: numeric strings, `2.0`, booleans where Pydantic takes them, every date and time form in the table, last repeated query value wins.
- **Issues (`S11`).** A body with several problems lists all of them, as Python does, with Python's `type` values and correct `loc`, including `[..., "jobs", i, "runner_labels", j]` (`H18`). Parse to `serde_json::Value` first if serde's stop-at-first behaviour blocks this.
- **Status codes (`S12`).** Restore the multipart garbage-header 400. Log the rest as the approved table says.

Build the expected values by running each case against the reference: extract it, run it from `review-135/repro/differential/lib.py`, and save the table you measured in your status note. The `S11` table is a sample, so also test each field type of `PipelineReportIn` for the forms Pydantic accepts.

## Done when

- [ ] `page=9223372036854775807` and `page=4611686018427387905&page_size=4` get a 4xx on `/api/tests` and `/api/jobs`, in dev and release profiles, and the MCP equivalents behave the same.
- [ ] `commit_sha=abc%00`, a pipeline `branch` holding `\u0000`, and `?q=a%00b` each get a 4xx.
- [ ] Every row of the `S11` table has a test with the reference's status, and `{}` lists all missing fields.
- [ ] The writers-app and samples consumer tests still pass.
- [ ] Each `S12` row is restored or has a `differences.md` row, and that file's `PUT /mcp/` row is true when measured.
- [ ] The shared rules in `00-fixes-context.md` are met.
