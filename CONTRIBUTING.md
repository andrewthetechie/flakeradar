# Contributing to FlakeRadar

Domain terms (Repo, Project, Report, Run, Execution, flaky/suspect/stable, …)
are defined in [CONTEXT.md](CONTEXT.md); architectural decisions live in
[docs/adr/](docs/adr/). Use the same words in code, UI text and docs.

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
