# 01 — Postgres + async foundation

## Tracer-Bullet Outcome
The backend starts against PostgreSQL with a fully async SQLAlchemy stack. Migrations run at startup from inside the event loop and are safe with several uvicorn workers. The final schema (repos, projects, test cases, runs, executions, reports) exists. The test suite runs against a real Postgres container. `GET /api/health` works and `docker compose up` brings up the app together with Postgres.

This is a **prefactor for the whole plan**. The obstacle it removes is that every later task needs the async session, the final schema and a Postgres test harness, and none of those exist yet.

## User Story
As the FlakeRadar maintainer, I want the app on async Postgres with a clean schema so that concurrent CI uploads can never lose data to SQLite's single-writer lock.

## Description
Replace the sync SQLite database layer with async SQLAlchemy + asyncpg on PostgreSQL. Squash the Alembic history into one fresh baseline that creates the **final** schema described in `00-shared-context.md`. Replace the SQLite test fixtures with a Postgres (testcontainers) harness. Add a Postgres service to `docker-compose.yml`.

All the code below has been **run and verified** (22 tests passing against `postgres:17-alpine`). Copy it exactly unless a rule below says otherwise.

This task **intentionally breaks** ingest, the read APIs and the GitHub integration. They are removed or left unimported, and later tasks rebuild them (see Non-goals).

## Context Pack
- Source decisions: Postgres only, fully async, squashed baseline, data wiped (ADR `docs/adr/0003-postgres-only-async.md`). Repo/Project split (ADR 0001). Queued ingest needs the `reports` table (ADR 0002). Tests use testcontainers `postgres:17-alpine`, with the `FLAKERADAR_TEST_DATABASE_URL` override. Compose ships Postgres.
- Repo facts (current state, before this task):
  - `backend/app/db.py` builds a **sync** engine with SQLite `check_same_thread`/`timeout` connect args.
  - `backend/app/models.py` has `TestCase`, `TestRun`, `TestExecution`, with a `project` string column and a `UTCDateTime` TypeDecorator that exists only because SQLite drops tzinfo.
  - `backend/app/migrate.py` stamps pre-Alembic SQLite DBs at revision `0001` and then upgrades. `backend/migrations/versions/` holds `0001_baseline.py` and `0002_project_and_quarantine.py`, both with SQLite `batch_alter_table`.
  - `backend/app/main.py` holds every route plus `require_token`. `backend/app/ingest.py` does parsing and persistence.
  - `backend/tests/conftest.py` uses in-memory SQLite and `fastapi.testclient.TestClient`.
  - `backend/Dockerfile` copies **only** `backend/app` into the image, so `alembic.ini` and `migrations/` are missing from the image today. That is a bug; fix it.
  - `docker-compose.yml` has one service, `flakeradar`, with a SQLite volume.
- Verified external contracts: see "Verified external contracts" in `00-shared-context.md` (testcontainers import path, pytest-asyncio ini keys, Alembic `config.attributes["connection"]` pattern).
- Non-goals (later tasks do these; do **not** start them):
  - JUnit parsing (02); ingest endpoint (03); processor (04); worker (05); retention (06).
  - GitHub integration rewrite (07). `app/github_integration.py` stays as is, unimported and broken.
  - Read endpoints (08, 09); MCP (10); frontend (11–13); README/docs (14).
  - Do not add ORM relationships.

## Delivery Strategy
- Shape: Wide refactor: Expand (this is the first step of the integration-branch sequence; see `00-shared-context.md` Delivery rules).
- Valid-state scope: Named integration branch `feat/repo-split` until task 15. After this task, only the validator commands below must pass. The frontend build and the removed endpoints are expected to be broken.

## Implementation Contract
- Expected files:
  - **Replace:** `backend/app/models.py`, `backend/app/db.py`, `backend/app/migrate.py`, `backend/app/config.py`, `backend/app/main.py`, `backend/migrations/env.py`, `backend/tests/conftest.py`, `backend/pytest.ini`, `backend/requirements.txt`, `backend/Dockerfile`, `docker-compose.yml`, `.env.example`.
  - **Edit:** `backend/alembic.ini`. Change the line `sqlalchemy.url = sqlite:///./data/flakeradar.db` to `sqlalchemy.url =` (empty). `env.py` then falls back to Settings.
  - **Create:** `backend/app/auth.py`, `backend/app/routers/__init__.py` (empty file), `backend/migrations/versions/0001_baseline.py` (new content), `backend/tests/factories.py`, `backend/tests/test_db.py`.
  - **Delete:** `backend/app/ingest.py`, `backend/migrations/versions/0002_project_and_quarantine.py` (and the old `0001_baseline.py`, replaced by the new one), `backend/tests/test_api.py`, `backend/tests/test_multiproject.py`, `backend/tests/test_quarantine.py`, `backend/tests/test_review_fixes.py`, `backend/tests/test_migration.py`, `backend/tests/test_github.py`.
  - **Keep untouched:** `backend/app/scoring.py`, `backend/tests/test_scoring.py`, `backend/app/github_integration.py` (broken until task 07), `backend/app/schemas.py` (rewritten by later tasks).
- Interfaces and names (target code — copy verbatim):

