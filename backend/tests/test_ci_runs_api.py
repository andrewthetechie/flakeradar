"""CI run failures API: which CI run attempt, report counts, 404s (issue #5)."""

from app.config import get_settings

from tests.conftest import AUTH, make_junit
from tests.factories import (
    make_job,
    make_job_execution,
    make_pipeline,
    make_pipeline_report,
    make_project,
    make_report,
    make_run,
)


def _url(ci_run_id: str, repo: str = "acme/app", **params) -> str:
    query = "&".join(f"{k}={v}" for k, v in {"repo": repo, **params}.items())
    return f"/api/ci-runs/{ci_run_id}/failures?{query}"


async def test_unknown_repo_is_404(client):
    resp = await client.get(_url("100"))
    assert resp.status_code == 404
    assert resp.json()["detail"] == "Repo not found"


async def test_invalid_repo_is_422(client):
    resp = await client.get(_url("100", repo="nope"))
    assert resp.status_code == 422


async def test_unknown_ci_run_is_404(client, db):
    await make_project(db, "acme/app")
    await db.commit()
    resp = await client.get(_url("100"))
    assert resp.status_code == 404
    assert resp.json()["detail"] == "CI run not found"


async def test_unknown_attempt_is_404(client, db):
    proj = await make_project(db, "acme/app")
    await make_run(db, proj, ci_run_id="100", ci_run_attempt=1)
    await db.commit()
    resp = await client.get(_url("100", ci_run_attempt=3))
    assert resp.status_code == 404
    assert resp.json()["detail"] == "CI run attempt not found"


async def test_attempt_defaults_to_latest_including_pending(client, db):
    proj = await make_project(db, "acme/app")
    await make_report(db, proj, b"<x/>", ci_run_id="100", status="processed", ci_run_attempt=1)
    await make_report(db, proj, b"<x/>", ci_run_id="100", status="pending", ci_run_attempt=2)
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert body["ci_run_attempt"] == 2
    assert body["reports"] == {"pending": 1, "processed": 0, "failed": 0}

    body = (await client.get(_url("100", ci_run_attempt=1))).json()
    assert body["reports"] == {"pending": 0, "processed": 1, "failed": 0}


async def test_null_attempt_counts_as_one(client, db):
    proj = await make_project(db, "acme/app")
    await make_report(db, proj, b"<x/>", ci_run_id="100", status="processed", ci_run_attempt=None)
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert body["ci_run_attempt"] == 1
    assert body["reports"]["processed"] == 1


async def test_pipeline_reports_are_counted(client, db):
    proj = await make_project(db, "acme/app")
    await make_report(db, proj, b"<x/>", ci_run_id="1", status="processed", ci_run_attempt=1)
    await make_pipeline_report(db, "acme/app")  # ci_run_id "1", attempt 1, pending
    await db.commit()

    body = (await client.get(_url("1"))).json()
    assert body["reports"] == {"pending": 1, "processed": 1, "failed": 0}


async def test_found_by_run_after_reports_are_pruned(client, db):
    proj = await make_project(db, "acme/app")
    await make_run(db, proj, ci_run_id="200", ci_run_attempt=1)
    await db.commit()

    resp = await client.get(_url("200"))
    assert resp.status_code == 200
    body = resp.json()
    assert body["reports"] == {"pending": 0, "processed": 0, "failed": 0}
    assert body["failures"] == []
    assert body["failed_jobs"] == []
    assert body["total_failures"] == 0
    assert body["truncated"] is False


async def test_found_by_job_execution(client, db):
    pipe = await make_pipeline(db, "acme/app", ".github/workflows/ci.yml")
    job = await make_job(db, pipe, name="backend")
    await make_job_execution(db, job, "passed", ci_run_id="300", ci_run_attempt=2)
    await db.commit()

    body = (await client.get(_url("300"))).json()
    assert body["ci_run_attempt"] == 2


async def test_ci_run_id_is_scoped_to_repo(client, db):
    other = await make_project(db, "other/repo")
    await make_report(db, other, b"<x/>", ci_run_id="100", status="processed")
    await make_project(db, "acme/app")
    await db.commit()

    resp = await client.get(_url("100"))
    assert resp.status_code == 404
    assert resp.json()["detail"] == "CI run not found"


async def test_ingest_then_ask_sees_pending(client):
    resp = await client.post(
        "/api/ingest?repo=acme/app&project=backend&commit_sha=abc&ci_run_id=555&ci_run_attempt=1",
        content=make_junit([("t1", "failed")]),
        headers=AUTH,
    )
    assert resp.status_code == 202, resp.text

    resp = await client.get(_url("555"))
    assert resp.status_code == 200
    body = resp.json()
    assert body["reports"] == {"pending": 1, "processed": 0, "failed": 0}
    assert body["failures"] == []


async def test_top_level_fields(client, db):
    proj = await make_project(db, "acme/app")
    await make_run(db, proj, ci_run_id="100", ci_run_attempt=1)
    await db.commit()

    body = (await client.get(_url("100", repo="ACME/App"))).json()
    assert body["repo"] == "acme/app"
    assert body["ci_run_id"] == "100"
    assert body["flake_threshold"] == get_settings().flake_threshold
