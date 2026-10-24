"""Retention: delete old processed Reports and old Executions.

The prune rescores every Test and Job whose Executions it deletes, in the
same transaction, so a Test or Job that stops running decays to a score of 0
(and leaves the default leaderboard) instead of keeping its last cached score
forever. Failed Reports are kept until a human deals with them.
"""

import logging
from dataclasses import dataclass
from datetime import datetime, timedelta

from sqlalchemy import delete, exists, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from .config import get_settings
from .models import (
    REPORT_PROCESSED,
    JobExecution,
    JobScoreHistory,
    Report,
    TestExecution,
    TestRun,
    TestScoreHistory,
    utcnow,
)
from .processing import rescore, rescore_jobs

logger = logging.getLogger("flakeradar.retention")


@dataclass(frozen=True)
class PruneResult:
    reports: int
    executions: int
    runs: int
    job_executions: int
    score_history: int = 0


async def prune(
    db: AsyncSession,
    *,
    now: datetime,
    report_days: int,
    execution_days: int,
    history_days: int = 365,
) -> PruneResult:
    """Delete expired rows. Caller owns the transaction."""
    report_cutoff = now - timedelta(days=report_days)
    execution_cutoff = now - timedelta(days=execution_days)

    reports = await db.execute(
        delete(Report).where(Report.status == REPORT_PROCESSED, Report.processed_at < report_cutoff)
    )
    # Collect the ids before the deletes, so the rows that will disappear can still be
    # rescored afterwards (the caller owns the transaction, so this stays atomic).
    stale_test_ids = list(
        (
            await db.execute(
                select(TestExecution.test_case_id).where(TestExecution.created_at < execution_cutoff).distinct()
            )
        ).scalars()
    )
    stale_job_ids = list(
        (
            await db.execute(
                select(JobExecution.job_id).where(JobExecution.created_at < execution_cutoff).distinct()
            )
        ).scalars()
    )
    executions = await db.execute(delete(TestExecution).where(TestExecution.created_at < execution_cutoff))
    job_executions = await db.execute(delete(JobExecution).where(JobExecution.created_at < execution_cutoff))
    runs = await db.execute(
        delete(TestRun).where(
            TestRun.created_at < execution_cutoff,
            ~exists(select(TestExecution.id).where(TestExecution.test_run_id == TestRun.id)),
        )
    )
    score_history = await db.execute(
        delete(TestScoreHistory).where(TestScoreHistory.day < (now - timedelta(days=history_days)).date())
    )
    job_score_history = await db.execute(
        delete(JobScoreHistory).where(JobScoreHistory.day < (now - timedelta(days=history_days)).date())
    )
    # Rescore the Tests and Jobs whose Executions were deleted, in this same
    # transaction: a Test with no Executions left scores 0 and leaves the default
    # leaderboard; one with Executions left is scored on what remains.
    if stale_test_ids:
        await rescore(db, stale_test_ids)
    if stale_job_ids:
        await rescore_jobs(db, stale_job_ids)
    return PruneResult(
        reports=reports.rowcount,
        executions=executions.rowcount,
        runs=runs.rowcount,
        job_executions=job_executions.rowcount,
        score_history=score_history.rowcount + job_score_history.rowcount,
    )


async def run_prune(session_factory: async_sessionmaker[AsyncSession]) -> PruneResult:
    settings = get_settings()
    async with session_factory() as db, db.begin():
        result = await prune(
            db,
            now=utcnow(),
            report_days=settings.report_retention_days,
            execution_days=settings.execution_retention_days,
            history_days=settings.score_history_retention_days,
        )
    logger.info(
        "Pruned %s reports, %s executions, %s job executions, %s runs, %s score history rows",
        result.reports,
        result.executions,
        result.job_executions,
        result.runs,
        result.score_history,
    )
    return result
