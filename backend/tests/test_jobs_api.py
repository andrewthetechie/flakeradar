"""Jobs read API: leaderboard, history/outcome/explained-by, summary, test links."""

from datetime import timedelta

from app.models import utcnow

from tests.factories import (
    make_execution,
    make_job,
    make_job_execution,
    make_job_score,
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
    # A non-summary Job is explained by failing Tests, never by sibling Jobs.
    assert outcomes["J-F"]["explained_by_jobs"] == []
    assert body["explained_failures"] == 1
    assert body["unexplained_failures"] == 0

    body2 = (await client.get(f"/api/jobs/{job2.id}/history")).json()
    assert body2["executions"][0]["outcome"] == "failed"
    assert body2["unexplained_failures"] == 1 and body2["explained_failures"] == 0


async def test_summary_job_history_lists_explaining_sibling_jobs(client, db):
    p = await make_pipeline(db, "acme/app", ".github/workflows/ci.yml")
    summary = await make_job(db, p, name="alls-green", summary=True)
    sibling = await make_job(db, p, name="test")
    await make_job_execution(db, summary, status="failed", ci_job_id="J-S", ci_run_id="run-1")
    await make_job_execution(
        db,
        sibling,
        status="failed",
        ci_job_id="J-T",
        ci_run_id="run-1",
        url="https://github.com/acme/app/actions/runs/1/jobs/2",
    )
    await make_job_execution(db, summary, status="failed", ci_job_id="J-S2", ci_run_id="run-2")
    await db.commit()

    body = (await client.get(f"/api/jobs/{summary.id}/history")).json()
    by_run = {e["ci_run_id"]: e for e in body["executions"]}
    assert by_run["run-1"]["outcome"] == "explained"
    assert by_run["run-1"]["explained_by"] == []
    assert by_run["run-1"]["explained_by_jobs"] == [
        {
            "job_id": sibling.id,
            "pipeline": ".github/workflows/ci.yml",
            "name": "test",
            "url": "https://github.com/acme/app/actions/runs/1/jobs/2",
        }
    ]
    assert body["explained_failures"] == 1
    assert by_run["run-2"]["outcome"] == "failed"  # no sibling failure in run-2
    assert by_run["run-2"]["explained_by_jobs"] == []


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
    assert body["new_flaky_7d"] == 1  # the flaky Job has no history at all, so it is new
    assert body["total_job_executions"] == 1


async def test_job_summary_new_flaky_7d(client, db):
    """new_flaky_7d on Jobs: flaky now, with no history row at least 7 days old at or above the threshold."""
    app = await make_pipeline(db, "acme/app", "ci.yml")
    other = await make_pipeline(db, "acme/other", "ci.yml")
    crossed = await make_job(db, app, name="crossed", flakiness_score=0.6)
    at_threshold = await make_job(db, app, name="at_threshold", flakiness_score=0.6)
    old_flaky = await make_job(db, app, name="old_flaky", flakiness_score=0.6)
    boundary = await make_job(db, app, name="boundary", flakiness_score=0.6)
    recent = await make_job(db, app, name="recent", flakiness_score=0.6)
    newest_old_low = await make_job(db, app, name="newest_old_low", flakiness_score=0.6)
    newest_old_high = await make_job(db, app, name="newest_old_high", flakiness_score=0.6)
    await make_job(db, app, name="suspect", flakiness_score=0.1)
    await make_job(db, other, name="elsewhere", flakiness_score=0.6)
    today = utcnow().date()
    await make_job_score(db, crossed, today - timedelta(days=20), 0.1)  # newest old row is below
    await make_job_score(db, crossed, today - timedelta(days=3), 0.6)  # several rows: counted once
    await make_job_score(db, at_threshold, today - timedelta(days=10), 0.3)  # exactly at the threshold
    await make_job_score(db, old_flaky, today - timedelta(days=30), 0.6)
    await make_job_score(db, boundary, today - timedelta(days=7), 0.6)  # exactly 7 days old counts as old
    await make_job_score(db, recent, today - timedelta(days=6), 0.6)  # nothing 7 days old yet
    await make_job_score(db, newest_old_low, today - timedelta(days=25), 0.6)
    await make_job_score(db, newest_old_low, today - timedelta(days=10), 0.1)  # the NEWEST old row is below
    await make_job_score(db, newest_old_high, today - timedelta(days=25), 0.1)
    await make_job_score(db, newest_old_high, today - timedelta(days=10), 0.6)  # the newest old row is at it
    await db.commit()

    scoped = (await client.get("/api/jobs/summary?repo=acme/app")).json()
    assert scoped["flaky_jobs"] == 7
    # crossed, recent and newest_old_low; the other four were already flaky 7 days ago
    assert scoped["new_flaky_7d"] == 3
    assert (await client.get("/api/jobs/summary?repo=acme/other")).json()["new_flaky_7d"] == 1
    assert (await client.get("/api/jobs/summary")).json()["new_flaky_7d"] == 4  # + elsewhere, with no history


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


async def test_job_out_includes_summary_flag(client, db):
    p = await make_pipeline(db, "acme/app", "ci.yml")
    await make_job(db, p, name="alls-green", summary=True)
    await make_job(db, p, name="test")
    await db.commit()

    body = (await client.get("/api/jobs?repo=acme/app&include_stable=true")).json()
    assert {j["name"]: j["summary"] for j in body["items"]} == {"alls-green": True, "test": False}


async def test_jobs_404_and_422(client, db):
    assert (await client.get("/api/jobs/99999/history")).status_code == 404
    assert (await client.get("/api/jobs?repo=bad")).status_code == 422
    assert (await client.get("/api/tests/99999/history")).status_code == 404


async def test_job_trend_from_history_and_score_history(client, db):
    p = await make_pipeline(db, "acme/app", ".github/workflows/ci.yml")
    job = await make_job(db, p, name="e2e", flakiness_score=0.6)
    today = utcnow().date()
    await make_job_score(db, job, today - timedelta(days=20), 0.1)
    await make_job_score(db, job, today, 0.6)
    await db.commit()

    body = (await client.get("/api/jobs?repo=acme/app")).json()
    assert body["items"][0]["trend"] == "worsening"
    assert body["items"][0]["clean_streak"] == 0

    hist = (await client.get(f"/api/jobs/{job.id}/history")).json()
    assert [p["day"] for p in hist["score_history"]] == [
        (today - timedelta(days=20)).isoformat(),
        today.isoformat(),
    ]


async def test_new_within_days_filter_on_jobs(client, db):
    """new_within_days=N on Jobs: flaky now, and no old-enough history row at or above the threshold."""
    p = await make_pipeline(db, "acme/app", ".github/workflows/ci.yml")
    crossed = await make_job(db, p, name="crossed", flakiness_score=0.6)
    old_flaky = await make_job(db, p, name="old_flaky", flakiness_score=0.6)
    no_old_history = await make_job(db, p, name="no_old_history", flakiness_score=0.6)
    await make_job(db, p, name="suspect", flakiness_score=0.1)
    today = utcnow().date()
    await make_job_score(db, crossed, today - timedelta(days=20), 0.1)
    await make_job_score(db, crossed, today - timedelta(days=3), 0.6)
    await make_job_score(db, old_flaky, today - timedelta(days=30), 0.6)
    await make_job_score(db, no_old_history, today - timedelta(days=1), 0.6)
    await db.commit()

    week = (await client.get("/api/jobs?repo=acme/app&new_within_days=7")).json()
    assert sorted(j["name"] for j in week["items"]) == ["crossed", "no_old_history"]
    assert week["total"] == 2  # old_flaky and the below-threshold suspect go before pagination
    # The filter runs in SQL, so it paginates over the filtered set only.
    first_page = (await client.get("/api/jobs?repo=acme/app&new_within_days=7&page_size=1&page=1")).json()
    assert len(first_page["items"]) == 1
    assert first_page["total"] == 2
    second_page = (await client.get("/api/jobs?repo=acme/app&new_within_days=7&page_size=1&page=2")).json()
    assert sorted(j["name"] for j in first_page["items"] + second_page["items"]) == ["crossed", "no_old_history"]

    days2 = (await client.get("/api/jobs?repo=acme/app&new_within_days=2")).json()
    assert sorted(j["name"] for j in days2["items"]) == ["no_old_history"]  # crossed's -3 row is at 0.6
    assert days2["total"] == 1

    assert (await client.get("/api/jobs?repo=acme/app&new_within_days=0")).status_code == 422
    assert (await client.get("/api/jobs?repo=acme/app&new_within_days=91")).status_code == 422


async def _seed_jobs_q(db):
    """ci.yml: build-thing 0.6, lint-fast 0.0 (stable). nightly.yml: e2e 0.3."""
    ci = await make_pipeline(db, "acme/app", ".github/workflows/ci.yml")
    await make_job(db, ci, name="build-thing", flakiness_score=0.6)
    await make_job(db, ci, name="lint-fast", flakiness_score=0.0)
    nightly = await make_pipeline(db, "acme/app", ".github/workflows/nightly.yml")
    await make_job(db, nightly, name="e2e", flakiness_score=0.3)
    await db.commit()


async def test_q_filter_matches_job_name_including_stable(client, db):
    await _seed_jobs_q(db)
    page = (await client.get("/api/jobs?repo=acme/app&q=FAST")).json()  # no include_stable=true
    assert [j["name"] for j in page["items"]] == ["lint-fast"]
    assert page["total"] == 1


async def test_q_filter_matches_pipeline_name(client, db):
    await _seed_jobs_q(db)
    page = (await client.get("/api/jobs?repo=acme/app&q=nightly")).json()
    assert [j["name"] for j in page["items"]] == ["e2e"]


async def test_q_filter_is_literal_and_length_bounded(client, db):
    await _seed_jobs_q(db)
    assert (await client.get("/api/jobs?repo=acme/app&q=%25")).json()["total"] == 0  # "%" is literal
    assert (await client.get("/api/jobs?repo=acme/app&q=")).json()["total"] == 2  # empty q: stable excluded
    assert (await client.get(f"/api/jobs?repo=acme/app&q={'a' * 201}")).status_code == 422


async def test_q_filter_composes_with_sort(client, db):
    await _seed_jobs_q(db)
    page = (await client.get("/api/jobs?repo=acme/app&q=fast&sort=last_seen")).json()
    assert [j["name"] for j in page["items"]] == ["lint-fast"]
    assert page["page"] == 1


async def test_pipeline_filter_is_an_exact_name_match(client, db):
    await _seed_jobs_q(db)
    page = (await client.get("/api/jobs?repo=acme/app&pipeline=.github/workflows/nightly.yml")).json()
    assert [j["name"] for j in page["items"]] == ["e2e"]
    # The stable job of the other pipeline stays hidden, as it does unfiltered.
    page = (await client.get("/api/jobs?repo=acme/app&pipeline=.github/workflows/ci.yml")).json()
    assert [j["name"] for j in page["items"]] == ["build-thing"]
    assert (await client.get("/api/jobs?repo=acme/app&pipeline=nightly.yml")).json()["total"] == 0
    assert (await client.get("/api/jobs?repo=acme/app&pipeline=.github/workflows/NIGHTLY.yml")).json()["total"] == 0


async def test_pipeline_filter_composes_with_include_stable(client, db):
    await _seed_jobs_q(db)
    page = (await client.get("/api/jobs?repo=acme/app&pipeline=.github/workflows/ci.yml&include_stable=true")).json()
    assert [j["name"] for j in page["items"]] == ["build-thing", "lint-fast"]


async def test_has_issue_filter_on_jobs(client, db):
    p = await make_pipeline(db, "acme/app", ".github/workflows/ci.yml")
    await make_job(db, p, name="tracked", flakiness_score=0.6, github_issue_number=7)
    await make_job(db, p, name="untracked", flakiness_score=0.4)
    await db.commit()

    assert [j["name"] for j in (await client.get("/api/jobs?repo=acme/app&has_issue=true")).json()["items"]] == [
        "tracked"
    ]
    assert [j["name"] for j in (await client.get("/api/jobs?repo=acme/app&has_issue=false")).json()["items"]] == [
        "untracked"
    ]
    assert (await client.get("/api/jobs?repo=acme/app")).json()["total"] == 2  # no filter: both


async def test_job_summary_lists_pipelines(client, db):
    await _seed_jobs_q(db)
    other = await make_pipeline(db, "acme/other", ".github/workflows/release.yml")
    await make_job(db, other, name="publish", flakiness_score=0.2)
    await db.commit()

    scoped = (await client.get("/api/jobs/summary?repo=acme/app")).json()
    assert scoped["pipelines"] == [".github/workflows/ci.yml", ".github/workflows/nightly.yml"]
    # Without a repo, the pipelines of every repo are listed.
    unscoped = (await client.get("/api/jobs/summary")).json()
    assert unscoped["pipelines"] == [
        ".github/workflows/ci.yml",
        ".github/workflows/nightly.yml",
        ".github/workflows/release.yml",
    ]
    assert (await client.get("/api/jobs/summary?repo=acme/nothing")).json()["pipelines"] == []
