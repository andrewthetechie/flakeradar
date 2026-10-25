"""Retention: delete old processed Reports and old Executions in bounded batches.

Each batch deletes at most `PRUNE_BATCH` rows per table and is its own committed
transaction, so a multi-million-row backlog drains a few thousand rows per interval
instead of issuing one huge DELETE that the statement timeout cancels and rolls back.
A run keeps going until a round deletes fewer than `PRUNE_BATCH` rows or until the
`PRUNE_TIME_BUDGET_SECONDS` budget is spent; a budget stop is a normal return and
the next interval resumes where the committed batches left off.

Each batch rescores the Tests and Jobs whose Executions it deleted, in the same
transaction, so a Test or Job that stops running decays to a score of 0
(and leaves the default leaderboard) instead of keeping its last cached score
forever. Failed Reports are kept until a human deals with them.
"""

import logging
import time
from dataclasses import dataclass
from datetime import datetime, timedelta

from sqlalchemy import delete, exists, select, tuple_
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

# Rows deleted per table in one batch, and the wall-clock budget of one prune run.
# Both are read at call time from this module, so tests can patch them.
PRUNE_BATCH = 5000
PRUNE_TIME_BUDGET_SECONDS = 60.0
_monotonic = time.monotonic  # patchable clock seam


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
    """Delete one batch of expired rows: at most `PRUNE_BATCH` per table. Caller owns the transaction.

    A count equal to `PRUNE_BATCH` means more expired rows may remain, so another round is worth running.
    """
    report_cutoff = now - timedelta(days=report_days)
    execution_cutoff = now - timedelta(days=execution_days)
    history_cutoff = (now - timedelta(days=history_days)).date()

    reports = await db.execute(
        delete(Report).where(
            Report.id.in_(
                select(Report.id)
                .where(Report.status == REPORT_PROCESSED, Report.processed_at < report_cutoff)
                .order_by(Report.id)
                .limit(PRUNE_BATCH)
            )
        )
    )
    # Collect the ids of this batch before deleting them, so the rows that will disappear can still be
    # rescored afterwards (the caller owns the transaction, so this stays atomic).
    batch_test_ids = list(
        (
            await db.execute(
                select(TestExecution.id)
                .where(TestExecution.created_at < execution_cutoff)
                .order_by(TestExecution.id)
                .limit(PRUNE_BATCH)
            )
        ).scalars()
    )
    stale_test_ids = (
        list(
            (
                await db.execute(
                    select(TestExecution.test_case_id).where(TestExecution.id.in_(batch_test_ids)).distinct()
                )
            ).scalars()
        )
        if batch_test_ids
        else []
    )
    batch_job_ids = list(
        (
            await db.execute(
                select(JobExecution.id)
                .where(JobExecution.created_at < execution_cutoff)
                .order_by(JobExecution.id)
                .limit(PRUNE_BATCH)
            )
        ).scalars()
    )
    stale_job_ids = (
        list(
            (
                await db.execute(select(JobExecution.job_id).where(JobExecution.id.in_(batch_job_ids)).distinct())
            ).scalars()
        )
        if batch_job_ids
        else []
    )
    executions = await db.execute(delete(TestExecution).where(TestExecution.id.in_(batch_test_ids)))
    job_executions = await db.execute(delete(JobExecution).where(JobExecution.id.in_(batch_job_ids)))
    # Runs only become deletable once this batch removed their last Execution.
    runs = await db.execute(
        delete(TestRun).where(
            TestRun.id.in_(
                select(TestRun.id)
                .where(
                    TestRun.created_at < execution_cutoff,
                    ~exists(select(TestExecution.id).where(TestExecution.test_run_id == TestRun.id)),
                )
                .order_by(TestRun.id)
                .limit(PRUNE_BATCH)
            )
        )
    )
    # The score history tables have composite primary keys, so the batch is identified by
    # the (owner id, day) row value rather than a single id column.
    score_history = await db.execute(
        delete(TestScoreHistory).where(
            tuple_(TestScoreHistory.test_case_id, TestScoreHistory.day).in_(
                select(TestScoreHistory.test_case_id, TestScoreHistory.day)
                .where(TestScoreHistory.day < history_cutoff)
                .order_by(TestScoreHistory.day)
                .limit(PRUNE_BATCH)
            )
        )
    )
    job_score_history = await db.execute(
        delete(JobScoreHistory).where(
            tuple_(JobScoreHistory.job_id, JobScoreHistory.day).in_(
                select(JobScoreHistory.job_id, JobScoreHistory.day)
                .where(JobScoreHistory.day < history_cutoff)
                .order_by(JobScoreHistory.day)
                .limit(PRUNE_BATCH)
            )
        )
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
    """Delete the expired backlog in committed batches, stopping at the time budget.

    Every batch is committed before the next one starts, so a later failure cannot roll back
    the rows already deleted, and running out of budget is a normal return: the worker sees
    no error and the next interval picks up the rest of the backlog.
    """
    settings = get_settings()
    deadline = _monotonic() + PRUNE_TIME_BUDGET_SECONDS
    totals = PruneResult(reports=0, executions=0, runs=0, job_executions=0, score_history=0)
    while True:
        async with session_factory() as db, db.begin():
            result = await prune(
                db,
                now=utcnow(),
                report_days=settings.report_retention_days,
                execution_days=settings.execution_retention_days,
                history_days=settings.score_history_retention_days,
            )
        totals = PruneResult(
            reports=totals.reports + result.reports,
            executions=totals.executions + result.executions,
            runs=totals.runs + result.runs,
            job_executions=totals.job_executions + result.job_executions,
            score_history=totals.score_history + result.score_history,
        )
        # A full batch means that table may have more expired rows; an all-empty round
        # deletes nothing, so the loop always terminates. A summed score history count
        # equal to the batch only costs one extra empty round.
        if not any(
            count == PRUNE_BATCH
            for count in (result.reports, result.executions, result.runs, result.job_executions, result.score_history)
        ):
            break
        if _monotonic() >= deadline:
            logger.info(
                "Retention prune hit its %ss budget; the rest waits for the next interval",
                PRUNE_TIME_BUDGET_SECONDS,
            )
            break
    logger.info(
        "Pruned %s reports, %s executions, %s job executions, %s runs, %s score history rows",
        totals.reports,
        totals.executions,
        totals.job_executions,
        totals.runs,
        totals.score_history,
    )
    return totals
