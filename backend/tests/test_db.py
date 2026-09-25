"""Foundation: migrations build the schema; the app boots its health route."""

import pytest
from app.config import Settings
from app.migrate import run_migrations
from app.models import Report, TestCase
from sqlalchemy import inspect, select, text
from sqlalchemy.exc import IntegrityError

from tests.factories import make_project, make_report


async def test_migrations_create_every_table(engine):
    async with engine.connect() as conn:
        tables = set(await conn.run_sync(lambda c: inspect(c).get_table_names()))
    assert {"repos", "projects", "test_cases", "test_runs", "test_executions", "reports", "alembic_version"} <= tables


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
