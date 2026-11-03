"""Who spends the retry budget, and who refills it (issue #13).

Only the processor's transient-failure path spends it: it applies
`report_intake.requeue_report` (`retry_count + 1`) and caps at `max_report_retries`.
The manual endpoint applies `report_intake.retry_failed_report` instead, which resets
`retry_count` to 0 — a human decision is never blocked by the automatic retries that
came before it, and the spent budget becomes a fresh one.
"""

import logging

from app import report_intake
from app.config import Settings
from app.models import REPORT_FAILED, REPORT_PENDING, utcnow
from app.processing import process_next
from sqlalchemy.exc import OperationalError
from sqlalchemy.orm import object_session

from tests.conftest import AUTH, make_junit
from tests.factories import make_project, make_report

_BODY = make_junit([("t", "failed")])
_MAX = 2  # one max_report_retries, patched into the processor's settings


async def _fail_transiently(monkeypatch, max_retries: int) -> None:
    """Make processing fail with a transient DB error, under `max_retries`.

    The manual endpoint no longer reads the budget, so only the processor's
    `app.processing.get_settings` needs patching.
    """

    async def boom(db, report):
        raise OperationalError("stmt", {}, Exception("connection reset"))

    monkeypatch.setattr("app.processing.process_report", boom)
    monkeypatch.setattr("app.processing.get_settings", lambda: Settings(max_report_retries=max_retries))


async def _seed_a_failed_junit_report(db, commit_sha: str = "sha1"):
    """A failed JUnit row, seeded the way `test_reports_api._seed` does, plus the
    `processed_at` the permanent-fail transition leaves behind."""
    proj = await make_project(db, "acme/app", "backend")
    rep = await make_report(db, proj, _BODY, commit_sha=commit_sha, status=REPORT_FAILED)
    rep.error = "ParseError: boom"
    rep.processed_at = utcnow()
    await db.commit()
    return proj, rep


async def test_manual_retry_applies_the_manual_retry_transition(client, db):
    _, rep = await _seed_a_failed_junit_report(db)
    rep.retry_count = _MAX  # the automatic budget is already spent
    await db.commit()

    assert (await client.post(f"/api/reports/{rep.id}/retry")).status_code == 401
    resp = await client.post(f"/api/reports/{rep.id}/retry", headers=AUTH)
    assert resp.status_code == 200
    assert resp.json()["status"] == REPORT_PENDING and resp.json()["error"] is None
    assert resp.json()["retry_count"] == 0

    await db.refresh(rep)
    assert rep.status == REPORT_PENDING
    # A manual retry starts clean: the error is cleared, the row is un-processed, and
    # the automatic-retry budget is brand new.
    assert (rep.error, rep.processed_at) == (None, None)
    assert rep.retry_count == 0


async def test_transient_retry_applies_the_requeue_transition(db, session_factory, monkeypatch, caplog):
    proj = await make_project(db, "acme/app", "backend")
    rep = await make_report(db, proj, _BODY, commit_sha="sha1")
    await db.commit()
    await _fail_transiently(monkeypatch, _MAX)

    with caplog.at_level(logging.WARNING, logger="flakeradar.processing"):
        outcome = await process_next(session_factory)
    assert outcome is not None and outcome.report_id == rep.id
    assert outcome.status == REPORT_PENDING
    # The warning names the retry it just spent, not the one before it.
    assert [r.getMessage() for r in caplog.records if "transient failure" in r.getMessage()] == [
        f"Report {rep.id} transient failure (retry 1/{_MAX}): {outcome.error}"
    ]

    await db.refresh(rep)
    assert rep.status == REPORT_PENDING
    assert rep.processed_at is None
    assert rep.retry_count == 1
    # A transient retry keeps its error visible instead of clearing it.
    assert outcome.error is not None
    assert rep.error == outcome.error


async def test_both_retry_kinds_reset_the_queue_fields(client, db, session_factory, monkeypatch):
    proj = await make_project(db, "acme/app", "backend")
    manual = await make_report(db, proj, _BODY, commit_sha="manual", status=REPORT_FAILED)
    transient = await make_report(db, proj, _BODY, commit_sha="transient")
    for rep in (manual, transient):
        rep.error = "ParseError: boom"
        rep.processed_at = utcnow()
    manual.retry_count = _MAX  # a spent budget, which the manual retry must reset
    await db.commit()
    await _fail_transiently(monkeypatch, _MAX)

    # The transient kind first: the manual retry would leave its row pending, and the
    # processor claims the oldest pending Report.
    outcome = await process_next(session_factory)
    assert outcome is not None and outcome.report_id == transient.id
    resp = await client.post(f"/api/reports/{manual.id}/retry", headers=AUTH)
    assert resp.status_code == 200

    await db.refresh(manual)
    await db.refresh(transient)
    # Back in the queue, both rows look the same.
    assert manual.status == transient.status == REPORT_PENDING
    assert manual.processed_at is None and transient.processed_at is None
    # `error` and `retry_count` are the fields the two kinds differ on: a transient
    # retry spends one automatic retry, a manual one resets the budget to 0.
    assert manual.error is None
    assert transient.error == outcome.error and transient.error is not None
    assert manual.retry_count == 0 and transient.retry_count == 1


