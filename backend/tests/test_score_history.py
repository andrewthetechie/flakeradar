"""Score history (ADR 0007): the daily snapshot write, and the trend read from it.

The trend cases are pure; the write cases run against Postgres through the shared
helpers, which is where the rescore paths land the day's row.
"""

from datetime import timedelta

from app.models import JobScoreHistory, TestScoreHistory, utcnow
from app.score_history import trend_for, upsert_history, write_today_history
from sqlalchemy import select

from tests.factories import make_job, make_pipeline, make_project, make_test_case


async def _rows(db, model: type[TestScoreHistory] | type[JobScoreHistory]):
    """Every history row, re-read from the DB: the write is raw SQL, so the session's
    identity map keeps whatever an earlier read in the same test already loaded."""
    result = await db.execute(select(model).execution_options(populate_existing=True))
    return list(result.scalars().all())


def test_trend_for_no_history_is_null():
    assert trend_for(0.5, None) is None


def test_trend_for_worsening():
    assert trend_for(0.5, 0.1) == "worsening"


def test_trend_for_improving():
    assert trend_for(0.1, 0.5) == "improving"


def test_trend_for_steady():
    assert trend_for(0.32, 0.30) == "steady"


async def test_write_today_history_replaces_score_and_adds_counts_for_a_test(db):
    proj = await make_project(db)
    tc = await make_test_case(db, proj)
    await write_today_history(
        db,
        TestScoreHistory,
        TestScoreHistory.test_case_id,
        [{"id": tc.id, "flakiness_score": 0.5, "confirmed_flake_count": 2}],
        day_counts={tc.id: (3, 1)},
    )
    await write_today_history(
        db,
        TestScoreHistory,
        TestScoreHistory.test_case_id,
        [{"id": tc.id, "flakiness_score": 0.25, "confirmed_flake_count": 4}],
        day_counts={tc.id: (2, 1)},
    )
    rows = await _rows(db, TestScoreHistory)
    assert len(rows) == 1
    row = rows[0]
    assert row.day == utcnow().date()
    assert row.flakiness_score == 0.25  # replaced, not averaged
    assert row.confirmed_flake_count == 4
    assert (row.executions, row.failures) == (5, 2)  # added


async def test_write_today_history_replaces_score_and_adds_counts_for_a_job(db):
    pipe = await make_pipeline(db)
    job = await make_job(db, pipe)
    await write_today_history(
        db,
        JobScoreHistory,
        JobScoreHistory.job_id,
        [{"id": job.id, "flakiness_score": 0.5, "confirmed_flake_count": 2}],
        day_counts={job.id: (3, 1)},
    )
    await write_today_history(
        db,
        JobScoreHistory,
        JobScoreHistory.job_id,
        [{"id": job.id, "flakiness_score": 0.25, "confirmed_flake_count": 4}],
        day_counts={job.id: (2, 1)},
    )
    rows = await _rows(db, JobScoreHistory)
    assert len(rows) == 1
    row = rows[0]
    assert row.day == utcnow().date()
    assert row.flakiness_score == 0.25
    assert row.confirmed_flake_count == 4
    assert (row.executions, row.failures) == (5, 2)


async def test_write_today_history_defaults_missing_counts_to_zero(db):
    proj = await make_project(db)
    tc = await make_test_case(db, proj, name="no_day_counts")
    await write_today_history(
        db,
        TestScoreHistory,
        TestScoreHistory.test_case_id,
        [{"id": tc.id, "flakiness_score": 0.75, "confirmed_flake_count": 1}],
    )
    rows = await _rows(db, TestScoreHistory)
    assert len(rows) == 1
    assert (rows[0].executions, rows[0].failures) == (0, 0)
    assert rows[0].flakiness_score == 0.75
    assert rows[0].confirmed_flake_count == 1

    # A same-day rescore with no counts rewrites the day's score without adding counts.
    await write_today_history(
        db,
        TestScoreHistory,
        TestScoreHistory.test_case_id,
        [{"id": tc.id, "flakiness_score": 0.1, "confirmed_flake_count": 0}],
    )
    rows = await _rows(db, TestScoreHistory)
    assert len(rows) == 1
    assert (rows[0].executions, rows[0].failures) == (0, 0)
    assert rows[0].flakiness_score == 0.1
    assert rows[0].confirmed_flake_count == 0

    covered = await make_test_case(db, proj, name="covered")
    uncovered = await make_test_case(db, proj, name="uncovered")
    await write_today_history(
        db,
        TestScoreHistory,
        TestScoreHistory.test_case_id,
        [
            {"id": covered.id, "flakiness_score": 0.2, "confirmed_flake_count": 0},
            {"id": uncovered.id, "flakiness_score": 0.4, "confirmed_flake_count": 1},
        ],
        day_counts={covered.id: (2, 1)},
    )
    by_id = {r.test_case_id: r for r in await _rows(db, TestScoreHistory)}
    assert (by_id[covered.id].executions, by_id[covered.id].failures) == (2, 1)
    assert (by_id[uncovered.id].executions, by_id[uncovered.id].failures) == (0, 0)

    # The uncovered owner hits the conflict path too: its counts stay (0, 0), its score is replaced.
    await write_today_history(
        db,
        TestScoreHistory,
        TestScoreHistory.test_case_id,
        [{"id": uncovered.id, "flakiness_score": 0.6, "confirmed_flake_count": 3}],
        day_counts={covered.id: (1, 1)},
    )
    by_id = {r.test_case_id: r for r in await _rows(db, TestScoreHistory)}
    assert len(by_id) == 3
    assert (by_id[uncovered.id].executions, by_id[uncovered.id].failures) == (0, 0)
    assert by_id[uncovered.id].flakiness_score == 0.6
    assert by_id[uncovered.id].confirmed_flake_count == 3


async def test_upsert_history_keys_the_conflict_on_owner_and_day(db):
    proj = await make_project(db)
    tc = await make_test_case(db, proj)
    today = utcnow().date()
    yesterday = today - timedelta(days=1)
    await upsert_history(
        db,
        TestScoreHistory,
        TestScoreHistory.test_case_id,
        [
            {
                "test_case_id": tc.id,
                "day": today,
                "flakiness_score": 0.5,
                "confirmed_flake_count": 2,
                "executions": 3,
                "failures": 1,
            },
            {
                "test_case_id": tc.id,
                "day": yesterday,
                "flakiness_score": 0.9,
                "confirmed_flake_count": 5,
                "executions": 4,
                "failures": 3,
            },
        ],
    )
    await upsert_history(
        db,
        TestScoreHistory,
        TestScoreHistory.test_case_id,
        [
            {
                "test_case_id": tc.id,
                "day": today,
                "flakiness_score": 0.1,
                "confirmed_flake_count": 0,
                "executions": 2,
                "failures": 1,
            }
        ],
    )
    rows = sorted(await _rows(db, TestScoreHistory), key=lambda r: r.day, reverse=True)
    assert len(rows) == 2
    today_row, past_row = rows
    assert today_row.day == today
    assert today_row.flakiness_score == 0.1  # replaced
    assert today_row.confirmed_flake_count == 0
    assert (today_row.executions, today_row.failures) == (5, 2)  # added
    assert past_row.day == yesterday
    assert past_row.flakiness_score == 0.9  # a different day is untouched
    assert past_row.confirmed_flake_count == 5
    assert (past_row.executions, past_row.failures) == (4, 3)
