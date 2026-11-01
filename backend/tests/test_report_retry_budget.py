"""One shared retry budget, spent through one requeue transition (ADR 0002, issue #91).

Both retry kinds — the manual endpoint (`POST /api/reports/{id}/retry`) and the
processor's transient-failure path — apply `report_intake.requeue_report`. That is what
this file pins: the same `retry_count + 1`, `status = pending`, `processed_at = None`
reset on both sides, `error` being the whole difference between them (a manual retry
clears it, a transient one keeps it), and a single `max_report_retries` budget that both
kinds spend out of the same column and cap at the same number.
"""

import logging

from app import report_intake
from app.config import Settings
from app.models import REPORT_FAILED, REPORT_PENDING, utcnow
from app.processing import process_next
from app.routers import reports as reports_router
from sqlalchemy.exc import OperationalError
from sqlalchemy.orm import object_session

from tests.conftest import AUTH, make_junit
from tests.factories import make_project, make_report

_BODY = make_junit([("t", "failed")])
_MAX = 2  # one max_report_retries, patched into both retry kinds' settings


async def _fail_transiently(monkeypatch, max_retries: int) -> None:
    """Make processing fail with a transient DB error, under `max_retries`.

    The two kinds read the budget from two places: the processor calls
    `app.processing.get_settings`, the endpoint the `get_settings` imported into the
    router module. Patching only one would silently leave the other at the default 3.
    """

    async def boom(db, report):
        raise OperationalError("stmt", {}, Exception("connection reset"))

    def settings() -> Settings:
        return Settings(max_report_retries=max_retries)

    monkeypatch.setattr("app.processing.process_report", boom)
    monkeypatch.setattr("app.processing.get_settings", settings)
    monkeypatch.setattr(reports_router, "get_settings", settings)


async def _seed_a_failed_junit_report(db, commit_sha: str = "sha1"):
    """A failed JUnit row, seeded the way `test_reports_api._seed` does, plus the
    `processed_at` the permanent-fail transition leaves behind."""
    proj = await make_project(db, "acme/app", "backend")
    rep = await make_report(db, proj, _BODY, commit_sha=commit_sha, status=REPORT_FAILED)
    rep.error = "ParseError: boom"
    rep.processed_at = utcnow()
    await db.commit()
    return proj, rep


async def test_manual_retry_applies_the_requeue_transition(client, db):
    _, rep = await _seed_a_failed_junit_report(db)

    assert (await client.post(f"/api/reports/{rep.id}/retry")).status_code == 401
    resp = await client.post(f"/api/reports/{rep.id}/retry", headers=AUTH)
    assert resp.status_code == 200
    assert resp.json()["status"] == REPORT_PENDING and resp.json()["error"] is None

    await db.refresh(rep)
    assert rep.status == REPORT_PENDING
    # A manual retry starts clean: the error is cleared, and the row is un-processed.
    assert (rep.error, rep.processed_at) == (None, None)
    assert rep.retry_count == 1


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


async def test_both_retry_kinds_reset_the_same_fields(client, db, session_factory, monkeypatch):
    proj = await make_project(db, "acme/app", "backend")
    manual = await make_report(db, proj, _BODY, commit_sha="manual", status=REPORT_FAILED)
    transient = await make_report(db, proj, _BODY, commit_sha="transient")
    for rep in (manual, transient):
        rep.error = "ParseError: boom"
        rep.processed_at = utcnow()
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
    assert (manual.status, manual.processed_at, manual.retry_count) == (
        transient.status,
        transient.processed_at,
        transient.retry_count,
    )
    assert manual.status == REPORT_PENDING and manual.retry_count == 1
    # `error` is the only field the two kinds differ on.
    assert manual.error is None
    assert transient.error == outcome.error and transient.error is not None


async def test_one_budget_is_spent_by_both_kinds(client, db, session_factory, monkeypatch):
    proj = await make_project(db, "acme/app", "backend")
    rep = await make_report(db, proj, _BODY, commit_sha="shared")
    await db.commit()
    await _fail_transiently(monkeypatch, _MAX)

    # One transient retry spends one unit of the budget...
    outcome = await process_next(session_factory)
    assert outcome is not None and outcome.status == REPORT_PENDING
    await db.refresh(rep)
    assert (rep.status, rep.retry_count) == (REPORT_PENDING, 1)

    # ...and the endpoint reads the same column: with max_report_retries=2 it allows
    # exactly the remaining retry.
    rep.status = REPORT_FAILED
    await db.commit()
    resp = await client.post(f"/api/reports/{rep.id}/retry", headers=AUTH)
    assert resp.status_code == 200
    await db.refresh(rep)
    assert (rep.status, rep.retry_count) == (REPORT_PENDING, 2)

    # Nothing left to spend: the partly-transient budget is what the endpoint 409s on.
    rep.status = REPORT_FAILED
    await db.commit()
    capped = await client.post(f"/api/reports/{rep.id}/retry", headers=AUTH)
    assert capped.status_code == 409
    assert capped.json()["detail"] == "Max retries reached for this report"
    await db.refresh(rep)
    assert (rep.status, rep.retry_count) == (REPORT_FAILED, 2)


async def test_both_kinds_cap_at_the_same_max_report_retries(client, db, session_factory, monkeypatch):
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

    # The manual endpoint 409s at exactly that N.
    resp = await client.post(f"/api/reports/{rep.id}/retry", headers=AUTH)
    assert resp.status_code == 409
    assert resp.json()["detail"] == "Max retries reached for this report"
    await db.refresh(rep)
    assert (rep.status, rep.retry_count) == (REPORT_FAILED, _MAX)


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
