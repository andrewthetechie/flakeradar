"""Retention: delete old processed Reports and old Executions.

Scores are cached on each Test, so pruning history never changes a score
until the Test's next Run rescores it from what remains. Failed Reports are
kept until a human deals with them.
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
    Report,
    TestExecution,
    TestRun,
    utcnow,
)

logger = logging.getLogger("flakeradar.retention")


@dataclass(frozen=True)
class PruneResult:
    reports: int
    executions: int
    runs: int
    job_executions: int


async def prune(db: AsyncSession, *, now: datetime, report_days: int, execution_days: int) -> PruneResult:
    """Delete expired rows. Caller owns the transaction."""
    report_cutoff = now - timedelta(days=report_days)
    execution_cutoff = now - timedelta(days=execution_days)

    reports = await db.execute(
        delete(Report).where(Report.status == REPORT_PROCESSED, Report.processed_at < report_cutoff)
    )
    executions = await db.execute(delete(TestExecution).where(TestExecution.created_at < execution_cutoff))
    job_executions = await db.execute(delete(JobExecution).where(JobExecution.created_at < execution_cutoff))
    runs = await db.execute(
        delete(TestRun).where(
            TestRun.created_at < execution_cutoff,
            ~exists(select(TestExecution.id).where(TestExecution.test_run_id == TestRun.id)),
        )
    )
    return PruneResult(
        reports=reports.rowcount,
        executions=executions.rowcount,
        runs=runs.rowcount,
        job_executions=job_executions.rowcount,
    )


async def run_prune(session_factory: async_sessionmaker[AsyncSession]) -> PruneResult:
    settings = get_settings()
    async with session_factory() as db, db.begin():
        result = await prune(
            db,
            now=utcnow(),
            report_days=settings.report_retention_days,
            execution_days=settings.execution_retention_days,
        )
    logger.info(
        "Pruned %s reports, %s executions, %s job executions, %s runs",
        result.reports,
        result.executions,
        result.job_executions,
        result.runs,
    )
    return result
