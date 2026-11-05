# 00 — Shared context for the Rust rewrite

**Read this whole file before you start any task in `docs/plans/rust-rewrite/`.** Each task file gives the contracts for its own step. This file gives the decisions, ground rules and traps that all tasks share. Also read `CONTEXT.md` and `docs/adr/0010-rust-rewrite.md`. For tasks 16–31, also read `00-fixes-context.md`, which overrides this file where they differ.

## What we are building

One Rust binary, `flakeradar`, that replaces the Python server in `backend/`. All anyone needs to run FlakeRadar is that binary and a PostgreSQL database. The React app stays a separate project in `frontend/`, and its build is embedded in the binary.

The Python code is the reference for behaviour. It is frozen at the branch point (commit `97b933d`) and stays in the tree until task 15 deletes it.

## Settled decisions

Do not reopen these. If one turns out to be impossible, follow "When you are blocked" below.

| Topic | Decision |
|---|---|
| Parity | Functional parity, not identical bytes. See "The compatibility rule" |
| Cutover | One PR adds Rust and deletes Python. Rust releases as `v2.0.0` |
| Database | PostgreSQL only. Fresh install, with no Alembic adoption and no import tool |
| Old databases | The binary refuses to start if it finds `alembic_version`, or app tables with no `seaql_migrations`. It never drops or changes anything |
| Schema | The baseline keeps today's tables, columns, constraint names and index names exactly. No renames |
| ORM | SeaORM 2.x. A raw `Statement` is allowed where the builder cannot express a query |
| HTTP | Axum, `tower-http` |
| MCP | Official `rmcp` SDK, Streamable HTTP at `/mcp/`, stateless (`NeverSessionManager`) |
| API docs | `utoipa` and `utoipa-redoc`. Redoc at `/docs`, spec at `/openapi.json`, with a pinned Redoc script embedded in the binary. No CDN |
| Frontend | Embedded with `rust-embed`. Cargo never runs npm. No SPA fallback. Unknown paths return 404 |
| Targets | `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc` |
| Container | `gcr.io/distroless/static-debian13:nonroot`, pinned by digest |
| Binary interface | `flakeradar` serves. Also `flakeradar healthcheck` and `--version`. Env and `.env` config only |
| Default bind | `127.0.0.1:8000`, set with `FLAKERADAR_HOST` and `FLAKERADAR_PORT`. The image sets `0.0.0.0` |
| Runner | Local agents work this plan on `feat/rust-rewrite`. Fabro stays off it until after merge |
| CI runners | GitHub-hosted, because the repo is public |

## Delivery rules

- **Branch:** every task lands on `feat/rust-rewrite`, in number order, one at a time. Commit straight to that branch. Do not create task branches. Do not merge to `main`.
- **Order:** tasks run **strictly in sequence**, 01 through 15. Assume every lower-numbered task has landed. Never start a task while an earlier one is unfinished.
- **Status:** when you finish a task, mark it done in `STATUS.md` with its last commit SHA, and push the branch.
- **The one PR:** task 15 opens it. No earlier task opens a PR.
- **Rust code:** invoke the `ms-rust` skill and load the guideline files it names **before writing any Rust**. From task 01 on, also follow `docs/agents/code-standards-rust.md`.
- **Library facts:** check current docs (the `context7` MCP server) before relying on an API from memory, especially SeaORM 2, `rmcp` and `utoipa`.
- **Commit messages** end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. If you are a different model, use your own name in the same format.
- **Do not change** `backend/` (except where task 15 deletes it), `CONTEXT.md` or existing ADRs. Task 15 updates docs.
- **Vocabulary:** use `CONTEXT.md`'s terms in code, docs and error text: Repo, Project, Test, Report, Run, Execution, Job execution, and so on.

### When you are blocked

If a contract here is wrong because a library behaves differently, do the smallest correct thing, keep the public names, and say what changed in the commit message.

Stop and ask the user, recording `BLOCKED: <question>` in `STATUS.md`, if you would need to:

- remove or rename a response field,
- change a status code, or
- change a decision in the table above.

## The compatibility rule

**The default is to keep behaviour the same.** You may change a response only if every consumer below keeps working, and you must log it in `differences.md` with the reason. Do not make improvements for their own sake.

**Must stay identical:**

- routes, methods and status codes
- JSON field names and types
- hand-written `detail` strings, apart from the entries pre-approved in `differences.md`
- the `FLAKERADAR_*` env var names and defaults
- MCP tool names, arguments and annotations, and the instructions text
- Test fingerprints, byte for byte
- scores, Proven flake counts, Clean streaks, Fail streaks and Failure categories
- the database schema

