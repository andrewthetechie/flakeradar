"""Issue links: link a GitHub issue filed elsewhere to a Test or Job (issue #6)."""

import pytest
from app import issue_links

from tests.conftest import AUTH
from tests.factories import make_job, make_pipeline, make_project, make_test_case


async def _test(db, name="t1", repo="acme/app", project="backend", **fields):
    proj = await make_project(db, repo, project)
    tc = await make_test_case(db, proj, name=name, **fields)
    await db.commit()
    return tc


async def _job(db, name="e2e", repo="acme/app", pipeline=".github/workflows/ci.yml", provider="github", **fields):
    pipe = await make_pipeline(db, repo, pipeline, provider=provider)
    job = await make_job(db, pipe, name=name, **fields)
    await db.commit()
    return job


async def test_link_test_issue(client, db):
    tc = await _test(db)
    resp = await client.put(f"/api/tests/{tc.id}/issue", json={"number": 123}, headers=AUTH)
    assert resp.status_code == 200
    body = resp.json()
    assert body["id"] == tc.id
    assert body["github_issue_number"] == 123
    assert body["github_issue_url"] == "https://github.com/acme/app/issues/123"
    await db.refresh(tc)
    assert tc.github_issue_number == 123

    history = await client.get(f"/api/tests/{tc.id}/history")
    assert history.json()["test"]["github_issue_number"] == 123


async def test_issue_link_needs_the_token(client, db):
    tc = await _test(db)
    assert (await client.put(f"/api/tests/{tc.id}/issue", json={"number": 1})).status_code == 401
    assert (
        await client.put(f"/api/tests/{tc.id}/issue", json={"number": 1}, headers={"X-API-Key": "wrong"})
    ).status_code == 401
    assert (await client.delete(f"/api/tests/{tc.id}/issue")).status_code == 401
    await db.refresh(tc)
    assert tc.github_issue_number is None


async def test_unknown_test_is_404(client, db):
    resp = await client.put("/api/tests/99999/issue", json={"number": 1}, headers=AUTH)
    assert resp.status_code == 404
    assert resp.json()["detail"] == "Test not found"

    resp = await client.delete("/api/tests/99999/issue", headers=AUTH)
    assert resp.status_code == 404
    assert resp.json()["detail"] == "Test not found"


async def test_same_number_again_is_a_no_op(client, db):
    tc = await _test(db, github_issue_number=5)
    resp = await client.put(f"/api/tests/{tc.id}/issue", json={"number": 5}, headers=AUTH)
    assert resp.status_code == 200
    assert resp.json()["github_issue_number"] == 5


async def test_different_number_is_409_unless_replace(client, db):
    tc = await _test(db, github_issue_number=5)
    resp = await client.put(f"/api/tests/{tc.id}/issue", json={"number": 6}, headers=AUTH)
    assert resp.status_code == 409
    assert resp.json()["detail"] == "Test already links issue #5; pass replace=true to replace it"
    await db.refresh(tc)
    assert tc.github_issue_number == 5

    resp = await client.put(f"/api/tests/{tc.id}/issue?replace=true", json={"number": 6}, headers=AUTH)
    assert resp.status_code == 200
    assert resp.json()["github_issue_number"] == 6


async def test_url_must_be_the_issue_in_the_tests_repo(client, db):
    tc = await _test(db)
    resp = await client.put(
        f"/api/tests/{tc.id}/issue",
        json={"number": 7, "url": "https://github.com/ACME/App/issues/7/"},
        headers=AUTH,
    )
    assert resp.status_code == 200
    assert resp.json()["github_issue_number"] == 7

    resp = await client.put(
        f"/api/tests/{tc.id}/issue?replace=true",
        json={"number": 8, "url": "https://github.com/other/repo/issues/8"},
        headers=AUTH,
    )
    assert resp.status_code == 422
    assert (
        resp.json()["detail"]
        == "url must be https://github.com/acme/app/issues/8: the issue must be in the Test's own Repo"
    )
    await db.refresh(tc)
    assert tc.github_issue_number == 7

    resp = await client.put(
        f"/api/tests/{tc.id}/issue?replace=true",
        json={"number": 8, "url": "https://github.com/acme/app/issues/9"},
        headers=AUTH,
    )
    assert resp.status_code == 422


async def test_invalid_body_is_422(client, db):
    tc = await _test(db)
    for body in (
        {"number": 0},
        {"number": -1},
        {"number": 2147483648},
        {"number": "abc"},
        {},
        {"number": 1, "url": "x" * 2049},
    ):
        resp = await client.put(f"/api/tests/{tc.id}/issue", json=body, headers=AUTH)
        assert resp.status_code == 422, body


