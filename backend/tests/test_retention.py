"""Retention: old processed Reports and old Executions go; the rest stays."""

import asyncio
from datetime import timedelta

from app.models import (
    REPORT_FAILED,
    REPORT_PROCESSED,
    JobExecution,
    Report,
    TestExecution,
    TestRun,
    TestScoreHistory,
    utcnow,
)
from app.retention import prune
from app.worker import ReportWorker
from sqlalchemy import func, select

from tests.factories import (
    make_execution,
    make_job,
    make_job_execution,
    make_pipeline,
    make_project,
    make_report,
    make_run,
    make_test_case,
    make_test_score,
)


async def _count(db, column) -> int:
    return (await db.execute(select(func.count(column)))).scalar()


async def test_prune_deletes_only_expired_rows(db):
    now = utcnow()
    old, recent = now - timedelta(days=100), now - timedelta(days=1)
    proj = await make_project(db)
    tc = await make_test_case(db, proj)

    old_run = await make_run(db, proj, commit_sha="old", created_at=old)
    await make_execution(db, tc, old_run, created_at=old)
    new_run = await make_run(db, proj, commit_sha="new", created_at=recent)
    await make_execution(db, tc, new_run, created_at=recent)

    stale = await make_report(db, proj, b"<x/>", status=REPORT_PROCESSED)
    stale.processed_at, stale.run_id = now - timedelta(days=8), old_run.id
    fresh = await make_report(db, proj, b"<x/>", status=REPORT_PROCESSED)
    fresh.processed_at = now - timedelta(days=6)
    failed = await make_report(db, proj, b"<x/>", status=REPORT_FAILED)
    failed.processed_at = now - timedelta(days=30)
    await db.commit()

    result = await prune(db, now=now, report_days=7, execution_days=90)
    await db.commit()

    assert (result.reports, result.executions, result.runs) == (1, 1, 1)
    remaining = set((await db.execute(select(Report.id))).scalars())
    assert remaining == {fresh.id, failed.id}
    assert await _count(db, TestExecution.id) == 1
    assert (await db.execute(select(TestRun.commit_sha))).scalars().all() == ["new"]


async def test_worker_prunes_once_per_interval(engine, session_factory):
    calls = []

    async def fake_prune():
        calls.append(1)

    worker = ReportWorker(
        engine, session_factory, poll_seconds=0.05, standby_seconds=0.1, prune=fake_prune, prune_interval_seconds=3600
    )
    await worker.start()
    try:
        await asyncio.sleep(0.5)
    finally:
        await worker.stop()
    assert calls == [1]


async def test_prune_deletes_old_job_executions(db):
    now = utcnow()
    pipe = await make_pipeline(db)
    job = await make_job(db, pipe)
    old = now - timedelta(days=100)
    recent = now - timedelta(days=1)
    await make_job_execution(db, job, ci_job_id="old", created_at=old)
    await make_job_execution(db, job, ci_job_id="new", created_at=recent)
    await db.commit()

    result = await prune(db, now=now, report_days=7, execution_days=90)
    await db.commit()

    assert result.job_executions == 1
    remaining = (await db.execute(select(JobExecution.ci_job_id))).scalars().all()
    assert remaining == ["new"]


async def test_prune_deletes_old_score_history_rows(db):
    now = utcnow()
    proj = await make_project(db)
    tc = await make_test_case(db, proj)
    await make_test_score(db, tc, (now - timedelta(days=400)).date(), 0.5)
    await make_test_score(db, tc, (now - timedelta(days=10)).date(), 0.5)
    await db.commit()

    result = await prune(db, now=now, report_days=7, execution_days=90, history_days=365)
    await db.commit()

    assert result.score_history == 1
    remaining = (await db.execute(select(func.count()).select_from(TestScoreHistory))).scalar()
    assert remaining == 1