`backend/app/models.py`
```python
"""ORM models — the final FlakeRadar schema (see CONTEXT.md for the terms).

Design notes:
- A Repo (``owner/name``) has Projects; a Test (``TestCase``) is unique by a
  stable fingerprint of (suite, classname, name) within one Project.
- A Report is the raw JUnit upload, queued until the processor turns it into
  a Run. Executions are keyed to a Run's commit SHA because a fail->pass flip
  on the SAME sha is proof of nondeterminism.
- No ORM relationships: async SQLAlchemy cannot lazy-load, so every read is
  an explicit select()/join.
"""
from datetime import datetime, timezone
from typing import Any

from sqlalchemy import (
    BigInteger, Boolean, DateTime, Float, ForeignKey, Index, Integer,
    LargeBinary, String, Text, UniqueConstraint, false,
)
from sqlalchemy.dialects.postgresql import JSONB
from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column

REPORT_PENDING = "pending"
REPORT_PROCESSED = "processed"
REPORT_FAILED = "failed"
REPORT_STATUSES = (REPORT_PENDING, REPORT_PROCESSED, REPORT_FAILED)


def utcnow() -> datetime:
    return datetime.now(timezone.utc)


class Base(DeclarativeBase):
    pass


class Repo(Base):
    __tablename__ = "repos"

    id: Mapped[int] = mapped_column(primary_key=True)
    name: Mapped[str] = mapped_column(String(255), unique=True)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow)


class Project(Base):
    __tablename__ = "projects"
    __table_args__ = (
        UniqueConstraint("repo_id", "name", name="uq_projects_repo_name"),
    )

    id: Mapped[int] = mapped_column(primary_key=True)
    repo_id: Mapped[int] = mapped_column(
        ForeignKey("repos.id", ondelete="CASCADE"), index=True
    )
    name: Mapped[str] = mapped_column(String(100))
    root: Mapped[str] = mapped_column(String(1024), default="", server_default="")
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow)


class TestCase(Base):
    __tablename__ = "test_cases"
    __test__ = False  # stop pytest trying to collect this class
    __table_args__ = (
        UniqueConstraint("project_id", "fingerprint",
                         name="uq_test_cases_project_fingerprint"),
        Index("ix_test_cases_project_score", "project_id", "flakiness_score"),
    )

    id: Mapped[int] = mapped_column(primary_key=True)
    project_id: Mapped[int] = mapped_column(
        ForeignKey("projects.id", ondelete="CASCADE"), index=True
    )
    fingerprint: Mapped[str] = mapped_column(String(40))
    suite: Mapped[str] = mapped_column(Text, default="", server_default="")
    classname: Mapped[str] = mapped_column(Text, default="", server_default="")
    name: Mapped[str] = mapped_column(Text)

    # Location: latest report wins; a report without `file` keeps the old one.
    file: Mapped[str | None] = mapped_column(Text, default=None)
    line: Mapped[int | None] = mapped_column(Integer, default=None)

    # Cached analytics, recomputed whenever a Run touches this test.
    flakiness_score: Mapped[float] = mapped_column(Float, default=0.0, server_default="0")
    confirmed_flake_count: Mapped[int] = mapped_column(Integer, default=0, server_default="0")
    last_status: Mapped[str] = mapped_column(String(16), default="passed", server_default="passed")
    last_seen_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow)

    # Manual quarantine: a human marks a test skippable by the runner.
    quarantined: Mapped[bool] = mapped_column(Boolean, default=False, server_default=false())
    quarantined_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), default=None)

    # GitHub issue automation bookkeeping.
    github_issue_number: Mapped[int | None] = mapped_column(Integer, default=None)


class TestRun(Base):
    __tablename__ = "test_runs"
    __test__ = False

    id: Mapped[int] = mapped_column(primary_key=True)
    project_id: Mapped[int] = mapped_column(
        ForeignKey("projects.id", ondelete="CASCADE"), index=True
    )
    commit_sha: Mapped[str] = mapped_column(String(64), index=True)
    branch: Mapped[str] = mapped_column(String(255), default="main")
    ci_run_id: Mapped[str] = mapped_column(String(255), default="", server_default="")
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow)


class TestExecution(Base):
    __tablename__ = "test_executions"
    __test__ = False
    __table_args__ = (
        Index("ix_exec_case_id", "test_case_id", "id"),
    )

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True)
    test_case_id: Mapped[int] = mapped_column(
        ForeignKey("test_cases.id", ondelete="CASCADE")
    )
    test_run_id: Mapped[int] = mapped_column(
        ForeignKey("test_runs.id", ondelete="CASCADE"), index=True
    )
    status: Mapped[str] = mapped_column(String(16))  # passed | failed | error | skipped
    duration: Mapped[float] = mapped_column(Float, default=0.0, server_default="0")
    message: Mapped[str] = mapped_column(Text, default="", server_default="")
    details: Mapped[str] = mapped_column(Text, default="", server_default="")
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=utcnow, index=True
    )


class Report(Base):
    __tablename__ = "reports"
    __table_args__ = (
        Index("ix_reports_status_id", "status", "id"),
    )

    id: Mapped[int] = mapped_column(primary_key=True)
    project_id: Mapped[int] = mapped_column(
        ForeignKey("projects.id", ondelete="CASCADE"), index=True
    )
    commit_sha: Mapped[str] = mapped_column(String(64))
    branch: Mapped[str] = mapped_column(String(255), default="main")
    ci_run_id: Mapped[str] = mapped_column(String(255), default="", server_default="")
    # Project root sent with this upload; None means "leave the Project's root alone".
    root: Mapped[str | None] = mapped_column(String(1024), default=None)
    body: Mapped[bytes] = mapped_column(LargeBinary)
    status: Mapped[str] = mapped_column(String(16), default=REPORT_PENDING,
                                        server_default=REPORT_PENDING)
    error: Mapped[str | None] = mapped_column(Text, default=None)
    counts: Mapped[dict[str, Any] | None] = mapped_column(JSONB, default=None)
    run_id: Mapped[int | None] = mapped_column(
        ForeignKey("test_runs.id", ondelete="SET NULL"), default=None
    )
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow)
    processed_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), default=None)
```