async def test_unlink_test_issue(client, db):
    tc = await _test(db, github_issue_number=5)
    resp = await client.delete(f"/api/tests/{tc.id}/issue", headers=AUTH)
    assert resp.status_code == 200
    body = resp.json()
    assert body["github_issue_number"] is None
    assert body["github_issue_url"] is None
    await db.refresh(tc)
    assert tc.github_issue_number is None

    resp = await client.delete(f"/api/tests/{tc.id}/issue", headers=AUTH)
    assert resp.status_code == 200
    assert resp.json()["github_issue_number"] is None


async def test_service_rules(db):
    tc = await _test(db)
    test_id = tc.id
    with pytest.raises(issue_links.IssueNotLinkable) as info:
        await issue_links.link_test_issue(db, test_id, 0, threshold=0.3)
    assert str(info.value) == "number must be between 1 and 2147483647"
    assert info.value.status_code == 422
    # The refused call still holds the FOR UPDATE row lock; release it.
    await db.rollback()

    assert issue_links.IssueLinkTargetNotFound.status_code == 404
    assert issue_links.IssueAlreadyLinked.status_code == 409

    out = await issue_links.link_test_issue(db, test_id, 11, threshold=0.3)
    assert out.github_issue_number == 11


# --- Jobs (issue #6) ---


async def test_link_job_issue(client, db):
    job = await _job(db)
    resp = await client.put(f"/api/jobs/{job.id}/issue", json={"number": 42}, headers=AUTH)
    assert resp.status_code == 200
    body = resp.json()
    assert body["id"] == job.id
    assert body["github_issue_number"] == 42
    assert body["github_issue_url"] == "https://github.com/acme/app/issues/42"
    await db.refresh(job)
    assert job.github_issue_number == 42

    history = await client.get(f"/api/jobs/{job.id}/history")
    assert history.json()["job"]["github_issue_number"] == 42


async def test_job_on_a_non_github_pipeline_cannot_be_linked(client, db):
    job = await _job(db, provider="gitlab", github_issue_number=3)
    resp = await client.put(f"/api/jobs/{job.id}/issue", json={"number": 4}, headers=AUTH)
    assert resp.status_code == 422
    assert resp.json()["detail"] == "Only Jobs of GitHub Pipelines can link a GitHub issue"
    await db.refresh(job)
    assert job.github_issue_number == 3

    resp = await client.delete(f"/api/jobs/{job.id}/issue", headers=AUTH)
    assert resp.status_code == 200
    assert resp.json()["github_issue_number"] is None


async def test_job_conflict_and_replace(client, db):
    job = await _job(db, github_issue_number=4)
    resp = await client.put(f"/api/jobs/{job.id}/issue", json={"number": 5}, headers=AUTH)
    assert resp.status_code == 409
    assert resp.json()["detail"] == "Job already links issue #4; pass replace=true to replace it"

    resp = await client.put(f"/api/jobs/{job.id}/issue?replace=true", json={"number": 5}, headers=AUTH)
    assert resp.status_code == 200
    assert resp.json()["github_issue_number"] == 5


async def test_job_url_must_be_the_issue_in_the_jobs_repo(client, db):
    job = await _job(db)
    resp = await client.put(
        f"/api/jobs/{job.id}/issue",
        json={"number": 6, "url": "https://github.com/other/repo/issues/6"},
        headers=AUTH,
    )
    assert resp.status_code == 422
    assert (
        resp.json()["detail"]
        == "url must be https://github.com/acme/app/issues/6: the issue must be in the Job's own Repo"
    )

    resp = await client.put(
        f"/api/jobs/{job.id}/issue",
        json={"number": 6, "url": "https://github.com/acme/app/issues/6"},
        headers=AUTH,
    )
    assert resp.status_code == 200


async def test_unknown_job_and_missing_token(client, db):
    resp = await client.put("/api/jobs/99999/issue", json={"number": 1}, headers=AUTH)
    assert resp.status_code == 404
    assert resp.json()["detail"] == "Job not found"
    assert (await client.delete("/api/jobs/99999/issue", headers=AUTH)).status_code == 404

    job = await _job(db)
    assert (await client.put(f"/api/jobs/{job.id}/issue", json={"number": 1})).status_code == 401
    assert (await client.delete(f"/api/jobs/{job.id}/issue")).status_code == 401


async def test_unlink_job_issue(client, db):
    job = await _job(db, github_issue_number=8)
    resp = await client.delete(f"/api/jobs/{job.id}/issue", headers=AUTH)
    assert resp.status_code == 200
    body = resp.json()
    assert body["github_issue_number"] is None
    assert body["github_issue_url"] is None

    resp = await client.delete(f"/api/jobs/{job.id}/issue", headers=AUTH)
    assert resp.status_code == 200
    assert resp.json()["github_issue_number"] is None
