"""Queued ingest: validate, store a pending Report, answer 202."""

from app.models import Project, Repo, Report, TestRun
from app.processing import process_next
from sqlalchemy import func, select

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


async def test_ingest_stores_and_trims_default_branch(client, db):
    resp = await client.post(_url(default_branch="  main  "), content=make_junit([("t1", "passed")]), headers=AUTH)
    assert resp.status_code == 202
    report = (await db.execute(select(Report).where(Report.id == resp.json()["report_id"]))).scalar_one()
    assert report.default_branch == "main"


async def test_ingest_empty_default_branch_becomes_none(client, db):
    resp = await client.post(_url(default_branch="   "), content=make_junit([("t1", "passed")]), headers=AUTH)
    assert resp.status_code == 202
    report = (await db.execute(select(Report).where(Report.id == resp.json()["report_id"]))).scalar_one()
    assert report.default_branch is None


async def test_ingest_attribution_params_round_trip(client, db):
    resp = await client.post(
        _url(ci_job_id="  42 ", ci_run_attempt="2", pipeline=".github/workflows/ci.yml"),
        content=make_junit([("t1", "passed")]),
        headers=AUTH,
    )
    assert resp.status_code == 202
    report = (await db.execute(select(Report).where(Report.id == resp.json()["report_id"]))).scalar_one()
    assert report.ci_job_id == "42"
    assert report.ci_run_attempt == 2
    assert report.pipeline == ".github/workflows/ci.yml"


async def test_ingest_empty_attribution_params_become_none(client, db):
    resp = await client.post(
        _url(ci_job_id="   ", pipeline="   "),
        content=make_junit([("t1", "passed")]),
        headers=AUTH,
    )
    assert resp.status_code == 202
    report = (await db.execute(select(Report).where(Report.id == resp.json()["report_id"]))).scalar_one()
    assert report.ci_job_id is None and report.pipeline is None and report.ci_run_attempt is None


async def test_ingest_rejects_ci_run_attempt_zero(client):
    resp = await client.post(
        _url(ci_run_attempt="0"),
        content=make_junit([("t1", "passed")]),
        headers=AUTH,
    )
    assert resp.status_code == 422


async def test_ingest_rejects_line_base_two(client):
    resp = await client.post(
        _url(line_base="2"),
        content=make_junit([("t1", "passed")]),
        headers=AUTH,
    )
    assert resp.status_code == 422


async def test_ingest_stores_line_base(client, db):
    resp = await client.post(
        _url(line_base="0"),
        content=make_junit([("t1", "passed")]),
        headers=AUTH,
    )
    assert resp.status_code == 202, resp.text
    report = (await db.execute(select(Report).where(Report.id == resp.json()["report_id"]))).scalar_one()
    assert report.line_base == 0


async def test_ingest_line_base_defaults_to_one(client, db):
    resp = await client.post(_url(), content=make_junit([("t1", "passed")]), headers=AUTH)
    assert resp.status_code == 202, resp.text
    report = (await db.execute(select(Report).where(Report.id == resp.json()["report_id"]))).scalar_one()
    assert report.line_base == 1


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


async def test_ingest_retried_upload_is_a_duplicate(client, db, session_factory):
    """curl --retry after a lost response must not create a second Run."""
    url = _url(ci_run_id="55", ci_job_id="7", ci_run_attempt="1")
    content = make_junit([("t1", "failed")])
    first = await client.post(url, content=content, headers=AUTH)
    second = await client.post(url, content=content, headers=AUTH)
    assert first.status_code == second.status_code == 202
    assert first.json()["report_id"] == second.json()["report_id"]
    assert first.json()["duplicate"] is False and second.json()["duplicate"] is True

    while await process_next(session_factory):
        pass
    assert (await db.execute(select(func.count()).select_from(Report))).scalar_one() == 1
    assert (await db.execute(select(func.count()).select_from(TestRun))).scalar_one() == 1


async def test_ingest_duplicate_check_keys_on_job_and_project(client, db):
    content = make_junit([("t1", "passed")])
    responses = [
        await client.post(url, content=content, headers=AUTH)
        for url in (
            _url(ci_job_id="1"),
            _url(ci_job_id="2"),
            _url(project="other", ci_job_id="1"),
        )
    ]
    assert all(r.status_code == 202 for r in responses)
    assert [r.json()["duplicate"] for r in responses] == [False, False, False]
    assert len({r.json()["report_id"] for r in responses}) == 3
    # The very same upload again is a duplicate of the first one.
    retry = await client.post(_url(ci_job_id="1"), content=content, headers=AUTH)
    assert retry.json()["duplicate"] is True
    assert retry.json()["report_id"] == responses[0].json()["report_id"]
    assert (await db.execute(select(func.count()).select_from(Report))).scalar_one() == 3


async def test_ingest_duplicate_check_keys_on_line_base(client, db):
    """A corrected re-upload (same bytes, other line_base) must be queued."""
    content = make_junit([("t1", "failed")])
    default = await client.post(_url(ci_run_id="61"), content=content, headers=AUTH)
    corrected = await client.post(_url(ci_run_id="61", line_base="0"), content=content, headers=AUTH)
    assert default.status_code == corrected.status_code == 202
    assert corrected.json()["duplicate"] is False
    assert corrected.json()["report_id"] != default.json()["report_id"]
    # The same line_base again is still the retry of that upload.
    retry = await client.post(_url(ci_run_id="61", line_base="0"), content=content, headers=AUTH)
    assert retry.json()["duplicate"] is True
    assert retry.json()["report_id"] == corrected.json()["report_id"]
    assert (await db.execute(select(func.count()).select_from(Report))).scalar_one() == 2
