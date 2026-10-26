"""GET /api/health/ready and GET /metrics (issue #41).

No request here sends auth headers: both reads are unauthenticated, like the
other read APIs. The client fixture runs no lifespan, so no worker exists and
processor_leader is false unless this test holds the lock itself.
"""

from app.db import WORKER_LOCK_KEY, get_db
from app.main import app
from app.models import REPORT_FAILED, REPORT_PROCESSED
from sqlalchemy import event, text
from sqlalchemy.exc import OperationalError

from tests.factories import make_job, make_pipeline, make_project, make_report, make_test_case

OLDEST = "flakeradar_oldest_pending_report_age_seconds"
LAST_PROCESSED = "flakeradar_last_report_processed_timestamp_seconds"


class _Broken:
    """Stands in for a session whose database is unreachable."""

    async def execute(self, *a, **kw):
        raise OperationalError("SELECT 1", {}, Exception("boom"))


async def _broken_db():
    yield _Broken()


def _samples(body: str) -> dict[str, str]:
    """Every sample line of a Prometheus body, keyed by name plus labels."""
    out = {}
    for line in body.splitlines():
        if line and not line.startswith("#"):
            key, _, value = line.rpartition(" ")
            out[key] = value
    return out


def _assert_series(body: str, expected: dict[str, str | None]) -> dict[str, str]:
    """Assert the scrape exposes exactly `expected` series, with these values.

    A value of None checks presence only, for the float-valued samples.
    """
    samples = _samples(body)
    assert set(samples) == set(expected)
    for key, value in expected.items():
        if value is not None:
            assert samples[key] == value, key
    return samples


async def _seed_pending_reports(db, count: int):
    """Seed `count` pending Reports, the oldest of them 90 seconds old."""
    proj = await make_project(db, "acme/app", "backend")
    reports = [await make_report(db, proj, b"<x/>", commit_sha=f"sha{i}") for i in range(count)]
    await db.commit()
    await db.execute(
        text("UPDATE reports SET created_at = now() - interval '90 seconds' WHERE id = :i"),
        {"i": reports[0].id},
    )
    await db.commit()
    return proj


async def test_ready_healthy(client, db):
    await _seed_pending_reports(db, 2)
    resp = await client.get("/api/health/ready")
    assert resp.status_code == 200
    body = resp.json()
    assert body["database"] is True
    assert body["processor_leader"] is False
    assert body["pending_reports"] == 2
    assert body["oldest_pending_age_seconds"] >= 90


async def test_ready_reports_leader_lock(client, engine):
    async with engine.connect() as conn:
        got = (await conn.execute(text("SELECT pg_try_advisory_lock(:k)"), {"k": WORKER_LOCK_KEY})).scalar()
        assert got is True
        resp = await client.get("/api/health/ready")
        assert resp.status_code == 200
        assert resp.json()["processor_leader"] is True
        await conn.execute(text("SELECT pg_advisory_unlock(:k)"), {"k": WORKER_LOCK_KEY})
    # A missing leader is reported, never a 503: during a redeploy the lock is
    # briefly held by nobody.
    resp = await client.get("/api/health/ready")
    assert resp.status_code == 200
    assert resp.json()["processor_leader"] is False


async def test_ready_503_when_database_fails(client):
    app.dependency_overrides[get_db] = _broken_db
    resp = await client.get("/api/health/ready")
    assert resp.status_code == 503
    assert resp.json() == {"database": False}