**May differ:**

- the wording of framework-generated 422 messages, as long as the `{"detail":[{"type","loc","msg"}]}` shape stays
- JSON key order
- timestamp formatting within the rule below
- log format
- process model

### Consumers

| Consumer | What it relies on |
|---|---|
| `frontend/src/api.ts` and its types | Every read API, quarantine |
| `actions/report/report.sh` | `POST /api/ingest`, reads `.detail // .error` from errors |
| `actions/summary/summary.sh` | `GET /api/ci-runs/{id}/failures` |
| `samples/` | `/api/ingest`, `/api/ingest/pipeline` |
| MCP clients | `/mcp/` with `Authorization: Bearer` |
| writers-app CI | `POST /api/ingest` with `Content-Type: application/xml` and a raw body, plus `root`, `pipeline`, `ci_job_id`, `ci_run_attempt` and `default_branch` query params. `POST /api/ingest/pipeline` with JSON |
| Fabro (`fabro-workflows/ops/fabro-io/src/flakeradar.rs`, `scout/flakeradar.rs`) | See below |

Fabro reads these fields, and merge decisions depend on them:

- `GET /api/tests` items: `id`, `project`, `fingerprint`, `suite`, `classname`, `name`, `file`, `line`, `flakiness_score`, `tier`, `confirmed_flake_count`, `failure_category`, `last_seen_at`, `quarantined`
- `GET /api/tests/{id}/history`: `executions[].{status,message,details,created_at,branch,ci_run_id}`, `last_failing_sha`, `last_failing_branch`
- `GET /api/ci-runs/{id}/failures`: `reports.{pending,failed}`, `truncated`, `failures[].{name,project,ci_job_id,status,tier,flakiness_score,confirmed_flake_count,failure_category,message,github_issue_number}`, `failed_jobs[].{ci_job_id,outcome,job_id}`
- `PUT /api/tests/{id}/issue`, `PUT /api/jobs/{id}/issue`

**Timestamp rule.** Fabro rejects any timestamp that is not `YYYY-MM-DDTHH:MM:SS`, then an optional `.` and digits, then `Z` or `+00:00`. Emit UTC as `YYYY-MM-DDTHH:MM:SS.ffffffZ` everywhere. Dates are `YYYY-MM-DD`.

## Target layout

```
Cargo.toml                 workspace, [workspace.lints], [workspace.dependencies]
rust-toolchain.toml
clippy.toml
.config/nextest.toml
.cargo/audit.toml
crates/
  flakeradar-core/         pure logic and DTOs, no database and no network
  flakeradar-db/           baseline migration, entities, queries, processing, retention
  flakeradar-test-support/ test databases and factories (dev-dependency only)
  flakeradar/              Axum, MCP, workers, GitHub and webhook clients, embedded assets, the binary
tests/parity/              JUnit corpus, scenarios, golden files
frontend/                  unchanged React app
scripts/                   coverage-gate.mjs, version.mjs, smoke.sh, check-test-manifest.mjs
```

`flakeradar-core` holds the response and request DTOs (`serde` and `utoipa::ToSchema` derives), so `flakeradar-db` and `flakeradar` share them. It must not depend on SeaORM, Axum or any network crate.

## Stack

`tokio`, `axum`, `tower-http`, `sea-orm` (SQLx driver, rustls), `reqwest` (rustls, OS trust store), `roxmltree` with `encoding_rs`, `regex`, `sha1`, `sha2`, `chrono`, `serde`, `serde_json`, `utoipa`, `utoipa-redoc`, `rmcp`, `schemars`, `rust-embed`, `clap`, `tracing`, `tracing-subscriber`, `dotenvy`, `anyhow`, `thiserror`, `tokio-util` (`CancellationToken`).

Tests: `cargo-nextest`, `testcontainers`, `wiremock`, `tokio::time::pause`.

Use the `ring` crypto provider for rustls wherever there is a choice, because it cross-compiles more simply than `aws-lc-rs` on Windows and musl.

## Verification

