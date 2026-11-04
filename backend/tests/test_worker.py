"""ReportWorker: processes the queue; exactly one leader; clean hand-over."""

import asyncio
import logging

import httpx
import pytest
from app.db import WORKER_LOCK_KEY
from app.models import Report
from app.processing import ProcessOutcome
from app.worker import GitHubWorker, ReportWorker, WebhookWorker
from sqlalchemy import select, text
from sqlalchemy.exc import DBAPIError

from tests.conftest import make_junit
from tests.factories import _notify_settings, make_project, make_report


async def _wait_for(predicate, timeout: float = 10.0) -> None:
    deadline = asyncio.get_running_loop().time() + timeout
    while not await predicate():
        assert asyncio.get_running_loop().time() < deadline, "timed out"
        await asyncio.sleep(0.05)


def _worker(engine, session_factory, **kw) -> ReportWorker:
    kw.setdefault("poll_seconds", 0.05)
    kw.setdefault("standby_seconds", 0.1)
    return ReportWorker(engine, session_factory, **kw)


async def _is_processed(session_factory, report_id: int) -> bool:
    async with session_factory() as session:
        report = await session.get(Report, report_id)
        return report.status == "processed"


async def _lock_is_free(engine) -> bool:
    """True when another session can take the worker lock right now."""
    async with engine.connect() as conn:
        got = (await conn.execute(text("SELECT pg_try_advisory_lock(:k)"), {"k": WORKER_LOCK_KEY})).scalar()
        if got:
            await conn.execute(text("SELECT pg_advisory_unlock(:k)"), {"k": WORKER_LOCK_KEY})
    return bool(got)


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
        got = (await conn.execute(text("SELECT pg_try_advisory_lock(:k)"), {"k": WORKER_LOCK_KEY})).scalar()
        assert got is True
        await conn.execute(text("SELECT pg_advisory_unlock(:k)"), {"k": WORKER_LOCK_KEY})


async def test_release_with_a_dead_lock_connection(engine, session_factory):
    """Unlocking a connection the server already closed invalidates it instead of raising."""
    worker = _worker(engine, session_factory)
    assert await worker._try_lead()
    pid = (await worker._lock_conn.execute(text("SELECT pg_backend_pid()"))).scalar()
    async with engine.connect() as conn:
        await conn.execute(text("SELECT pg_terminate_backend(:pid)"), {"pid": pid})

    await worker._release()

    assert not worker.is_leader
    async with engine.connect() as conn:
        got = (await conn.execute(text("SELECT pg_try_advisory_lock(:k)"), {"k": WORKER_LOCK_KEY})).scalar()
        assert got is True  # the server dropped the lock with the connection
        await conn.execute(text("SELECT pg_advisory_unlock(:k)"), {"k": WORKER_LOCK_KEY})


async def test_stop_without_start_returns(engine, session_factory):
    """Stopping a worker that never started is a no-op: there is no task to cancel."""
    worker = _worker(engine, session_factory)
    await worker.stop()
    assert not worker.is_leader


async def test_try_lead_failure_closes_the_connection(engine, session_factory, monkeypatch):
    monkeypatch.setattr("app.worker.WORKER_LOCK_KEY", "not-a-number")
    worker = _worker(engine, session_factory)
    with pytest.raises(DBAPIError):
        await worker._try_lead()
    assert not worker.is_leader
    assert engine.sync_engine.pool.checkedout() == 0


async def test_failed_prune_is_logged_and_processing_continues(engine, session_factory, db, caplog):
    async def failing_prune():
        raise RuntimeError("prune failed")

    proj = await make_project(db)
    report = await make_report(db, proj, make_junit([("t", "passed")]))
    await db.commit()

    worker = _worker(engine, session_factory, prune=failing_prune)
    with caplog.at_level(logging.ERROR, logger="flakeradar.worker"):
        await worker.start()
        try:
            await _wait_for(lambda: _is_processed(session_factory, report.id))
        finally:
            await worker.stop()
    assert [r.getMessage() for r in caplog.records if "Retention prune failed" in r.getMessage()] == [
        "Retention prune failed; will retry next interval"
    ]


async def test_processes_without_a_hook(engine, session_factory, db):
    proj = await make_project(db)
    report = await make_report(db, proj, make_junit([("t", "passed")]))
    await db.commit()

    worker = _worker(engine, session_factory)
    await worker.start()
    try:
        await _wait_for(lambda: _is_processed(session_factory, report.id))
    finally:
        await worker.stop()


class _StopFailsGitHubWorker(GitHubWorker):
    """A GitHub task whose stop raises, like a shutdown that fails."""

    async def stop(self) -> None:
        await super().stop()  # cancel the real task so nothing leaks into the loop
        raise RuntimeError("stop failed")


async def test_release_survives_a_failing_github_stop(engine, session_factory, caplog):
    """A GitHub task that cannot be stopped is logged; the lock is released anyway."""
    github = _StopFailsGitHubWorker(session_factory, sync_interval_seconds=3600.0)
    worker = _worker(engine, session_factory, github=github)
    assert await worker._try_lead()

    with caplog.at_level(logging.ERROR, logger="flakeradar.worker"):
        await worker._release()

    assert not worker.is_leader
    assert await _lock_is_free(engine)
    assert [r.getMessage() for r in caplog.records if "Stopping the GitHub task failed" in r.getMessage()] == [
        "Stopping the GitHub task failed; releasing the lock anyway"
    ]


