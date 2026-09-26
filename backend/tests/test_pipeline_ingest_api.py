"""Pipeline report ingest: validation, storage, and the pre-task-04 failure path."""

import json

from app.models import Repo, Report
from sqlalchemy import select

from tests.conftest import AUTH


def _payload(**over):
    body = {
        "repo": "Acme/App",
        "provider": "github",
        "pipeline": ".github/workflows/ci.yml",
        "commit_sha": "deadbeef",
        "branch": "main",
        "default_branch": "main",
        "ci_run_id": "42",
        "ci_run_attempt": 2,
        "jobs": [
            {
                "ci_job_id": "100",
                "name": "test (ubuntu, 3.12)",
                "status": "passed",
                "url": "https://github.com/acme/app/actions/runs/42/jobs/100",
                "runner_name": "GitHub Actions 4",
                "runner_labels": ["ubuntu-latest", "3.12"],
            }
        ],
    }
    body.update(over)
    return body


async def test_valid_pipeline_report_is_queued(client, db):
    resp = await client.post("/api/ingest/pipeline", json=_payload(), headers=AUTH)
    assert resp.status_code == 202, resp.text
    body = resp.json()
    assert body["status"] == "pending"

    rep = (await db.execute(select(Report).where(Report.id == body["report_id"]))).scalar_one()
    assert rep.kind == "pipeline" and rep.project_id is None
    assert rep.pipeline == ".github/workflows/ci.yml"
    assert rep.ci_run_attempt == 2
    repo = (await db.execute(select(Repo).where(Repo.id == rep.repo_id))).scalar_one()
    assert repo.name == "acme/app"
    # The stored body is the normalized payload (repo lowercased, provider lowercased).
    stored = json.loads(rep.body)
    assert stored["repo"] == "acme/app" and stored["provider"] == "github"


async def test_pipeline_ingest_strips_names(client, db):
    payload = _payload(pipeline="  ci.yml ", branch=" feat ", default_branch="  ")
    payload["jobs"][0]["name"] = " build "
    resp = await client.post("/api/ingest/pipeline", json=payload, headers=AUTH)
    assert resp.status_code == 202, resp.text
    rep = (await db.execute(select(Report).where(Report.id == resp.json()["report_id"]))).scalar_one()
    stored = json.loads(rep.body)
    assert (stored["pipeline"], stored["branch"], stored["default_branch"]) == ("ci.yml", "feat", None)
    assert stored["jobs"][0]["name"] == "build"
    assert (rep.pipeline, rep.branch, rep.default_branch) == ("ci.yml", "feat", None)


async def test_pipeline_ingest_requires_token(client):
    assert (await client.post("/api/ingest/pipeline", json=_payload())).status_code == 401


async def test_pipeline_ingest_validation(client):
    cases = [
        {"repo": "bad"},  # not owner/name
        {"provider": "has space"},
        {"jobs": []},  # empty
        {"jobs": [{"ci_job_id": "1", "name": "n", "status": "bogus"}]},  # unknown status
        {"pipeline": "   "},  # blank after stripping
        {"branch": " "},
        {"jobs": [{"ci_job_id": "1", "name": "  ", "status": "passed"}]},
        {"jobs": [{"ci_job_id": " ", "name": "n", "status": "passed"}]},
    ]
    for kwargs in cases:
        resp = await client.post("/api/ingest/pipeline", json=_payload(**kwargs), headers=AUTH)
        assert resp.status_code == 422, (kwargs, resp.text)


async def test_pipeline_ingest_rejects_duplicate_ci_job_id(client, db):
    payload = _payload()
    payload["jobs"] = [
        {"ci_job_id": "9", "name": "a", "status": "passed"},
        {"ci_job_id": "9", "name": "b", "status": "passed"},
    ]
    resp = await client.post("/api/ingest/pipeline", json=payload, headers=AUTH)
    assert resp.status_code == 422
    assert "duplicate ci_job_id in jobs" in resp.text


async def test_pipeline_report_auto_creates_repo(client, db):
    await client.post(
        "/api/ingest/pipeline",
        json=_payload(repo="brand/NewRepo"),
        headers=AUTH,
    )
    assert (await db.execute(select(Repo.name))).scalars().all() == ["brand/newrepo"]
