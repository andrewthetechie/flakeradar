"""Report status, listing, summary and retry."""
from app.models import REPORT_FAILED, REPORT_PROCESSED
from tests.conftest import AUTH
from tests.factories import make_project, make_report


async def _seed(db):
    proj = await make_project(db, "acme/app", "backend")
    pending = await make_report(db, proj, b"<x/>", commit_sha="a")
    failed = await make_report(db, proj, b"<x/>", commit_sha="b", status=REPORT_FAILED)
    failed.error = "ParseError: boom"
    done = await make_report(db, proj, b"<x/>", commit_sha="c", status=REPORT_PROCESSED)
    await db.commit()
    return pending, failed, done


async def test_get_report(client, db):
    pending, _, _ = await _seed(db)
    body = (await client.get(f"/api/reports/{pending.id}")).json()
    assert body["repo"] == "acme/app" and body["project"] == "backend"
    assert body["status"] == "pending" and body["counts"] is None
    assert "body" not in body
    assert (await client.get("/api/reports/9999")).status_code == 404


async def test_list_and_summary(client, db):
    pending, failed, done = await _seed(db)
    ids = [r["id"] for r in (await client.get("/api/reports")).json()]
    assert ids == [done.id, failed.id, pending.id]  # newest first
    only_failed = (await client.get("/api/reports?status=failed")).json()
    assert [r["error"] for r in only_failed] == ["ParseError: boom"]
    assert (await client.get("/api/reports?status=bogus")).status_code == 422
    assert (await client.get("/api/reports/summary")).json() == {"pending": 1, "failed": 1}


async def test_retry(client, db):
    pending, failed, _ = await _seed(db)
    assert (await client.post(f"/api/reports/{failed.id}/retry")).status_code == 401
    resp = await client.post(f"/api/reports/{failed.id}/retry", headers=AUTH)
    assert resp.status_code == 200
    assert resp.json()["status"] == "pending" and resp.json()["error"] is None
    conflict = await client.post(f"/api/reports/{pending.id}/retry", headers=AUTH)
    assert conflict.status_code == 409
    assert conflict.json()["detail"] == "Only failed reports can be retried"
