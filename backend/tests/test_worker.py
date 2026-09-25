"""ReportWorker: processes the queue; exactly one leader; clean hand-over."""
import asyncio

from sqlalchemy import select, text

from app.db import WORKER_LOCK_KEY
from app.models import Report
from app.worker import ReportWorker
from tests.conftest import make_junit
from tests.factories import make_project, make_report


async def _wait_for(predicate, timeout: float = 10.0) -> None:
    deadline = asyncio.get_running_loop().time() + timeout
    while not await predicate():
        assert asyncio.get_running_loop().time() < deadline, "timed out"
        await asyncio.sleep(0.05)


def _worker(engine, session_factory, **kw) -> ReportWorker:
    return ReportWorker(engine, session_factory, poll_seconds=0.05,
                        standby_seconds=0.1, **kw)


async def test_worker_processes_pending_reports(engine, session_factory, db):
    proj = await make_project(db)
    for sha in ("a", "b"):
        await make_report(db, proj, make_junit([("t", "passed")]), commit_sha=sha)
    await db.commit()
    seen = []

    async def on_processed(outcome):
        seen.append(outcome.report_id)

    worker = _worker(engine, session_factory, on_processed=on_processed)
    await worker.start()
    try:
        async def all_done():
            async with session_factory() as s:
                statuses = (await s.execute(select(Report.status))).scalars().all()
            return statuses == ["processed", "processed"]
        await _wait_for(all_done)
    finally:
        await worker.stop()
    assert sorted(seen) == [1, 2]


async def test_single_leader_and_takeover(engine, session_factory):
    first = _worker(engine, session_factory)
    second = _worker(engine, session_factory)
    await first.start()

    async def first_leads():
        return first.is_leader
    await _wait_for(first_leads)
    await second.start()
    await asyncio.sleep(0.3)
    assert first.is_leader and not second.is_leader

    await first.stop()

    async def second_leads():
        return second.is_leader
    await _wait_for(second_leads)
    await second.stop()


async def test_stop_releases_the_lock(engine, session_factory):
    worker = _worker(engine, session_factory)
    await worker.start()

    async def leads():
        return worker.is_leader
    await _wait_for(leads)
    await worker.stop()
    async with engine.connect() as conn:
        got = (await conn.execute(text("SELECT pg_try_advisory_lock(:k)"),
                                  {"k": WORKER_LOCK_KEY})).scalar()
        assert got is True
        await conn.execute(text("SELECT pg_advisory_unlock(:k)"), {"k": WORKER_LOCK_KEY})
