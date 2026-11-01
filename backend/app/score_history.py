"""Score history (ADR 0007): daily snapshots, and the reads from them.

The write happens in the same transaction as each rescore. The score and Proven flake
count are replaced (end-of-day value); executions and failures are added. The reads —
the past-score lookup, its lateral twin, and the detail window — are generic over the
history model and its owner column exactly as the write is, surfaced per entity.
"""

from datetime import timedelta
from typing import Any, Literal, TypeVar

from sqlalchemy import ColumnElement, func, select
from sqlalchemy.dialects.postgresql import insert as pg_insert
from sqlalchemy.ext.asyncio import AsyncSession

from .batching import chunks
from .models import JobScoreHistory, TestScoreHistory, utcnow

TREND_DAYS = 14
TREND_DELTA = 0.05
HISTORY_DAYS = 90  # how much history the detail responses embed

Trend = Literal["worsening", "improving", "steady"]
_HistoryModel = TypeVar("_HistoryModel", TestScoreHistory, JobScoreHistory)


def trend_for(current: float, past: float | None) -> Trend | None:
    if past is None:
        return None
    delta = current - past
    if delta >= TREND_DELTA:
        return "worsening"
    if delta <= -TREND_DELTA:
        return "improving"
    return "steady"


async def past_scores(
    db: AsyncSession,
    model: type[TestScoreHistory] | type[JobScoreHistory],
    owner_col: ColumnElement[int],
    ids: list[int],
) -> dict[int, float]:
    """Score of each entity's newest history row at least TREND_DAYS old.

    The one past-scores lookup, shared by the Test and Job halves: `model` is the
    Score-history model and `owner_col` its column back to the parent row. Owners with
    no such row get no key at all, which is what makes `trend_for` answer None for them.
    """
    cutoff = utcnow().date() - timedelta(days=TREND_DAYS)
    found: dict[int, float] = {}
    for chunk in chunks(ids):
        rn = func.row_number().over(partition_by=owner_col, order_by=model.day.desc()).label("rn")
        ranked = (
            select(owner_col.label("entity_id"), model.flakiness_score.label("score"), rn)
            .where(owner_col.in_(chunk), model.day <= cutoff)
            .subquery()
        )
        rows = await db.execute(select(ranked.c.entity_id, ranked.c.score).where(ranked.c.rn == 1))
        found.update(dict(rows.all()))
    return found


async def past_test_scores(db: AsyncSession, test_ids: list[int]) -> dict[int, float]:
    """Score of each Test's newest history row at least TREND_DAYS old."""
    return await past_scores(db, TestScoreHistory, TestScoreHistory.test_case_id, test_ids)


async def past_job_scores(db: AsyncSession, job_ids: list[int]) -> dict[int, float]:
    """Score of each Job's newest history row at least TREND_DAYS old."""
    return await past_scores(db, JobScoreHistory, JobScoreHistory.job_id, job_ids)


def newest_history_before(
    model: type[TestScoreHistory] | type[JobScoreHistory],
    owner_col: ColumnElement[int],
    parent_col: ColumnElement[int],
    days: int,
):
    """Lateral subquery: this row's newest Score history at least `days` old, or NULL (ADR 0007).

    The same lookup `past_scores` does in Python, as a per-row SQL condition so a list
    query can filter with it before counting and paginating. Unlike the lookup, `days` is
    an argument: callers pass their own window (`new_within_days`, the dashboard's).
    """
    cutoff = utcnow().date() - timedelta(days=days)
    return (
        select(model.flakiness_score.label("score"))
        .where(owner_col == parent_col, model.day <= cutoff)
        .order_by(model.day.desc())
        .limit(1)
        .lateral()
    )


async def window_rows(
    db: AsyncSession,
    model: type[_HistoryModel],
    owner_col: ColumnElement[int],
    owner_id: int,
    *,
    days: int = HISTORY_DAYS,
) -> list[_HistoryModel]:
    """One owner's history rows inside the detail window, oldest first.

    The rows the detail responses serialize into `ScorePointOut`: everything strictly
    newer than `days` back, the complement of the `day <=` cutoff the trend lookup uses.
    """
    result = await db.execute(
        select(model)
        .where(owner_col == owner_id, model.day > utcnow().date() - timedelta(days=days))
        .order_by(model.day)
    )
    return list(result.scalars().all())


async def test_window_rows(db: AsyncSession, test_id: int) -> list[TestScoreHistory]:
    """A Test's history rows inside the detail window, oldest first."""
    return await window_rows(db, TestScoreHistory, TestScoreHistory.test_case_id, test_id)


async def job_window_rows(db: AsyncSession, job_id: int) -> list[JobScoreHistory]:
    """A Job's history rows inside the detail window, oldest first."""
    return await window_rows(db, JobScoreHistory, JobScoreHistory.job_id, job_id)


async def upsert_history(
    db: AsyncSession,
    model: type[TestScoreHistory] | type[JobScoreHistory],
    owner_col: ColumnElement[int],
    rows: list[dict[str, Any]],
) -> None:
    """rows: {<owner column name>, day, flakiness_score, confirmed_flake_count, executions, failures}.

    (owner, day) must be unique within `rows`.
    """
    for chunk in chunks(rows):
        stmt = pg_insert(model).values(list(chunk))
        stmt = stmt.on_conflict_do_update(
            index_elements=[owner_col, model.day],
            set_={
                "flakiness_score": stmt.excluded.flakiness_score,
                "confirmed_flake_count": stmt.excluded.confirmed_flake_count,
                "executions": model.executions + stmt.excluded.executions,
                "failures": model.failures + stmt.excluded.failures,
            },
        )
        await db.execute(stmt)


async def write_today_history(
    db: AsyncSession,
    model: type[TestScoreHistory] | type[JobScoreHistory],
    owner_col: ColumnElement[int],
    updates: list[dict[str, Any]],
    *,
    day_counts: dict[int, tuple[int, int]] | None = None,
) -> None:
    """Upsert today's history row for each rescore update row.

    `updates` items carry at least {id, flakiness_score, confirmed_flake_count}
    (the rescore update dicts). Owners missing from `day_counts` add (0, 0).
    """
    owner_key = owner_col.key
    today = utcnow().date()
    counts = day_counts or {}
    rows = []
    for u in updates:
        executions, failures = counts.get(u["id"], (0, 0))
        rows.append(
            {
                owner_key: u["id"],
                "day": today,
                "flakiness_score": u["flakiness_score"],
                "confirmed_flake_count": u["confirmed_flake_count"],
                "executions": executions,
                "failures": failures,
            }
        )
    await upsert_history(db, model, owner_col, rows)
