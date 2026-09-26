"""Shared fixtures: one Postgres per test session, clean tables per test.

Docker must be running (testcontainers). Set FLAKERADAR_TEST_DATABASE_URL to
use an existing Postgres instead; its tables are truncated between tests.
"""

import os
from collections.abc import AsyncIterator, Iterator

os.environ.setdefault("FLAKERADAR_ALLOW_INSECURE", "1")

import httpx
import pytest
from app.db import get_db, make_engine, make_session_factory
from app.main import app
from app.migrate import run_migrations
from sqlalchemy import text
from sqlalchemy.ext.asyncio import AsyncEngine, AsyncSession, async_sessionmaker

TOKEN = "changeme"  # default Settings token; tests send it explicitly
AUTH = {"X-API-Key": TOKEN}

# Every app table, children first. RESTART IDENTITY makes ids start at 1.
_TABLES = "reports, job_executions, jobs, pipelines, test_score_history, test_executions, test_runs, test_cases, projects, repos"


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
