"""Score history (ADR 0007): daily snapshots, and the trend read from them.

Written in the same transaction as each rescore. The score and Proven flake
count are replaced (end-of-day value); executions and failures are added.
"""

from typing import Any, Literal

from sqlalchemy import ColumnElement
from sqlalchemy.dialects.postgresql import insert as pg_insert
from sqlalchemy.ext.asyncio import AsyncSession

from .batching import chunks
from .models import JobScoreHistory, TestScoreHistory, utcnow

TREND_DAYS = 14
TREND_DELTA = 0.05
HISTORY_DAYS = 90  # how much history the detail responses embed

Trend = Literal["worsening", "improving", "steady"]


def trend_for(current: float, past: float | None) -> Trend | None:
    if past is None:
        return None
    delta = current - past
    if delta >= TREND_DELTA:
        return "worsening"
    if delta <= -TREND_DELTA:
        return "improving"
    return "steady"


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
