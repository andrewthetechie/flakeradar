"""Jobs read API: leaderboard, history/outcome/explained-by, summary, test links."""

from tests.factories import (
    make_execution,
    make_job,
    make_job_execution,
    make_pipeline,
    make_project,
    make_run,
    make_test_case,
)


async def test_list_jobs_filters_stable_and_sorts_worst_first(client, db):
    p = await make_pipeline(db, "acme/app", ".github/workflows/ci.yml")
    await make_job(db, p, name="a", flakiness_score=0.6)
    await make_job(db, p, name="b", flakiness_score=0.1)
    await make_job(db, p, name="c", flakiness_score=0.0)
    await db.commit()

    body = (await client.get("/api/jobs?repo=acme/app")).json()
    assert [j["name"] for j in body["items"]] == ["a", "b"]  # non-stable, worst first
    assert body["total"] == 2

    all_jobs = (await client.get("/api/jobs?repo=acme/app&include_stable=true")).json()
    assert [j["name"] for j in all_jobs["items"]] == ["a", "b", "c"]
    assert all_jobs["total"] == 3


async def test_job_history_outcomes_and_explained_by(client, db):
    proj = await make_project(db, "acme/app", "backend")
    p = await make_pipeline(db, "acme/app", ".github/workflows/ci.yml")
    job = await make_job(db, p, name="test")
    # Two executions: a failing one (explained) and a passing one.
    await make_job_execution(db, job, status="failed", ci_job_id="J-F", branch="main")
    await make_job_execution(db, job, status="passed", ci_job_id="J-P", branch="main")
    # Link a failing Test execution to the J-F Job execution (same ci_job_id).
    tc = await make_test_case(db, proj, name="t1")
    run = await make_run(db, proj, commit_sha="s", ci_job_id="J-F")
    await make_execution(db, tc, run, status="failed")
    # An unlinked failure in a second Job counts as unexplained.
    job2 = await make_job(db, p, name="lint")
    await make_job_execution(db, job2, status="failed", ci_job_id="UNLINK", branch="main")
    await db.commit()

    body = (await client.get(f"/api/jobs/{job.id}/history")).json()
    outcomes = {e["ci_job_id"]: e for e in body["executions"]}
    assert outcomes["J-P"]["outcome"] == "passed"
    assert outcomes["J-F"]["outcome"] == "explained"
    assert outcomes["J-F"]["status"] == "failed"
    assert [(x["name"], x["status"]) for x in outcomes["J-F"]["explained_by"]] == [("t1", "failed")]
    assert body["explained_failures"] == 1
    assert body["unexplained_failures"] == 0

    body2 = (await client.get(f"/api/jobs/{job2.id}/history")).json()
    assert body2["executions"][0]["outcome"] == "failed"
    assert body2["unexplained_failures"] == 1 and body2["explained_failures"] == 0


async def test_github_issue_url_null_for_non_github(client, db):
    p = await make_pipeline(db, "acme/app", "pipeline", provider="gitlab")
    await make_job(db, p, name="build", github_issue_number=5)
    await db.commit()
    body = (await client.get("/api/jobs?repo=acme/app&include_stable=true")).json()
    j = next(x for x in body["items"] if x["name"] == "build")
    assert j["provider"] == "gitlab" and j["github_issue_url"] is None
    assert j["github_issue_number"] == 5


async def test_job_summary_counts(client, db):
    p1 = await make_pipeline(db, "acme/app", "ci.yml")
    await make_job(db, p1, name="flaky", flakiness_score=0.6, confirmed_flake_count=2)
    await make_job(db, p1, name="suspect", flakiness_score=0.1)
    await make_job(db, p1, name="stable", flakiness_score=0.0)
    await make_job_execution(db, await make_job(db, p1, name="x"), status="passed")
    await db.commit()
    body = (await client.get("/api/jobs/summary?repo=acme/app")).json()
    assert body["total_jobs"] == 4
    assert body["flaky_jobs"] == 1
    assert body["suspect_jobs"] == 1
    assert body["confirmed_flaky_jobs"] == 1
    assert body["total_job_executions"] == 1


async def test_test_history_includes_seen_in_jobs(client, db):
    proj = await make_project(db, "acme/app", "backend")
    p = await make_pipeline(db, "acme/app", "ci.yml")
    job = await make_job(db, p, name="test")
    await make_job_execution(db, job, status="passed", ci_job_id="X")
    tc = await make_test_case(db, proj, name="t1")
    run = await make_run(db, proj, commit_sha="s", ci_job_id="X")
    await make_execution(db, tc, run, status="passed")
    await db.commit()
    body = (await client.get(f"/api/tests/{tc.id}/history")).json()
    assert body["jobs"] == [{"job_id": job.id, "pipeline": "ci.yml", "name": "test"}]


async def test_jobs_404_and_422(client, db):
    assert (await client.get("/api/jobs/99999/history")).status_code == 404
    assert (await client.get("/api/jobs?repo=bad")).status_code == 422
    assert (await client.get("/api/tests/99999/history")).status_code == 404