`backend/migrations/versions/0001_baseline.py`
```python
"""fresh baseline: repos, projects, tests, reports, runs, executions (Postgres)

Revision ID: 0001
Revises:
Create Date: 2026-09-25
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql

revision = "0001"
down_revision = None
branch_labels = None
depends_on = None

TS = sa.DateTime(timezone=True)


def upgrade() -> None:
    op.create_table(
        "repos",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("name", sa.String(length=255), nullable=False),
        sa.Column("created_at", TS, nullable=False),
        sa.UniqueConstraint("name", name="uq_repos_name"),
    )

    op.create_table(
        "projects",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("repo_id", sa.Integer(),
                  sa.ForeignKey("repos.id", ondelete="CASCADE"), nullable=False),
        sa.Column("name", sa.String(length=100), nullable=False),
        sa.Column("root", sa.String(length=1024), nullable=False, server_default=""),
        sa.Column("created_at", TS, nullable=False),
        sa.UniqueConstraint("repo_id", "name", name="uq_projects_repo_name"),
    )
    op.create_index("ix_projects_repo_id", "projects", ["repo_id"])

    op.create_table(
        "test_cases",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("project_id", sa.Integer(),
                  sa.ForeignKey("projects.id", ondelete="CASCADE"), nullable=False),
        sa.Column("fingerprint", sa.String(length=40), nullable=False),
        sa.Column("suite", sa.Text(), nullable=False, server_default=""),
        sa.Column("classname", sa.Text(), nullable=False, server_default=""),
        sa.Column("name", sa.Text(), nullable=False),
        sa.Column("file", sa.Text(), nullable=True),
        sa.Column("line", sa.Integer(), nullable=True),
        sa.Column("flakiness_score", sa.Float(), nullable=False, server_default="0"),
        sa.Column("confirmed_flake_count", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("last_status", sa.String(length=16), nullable=False, server_default="passed"),
        sa.Column("last_seen_at", TS, nullable=False),
        sa.Column("quarantined", sa.Boolean(), nullable=False, server_default=sa.false()),
        sa.Column("quarantined_at", TS, nullable=True),
        sa.Column("github_issue_number", sa.Integer(), nullable=True),
        sa.UniqueConstraint("project_id", "fingerprint",
                            name="uq_test_cases_project_fingerprint"),
    )
    op.create_index("ix_test_cases_project_id", "test_cases", ["project_id"])
    op.create_index("ix_test_cases_project_score", "test_cases",
                    ["project_id", "flakiness_score"])

    op.create_table(
        "test_runs",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("project_id", sa.Integer(),
                  sa.ForeignKey("projects.id", ondelete="CASCADE"), nullable=False),
        sa.Column("commit_sha", sa.String(length=64), nullable=False),
        sa.Column("branch", sa.String(length=255), nullable=False),
        sa.Column("ci_run_id", sa.String(length=255), nullable=False, server_default=""),
        sa.Column("created_at", TS, nullable=False),
    )
    op.create_index("ix_test_runs_project_id", "test_runs", ["project_id"])
    op.create_index("ix_test_runs_commit_sha", "test_runs", ["commit_sha"])

    op.create_table(
        "test_executions",
        sa.Column("id", sa.BigInteger(), primary_key=True),
        sa.Column("test_case_id", sa.Integer(),
                  sa.ForeignKey("test_cases.id", ondelete="CASCADE"), nullable=False),
        sa.Column("test_run_id", sa.Integer(),
                  sa.ForeignKey("test_runs.id", ondelete="CASCADE"), nullable=False),
        sa.Column("status", sa.String(length=16), nullable=False),
        sa.Column("duration", sa.Float(), nullable=False, server_default="0"),
        sa.Column("message", sa.Text(), nullable=False, server_default=""),
        sa.Column("details", sa.Text(), nullable=False, server_default=""),
        sa.Column("created_at", TS, nullable=False),
    )
    op.create_index("ix_exec_case_id", "test_executions", ["test_case_id", "id"])
    op.create_index("ix_test_executions_test_run_id", "test_executions", ["test_run_id"])
    op.create_index("ix_test_executions_created_at", "test_executions", ["created_at"])

    op.create_table(
        "reports",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("project_id", sa.Integer(),
                  sa.ForeignKey("projects.id", ondelete="CASCADE"), nullable=False),
        sa.Column("commit_sha", sa.String(length=64), nullable=False),
        sa.Column("branch", sa.String(length=255), nullable=False),
        sa.Column("ci_run_id", sa.String(length=255), nullable=False, server_default=""),
        sa.Column("root", sa.String(length=1024), nullable=True),
        sa.Column("body", sa.LargeBinary(), nullable=False),
        sa.Column("status", sa.String(length=16), nullable=False, server_default="pending"),
        sa.Column("error", sa.Text(), nullable=True),
        sa.Column("counts", postgresql.JSONB(), nullable=True),
        sa.Column("run_id", sa.Integer(),
                  sa.ForeignKey("test_runs.id", ondelete="SET NULL"), nullable=True),
        sa.Column("created_at", TS, nullable=False),
        sa.Column("processed_at", TS, nullable=True),
    )
    op.create_index("ix_reports_status_id", "reports", ["status", "id"])
    op.create_index("ix_reports_project_id", "reports", ["project_id"])


def downgrade() -> None:
    op.drop_table("reports")
    op.drop_table("test_executions")
    op.drop_table("test_runs")
    op.drop_table("test_cases")
    op.drop_table("projects")
    op.drop_table("repos")
```

