"""Score history (ADR 0007): daily snapshots, and the trend read from them.

Written in the same transaction as each rescore. The score and Proven flake
count are replaced (end-of-day value); executions and failures are added.
"""

from typing import Any, Literal

from sqlalchemy.dialects.postgresql import insert as pg_insert
from sqlalchemy.ext.asyncio import AsyncSession

from .batching import chunks
from .models import TestScoreHistory

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


async def upsert_test_history(db: AsyncSession, rows: list[dict[str, Any]]) -> None:
    """rows: {test_case_id, day, flakiness_score, confirmed_flake_count, executions, failures}.

    (test_case_id, day) must be unique within `rows`.
    """
    for chunk in chunks(rows):
        stmt = pg_insert(TestScoreHistory).values(list(chunk))
        stmt = stmt.on_conflict_do_update(
            index_elements=[TestScoreHistory.test_case_id, TestScoreHistory.day],
            set_={
                "flakiness_score": stmt.excluded.flakiness_score,
                "confirmed_flake_count": stmt.excluded.confirmed_flake_count,
                "executions": TestScoreHistory.executions + stmt.excluded.executions,
                "failures": TestScoreHistory.failures + stmt.excluded.failures,
            },
        )
        await db.execute(stmt)
