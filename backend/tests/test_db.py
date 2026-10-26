"""Foundation: migrations build the schema; the app boots its health route."""

import importlib.util
from pathlib import Path

import pytest
from alembic import command
from app import db as db_module
from app.config import Settings
from app.migrate import _alembic_config, run_migrations
from app.models import Job, JobExecution, Report, TestCase, TestExecution
from sqlalchemy import inspect, select, text
from sqlalchemy.exc import IntegrityError

from tests.factories import (
    make_execution,
    make_job,
    make_job_execution,
    make_pipeline,
    make_project,
    make_report,
    make_run,
    make_test_case,
)


def _migration(name: str):
    path = Path(__file__).resolve().parents[1] / "migrations" / "versions" / f"{name}.py"
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


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
    assert version == "0015"


def test_make_engine_sets_command_timeout(monkeypatch):
    """The asyncpg command_timeout is forwarded as a connect argument."""
    captured = {}

    def fake_create_engine(url, **kwargs):
        captured["url"] = url
        captured["kwargs"] = kwargs
        raise AssertionError("stop before creating a real engine")

    monkeypatch.setattr(db_module, "create_async_engine", fake_create_engine)
    with pytest.raises(AssertionError):
        db_module.make_engine("postgresql+asyncpg://u:p@h:5432/d")
    assert captured["kwargs"]["connect_args"] == {"command_timeout": 30.0}


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


async def test_upgrade_marks_nextest_flaky_marker_passed(db):
    """0009 turns stored nextest flaky markers that follow a failed attempt into passes."""
    marker = "test passed on attempt 2/2 but is configured to fail when flaky"
    proj = await make_project(db)
    tc = await make_test_case(db, proj, name="t")
    other = await make_test_case(db, proj, name="u")
    run = await make_run(db, proj)

    failed_first = await make_execution(
        db, tc, run, status="failed", message="panicked", attempt=0, failure_category="assertion"
    )
    marker_after_failure = await make_execution(
        db, tc, run, status="failed", message=marker, details="d", attempt=1, failure_category="other"
    )
    marker_at_zero = await make_execution(db, other, run, status="failed", message=marker, attempt=0)
    marker_after_zero = await make_execution(db, other, run, status="failed", message=marker, attempt=1)
    # A second Run: the try before the marker passed, so the marker stays failed.
    run2 = await make_run(db, proj, commit_sha="sha2")
    await make_execution(db, tc, run2, status="passed", attempt=0)
    marker_after_pass = await make_execution(db, tc, run2, status="failed", message=marker, attempt=1)
    partial_marker = await make_execution(db, other, run2, status="failed", message=marker + " and more", attempt=1)
    await make_execution(db, other, run2, status="failed", message="x", attempt=0)

    await db.commit()
    await db.execute(text(_migration("0009_nextest_flaky_marker").BACKFILL_SQL))
    await db.commit()

    # Column select, so nothing stale comes back from the identity map.
    by_id = {
        row.id: (row.status, row.message, row.details, row.failure_category)
        for row in (
            await db.execute(
                select(
                    TestExecution.id,
                    TestExecution.status,
                    TestExecution.message,
                    TestExecution.details,
                    TestExecution.failure_category,
                )
            )
        ).all()
    }
    assert by_id[failed_first.id] == ("failed", "panicked", "", "assertion")
    assert by_id[marker_after_failure.id] == ("passed", "", "", None)
    assert by_id[marker_at_zero.id] == ("failed", marker, "", None)
    assert by_id[marker_after_zero.id] == ("passed", "", "", None)
    assert by_id[marker_after_pass.id] == ("failed", marker, "", None)
    assert by_id[partial_marker.id] == ("failed", marker + " and more", "", None)


async def test_upgrade_leaves_stored_issue_numbers_unmarked(db):
    """0015 backfills nothing: a number stored before the marker existed keeps
    NULL, which the admin delete reads as linked and never auto-closes.

    The human-link endpoints (issue #11) predate the migration, so a row
    upgrading from 0014 can hold either kind of link and the two are
    indistinguishable after the fact.
    """
    proj = await make_project(db)
    linked = await make_test_case(db, proj, name="t", github_issue_number=123)
    pipe = await make_pipeline(db)
    job = await make_job(db, pipe, name="e2e", github_issue_number=456)
    test_id, job_id = linked.id, job.id
    await db.commit()

    # Replay the real 0015 over the pre-existing rows: back to a 0014 database
    # (the flag columns gone), then upgrade to head.
    def replay(conn) -> None:
        cfg = _alembic_config()
        cfg.attributes["connection"] = conn  # see migrations/env.py
        command.downgrade(cfg, "0014")
        command.upgrade(cfg, "head")

    async with db.bind.connect() as conn:
        await conn.run_sync(replay)

    # Column select, so nothing stale comes back from the identity map.
    test_flags = dict((await db.execute(select(TestCase.id, TestCase.github_issue_filed))).all())
    job_flags = dict((await db.execute(select(Job.id, Job.github_issue_filed))).all())
    assert test_flags[test_id] is None  # pre-existing number -> linked, never auto-closed
    assert job_flags[job_id] is None


def test_database_url_rewritten_to_asyncpg():
    s = Settings(database_url="postgresql://u:p@h:5432/d")
    assert s.database_url == "postgresql+asyncpg://u:p@h:5432/d"
    with pytest.raises(ValueError):
        Settings(database_url="sqlite:///./x.db")