`backend/migrations/env.py`
```python
"""Alembic environment — async (asyncpg) and callable from a running loop.

Two entry points:
- The app lifespan (app/migrate.py) hands in a sync Connection via
  ``config.attributes["connection"]`` from ``AsyncConnection.run_sync``.
  asyncio.run() would crash there because a loop is already running.
- The CLI (``alembic upgrade head`` / ``alembic revision --autogenerate``)
  has no connection, so we build an async engine from Settings.
"""
import asyncio
from logging.config import fileConfig

from alembic import context
from sqlalchemy import pool
from sqlalchemy.engine import Connection
from sqlalchemy.ext.asyncio import create_async_engine

from app.config import get_settings
from app.models import Base

config = context.config
if config.config_file_name is not None:
    # Keep the app's own loggers alive (the default would disable them).
    fileConfig(config.config_file_name, disable_existing_loggers=False)

target_metadata = Base.metadata


def _url() -> str:
    return config.get_main_option("sqlalchemy.url") or get_settings().database_url


def run_migrations_offline() -> None:
    context.configure(
        url=_url(),
        target_metadata=target_metadata,
        literal_binds=True,
        dialect_opts={"paramstyle": "named"},
    )
    with context.begin_transaction():
        context.run_migrations()


def do_run_migrations(connection: Connection) -> None:
    context.configure(connection=connection, target_metadata=target_metadata)
    with context.begin_transaction():
        context.run_migrations()


async def run_async_migrations() -> None:
    engine = create_async_engine(_url(), poolclass=pool.NullPool)
    async with engine.connect() as connection:
        await connection.run_sync(do_run_migrations)
    await engine.dispose()


if context.is_offline_mode():
    run_migrations_offline()
else:
    connection = config.attributes.get("connection")
    if connection is not None:
        do_run_migrations(connection)
    else:
        asyncio.run(run_async_migrations())
```

`backend/app/config.py` (adds the `database_url` validator and the Postgres default. `github_repo` stays for now; task 07 removes it)
```python
"""Application configuration, sourced from environment variables (.env supported)."""
import os
from functools import lru_cache

from pydantic import field_validator
from pydantic_settings import BaseSettings, SettingsConfigDict

DEFAULT_INSECURE_TOKEN = "changeme"
DEFAULT_DATABASE_URL = "postgresql+asyncpg://flakeradar:flakeradar@localhost:5432/flakeradar"


class Settings(BaseSettings):
    model_config = SettingsConfigDict(
        env_file=".env", env_prefix="FLAKERADAR_", extra="ignore"
    )

    # Auth token CI systems send in the X-API-Key header (and MCP clients as a Bearer token).
    api_token: str = DEFAULT_INSECURE_TOKEN

    # PostgreSQL only (ADR 0003). postgresql:// and postgres:// are rewritten to asyncpg.
    database_url: str = DEFAULT_DATABASE_URL

    # Scoring parameters. window: how many recent executions to consider.
    # decay: geometric weight applied per step into the past (recent flips matter more).
    score_window: int = 50
    score_decay: float = 0.85

    # GitHub integration. Leave token/repo empty to disable (graceful no-op).
    github_token: str = ""
    github_repo: str = ""  # "owner/name"
    flake_threshold: float = 0.30

    cors_origins: str = "http://localhost:5173"

    @field_validator("database_url")
    @classmethod
    def _require_asyncpg(cls, value: str) -> str:
        for prefix in ("postgresql://", "postgres://"):
            if value.startswith(prefix):
                return "postgresql+asyncpg://" + value[len(prefix):]
        if not value.startswith("postgresql+asyncpg://"):
            raise ValueError(
                "FLAKERADAR_DATABASE_URL must be a PostgreSQL URL "
                "(postgresql+asyncpg://user:pass@host:5432/db)"
            )
        return value


@lru_cache
def get_settings() -> Settings:
    return Settings()


def assert_secure_token(token: str) -> None:
    """Refuse to boot on the shipped default token unless explicitly allowed.

    Tests and throwaway local demos set ``FLAKERADAR_ALLOW_INSECURE=1``.
    Docker/production must set a real ``FLAKERADAR_API_TOKEN``.
    """
    if token != DEFAULT_INSECURE_TOKEN:
        return
    if os.getenv("FLAKERADAR_ALLOW_INSECURE", "").strip() == "1":
        return
    raise RuntimeError(
        "FLAKERADAR_API_TOKEN is still the default 'changeme'. Set a real "
        "token (see .env.example) or set FLAKERADAR_ALLOW_INSECURE=1 for a "
        "throwaway local demo."
    )
```

