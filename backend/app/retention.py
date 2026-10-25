"""Retention: delete old processed Reports and old Executions in bounded batches.

Each batch deletes at most ``prune_batch`` rows per table
(FLAKERADAR_PRUNE_BATCH) and is its own committed transaction, so a
multi-million-row backlog drains a few thousand rows per interval instead of
issuing one huge DELETE that the statement timeout cancels and rolls back.
A run keeps going until a round deletes fewer than ``prune_batch`` rows or
until the ``prune_time_budget_seconds`` budget is spent; a budget stop is a
normal return and the next interval resumes where the committed batches left
off.

Each batch rescores the Tests and Jobs whose Executions it deleted, in the same
transaction, so a Test or Job that stops running decays to a score of 0
(and leaves the default leaderboard) instead of keeping its last cached score
forever. Failed Reports are kept until a human deals with them.
"""

import logging
import time
from dataclasses import dataclass
from datetime import date, datetime, timedelta

from sqlalchemy import ColumnElement, delete, exists, select, tuple_
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

_monotonic = time.monotonic  # patchable clock seam


@dataclass(frozen=True)
class PruneResult:
    reports: int
    executions: int
    runs: int
    job_executions: int
    score_history: int = 0
    # Per-table history counts whose sum is score_history; run_prune's
    # continuation check needs each table separately.
    test_score_history: int = 0
    job_score_history: int = 0


async def _oldest_ids(
    db: AsyncSession, id_col: ColumnElement[int], expired: ColumnElement[bool], batch: int
) -> list[int]:
    """The `batch` lowest primary keys satisfying `expired`: one batch of deletable rows."""
    rows = await db.execute(select(id_col).where(expired).order_by(id_col).limit(batch))
    return list(rows.scalars())


async def _owner_ids(
    db: AsyncSession, id_col: ColumnElement[int], owner_col: ColumnElement[int], batch_ids: list[int]
) -> list[int]:
    """Distinct owners (Tests, Jobs) touched by one batch of Execution deletes."""
    if not batch_ids:
        return []
    rows = await db.execute(select(owner_col).where(id_col.in_(batch_ids)).distinct())
    return list(rows.scalars())


async def _delete_history_batch(
    db: AsyncSession,
    model: type[TestScoreHistory] | type[JobScoreHistory],
    owner_col: ColumnElement[int],
    day_col: ColumnElement[date],
    history_cutoff: date,
    batch: int,
) -> int:
    """Delete one batch of a composite-(owner, day)-keyed score history table."""
    result = await db.execute(
        delete(model).where(
            tuple_(owner_col, day_col).in_(
                select(owner_col, day_col).where(day_col < history_cutoff).order_by(day_col).limit(batch)
            )
        )
    )
    return result.rowcount


async def prune(
    db: AsyncSession,
    *,
    now: datetime,
    report_days: int,
    execution_days: int,
    history_days: int = 365,
    batch: int | None = None,
) -> PruneResult:
    """Delete one batch of expired rows: at most `batch` per table. Caller owns the transaction.

    A count equal to `batch` means more expired rows may remain, so another round is worth running.
    """
    if batch is None:
        batch = get_settings().prune_batch
    report_cutoff = now - timedelta(days=report_days)
    execution_cutoff = now - timedelta(days=execution_days)
    history_cutoff = (now - timedelta(days=history_days)).date()

    reports = await db.execute(
        delete(Report).where(
            Report.id.in_(
                select(Report.id)
                .where(Report.status == REPORT_PROCESSED, Report.processed_at < report_cutoff)
                .order_by(Report.id)
                .limit(batch)
            )
        )
    )
    # Collect the ids of this batch before deleting them, so the rows that will disappear can still be
    # rescored afterwards (the caller owns the transaction, so this stays atomic).
    batch_test_ids = await _oldest_ids(db, TestExecution.id, TestExecution.created_at < execution_cutoff, batch)
    stale_test_ids = await _owner_ids(db, TestExecution.id, TestExecution.test_case_id, batch_test_ids)
    batch_job_ids = await _oldest_ids(db, JobExecution.id, JobExecution.created_at < execution_cutoff, batch)
    stale_job_ids = await _owner_ids(db, JobExecution.id, JobExecution.job_id, batch_job_ids)
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
                .limit(batch)
            )
        )
    )
    # The score history tables have composite primary keys, so the batch is identified by
    # the (owner id, day) row value rather than a single id column.
    test_history = await _delete_history_batch(
        db, TestScoreHistory, TestScoreHistory.test_case_id, TestScoreHistory.day, history_cutoff, batch
    )
    job_history = await _delete_history_batch(
        db, JobScoreHistory, JobScoreHistory.job_id, JobScoreHistory.day, history_cutoff, batch
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
        score_history=test_history + job_history,
        test_score_history=test_history,
        job_score_history=job_history,
    )


async def run_prune(session_factory: async_sessionmaker[AsyncSession]) -> PruneResult:
    """Delete the expired backlog in committed batches, stopping at the time budget.

    Every batch is committed before the next one starts, so a later failure cannot roll back
    the rows already deleted, and running out of budget is a normal return: the worker sees
    no error and the next interval picks up the rest of the backlog.
    """
    settings = get_settings()
    batch = settings.prune_batch
    budget = settings.prune_time_budget_seconds
    deadline = _monotonic() + budget
    totals = PruneResult(reports=0, executions=0, runs=0, job_executions=0, score_history=0)
    while True:
        async with session_factory() as db, db.begin():
            result = await prune(
                db,
                now=utcnow(),
                report_days=settings.report_retention_days,
                execution_days=settings.execution_retention_days,
                history_days=settings.score_history_retention_days,
                batch=batch,
            )
        totals = PruneResult(
            reports=totals.reports + result.reports,
            executions=totals.executions + result.executions,
            runs=totals.runs + result.runs,
            job_executions=totals.job_executions + result.job_executions,
            score_history=totals.score_history + result.score_history,
            test_score_history=totals.test_score_history + result.test_score_history,
            job_score_history=totals.job_score_history + result.job_score_history,
        )
        # A full batch means that table may have more expired rows; an all-empty round
        # deletes nothing, so the loop always terminates. The two score history tables
        # are checked separately: their summed total would miss a full batch in each.
        if not any(
            count == batch
            for count in (
                result.reports,
                result.executions,
                result.runs,
                result.job_executions,
                result.test_score_history,
                result.job_score_history,
            )
        ):
            break
        if _monotonic() >= deadline:
            logger.info(
                "Retention prune hit its %ss budget; the rest waits for the next interval",
                budget,
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