async def test_failing_hook_is_logged_and_processing_continues(engine, session_factory, db, caplog):
    proj = await make_project(db)
    for sha in ("a", "b"):
        await make_report(db, proj, make_junit([("t", "passed")]), commit_sha=sha)
    await db.commit()

    async def failing_hook(outcome):
        raise RuntimeError("hook failed")

    worker = _worker(engine, session_factory, on_processed=failing_hook)
    with caplog.at_level(logging.ERROR, logger="flakeradar.worker"):
        await worker.start()

        async def all_done():
            async with session_factory() as s:
                statuses = (await s.execute(select(Report.status).order_by(Report.id))).scalars().all()
            return statuses == ["processed", "processed"]

        try:
            await _wait_for(all_done)
        finally:
            await worker.stop()

    assert [r.getMessage() for r in caplog.records if "on_processed hook failed" in r.getMessage()] == [
        "on_processed hook failed for Report 1",
        "on_processed hook failed for Report 2",
    ]


async def test_loop_error_drops_leadership_and_retries(engine, session_factory, db, caplog, monkeypatch):
    """An unexpected error in the loop is logged; the worker re-leads and processing goes on."""
    from app.processing import process_next as real_process_next

    calls = []

    async def flaky_process_next(session_factory_):
        calls.append(session_factory_)
        if len(calls) == 1:
            raise RuntimeError("unexpected loop error")
        return await real_process_next(session_factory_)

    monkeypatch.setattr("app.worker.process_next", flaky_process_next)

    proj = await make_project(db)
    report = await make_report(db, proj, make_junit([("t", "passed")]))
    await db.commit()

    worker = _worker(engine, session_factory)
    with caplog.at_level(logging.ERROR, logger="flakeradar.worker"):
        await worker.start()
        try:
            await _wait_for(lambda: _is_processed(session_factory, report.id))
            assert worker.is_leader  # leadership was dropped after the error and taken again
        finally:
            await worker.stop()

    assert [r.getMessage() for r in caplog.records if "Report worker error" in r.getMessage()] == [
        "Report worker error; dropping leadership and retrying"
    ]


async def test_transient_retries_back_off_before_the_next_claim(engine, session_factory, monkeypatch):
    """A pending outcome makes the worker wait poll_seconds * 2 ** retry_count, capped at 30 s."""
    # Each fake outcome is a transient failure that re-queued the Report; the
    # 99 exceeds the retry cap to show the wait is bounded.
    retry_counts = [1, 2, 3, 99]
    claims: list[int] = []

    async def retrying_process_next(session_factory_):
        claims.append(len(claims))
        if not retry_counts:
            return None
        return ProcessOutcome(
            report_id=1,
            status="pending",
            run_id=None,
            counts=None,
            touched_test_ids=[],
            error="OperationalError: db blip",
            retry_count=retry_counts.pop(0),
        )

    real_sleep = asyncio.sleep
    delays: list[float] = []

    async def fake_sleep(delay, *args):
        delays.append(delay)
        await real_sleep(min(delay, 0.01))

    monkeypatch.setattr("app.worker.process_next", retrying_process_next)
    monkeypatch.setattr(asyncio, "sleep", fake_sleep)

    # standby_seconds is far above the cap so only backoff and poll sleeps appear
    worker = _worker(engine, session_factory, standby_seconds=3600.0)

    async def claimed_enough() -> bool:
        return len(claims) >= 5

    await worker.start()
    try:
        await _wait_for(claimed_enough)
    finally:
        await worker.stop()

    # poll sleeps (exactly 0.05) and the standby sleep are filtered out
    assert [d for d in delays if 0.05 < d <= 30] == [0.1, 0.2, 0.4, 30]


async def test_a_tier_change_reaches_the_webhook_task(engine, session_factory, db, monkeypatch):
    monkeypatch.setattr("app.processing.get_settings", lambda: _notify_settings())
    proj = await make_project(db)
    await make_report(db, proj, make_junit([("t", "failed")]), commit_sha="a")
    await make_report(db, proj, make_junit([("t", "passed")]), commit_sha="b")
    await db.commit()
    bodies: list[bytes] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        bodies.append(request.content)
        return httpx.Response(200)

    webhook = WebhookWorker(["https://hooks.example.com/services/x"], transport=httpx.MockTransport(handler))
    worker = _worker(engine, session_factory, webhook=webhook)
    await worker.start()
    try:

        async def is_running() -> bool:
            return webhook.is_running

        async def delivered() -> bool:
            return len(bodies) == 1

        await _wait_for(is_running)
        await _wait_for(delivered)
        assert b"test.flaky" in bodies[0]
    finally:
        await worker.stop()
    assert not webhook.is_running  # stopped with leadership...
    assert await _lock_is_free(engine)  # ...before the lock was given up


class _StopFailsWebhookWorker(WebhookWorker):
    """A Notification task whose stop raises, like a shutdown that fails."""

    async def stop(self) -> None:
        await super().stop()  # cancel the real task so nothing leaks into the loop
        raise RuntimeError("stop failed")


async def test_release_survives_a_failing_webhook_stop(engine, session_factory, caplog):
    webhook = _StopFailsWebhookWorker(["https://hooks.example.com/x"])
    worker = _worker(engine, session_factory, webhook=webhook)
    assert await worker._try_lead()

    with caplog.at_level(logging.ERROR, logger="flakeradar.worker"):
        await worker._release()

    assert not worker.is_leader
    assert await _lock_is_free(engine)
    assert "Stopping the Notification task failed; releasing the lock anyway" in caplog.text