`backend/app/db.py`
```python
"""Async database engine and session management (PostgreSQL + asyncpg)."""
from collections.abc import AsyncIterator

from sqlalchemy.ext.asyncio import (
    AsyncEngine, AsyncSession, async_sessionmaker, create_async_engine,
)

from .config import get_settings

# Postgres advisory-lock keys (any stable bigint; must not collide).
MIGRATION_LOCK_KEY = 726_300_001  # serializes startup migrations across uvicorn workers
WORKER_LOCK_KEY = 726_300_002     # elects the single Report processor (ADR 0002)


def make_engine(url: str | None = None) -> AsyncEngine:
    return create_async_engine(url or get_settings().database_url, pool_pre_ping=True)


def make_session_factory(engine: AsyncEngine) -> async_sessionmaker[AsyncSession]:
    return async_sessionmaker(engine, expire_on_commit=False)


# Creating the engine does not connect; the first query does.
engine: AsyncEngine = make_engine()
SessionLocal: async_sessionmaker[AsyncSession] = make_session_factory(engine)


async def get_db() -> AsyncIterator[AsyncSession]:
    async with SessionLocal() as session:
        yield session
```

`backend/app/migrate.py`
```python
"""Run Alembic migrations at startup, from inside the running event loop.

Several uvicorn workers start at once, so the upgrade runs under a
transaction-scoped advisory lock: the first worker migrates, the others wait
and then find nothing to do.
"""
from pathlib import Path

from alembic import command
from alembic.config import Config
from sqlalchemy import text
from sqlalchemy.engine import Connection
from sqlalchemy.ext.asyncio import AsyncEngine

from .db import MIGRATION_LOCK_KEY


def _alembic_config() -> Config:
    backend_root = Path(__file__).resolve().parents[1]  # backend/
    cfg = Config(str(backend_root / "alembic.ini"))
    cfg.set_main_option("script_location", str(backend_root / "migrations"))
    return cfg


def _upgrade(connection: Connection) -> None:
    cfg = _alembic_config()
    cfg.attributes["connection"] = connection  # see migrations/env.py
    command.upgrade(cfg, "head")


async def run_migrations(engine: AsyncEngine) -> None:
    async with engine.begin() as conn:
        await conn.execute(
            text("SELECT pg_advisory_xact_lock(:key)"), {"key": MIGRATION_LOCK_KEY}
        )
        await conn.run_sync(_upgrade)
```

`backend/app/auth.py` (moved out of `main.py` unchanged, so routers can import it)
```python
"""Shared-token auth for CI-facing endpoints."""
import secrets

from fastapi import Header, HTTPException

from .config import get_settings


def require_token(x_api_key: str = Header(default="")) -> None:
    expected = get_settings().api_token
    if not secrets.compare_digest(x_api_key.encode(), expected.encode()):
        raise HTTPException(status_code=401, detail="Invalid or missing X-API-Key")
```

`backend/app/main.py` (skeleton. Later tasks add `app.include_router(...)` lines in the marked spot, **above** the static mount)
```python
"""FastAPI application: lifespan, API routers, static frontend hosting."""
import logging
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.staticfiles import StaticFiles

from .config import DEFAULT_INSECURE_TOKEN, assert_secure_token, get_settings
from .db import engine
from .migrate import run_migrations

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("flakeradar")
# Alembic's fileConfig sets the root logger to WARNING at startup; pin our
# own level so flakeradar.* INFO logs (e.g. processor election) still show.
logger.setLevel(logging.INFO)


@asynccontextmanager
async def lifespan(app: FastAPI):
    settings = get_settings()
    assert_secure_token(settings.api_token)
    if settings.api_token == DEFAULT_INSECURE_TOKEN:
        logger.warning(
            "FLAKERADAR_API_TOKEN is the default 'changeme' — set a real token."
        )
    await run_migrations(engine)
    yield
    await engine.dispose()


app = FastAPI(title="FlakeRadar", version="2.0.0", lifespan=lifespan)

app.add_middleware(
    CORSMiddleware,
    allow_origins=get_settings().cors_origins.split(","),
    allow_methods=["*"],
    allow_headers=["*"],
)


@app.get("/api/health")
async def health() -> dict:
    return {"status": "ok"}


# --- API routers: include them HERE, above the static mount. -------------
# A Mount("/") registered earlier would swallow every later route.


# Serve the built frontend (frontend/dist) if present — single-container self-host.
_dist = Path(__file__).resolve().parents[2] / "frontend" / "dist"
if _dist.is_dir():
    app.mount("/", StaticFiles(directory=_dist, html=True), name="frontend")
```