async def test_transient_retries_cap_at_max_report_retries(client, db, session_factory, monkeypatch):
    proj = await make_project(db, "acme/app", "backend")
    rep = await make_report(db, proj, _BODY, commit_sha="cap")
    await db.commit()
    await _fail_transiently(monkeypatch, _MAX)

    for expected_rc in range(1, _MAX + 1):
        outcome = await process_next(session_factory)
        assert outcome is not None and outcome.status == REPORT_PENDING
        await db.refresh(rep)
        assert (rep.status, rep.retry_count) == (REPORT_PENDING, expected_rc)

    # The cap: the last attempt fails permanently at the same retry_count.
    outcome = await process_next(session_factory)
    assert outcome is not None and outcome.status == REPORT_FAILED
    await db.refresh(rep)
    assert (rep.status, rep.retry_count) == (REPORT_FAILED, _MAX)
    assert await process_next(session_factory) is None  # never pending again

    # A manual retry is not capped: at exactly that N it returns 200 and resets.
    resp = await client.post(f"/api/reports/{rep.id}/retry", headers=AUTH)
    assert resp.status_code == 200
    await db.refresh(rep)
    assert (rep.status, rep.retry_count) == (REPORT_PENDING, 0)


async def test_manual_retry_gives_a_fresh_budget(client, db, session_factory, monkeypatch):
    """After the reset the budget is really usable: the next transient failure retries."""
    proj = await make_project(db, "acme/app", "backend")
    rep = await make_report(db, proj, _BODY, commit_sha="refill", status=REPORT_FAILED)
    rep.retry_count = _MAX
    rep.error = "OperationalError: connection reset"
    rep.processed_at = utcnow()
    await db.commit()
    await _fail_transiently(monkeypatch, _MAX)

    resp = await client.post(f"/api/reports/{rep.id}/retry", headers=AUTH)
    assert resp.status_code == 200
    await db.refresh(rep)
    assert (rep.status, rep.retry_count, rep.error) == (REPORT_PENDING, 0, None)

    outcome = await process_next(session_factory)
    assert outcome is not None and outcome.status == REPORT_PENDING
    await db.refresh(rep)
    assert (rep.status, rep.retry_count) == (REPORT_PENDING, 1)


async def test_requeue_report_is_the_requeue_transition(db):
    proj = await make_project(db, "acme/app", "backend")
    row = report_intake.build_junit_report(project=proj, body=_BODY, commit_sha="sha1")
    row.retry_count = 0
    row.error = "ParseError: boom"
    row.processed_at = utcnow()
    row.counts = {"passed": 1}
    row.run_id = 7

    report_intake.requeue_report(row, error=None)

    assert row.status == REPORT_PENDING
    assert (row.error, row.processed_at) == (None, None)
    assert row.retry_count == 1
    # Field-only: the complete-transition fields stay as they were, and the caller owns
    # the transaction.
    assert (row.counts, row.run_id) == ({"passed": 1}, 7)
    assert object_session(row) is None

    # An already-spent budget keeps counting, and a message overwrites the old error.
    spent = report_intake.build_junit_report(project=proj, body=_BODY, commit_sha="sha2")
    spent.retry_count = 2
    report_intake.requeue_report(spent, error="OperationalError: db blip")
    assert (spent.retry_count, spent.status, spent.error) == (3, REPORT_PENDING, "OperationalError: db blip")


async def test_retry_failed_report_is_the_manual_retry_transition(db):
    proj = await make_project(db, "acme/app", "backend")
    row = report_intake.build_junit_report(project=proj, body=_BODY, commit_sha="sha1")
    row.status = REPORT_FAILED
    row.retry_count = 5
    row.error = "OperationalError: connection reset"
    row.processed_at = utcnow()
    row.counts = {"passed": 1}
    row.run_id = 7

    report_intake.retry_failed_report(row)

    assert row.status == REPORT_PENDING
    assert (row.error, row.processed_at) == (None, None)
    # Whatever the automatic retries spent, a manual retry starts from a fresh budget.
    assert row.retry_count == 0
    # Field-only: the complete-transition fields stay as they were, and the caller owns
    # the transaction.
    assert (row.counts, row.run_id) == ({"passed": 1}, 7)
    assert object_session(row) is None
