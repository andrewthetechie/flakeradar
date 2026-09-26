# Contributing to FlakeRadar

Domain terms (Repo, Project, Report, Run, Execution, flaky/suspect/stable, …)
are defined in [CONTEXT.md](CONTEXT.md). Architecture decisions are in
[docs/adr/](docs/adr/). The [design notes](#design-notes) below link to each one. Use the same words in code, UI text and docs.

## Dev setup

```bash
# PostgreSQL (the backend's default DATABASE_URL points here)
docker run -d --name flakeradar-pg -p 5432:5432 \
  -e POSTGRES_USER=flakeradar -e POSTGRES_PASSWORD=flakeradar -e POSTGRES_DB=flakeradar \
  postgres:17-alpine

# Backend (Python 3.12)
cd backend
python3.12 -m venv .venv
.venv/bin/pip install -r requirements.txt
FLAKERADAR_ALLOW_INSECURE=1 .venv/bin/python -m uvicorn app.main:app --port 8000

# Frontend
cd frontend
npm install
npm run dev
```

## Test gate

All must pass before a PR is merged:

```bash
cd backend && .venv/bin/python -m pytest -q   # needs Docker: testcontainers starts Postgres
cd frontend && npm test                        # Vitest + Testing Library
cd frontend && npm run build                   # strict TypeScript build
```

Set `FLAKERADAR_TEST_DATABASE_URL=postgresql+asyncpg://…` to run the backend
tests against an existing (throwaway!) database instead — its tables are
truncated between tests.

## Database migrations

Schema changes go through Alembic. After editing `backend/app/models.py`, with
a Postgres reachable at `FLAKERADAR_DATABASE_URL`:

```bash
cd backend
.venv/bin/python -m alembic upgrade head
.venv/bin/python -m alembic revision --autogenerate -m "describe change"
# review the generated file
```

Migrations run automatically on app startup (serialized across uvicorn
workers by an advisory lock).

## Architecture

```
backend/   FastAPI + async SQLAlchemy 2 + asyncpg + PostgreSQL
  app/
    routers/reports.py  ingest (validate, queue, 202) and Report status
    routers/tests.py    Repos, Test leaderboard, summary, Test detail, quarantine
    routers/jobs.py     Job leaderboard, summary, Job detail
    queries.py          read services that REST and MCP share
    parsing.py          JUnit parsing: identity, Location, Failure details, retries
    processing.py       one Report -> one Run (or Job executions), batched upserts, rescoring
    attribution.py      explained vs unexplained Job failures
    classify.py         Failure category rules
    score_history.py    daily Score history, trend
    worker.py           the single Report processor (Postgres advisory lock)
    retention.py        pruning
    scoring.py          flip score and same-SHA proof (pure functions)
    github_integration.py  per-Repo issue filing
    mcp_server.py       read-only MCP tools (fastmcp)
  migrations/     Alembic (async), applied on startup
  tests/          pytest against a real Postgres (testcontainers)
frontend/  React 18 + Vite + TypeScript, Vitest; no runtime chart dependencies
samples/   CI snippets and a demo-data simulator
```

## Design notes

- Queued ingest ([ADR 0002](docs/adr/0002-queued-ingest.md)). An upload only
  validates and stores the raw Report. One Report processor, elected with a
  Postgres advisory lock across uvicorn workers, turns Reports into Runs in
  upload order. Scoring depends on that order.
- PostgreSQL only, fully async ([ADR 0003](docs/adr/0003-postgres-only-async.md)).
- Repos and Projects ([ADR 0001](docs/adr/0001-repo-project-split.md)).
- Pushed CI jobs and attribution ([ADR 0004](docs/adr/0004-pushed-ci-jobs-and-attribution.md)).
- Retry attempts from JUnit ([ADR 0005](docs/adr/0005-retry-attempts-from-junit.md)).
- Rule-based Failure categories ([ADR 0006](docs/adr/0006-rule-based-failure-categories.md)).
- Daily Score history ([ADR 0007](docs/adr/0007-daily-score-history.md)).
- The read APIs and the quarantine toggle are unauthenticated by design,
  because the dashboard is expected to be on a private network. The CI
  endpoints and the MCP server need the token.
- The UI marks each Execution status with a shape (filled circle, filled
  square, filled diamond, hollow circle) as well as a color. Green and red are
  hard to tell apart with deuteranopia, so color never carries meaning alone.