`backend/pytest.ini`
```ini
[pytest]
pythonpath = .
testpaths = tests
asyncio_mode = auto
asyncio_default_fixture_loop_scope = session
asyncio_default_test_loop_scope = session
```

`backend/requirements.txt`
```text
fastapi>=0.115
uvicorn[standard]>=0.30
sqlalchemy[asyncio]>=2.0.36
asyncpg>=0.30
pydantic-settings>=2.5
junitparser>=5.0
httpx>=0.27
python-multipart>=0.0.9
alembic>=1.13
pytest>=8.3
pytest-asyncio>=1.0
testcontainers[postgres]>=4.10
```

`backend/tests/conftest.py`
```python
"""Shared fixtures: one Postgres per test session, clean tables per test.

Docker must be running (testcontainers). Set FLAKERADAR_TEST_DATABASE_URL to
use an existing Postgres instead; its tables are truncated between tests.
"""
import os
from collections.abc import AsyncIterator, Iterator

os.environ.setdefault("FLAKERADAR_ALLOW_INSECURE", "1")

import httpx
import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import AsyncEngine, AsyncSession, async_sessionmaker

from app.db import get_db, make_engine, make_session_factory
from app.main import app
from app.migrate import run_migrations

TOKEN = "changeme"  # default Settings token; tests send it explicitly
AUTH = {"X-API-Key": TOKEN}

# Every app table, children first. RESTART IDENTITY makes ids start at 1.
_TABLES = "reports, test_executions, test_runs, test_cases, projects, repos"


@pytest.fixture(scope="session")
def database_url() -> Iterator[str]:
    override = os.environ.get("FLAKERADAR_TEST_DATABASE_URL")
    if override:
        yield override
        return
    from testcontainers.community.postgres import PostgresContainer

    with PostgresContainer("postgres:17-alpine", driver="asyncpg") as pg:
        yield pg.get_connection_url()


@pytest.fixture(scope="session")
async def engine(database_url: str) -> AsyncIterator[AsyncEngine]:
    eng = make_engine(database_url)
    await run_migrations(eng)
    yield eng
    await eng.dispose()


@pytest.fixture()
async def session_factory(engine: AsyncEngine) -> AsyncIterator[async_sessionmaker[AsyncSession]]:
    async with engine.begin() as conn:
        await conn.execute(text(f"TRUNCATE {_TABLES} RESTART IDENTITY CASCADE"))
    yield make_session_factory(engine)


@pytest.fixture()
async def db(session_factory: async_sessionmaker[AsyncSession]) -> AsyncIterator[AsyncSession]:
    async with session_factory() as session:
        yield session


@pytest.fixture()
async def client(session_factory: async_sessionmaker[AsyncSession]) -> AsyncIterator[httpx.AsyncClient]:
    async def override_get_db() -> AsyncIterator[AsyncSession]:
        async with session_factory() as session:
            yield session

    app.dependency_overrides[get_db] = override_get_db
    # ASGITransport does not run the lifespan: no startup migrations, no worker.
    transport = httpx.ASGITransport(app=app)
    async with httpx.AsyncClient(transport=transport, base_url="http://test") as c:
        yield c
    app.dependency_overrides.clear()


def make_junit(
    cases: list[tuple[str, str]],
    suite: str = "unit",
    classname: str = "tests.test_mod",
) -> bytes:
    """Build a minimal JUnit XML report.

    `cases` is a list of (test_name, status) where status is
    passed | failed | error | skipped.
    """
    inner = ""
    for name, status in cases:
        body = ""
        if status == "failed":
            body = '<failure message="assert 1 == 2">trace</failure>'
        elif status == "error":
            body = '<error message="boom">trace</error>'
        elif status == "skipped":
            body = '<skipped message="not on windows"/>'
        inner += f'<testcase classname="{classname}" name="{name}" time="0.01">{body}</testcase>'
    xml = f'<testsuites><testsuite name="{suite}" tests="{len(cases)}">{inner}</testsuite></testsuites>'
    return xml.encode()
```