async def test_metrics_names_and_labels(client, db):
    proj = await _seed_pending_reports(db, 1)
    # One Report per status; the processed one carries processed_at.
    await make_report(db, proj, b"<x/>", commit_sha="failed", status=REPORT_FAILED)
    await make_report(db, proj, b"<x/>", commit_sha="done", status=REPORT_PROCESSED)
    await db.commit()
    await db.execute(text("UPDATE reports SET processed_at = now() WHERE status = 'processed'"))
    # Cached tiers: at or above the 0.30 threshold is flaky, above 0 suspect, 0 stable.
    await make_test_case(db, proj, name="t_flaky", flakiness_score=0.5)
    await make_test_case(db, proj, name="t_suspect", flakiness_score=0.1)
    await make_test_case(db, proj, name="t_stable", flakiness_score=0.0)
    pipe = await make_pipeline(db)
    await make_job(db, pipe, name="j_flaky", flakiness_score=0.4)
    await make_job(db, pipe, name="j_stable", flakiness_score=0.0)
    await db.commit()

    resp = await client.get("/metrics")
    assert resp.status_code == 200
    assert resp.headers["content-type"] == "text/plain; version=0.0.4; charset=utf-8"

    samples = _assert_series(
        resp.text,
        {
            'flakeradar_reports{status="pending"}': "1",
            'flakeradar_reports{status="processed"}': "1",
            'flakeradar_reports{status="failed"}': "1",
            OLDEST: None,
            "flakeradar_processor_leader": "0",
            'flakeradar_tests{tier="flaky"}': "1",
            'flakeradar_tests{tier="suspect"}': "1",
            'flakeradar_tests{tier="stable"}': "1",
            'flakeradar_jobs{tier="flaky"}': "1",
            'flakeradar_jobs{tier="suspect"}': "0",
            'flakeradar_jobs{tier="stable"}': "1",
            LAST_PROCESSED: None,
        },
    )
    assert float(samples[OLDEST]) >= 90
    assert float(samples[LAST_PROCESSED]) > 0
    # Every family declares its type exactly once.
    for metric in ("flakeradar_reports", OLDEST, "flakeradar_tests", "flakeradar_jobs", LAST_PROCESSED):
        assert resp.text.count(f"# TYPE {metric} gauge") == 1


async def test_metrics_zero_series_and_absent_timestamp(client, db):
    resp = await client.get("/metrics")
    assert resp.status_code == 200
    samples = _assert_series(
        resp.text,
        {
            'flakeradar_reports{status="pending"}': "0",
            'flakeradar_reports{status="processed"}': "0",
            'flakeradar_reports{status="failed"}': "0",
            OLDEST: "0.0",
            "flakeradar_processor_leader": "0",
            'flakeradar_tests{tier="flaky"}': "0",
            'flakeradar_tests{tier="suspect"}': "0",
            'flakeradar_tests{tier="stable"}': "0",
            'flakeradar_jobs{tier="flaky"}': "0",
            'flakeradar_jobs{tier="suspect"}': "0",
            'flakeradar_jobs{tier="stable"}': "0",
        },
    )
    # A 0 would falsely mean the epoch, so the sample line is absent until the
    # first Report is processed; its HELP/TYPE header stays, keeping the scrape
    # stable.
    assert LAST_PROCESSED not in samples
    assert f"# TYPE {LAST_PROCESSED} gauge" in resp.text


async def test_metrics_503_when_database_fails(client):
    app.dependency_overrides[get_db] = _broken_db
    resp = await client.get("/metrics")
    assert resp.status_code == 503
    assert resp.text == ""


async def test_ops_queries_never_touch_test_executions(client, db, engine):
    """The ready and metrics queries stay on their own small tables."""
    statements: list[str] = []

    def _capture(_conn, _cursor, statement, _params, _context, _executemany):
        statements.append(statement)

    event.listen(engine.sync_engine, "before_cursor_execute", _capture)
    try:
        assert (await client.get("/api/health/ready")).status_code == 200
        assert (await client.get("/metrics")).status_code == 200
    finally:
        event.remove(engine.sync_engine, "before_cursor_execute", _capture)
    assert statements
    sql = " ".join(statements).lower()
    # Sanity check: the capture really saw the ops queries themselves.
    assert "pg_locks" in sql
    assert "test_executions" not in sql
    assert "test_runs" not in sql
    assert "job_executions" not in sql
