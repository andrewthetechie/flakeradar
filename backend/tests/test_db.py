"""Foundation: migrations build the schema; the app boots its health route."""

import pytest
from app.config import Settings
from app.migrate import run_migrations
from app.models import Job, JobExecution, Report, TestCase
from sqlalchemy import inspect, select, text
from sqlalchemy.exc import IntegrityError

from tests.factories import make_job, make_job_execution, make_pipeline, make_project, make_report


async def test_migrations_create_every_table(engine):
    async with engine.connect() as conn:
        tables = set(await conn.run_sync(lambda c: inspect(c).get_table_names()))
    assert {
        "repos",
        "projects",
        "test_cases",
        "test_runs",
        "test_executions",
        "reports",
        "pipelines",
        "jobs",
        "job_executions",
        "alembic_version",
    } <= tables


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
    assert version == "0002"


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


async def test_junit_report_requires_project(db):
    proj = await make_project(db)
    db.add(Report(kind="junit", repo_id=proj.repo_id, body=b"<testsuites/>"))
    with pytest.raises(IntegrityError):
        await db.flush()


async def test_pipeline_report_allows_no_project(db):
    proj = await make_project(db)
    db.add(Report(kind="pipeline", repo_id=proj.repo_id, body=b"{}", commit_sha="sha1"))
    await db.flush()  # no project_id is fine for a pipeline report


async def test_job_unique_per_pipeline(db):
    pipe = await make_pipeline(db)
    await make_job(db, pipe, name="test")
    db.add(Job(pipeline_id=pipe.id, name="test"))
    with pytest.raises(IntegrityError):
        await db.flush()


async def test_job_execution_unique_per_ci_job_id(db):
    pipe = await make_pipeline(db)
    job = await make_job(db, pipe)
    await make_job_execution(db, job, ci_job_id="42")
    db.add(
        JobExecution(
            job_id=job.id,
            ci_job_id="42",
            commit_sha="sha1",
            branch="main",
            status="passed",
        )
    )
    with pytest.raises(IntegrityError):
        await db.flush()


async def test_upgrade_backfills_report_repo_id(db):
    """The 0002 backfill must give every pre-existing Report its Project's Repo.

    The shared session engine is already at head, so repo_id is NOT NULL; we
    temporarily relax that to simulate the pre-0002 state, run the exact
    backfill SQL, then restore. See the task-01 commit message.
    """
    proj = await make_project(db, repo="acme/app", project="x")
    rep = await make_report(db, proj, b"<testsuites/>")
    await db.commit()
    try:
        # Simulate the pre-0002 schema: repo_id may be NULL on existing reports.
        await db.execute(text("ALTER TABLE reports ALTER COLUMN repo_id DROP NOT NULL"))
        await db.execute(text("UPDATE reports SET repo_id = NULL"))
        await db.execute(
            text("UPDATE reports SET repo_id = projects.repo_id FROM projects WHERE projects.id = reports.project_id")
        )
    finally:
        await db.execute(text("ALTER TABLE reports ALTER COLUMN repo_id SET NOT NULL"))
    await db.commit()
    row = (await db.execute(select(Report).where(Report.id == rep.id))).scalar_one()
    assert row.repo_id == proj.repo_id
    assert row.kind == "junit"


def test_database_url_rewritten_to_asyncpg():
    s = Settings(database_url="postgresql://u:p@h:5432/d")
    assert s.database_url == "postgresql+asyncpg://u:p@h:5432/d"
    with pytest.raises(ValueError):
        Settings(database_url="sqlite:///./x.db")