`backend/tests/factories.py` (note: `make_test_case` imports `app.parsing.fingerprint`, which task 02 creates. Until then, do not call `make_test_case` in tests)
```python
"""Async seed helpers: build rows directly, bypassing ingest.

Each helper flushes (so ids are assigned) but does not commit; call
``await db.commit()`` when another session must see the rows.
"""
from datetime import datetime

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models import (
    REPORT_PENDING, Project, Repo, Report, TestCase, TestExecution, TestRun, utcnow,
)


async def make_project(
    db: AsyncSession, repo: str = "acme/app", project: str = "default", root: str = ""
) -> Project:
    repo_row = (await db.execute(select(Repo).where(Repo.name == repo))).scalar_one_or_none()
    if repo_row is None:
        repo_row = Repo(name=repo)
        db.add(repo_row)
        await db.flush()
    proj = Project(repo_id=repo_row.id, name=project, root=root)
    db.add(proj)
    await db.flush()
    return proj


async def make_test_case(
    db: AsyncSession,
    project: Project,
    name: str = "t1",
    classname: str = "tests.test_mod",
    suite: str = "unit",
    **fields,
) -> TestCase:
    from app.parsing import fingerprint  # created in task 02

    tc = TestCase(
        project_id=project.id,
        fingerprint=fingerprint(suite, classname, name),
        suite=suite, classname=classname, name=name,
        **fields,
    )
    db.add(tc)
    await db.flush()
    return tc


async def make_run(
    db: AsyncSession,
    project: Project,
    commit_sha: str = "sha1",
    branch: str = "main",
    ci_run_id: str = "",
    created_at: datetime | None = None,
) -> TestRun:
    run = TestRun(project_id=project.id, commit_sha=commit_sha, branch=branch,
                  ci_run_id=ci_run_id, created_at=created_at or utcnow())
    db.add(run)
    await db.flush()
    return run


async def make_execution(
    db: AsyncSession,
    test_case: TestCase,
    run: TestRun,
    status: str = "passed",
    message: str = "",
    details: str = "",
    created_at: datetime | None = None,
) -> TestExecution:
    ex = TestExecution(test_case_id=test_case.id, test_run_id=run.id, status=status,
                       message=message, details=details,
                       created_at=created_at or utcnow())
    db.add(ex)
    await db.flush()
    return ex


async def make_report(
    db: AsyncSession,
    project: Project,
    body: bytes,
    commit_sha: str = "sha1",
    branch: str = "main",
    ci_run_id: str = "",
    root: str | None = None,
    status: str = REPORT_PENDING,
) -> Report:
    rep = Report(project_id=project.id, commit_sha=commit_sha, branch=branch,
                 ci_run_id=ci_run_id, root=root, body=body, status=status)
    db.add(rep)
    await db.flush()
    return rep
```

`backend/Dockerfile`
```dockerfile
FROM node:22-slim AS frontend
WORKDIR /build
COPY frontend/package.json frontend/package-lock.json* ./
RUN npm install
COPY frontend/ ./
RUN npm run build

FROM python:3.12-slim
# Mirror the repo layout: main.py resolves ../../frontend/dist from app/.
WORKDIR /srv/backend
COPY backend/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY backend/alembic.ini ./
COPY backend/migrations ./migrations
COPY backend/app ./app
COPY --from=frontend /build/dist /srv/frontend/dist
EXPOSE 8000
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000"]
```
(Task 05 adds `--workers 2`. Keep a single worker here.)

`docker-compose.yml`
```yaml
services:
  db:
    image: postgres:17-alpine
    environment:
      POSTGRES_USER: flakeradar
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:-flakeradar}
      POSTGRES_DB: flakeradar
    volumes:
      - flakeradar-pg:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U flakeradar -d flakeradar"]
      interval: 5s
      timeout: 3s
      retries: 10
    restart: unless-stopped

  flakeradar:
    build:
      context: .
      dockerfile: backend/Dockerfile
    ports:
      - "127.0.0.1:8000:8000"
    env_file: .env
    environment:
      FLAKERADAR_DATABASE_URL: postgresql+asyncpg://flakeradar:${POSTGRES_PASSWORD:-flakeradar}@db:5432/flakeradar
    depends_on:
      db:
        condition: service_healthy
    restart: unless-stopped

volumes:
  flakeradar-pg:
```

`.env.example` (replace the whole file)
```bash
# --- Required ---
# Token CI systems must send in the X-API-Key header. Generate one:
#   python -c "import secrets; print(secrets.token_urlsafe(32))"
FLAKERADAR_API_TOKEN=changeme
# Docker refuses to start on 'changeme' unless you also set:
# FLAKERADAR_ALLOW_INSECURE=1

# --- Database (PostgreSQL only) ---
# docker compose builds this URL itself from POSTGRES_PASSWORD; set
# FLAKERADAR_DATABASE_URL only when running the backend outside compose.
POSTGRES_PASSWORD=flakeradar
# FLAKERADAR_DATABASE_URL=postgresql+asyncpg://flakeradar:flakeradar@localhost:5432/flakeradar

# --- Optional: GitHub issue automation (leave blank to disable) ---
FLAKERADAR_GITHUB_TOKEN=
FLAKERADAR_GITHUB_REPO=

# --- Tuning (defaults shown) ---
FLAKERADAR_FLAKE_THRESHOLD=0.30
FLAKERADAR_SCORE_WINDOW=50
FLAKERADAR_SCORE_DECAY=0.85
FLAKERADAR_CORS_ORIGINS=http://localhost:5173
```

- Verified external contracts: all of the code above was executed against SQLAlchemy 2.1.0, asyncpg 0.31.0, alembic 1.20.0, pytest-asyncio 1.4.0 and testcontainers 4.15.0 with `postgres:17-alpine` (22 tests passed).
- Behavior rules:
  - `run_migrations` takes `pg_advisory_xact_lock(726300001)` before upgrading, so concurrent uvicorn workers don't race to create tables.
  - `Settings.database_url` accepts `postgresql://…` and `postgres://…` and rewrites them to `postgresql+asyncpg://…`. Any other scheme (e.g. `sqlite:///…`) raises a validation error.
  - Timestamps are `timestamptz`. The `UTCDateTime` TypeDecorator is deleted.
  - `__test__ = False` on `TestCase`, `TestRun` and `TestExecution` stops pytest from trying to collect them.
