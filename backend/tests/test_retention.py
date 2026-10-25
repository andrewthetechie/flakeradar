"""Retention: old processed Reports and old Executions go; the rest stays."""

import asyncio
from datetime import timedelta

from app import retention
from app.config import Settings
from app.models import (
    REPORT_FAILED,
    REPORT_PROCESSED,
    Job,
    JobExecution,
    JobScoreHistory,
    Report,
    TestCase,
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
    make_job_score,
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


async def test_prune_deletes_old_job_score_history_rows(db):
    now = utcnow()
    pipe = await make_pipeline(db)
    job = await make_job(db, pipe)
    await make_job_score(db, job, (now - timedelta(days=400)).date(), 0.5)
    await make_job_score(db, job, (now - timedelta(days=10)).date(), 0.5)
    await db.commit()

    result = await prune(db, now=now, report_days=7, execution_days=90, history_days=365)
    await db.commit()

    assert result.score_history == 1
    remaining = (await db.execute(select(func.count()).select_from(JobScoreHistory))).scalar()
    assert remaining == 1


async def test_prune_rescores_test_with_no_executions_left(db, client):
    now = utcnow()
    old = now - timedelta(days=100)
    proj = await make_project(db)
    tc = await make_test_case(db, proj, flakiness_score=0.5, clean_streak=3)
    run = await make_run(db, proj, commit_sha="old", created_at=old)
    await make_execution(db, tc, run, created_at=old)
    await db.commit()

    result = await prune(db, now=now, report_days=7, execution_days=90)
    await db.commit()
    assert result.executions == 1

    row = (await db.execute(select(TestCase).where(TestCase.id == tc.id))).scalar_one()
    await db.refresh(row)
    assert row.flakiness_score == 0.0
    assert row.clean_streak == 0

    page = (await client.get("/api/tests")).json()
    assert page["total"] == 0
    page = (await client.get("/api/tests?include_stable=true")).json()
    assert [t["tier"] for t in page["items"]] == ["stable"]


async def test_prune_rescores_test_from_remaining_executions(db):
    now = utcnow()
    old, recent = now - timedelta(days=100), now - timedelta(days=1)
    proj = await make_project(db)
    tc = await make_test_case(db, proj, flakiness_score=0.9, clean_streak=3)
    old_run = await make_run(db, proj, commit_sha="old1", created_at=old)
    await make_execution(db, tc, old_run, status="failed", created_at=old)
    old_run2 = await make_run(db, proj, commit_sha="old2", created_at=old)
    await make_execution(db, tc, old_run2, status="passed", created_at=old)
    new_run = await make_run(db, proj, commit_sha="new1", created_at=recent)
    await make_execution(db, tc, new_run, created_at=recent)
    new_run2 = await make_run(db, proj, commit_sha="new2", created_at=recent)
    await make_execution(db, tc, new_run2, created_at=recent)
    await db.commit()

    result = await prune(db, now=now, report_days=7, execution_days=90)
    await db.commit()
    assert result.executions == 2

    row = (await db.execute(select(TestCase).where(TestCase.id == tc.id))).scalar_one()
    await db.refresh(row)
    assert row.flakiness_score == 0.0  # not the stale 0.9: the two remaining passes score 0
    assert row.clean_streak == 2


async def test_prune_rescores_job_with_no_executions_left(db, client):
    now = utcnow()
    old = now - timedelta(days=100)
    pipe = await make_pipeline(db)
    job = await make_job(db, pipe, flakiness_score=0.5, clean_streak=3)
    await make_job_execution(db, job, ci_job_id="old", created_at=old)
    await db.commit()

    result = await prune(db, now=now, report_days=7, execution_days=90)
    await db.commit()
    assert result.job_executions == 1

    row = (await db.execute(select(Job).where(Job.id == job.id))).scalar_one()
    await db.refresh(row)
    assert row.flakiness_score == 0.0
    assert row.clean_streak == 0

    page = (await client.get("/api/jobs?repo=acme/app")).json()
    assert page["total"] == 0
    page = (await client.get("/api/jobs?repo=acme/app&include_stable=true")).json()
    assert [j["tier"] for j in page["items"]] == ["stable"]


async def test_prune_rescores_job_from_remaining_executions(db):
    now = utcnow()
    old, recent = now - timedelta(days=100), now - timedelta(days=1)
    pipe = await make_pipeline(db)
    job = await make_job(db, pipe, flakiness_score=0.9, clean_streak=3)
    await make_job_execution(db, job, status="failed", ci_job_id="old", created_at=old)
    await make_job_execution(db, job, ci_job_id="new1", created_at=recent)
    await make_job_execution(db, job, ci_job_id="new2", created_at=recent)
    await db.commit()

    result = await prune(db, now=now, report_days=7, execution_days=90)
    await db.commit()
    assert result.job_executions == 1

    row = (await db.execute(select(Job).where(Job.id == job.id))).scalar_one()
    await db.refresh(row)
    assert row.flakiness_score == 0.0  # not the stale 0.9: the two remaining passes score 0
    assert row.clean_streak == 2


async def test_run_prune_deletes_in_batches(db, session_factory, monkeypatch):
    now = utcnow()
    old = now - timedelta(days=100)  # older than the default execution_retention_days
    proj = await make_project(db)
    tc = await make_test_case(db, proj, flakiness_score=0.5, clean_streak=3)
    run = await make_run(db, proj, commit_sha="old", created_at=old)
    for _ in range(5):
        await make_execution(db, tc, run, created_at=old)
    await db.commit()

    monkeypatch.setattr(retention, "get_settings", lambda: Settings(prune_batch=2))
    rounds: list[int] = []
    real_prune = retention.prune

    async def counting_prune(session, **kwargs):
        rounds.append(1)
        return await real_prune(session, **kwargs)

    monkeypatch.setattr(retention, "prune", counting_prune)

    result = await retention.run_prune(session_factory)

    assert len(rounds) == 3  # 2 + 2 + 1 rows
    assert result.executions == 5
    assert await _count(db, TestExecution.id) == 0
    row = (await db.execute(select(TestCase).where(TestCase.id == tc.id))).scalar_one()
    await db.refresh(row)
    assert row.flakiness_score == 0.0  # every batch rescored the Test it deleted
    assert row.clean_streak == 0


async def test_run_prune_drains_both_score_history_tables(db, session_factory, monkeypatch):
    now = utcnow()
    proj = await make_project(db)
    tc = await make_test_case(db, proj)
    pipe = await make_pipeline(db)
    job = await make_job(db, pipe)
    for i in range(5):  # composite primary keys require distinct days
        await make_test_score(db, tc, (now - timedelta(days=400 + i)).date(), 0.5)
        await make_job_score(db, job, (now - timedelta(days=400 + i)).date(), 0.5)
    await db.commit()

    monkeypatch.setattr(retention, "get_settings", lambda: Settings(prune_batch=2))
    rounds: list[int] = []
    real_prune = retention.prune

    async def counting_prune(session, **kwargs):
        rounds.append(1)
        return await real_prune(session, **kwargs)

    monkeypatch.setattr(retention, "prune", counting_prune)

    result = await retention.run_prune(session_factory)

    # Each round deletes a full batch from BOTH history tables, so the summed count would
    # never equal the batch size; the loop must check the tables separately.
    assert len(rounds) == 3  # (2, 2) + (2, 2) + (1, 1) rows
    assert result.score_history == 10
    assert (await db.execute(select(func.count()).select_from(TestScoreHistory))).scalar() == 0
    assert (await db.execute(select(func.count()).select_from(JobScoreHistory))).scalar() == 0


async def test_run_prune_stops_at_time_budget(db, session_factory, monkeypatch):
    now = utcnow()
    old = now - timedelta(days=100)
    proj = await make_project(db)
    tc = await make_test_case(db, proj)
    run = await make_run(db, proj, commit_sha="old", created_at=old)
    for _ in range(6):
        await make_execution(db, tc, run, created_at=old)
    await db.commit()

    settings = Settings(prune_batch=2)
    monkeypatch.setattr(retention, "get_settings", lambda: settings)
    # The first reading sets the deadline; the later ones are past it.
    past_deadline = iter([0.0] + [settings.prune_time_budget_seconds + 1.0] * 8)
    monkeypatch.setattr(retention, "_monotonic", lambda: next(past_deadline))

    result = await retention.run_prune(session_factory)  # returns normally, no exception

    assert result.executions == 2  # only the first committed batch
    assert await _count(db, TestExecution.id) == 4  # the backlog waits for the next interval
