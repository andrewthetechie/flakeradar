"""Queued ingest: validate, store a pending Report, answer 202."""

from app.models import Project, Repo, Report
from sqlalchemy import select

from tests.conftest import AUTH, make_junit


def _url(**params) -> str:
    base = {"repo": "Acme/App", "commit_sha": "sha1"}
    base.update(params)
    return "/api/ingest?" + "&".join(f"{k}={v}" for k, v in base.items())


async def test_ingest_requires_token(client):
    resp = await client.post(_url(), content=make_junit([("t1", "passed")]))
    assert resp.status_code == 401


async def test_ingest_queues_report(client, db):
    resp = await client.post(
        _url(project="Backend", root="./server/", branch="feat", ci_run_id="42-1"),
        content=make_junit([("t1", "passed")]),
        headers=AUTH,
    )
    assert resp.status_code == 202, resp.text
    body = resp.json()
    assert body["status"] == "pending"

    report = (await db.execute(select(Report).where(Report.id == body["report_id"]))).scalar_one()
    assert (report.status, report.commit_sha, report.branch, report.ci_run_id, report.root) == (
        "pending",
        "sha1",
        "feat",
        "42-1",
        "server",
    )
    assert report.body == make_junit([("t1", "passed")])
    proj = (await db.execute(select(Project).where(Project.id == report.project_id))).scalar_one()
    repo = (await db.execute(select(Repo).where(Repo.id == proj.repo_id))).scalar_one()
    assert (repo.name, proj.name) == ("acme/app", "backend")


async def test_ingest_defaults_project_to_default(client, db):
    resp = await client.post(_url(), content=make_junit([("t1", "passed")]), headers=AUTH)
    assert resp.status_code == 202
    names = (await db.execute(select(Project.name))).scalars().all()
    assert names == ["default"]


async def test_ingest_multipart_upload(client):
    resp = await client.post(
        _url(),
        headers=AUTH,
        files={"report": ("junit.xml", make_junit([("t1", "passed")]), "text/xml")},
    )
    assert resp.status_code == 202


async def test_ingest_raw_body_any_content_type(client):
    # curl --data-binary @junit.xml sends application/x-www-form-urlencoded.
    for ctype in ("application/x-www-form-urlencoded", "application/xml", "text/plain"):
        resp = await client.post(
            _url(), content=make_junit([("t1", "passed")]), headers={**AUTH, "Content-Type": ctype}
        )
        assert resp.status_code == 202, (ctype, resp.text)


async def test_ingest_rejects_missing_repo(client):
    resp = await client.post("/api/ingest?commit_sha=s", content=make_junit([("t", "passed")]), headers=AUTH)
    assert resp.status_code == 422


async def test_ingest_rejects_bad_names(client):
    for url in (_url(repo="writers-app"), _url(project="front end"), _url(root="../x")):
        resp = await client.post(url, content=make_junit([("t", "passed")]), headers=AUTH)
        assert resp.status_code == 422, url


async def test_ingest_rejects_empty_and_garbage(client, db):
    assert (await client.post(_url(), headers=AUTH)).status_code == 400
    garbage = await client.post(_url(), content=b"not xml at all", headers=AUTH)
    assert garbage.status_code == 422
    assert "Not a valid JUnit XML report" in garbage.json()["detail"]
    assert (await db.execute(select(Report))).first() is None
