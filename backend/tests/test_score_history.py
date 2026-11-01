"""Score history (ADR 0007): the daily snapshot write, and the reads from it.

The trend cases are pure; the write and read cases run against Postgres through the
shared helpers, which is where the rescore paths land the day's row. The read cases pin
the two evaluation strategies of one semantic — the Python past-score lookup and the
SQL lateral condition — to the same answers for both entities.
"""

from datetime import timedelta

from app import score_history  # read through the module, so pytest cannot collect test_window_rows as a test
from app.models import Job, JobScoreHistory, TestCase, TestScoreHistory, utcnow
from app.score_history import HISTORY_DAYS, TREND_DAYS, trend_for, upsert_history, write_today_history
from sqlalchemy import select, true

from tests.factories import make_job, make_job_score, make_pipeline, make_project, make_test_case, make_test_score


async def _rows(db, model: type[TestScoreHistory] | type[JobScoreHistory]):
    """Every history row, re-read from the DB: the write is raw SQL, so the session's
    identity map keeps whatever an earlier read in the same test already loaded."""
    result = await db.execute(select(model).execution_options(populate_existing=True))
    return list(result.scalars().all())


async def _lateral_scores(db, model, owner_col, parent_col):
    """Each owner's newest Score history at least TREND_DAYS old, as the listing queries read it."""
    oldest = score_history.newest_history_before(model, owner_col, parent_col, TREND_DAYS)
    rows = await db.execute(select(parent_col, oldest.c.score).outerjoin(oldest, true()))
    return dict(rows.all())


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


async def test_score_history_reads_agree_for_a_test(db):
    proj = await make_project(db)
    seasoned = await make_test_case(db, proj, name="seasoned")
    fresh = await make_test_case(db, proj, name="fresh")
    today = utcnow().date()
    cutoff = today - timedelta(days=TREND_DAYS)

    await make_test_score(db, seasoned, cutoff, 0.2)  # exactly on the cutoff: old, and the newest old row
    await make_test_score(db, seasoned, cutoff + timedelta(days=1), 0.9)  # one day too recent to count
    await make_test_score(db, seasoned, cutoff - timedelta(days=3), 0.1)  # older
    await make_test_score(db, seasoned, cutoff - timedelta(days=10), 0.05)  # oldest of the old rows
    await make_test_score(db, seasoned, today - timedelta(days=HISTORY_DAYS), 0.4)  # outside the detail window
    await make_test_score(db, seasoned, today - timedelta(days=HISTORY_DAYS - 1), 0.3)  # first row inside it
    await make_test_score(db, fresh, today, 0.6)  # no row at or before the cutoff

    ids = [seasoned.id, fresh.id]
    past = await score_history.past_test_scores(db, ids)
    assert past == {seasoned.id: 0.2}  # the newest old row wins; the newer row does not count
    assert trend_for(0.6, past.get(fresh.id)) is None  # no old row, so no key and no trend

    lateral = await _lateral_scores(db, TestScoreHistory, TestScoreHistory.test_case_id, TestCase.id)
    assert lateral == {seasoned.id: 0.2, fresh.id: None}  # same semantic, as a per-row condition
    assert {owner: past.get(owner) for owner in lateral} == lateral  # the two strategies agree

    rows = await score_history.test_window_rows(db, seasoned.id)
    assert [(row.day, row.flakiness_score) for row in rows] == [
        (today - timedelta(days=HISTORY_DAYS - 1), 0.3),
        (cutoff - timedelta(days=10), 0.05),
        (cutoff - timedelta(days=3), 0.1),
        (cutoff, 0.2),
        (cutoff + timedelta(days=1), 0.9),
    ]  # the row exactly on the window edge is out; oldest first
    assert [row.day for row in await score_history.test_window_rows(db, fresh.id)] == [today]
    # The per-entity surface is the generic one, with the model and owner column fixed.
    assert await score_history.window_rows(db, TestScoreHistory, TestScoreHistory.test_case_id, seasoned.id) == rows
    assert await score_history.past_scores(db, TestScoreHistory, TestScoreHistory.test_case_id, ids) == past


async def test_score_history_reads_agree_for_a_job(db):
    pipe = await make_pipeline(db)
    seasoned = await make_job(db, pipe, name="seasoned")
    fresh = await make_job(db, pipe, name="fresh")
    today = utcnow().date()
    cutoff = today - timedelta(days=TREND_DAYS)

    await make_job_score(db, seasoned, cutoff, 0.2)
    await make_job_score(db, seasoned, cutoff + timedelta(days=1), 0.9)
    await make_job_score(db, seasoned, cutoff - timedelta(days=3), 0.1)
    await make_job_score(db, seasoned, cutoff - timedelta(days=10), 0.05)
    await make_job_score(db, seasoned, today - timedelta(days=HISTORY_DAYS), 0.4)
    await make_job_score(db, seasoned, today - timedelta(days=HISTORY_DAYS - 1), 0.3)
    await make_job_score(db, fresh, today, 0.6)

    ids = [seasoned.id, fresh.id]
    past = await score_history.past_job_scores(db, ids)
    assert past == {seasoned.id: 0.2}
    assert trend_for(0.6, past.get(fresh.id)) is None

    lateral = await _lateral_scores(db, JobScoreHistory, JobScoreHistory.job_id, Job.id)
    assert lateral == {seasoned.id: 0.2, fresh.id: None}
    assert {owner: past.get(owner) for owner in lateral} == lateral

    rows = await score_history.job_window_rows(db, seasoned.id)
    assert [(row.day, row.flakiness_score) for row in rows] == [
        (today - timedelta(days=HISTORY_DAYS - 1), 0.3),
        (cutoff - timedelta(days=10), 0.05),
        (cutoff - timedelta(days=3), 0.1),
        (cutoff, 0.2),
        (cutoff + timedelta(days=1), 0.9),
    ]
    assert [row.day for row in await score_history.job_window_rows(db, fresh.id)] == [today]
    assert await score_history.window_rows(db, JobScoreHistory, JobScoreHistory.job_id, seasoned.id) == rows
    assert await score_history.past_scores(db, JobScoreHistory, JobScoreHistory.job_id, ids) == past
