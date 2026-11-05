# Contributing to FlakeRadar

Domain terms (Repo, Project, Report, Run, Execution, flaky/suspect/stable, …)
are defined in [CONTEXT.md](CONTEXT.md). Architecture decisions are in
[docs/adr/](docs/adr/). The [design notes](#design-notes) below link to each one. Use the same words in code, UI text and docs.

Rust code follows [docs/agents/code-standards-rust.md](docs/agents/code-standards-rust.md):
the lint set, the error type per crate, and the testing rules.

## Dev setup

FlakeRadar is one Rust binary plus PostgreSQL. The React app stays a separate
project in `frontend/`.

```bash
# PostgreSQL
docker run -d --name flakeradar-pg -p 5432:5432 \
  -e POSTGRES_USER=flakeradar -e POSTGRES_PASSWORD=flakeradar -e POSTGRES_DB=flakeradar \
  postgres:17-alpine

# The server. The crate embeds frontend/dist when it exists, so build the
# dashboard first if you want it served from the binary; `cargo` never runs npm.
(cd frontend && npm ci && npm run build)
FLAKERADAR_ALLOW_INSECURE=1 \
FLAKERADAR_DATABASE_URL=postgresql://flakeradar:flakeradar@localhost:5432/flakeradar \
  cargo run -p flakeradar

# Frontend, with hot reload against the server above
cd frontend && npm install && npm run dev
```

`flakeradar --version` prints the workspace version, and
`flakeradar healthcheck` probes `/api/health` for the container's `HEALTHCHECK`.

## Test gate

All must pass before a PR is merged. CI runs the same commands, in the `rust`
and `frontend` jobs of `.github/workflows/ci.yml`.

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --all-features --profile ci   # needs Docker: testcontainers starts Postgres
cargo test --doc --all-features
cargo audit
cd frontend && npm run typecheck && npm run lint && npm run format:check && npm test
```

Set `FLAKERADAR_TEST_DATABASE_URL=postgresql://…` to run the Rust tests
against an existing (throwaway!) database instead of a testcontainers one; its
tables are truncated between tests. `scripts/smoke.sh <binary> <database-url>`
exercises a built binary end to end — every route, an ingest that reaches
`processed`, `/docs`, MCP `tools/list` and the `/metrics` series — and is what
the release matrix runs on each of the five targets.

### Rust coverage

CI measures line and branch coverage on a pinned nightly with
`cargo-llvm-cov`, against `coverage-floor.json` at the repo root:

```bash
rustup toolchain install nightly-2026-10-25 --profile minimal -c llvm-tools-preview
cargo +nightly-2026-10-25 llvm-cov nextest --branch --all-features --profile ci --html
```

The floors are the measured totals, rounded down, and they ratchet: a PR fails
when either total is below the baseline `main` last uploaded as the
`rust-coverage` artifact, and a PR that adds tests raises the floors in the same
change. Never lower a floor, and never add a scenario, allowance, `#[ignore]` or
`coverage(off)` to make a gate pass.

Every line a test cannot reach is a row in
[docs/plans/rust-rewrite/coverage-gaps.md](docs/plans/rust-rewrite/coverage-gaps.md),
with the file, the function and why. `#[cfg_attr(coverage_nightly,
coverage(off))]` is limited to code a test process cannot run (the process edge
in `main.rs`, signal wiring) and each use is logged there too.

### Frontend coverage

Coverage (v8) is measured in CI only, where `CI` is set; local `npm test` stays
fast. To see per-file coverage locally:

```bash
cd frontend
npx vitest run --coverage.enabled --coverage.reporter=text
```

CI fails the frontend job in two cases:

- Statements, branches, functions or lines is below its threshold in
  `frontend/vite.config.ts`. The thresholds are the measured baseline, rounded
  down. Raise them as coverage grows. Never lower them.
- On a PR, any of the four is lower than the newest push to `main`. Each push
  to `main` uploads `coverage-summary.json` as the `frontend-coverage`
  artifact for this comparison.

## Database migrations

The schema is created by one baseline SQL file,
`crates/flakeradar-db/migrations/0001_baseline.sql`, which
`crates/flakeradar-db/src/migration.rs` embeds and applies under an advisory
lock. The entities under `crates/flakeradar-db/src/entities/` are generated from
a migrated database, never written by hand.

To change the schema:

1. Add a new numbered SQL file beside the baseline and register it in the
   migration set.
2. Apply it to a scratch database and regenerate the entities:

```bash
export FLAKERADAR_TEST_DATABASE_URL=postgresql://flakeradar:flakeradar@localhost:5432/scratch
cargo run -q -p flakeradar-db --example migrate -- "$FLAKERADAR_TEST_DATABASE_URL"
cargo install sea-orm-cli --version 2.0.4 --locked   # once
sea-orm-cli generate entity \
  --database-url "$FLAKERADAR_TEST_DATABASE_URL" \
  --output-dir crates/flakeradar-db/src/entities \
  --with-prelude none
```

3. Commit the SQL, the regenerated entities and the query changes together. CI
   regenerates the entities from a migrated database and fails on a diff, so a
   schema change that the entities do not reflect cannot land.

Constraint and index names are part of the schema contract: the baseline keeps
the names the 1.x database used, and a new migration keeps that convention.

## Architecture

```
crates/
  flakeradar-core/        pure logic and DTOs: no database, no network
    parsing.rs            JUnit parsing: identity, Location, Failure details, retries
    identity.rs           the Test fingerprint (byte-identical to 1.x)
    scoring.rs            flip score and same-SHA proof
    classify.rs           Failure category rules
    notify.rs             render a Notification from a Tier change
    dto.rs                every request and response body, with utoipa schemas
    py.rs, helpers.rs     the Python string/cap/round rules behaviour depends on
  flakeradar-db/          baseline migration, entities, queries, processing
    migration.rs          the baseline SQL and the old-database refusal
    report_intake.rs      queue a Report, duplicate detection, the summary
    processing.rs         one Report -> one Run (or Job executions), rescoring
    attribution.rs        explained vs unexplained Job failures
    score_history.rs      daily Score history, trend
    reads.rs              every read: leaderboards, summaries, detail, failures
    issue_links.rs        set or clear a Test's or Job's Issue link
    deletes.rs            admin delete of a Repo or Project (cascade)
    retention.rs          pruning
    connect.rs, error.rs  the pool, the statement timeout, the error type
  flakeradar-test-support/ test databases and factories (dev-dependency only)
  flakeradar/             Axum, MCP, workers, the GitHub and webhook clients, the binary
    app.rs                the router and the 404/405/trailing-slash rules
    routes/               reports (ingest, queue), reads, issues, admin, ops
    extract.rs            the 422 query/path/JSON extractors
    mcp.rs                the MCP server (rmcp): eleven tools, stateless
    worker.rs             the single Report processor (Postgres advisory lock)
    github.rs, github_worker.rs   the GitHub client and the issue-filing task
    webhooks.rs           the Notification delivery task
    assets.rs             the embedded dashboard (rust-embed)
    settings.rs, cli.rs   every FLAKERADAR_* setting, and the command line
frontend/   React 18 + Vite + TypeScript, Vitest; embedded at build time
tests/parity/  the recorded Python corpus: scenarios, goldens, the JUnit corpus
samples/    CI snippets and a demo-data simulator
```

How the 1.x modules map onto it: `app/routers/*` became
`crates/flakeradar/src/routes/*` with the queries in `flakeradar-db`'s `reads.rs`;
`app/queries.py` (the read services REST and MCP shared) became `reads.rs`, and
`mcp.rs` calls it rather than the routes; `app/processing.py`, `attribution.py`,
`score_history.py`, `retention.py` and `issue_links.py` moved to the same-named
`flakeradar-db` modules; `app/parsing.py`, `classify.py`, `scoring.py` and
`notify.py` became `flakeradar-core` modules; `app/worker.py`,
`github_integration.py` and `notify.py`'s delivery became `worker.rs`,
`github.rs`/`github_worker.rs` and `webhooks.rs`; `app/config.py` became
`settings.rs`.

## Building with the embedded frontend

`crates/flakeradar/build.rs` sets `cfg(has_frontend)` when
`frontend/dist/index.html` exists and embeds the whole directory with
`rust-embed` (`debug-embed`, so a debug build carries it too). Without a build it
warns and produces an API-only binary that answers 404 at `/`. Cargo never runs
npm: build the dashboard first, or let CI's `frontend-dist` artifact do it, as
`release.yml` and the `targets` matrix do.

## Design notes

- Queued ingest ([ADR 0002](docs/adr/0002-queued-ingest.md)). An upload only
  validates and stores the raw Report. One Report processor, elected with a
  Postgres advisory lock across replicas, turns Reports into Runs in upload
  order. Scoring depends on that order.
- PostgreSQL only ([ADR 0003](docs/adr/0003-postgres-only-async.md)).
- Repos and Projects ([ADR 0001](docs/adr/0001-repo-project-split.md)).
- Pushed CI jobs and attribution ([ADR 0004](docs/adr/0004-pushed-ci-jobs-and-attribution.md)).
- Retry attempts from JUnit ([ADR 0005](docs/adr/0005-retry-attempts-from-junit.md)).
- Rule-based Failure categories ([ADR 0006](docs/adr/0006-rule-based-failure-categories.md)).
- Daily Score history ([ADR 0007](docs/adr/0007-daily-score-history.md)).
- Issue links from other filers ([ADR 0008](docs/adr/0008-issue-links-from-other-filers.md)).
- Webhook Notifications ([ADR 0009](docs/adr/0009-webhook-notifications.md)).
- One Rust binary replaces the Python server
  ([ADR 0010](docs/adr/0010-rust-rewrite.md)).
- The read APIs and the quarantine toggle are unauthenticated by design,
  because the dashboard is expected to be on a private network. The CI
  endpoints, the Issue link endpoints (`PUT`/`DELETE …/issue`), the admin
  Repo/Project delete (`DELETE /api/repos/…`) and the MCP server need the
  token.
- The UI marks each Execution status with a shape (filled circle, filled
  square, filled diamond, hollow circle) as well as a color. Green and red are
  hard to tell apart with deuteranopia, so color never carries meaning alone.