- Logging: `alembic.ini`'s `[logger_root] level = WARNING` is applied by `fileConfig` during startup migrations and would hide every `flakeradar.*` INFO log. `main.py` therefore pins `logging.getLogger("flakeradar").setLevel(logging.INFO)` (verified in a live 2-worker smoke run).
- Error and security rules: `require_token` keeps its constant-time compare and its 401 text `Invalid or missing X-API-Key`. Do not log `database_url` (it contains the password).

## Acceptance Criteria
- [ ] `alembic_version` holds `0001` after `run_migrations`, and a second `run_migrations` call is a no-op.
- [ ] Tables `repos`, `projects`, `test_cases`, `test_runs`, `test_executions` and `reports` exist with the constraint names `uq_projects_repo_name` and `uq_test_cases_project_fingerprint`.
- [ ] Inserting a second `Project` with the same (`repo_id`, `name`) raises `IntegrityError`. The same name under a different Repo succeeds.
- [ ] `GET /api/health` → `200 {"status": "ok"}` through the `client` fixture.
- [ ] `Settings(database_url="postgresql://u:p@h:5432/d").database_url == "postgresql+asyncpg://u:p@h:5432/d"`.
- [ ] `backend/app/ingest.py` and the six deleted test files are gone. `tests/test_scoring.py` still passes.
- [ ] `test_models_match_migrations` passes: Alembic autogenerate finds no difference between `models.py` and the baseline migration.
- [ ] `docker compose config` succeeds.

## Test Expectations
- Framework: pytest + pytest-asyncio (auto mode), with a Postgres container from testcontainers (Docker must be running).
- Create `backend/tests/test_db.py` with exactly this content (verified passing):
```python
"""Foundation: migrations build the schema; the app boots its health route."""
import pytest
from sqlalchemy import inspect, select, text
from sqlalchemy.exc import IntegrityError

from app.config import Settings
from app.migrate import run_migrations
from app.models import Report, TestCase
from tests.factories import make_project, make_report


async def test_migrations_create_every_table(engine):
    async with engine.connect() as conn:
        tables = set(await conn.run_sync(lambda c: inspect(c).get_table_names()))
    assert {"repos", "projects", "test_cases", "test_runs",
            "test_executions", "reports", "alembic_version"} <= tables


async def test_models_match_migrations(engine):
    """Autogenerate finds nothing: models.py and the migrations agree."""
    from alembic.autogenerate import compare_metadata
    from alembic.migration import MigrationContext

    from app.models import Base

    def diff(sync_conn):
        return compare_metadata(MigrationContext.configure(sync_conn), Base.metadata)

    async with engine.connect() as conn:
        assert await conn.run_sync(diff) == []


async def test_migrations_are_idempotent(engine):
    await run_migrations(engine)  # second run is a no-op, must not raise
    async with engine.connect() as conn:
        version = (await conn.execute(text("SELECT version_num FROM alembic_version"))).scalar()
    assert version == "0001"


async def test_project_name_unique_within_repo(db):
    await make_project(db, "acme/app", "backend")
    await make_project(db, "other/app", "backend")  # same name, other repo: fine
    with pytest.raises(IntegrityError):
        await make_project(db, "acme/app", "backend")


async def test_test_case_unique_per_project(db):
    proj = await make_project(db)
    db.add(TestCase(project_id=proj.id, fingerprint="f" * 40, name="t"))
    await db.flush()
    db.add(TestCase(project_id=proj.id, fingerprint="f" * 40, name="t"))
    with pytest.raises(IntegrityError):
        await db.flush()


async def test_report_defaults_and_utc_timestamps(db):
    proj = await make_project(db)
    rep = await make_report(db, proj, b"<testsuites/>")
    await db.commit()
    row = (await db.execute(select(Report).where(Report.id == rep.id))).scalar_one()
    assert row.status == "pending"
    assert row.created_at.utcoffset().total_seconds() == 0


async def test_health(client):
    resp = await client.get("/api/health")
    assert resp.json() == {"status": "ok"}


def test_database_url_rewritten_to_asyncpg():
    s = Settings(database_url="postgresql://u:p@h:5432/d")
    assert s.database_url == "postgresql+asyncpg://u:p@h:5432/d"
    with pytest.raises(ValueError):
        Settings(database_url="sqlite:///./x.db")
```

## Dependencies
- Blocked by: None
- Why blocked: N/A
- Blocks: every later task (02 uses `factories.py`; 03–10 use the schema, `db.py`, `auth.py` and the fixtures)

## Labels
`chore`, `backend`, `database`, `priority:high`

## Estimate
Large

## Risk
4 - Touches every file that talks to the DB and deletes most tests. It is contained by the integration branch, and the code is pre-verified.

## Validator Stopping Point
```bash
cd backend
python3.12 -m venv .venv && .venv/bin/pip install -r requirements.txt
.venv/bin/python -m pytest -q tests/test_db.py tests/test_scoring.py   # expect: 22 passed
cd .. && docker compose config >/dev/null && echo compose-ok
```
Expected to be broken after this task (fixed later): `npm run build` still passes, but the running app serves only `/api/health`, so the UI shows its error banner.