Every task must leave all of this passing:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --all-features --profile ci     # needs Docker, or FLAKERADAR_TEST_DATABASE_URL
cd frontend && npm run typecheck && npm test      # only if you touched frontend/
```

## The coverage goal

**Aim for as close to 100% line and branch coverage as possible.** The floors (95% lines, 80% branches) are only the minimum that task 15 must clear.

For every task:

1. Test every reachable line and branch you add. Run the coverage command below and read the report for your files before you finish.
2. Design for testability: inject the clock, HTTP base URLs, poll intervals and settings. Use `tokio::time::pause` rather than real sleeps.
3. Tests must be deterministic. The gate has no tolerance, so a path reached only by timing makes CI fail at random.
4. Log every line or branch you could not cover in `coverage-gaps.md`, with the file, the function and why. Do not silently leave gaps.
5. Use `#[cfg_attr(coverage_nightly, coverage(off))]` only for code a test process cannot run, such as `main()` and signal wiring. Log each use in `coverage-gaps.md`.

```bash
rustup toolchain install nightly-2026-10-25 --profile minimal -c llvm-tools-preview
cargo install cargo-llvm-cov --version 0.9.1 --locked
cargo +nightly-2026-10-25 llvm-cov nextest --branch --all-features --profile ci --html
```

## Porting tests

`test-manifest.csv` (task 02) lists all 575 pytest functions and the task that owns each one. When you port one, set its `status` to `ported` and fill in `rust_test`. If a test cannot apply in Rust, set `status` to `na` and give a `reason`. Port the **behaviour** a test proves, not its Python mechanics. Keep the test's intent readable from its Rust name.

## Python behaviour that changes silently in Rust

Each row needs an explicit test in the task that owns it.

| Python | Rust trap | Do this |
|---|---|---|
| `round(x, 4)` rounds the exact binary value, half to even on true ties | `(x * 10000.0).round() / 10000.0` gives different answers | Round through exact decimal formatting. Test against this table from CPython: `0.03125→0.0312`, `0.09375→0.0938`, `0.15625→0.1562`, `0.00005→0.0001`, `0.12345→0.1235`, `0.29995→0.2999`, `0.30005→0.3`, `1/3→0.3333`, `2/3→0.6667` |
| `len()` and slicing count code points | `str::len()` counts bytes, and slicing can panic mid-character | Cap with `chars()`. Applies to `MESSAGE_MAX`, `DETAILS_MAX`, `ERROR_MAX` and the MCP `DETAILS_MAX`. Keep the marker `\n…[truncated]` |
| `str.strip()` also strips `\x1c`–`\x1f` | `trim()` keeps them | Write one `py_strip` helper |
| `str.splitlines()` also splits on `\v`, `\f`, `\x1c`–`\x1e`, `\x85`, ` `, ` ` | `lines()` splits only on `\n` and `\r\n` | Write one `py_splitlines` helper for `_first_line` |
| `re.IGNORECASE` on `str` | `regex` matches too, with Unicode folding | Use `(?i)`. `\b` is Unicode-aware in both. Test the classifier against the corpus |
| ElementTree honours the declared encoding, then falls back to lossy UTF-8 | `roxmltree` takes only UTF-8 | Transcode with `encoding_rs` first. Keep the fallback (see task 04) |
| `{raw!r}` in error text gives `'value'` | `{:?}` gives `"value"` | Single-quote the value. Escaping may differ; see `differences.md` |
| `f"{score:.2f}"` | `{:.2}` should agree | Test it in the Slack rendering |
| `str.isdigit()` accepts Unicode digits | | Accept ASCII digits only for `line`. Logged in `differences.md` |
| Axum's body limit is 2 MB by default | Large uploads fail with the wrong error | Set limits explicitly (tasks 06 and 05) |
| FastAPI returns 422 for bad query, path and JSON values | Axum extractors return 400 | Use custom extractors that return the 422 shape (task 05) |
| Starlette redirects `/x/` to `/x` with 307 when `/x` exists | Axum returns 404 | Handle it in the fallback (task 05) |
| `asyncpg` `command_timeout` cancels on the client | Not automatic | Set server-side `statement_timeout` on every pooled connection (task 03) |
| SQLAlchemy `update(Model), [dicts]` leaves out columns missing from a dict | A naive bulk update overwrites them | `github_issue_fixed_noted_at`, `file` and `line` must keep their value unless set (task 07) |
| Python's `max_retries` backoff `poll * 2**n` | | Cap at 30 seconds (task 08) |

## Files in this plan

| File | Purpose |
|---|---|
| `STATUS.md` | Which tasks are done, and any blocker |
| `differences.md` | Every intentional behaviour change, with the reason |
| `coverage-gaps.md` | Every uncovered line or branch, with the reason |
| `spike-notes.md` | Written by task 03: which queries use the builder and which are raw |
| `test-manifest.csv` | Written by task 02: every pytest function and its Rust port |
