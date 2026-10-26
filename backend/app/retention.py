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
forever. Failed Reports are kept until a human deals with them, unless
``failed_report_retention_days`` (FLAKERADAR_FAILED_REPORT_RETENTION_DAYS) ages
them out too.
"""

import logging
import time
from dataclasses import dataclass
from datetime import date, datetime, timedelta

from sqlalchemy import ColumnElement, delete, exists, func, select, tuple_
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from .config import get_settings
from .models import (
    REPORT_FAILED,
    REPORT_PROCESSED,
    JobExecution,
    JobScoreHistory,
    Report,
    TestCase,
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
    # Quarantines whose human-set expiry has passed, cleared in this round.
    expired_quarantines: int = 0
    # Failed Reports deleted by the optional failed_report_retention_days setting.
    failed_reports: int = 0


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
    failed_report_days: int = 0,
    history_days: int = 365,
    batch: int | None = None,
) -> PruneResult:
    """Delete one batch of expired rows: at most `batch` per table. Caller owns the transaction.

    A count equal to `batch` means more expired rows may remain, so another round is worth running.

    Failed Reports are kept forever unless `failed_report_days` is positive; their
    age starts when they failed, or when they were ingested if they never got that far.
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
    # Zero and negative values keep the current behaviour of never pruning a
    # failed Report, so a misconfigured zero cannot empty the queue.
    failed_reports = 0
    if failed_report_days > 0:
        failed_cutoff = now - timedelta(days=failed_report_days)
        failed_reports = (
            await db.execute(
                delete(Report).where(
                    Report.id.in_(
                        select(Report.id)
                        .where(
                            Report.status == REPORT_FAILED,
                            # A failed Report carries the moment it failed; the
                            # fallback covers a row that failed before it was processed.
                            func.coalesce(Report.processed_at, Report.created_at) < failed_cutoff,
                        )
                        .order_by(Report.id)
                        .limit(batch)
                    )
                )
            )
        ).rowcount
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
    # A quarantine is a human decision with an optional expiry: once that expiry passes the
    # leader clears it so the Test runs again. Not batched, and not part of the continuation
    # check in run_prune, because these are few and one round clears all of them.
    due_quarantines = (
        (
            await db.execute(
                select(TestCase).where(TestCase.quarantined.is_(True), TestCase.quarantine_expires_at <= now)
            )
        )
        .scalars()
        .all()
    )
    for tc in due_quarantines:
        logger.info("Quarantine expired; cleared for Test %s (%s)", tc.id, tc.name)
        tc.quarantined = False
        tc.quarantined_at = None
        tc.quarantine_reason = None
        tc.quarantine_expires_at = None
    return PruneResult(
        reports=reports.rowcount,
        executions=executions.rowcount,
        runs=runs.rowcount,
        job_executions=job_executions.rowcount,
        score_history=test_history + job_history,
        test_score_history=test_history,
        job_score_history=job_history,
        expired_quarantines=len(due_quarantines),
        failed_reports=failed_reports,
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
                failed_report_days=settings.failed_report_retention_days,
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
            expired_quarantines=totals.expired_quarantines + result.expired_quarantines,
            failed_reports=totals.failed_reports + result.failed_reports,
        )
        # A full batch means that table may have more expired rows; an all-empty round
        # deletes nothing, so the loop always terminates. The two score history tables
        # are checked separately: their summed total would miss a full batch in each.
        if not any(
            count == batch
            for count in (
                result.reports,
                result.failed_reports,
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
        "Pruned %s reports, %s failed reports, %s executions, %s job executions, %s runs, %s score history rows, "
        "cleared %s expired quarantines",
        totals.reports,
        totals.failed_reports,
        totals.executions,
        totals.job_executions,
        totals.runs,
        totals.score_history,
        totals.expired_quarantines,
    )
    return totals
