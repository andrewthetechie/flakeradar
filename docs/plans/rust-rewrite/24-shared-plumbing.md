# 24 — Shared plumbing

Read `00-fixes-context.md` first. Behaviour stays fixed. Findings: `B5`, `B6`, `B10`, `J7`, `J9`, `H13`, `J17` (`M-PUBLIC-DISPLAY`).

## Goal

Each piece of plumbing exists once, which makes tasks 25–27 small.

## Contracts

- **SQL (`B5`, `J7`).** One `flakeradar_db::sql` module: one placeholder scheme and one copy of the row helpers. The five copies of `get` and `rows` go.
- **Queries in their crate (`J9`).** Raw SQL leaves `github.rs`, `routes/reports.rs` and `routes/ops.rs` for `flakeradar-db`. The DB crate stops exporting SQL text and HTTP status codes (`attribution.rs:44`, `issue_links.rs:99`).
- **Route errors (`B6`, `J7`).** One `RouteError` with `Display` and `Error` replaces the route enums, and the same plain-text 500 stays.
- **Extractors (`B6`, `J7`).** One generic validated JSON extractor replaces the `FromRequest` copies and the unused `ApiJson`. `TestPath`, `JobPath`, `ScopeQuery` and `SummaryQuery` are defined once. REST and MCP share validation bounds.
- **MCP (`B10`).** Each tool lives in one place, and list-ness is derived once.
- **Errors (`H13`).** `Scope::new` returns a real error type, and `ProcessError` becomes private or a real error.
- **Duplicates (`J7`).** The remaining pairs in that finding (permalink, `category_from_wire`, `values_of`, `REPORT_STATUSES`, `scope_conditions`) keep one copy.

## Done when

- [ ] A grep shows one definition each for `get`, `rows` and the route error, and no raw SQL outside `flakeradar-db` (list exceptions with reasons).
- [ ] All goldens replay with `allowed-differences.json` untouched.
- [ ] The shared rules in `00-fixes-context.md` are met.
