"""CI run failures API: which CI run attempt, report counts, 404s (issue #5)."""

from pathlib import Path

from app import queries
from app.attribution import explained_ci_job_ids
from app.config import get_settings
from app.processing import process_next

from tests.conftest import AUTH, make_junit
from tests.factories import (
    make_execution,
    make_job,
    make_job_execution,
    make_pipeline,
    make_pipeline_report,
    make_project,
    make_report,
    make_run,
    make_test_case,
)

FIXTURES = Path(__file__).parent / "fixtures"


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


async def test_failures_of_two_attempts(client, db):
    proj = await make_project(db, "acme/app", "backend")
    t1 = await make_test_case(db, proj, name="t1")
    t2 = await make_test_case(db, proj, name="t2")
    run_a = await make_run(db, proj, ci_run_id="100", ci_run_attempt=1)
    await make_execution(db, t1, run_a, status="failed")
    await make_execution(db, t2, run_a, status="passed")
    run_b = await make_run(db, proj, ci_run_id="100", ci_run_attempt=2)
    await make_execution(db, t1, run_b, status="passed")
    await make_execution(db, t2, run_b, status="failed")
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert body["ci_run_attempt"] == 2
    assert [f["name"] for f in body["failures"]] == ["t2"]

    body = (await client.get(_url("100", ci_run_attempt=1))).json()
    assert [f["name"] for f in body["failures"]] == ["t1"]


async def test_failure_entry_fields(client, db):
    proj = await make_project(db, "acme/app", "backend")
    t1 = await make_test_case(
        db,
        proj,
        name="t1",
        flakiness_score=0.5,
        confirmed_flake_count=2,
        clean_streak=3,
        quarantined=True,
        github_issue_number=9,
    )
    run = await make_run(db, proj, ci_run_id="100", ci_run_attempt=1, ci_job_id="J1")
    await make_execution(db, t1, run, status="error", message="boom", failure_category="network")
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert body["failures"] == [
        {
            "test_id": t1.id,
            "project": "backend",
            "suite": "unit",
            "classname": "tests.test_mod",
            "name": "t1",
            "run_id": run.id,
            "ci_job_id": "J1",
            "status": "error",
            "failed_attempts": 1,
            "failure_category": "network",
            "message": "boom",
            "tier": "flaky",
            "flakiness_score": 0.5,
            "confirmed_flake_count": 2,
            "clean_streak": 3,
            "quarantined": True,
            "github_issue_number": 9,
            "github_issue_url": "https://github.com/acme/app/issues/9",
        }
    ]
    assert body["total_failures"] == 1
    assert body["truncated"] is False


async def test_retries_in_one_run(client, db):
    proj = await make_project(db, "acme/app", "backend")
    t1 = await make_test_case(db, proj, name="t1")
    t2 = await make_test_case(db, proj, name="t2")
    t3 = await make_test_case(db, proj, name="t3")
    run = await make_run(db, proj, ci_run_id="100", ci_run_attempt=1)
    await make_execution(db, t1, run, status="failed", message="first", failure_category="timing", attempt=0)
    await make_execution(db, t1, run, status="passed", attempt=1)
    await make_execution(db, t2, run, status="failed", message="a", attempt=0)
    await make_execution(db, t2, run, status="failed", message="b", failure_category="assertion", attempt=1)
    await make_execution(db, t3, run, status="passed")
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert len(body["failures"]) == 2
    by_name = {f["name"]: f for f in body["failures"]}
    assert (
        by_name["t1"]["status"],
        by_name["t1"]["failed_attempts"],
        by_name["t1"]["message"],
        by_name["t1"]["failure_category"],
    ) == ("passed", 1, "first", "timing")
    assert (
        by_name["t2"]["status"],
        by_name["t2"]["failed_attempts"],
        by_name["t2"]["message"],
        by_name["t2"]["failure_category"],
    ) == ("failed", 2, "b", "assertion")


async def test_same_test_in_two_runs_is_two_entries(client, db):
    proj = await make_project(db, "acme/app", "backend")
    t1 = await make_test_case(db, proj, name="t1")
    run_a = await make_run(db, proj, ci_run_id="100", ci_run_attempt=1, ci_job_id="A")
    run_b = await make_run(db, proj, ci_run_id="100", ci_run_attempt=1, ci_job_id="B")
    await make_execution(db, t1, run_a, status="failed")
    await make_execution(db, t1, run_b, status="failed")
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert [f["ci_job_id"] for f in body["failures"]] == ["A", "B"]
    assert [f["run_id"] for f in body["failures"]] == [run_a.id, run_b.id]


async def test_skipped_and_passing_tests_are_not_listed(client, db):
    proj = await make_project(db, "acme/app", "backend")
    t1 = await make_test_case(db, proj, name="t1")
    t2 = await make_test_case(db, proj, name="t2")
    run = await make_run(db, proj, ci_run_id="100", ci_run_attempt=1)
    await make_execution(db, t1, run, status="passed")
    await make_execution(db, t2, run, status="skipped")
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert body["failures"] == []
    assert body["total_failures"] == 0


async def test_message_is_capped(client, db):
    proj = await make_project(db, "acme/app", "backend")
    t1 = await make_test_case(db, proj, name="t1")
    run = await make_run(db, proj, ci_run_id="100", ci_run_attempt=1)
    await make_execution(db, t1, run, status="failed", message="x" * 5000)
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert len(body["failures"][0]["message"]) == 1024


async def test_order(client, db):
    api = await make_project(db, "acme/app", "api")
    backend = await make_project(db, "acme/app", "backend")
    z = await make_test_case(db, backend, name="z", classname="tests.b")
    y = await make_test_case(db, backend, name="y", classname="tests.a")
    x = await make_test_case(db, api, name="x", classname="tests.a")
    run = await make_run(db, api, ci_run_id="100", ci_run_attempt=1)
    for tc in (z, y, x):
        await make_execution(db, tc, run, status="failed")
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert [(f["project"], f["name"]) for f in body["failures"]] == [
        ("api", "x"),
        ("backend", "y"),
        ("backend", "z"),
    ]


async def test_other_ci_runs_are_ignored(client, db):
    proj = await make_project(db, "acme/app", "backend")
    t1 = await make_test_case(db, proj, name="t1")
    other = await make_run(db, proj, ci_run_id="999", ci_run_attempt=1)
    await make_execution(db, t1, other, status="failed")
    this = await make_run(db, proj, ci_run_id="100", ci_run_attempt=1)
    await make_execution(db, t1, this, status="passed")
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert body["failures"] == []


async def test_cap_and_truncated(db):
    proj = await make_project(db, "acme/app", "backend")
    run = await make_run(db, proj, ci_run_id="100", ci_run_attempt=1)
    for name in ("t1", "t2", "t3"):
        tc = await make_test_case(db, proj, name=name)
        await make_execution(db, tc, run, status="failed")
    await db.commit()

    result = await queries.ci_run_failures(db, "acme/app", "100", None, threshold=0.3, limit=2)
    assert result.total_failures == 3
    assert result.truncated is True
    assert len(result.failures) == 2
    assert [f.name for f in result.failures] == ["t1", "t2"]


async def test_failed_jobs_explained_and_unexplained(client, db):
    proj = await make_project(db, "acme/app", "backend")
    pipe = await make_pipeline(db, "acme/app", ".github/workflows/ci.yml")
    backend = await make_job(db, pipe, name="backend")
    e2e = await make_job(db, pipe, name="e2e")
    lint = await make_job(db, pipe, name="lint")
    await make_job_execution(db, backend, "failed", ci_job_id="J-B", ci_run_id="100", ci_run_attempt=1)
    await make_job_execution(db, e2e, "failed", ci_job_id="J-E", ci_run_id="100", ci_run_attempt=1)
    await make_job_execution(db, lint, "passed", ci_job_id="J-L", ci_run_id="100", ci_run_attempt=1)
    tc = await make_test_case(db, proj, name="t1")
    run = await make_run(db, proj, ci_run_id="100", ci_run_attempt=1, ci_job_id="J-B")
    await make_execution(db, tc, run, status="failed")
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert [(j["name"], j["outcome"]) for j in body["failed_jobs"]] == [
        ("backend", "explained"),
        ("e2e", "unexplained"),
    ]
    assert [f["name"] for f in body["failures"]] == ["t1"]


async def test_red_without_failing_tests(client, db):
    pipe = await make_pipeline(db, "acme/app", ".github/workflows/ci.yml")
    e2e = await make_job(db, pipe, name="e2e")
    await make_job_execution(db, e2e, "failed", ci_job_id="J-E", ci_run_id="100", ci_run_attempt=1)
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert body["failures"] == []
    assert body["reports"] == {"pending": 0, "processed": 0, "failed": 0}
    assert len(body["failed_jobs"]) == 1
    assert body["failed_jobs"][0]["outcome"] == "unexplained"


async def test_failed_job_fields(client, db):
    pipe = await make_pipeline(db, "acme/app", ".github/workflows/ci.yml")
    job = await make_job(
        db,
        pipe,
        name="e2e",
        flakiness_score=0.1,
        confirmed_flake_count=1,
        clean_streak=2,
        github_issue_number=4,
    )
    await make_job_execution(db, job, "failed", ci_job_id="J-E", ci_run_id="100", ci_run_attempt=1, url="https://ci/1")
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert body["failed_jobs"] == [
        {
            "job_id": job.id,
            "pipeline": ".github/workflows/ci.yml",
            "name": "e2e",
            "ci_job_id": "J-E",
            "url": "https://ci/1",
            "outcome": "unexplained",
            "summary": False,
            "tier": "suspect",
            "flakiness_score": 0.1,
            "confirmed_flake_count": 1,
            "clean_streak": 2,
            "github_issue_number": 4,
            "github_issue_url": "https://github.com/acme/app/issues/4",
        }
    ]


async def test_summary_job_explained_by_a_failed_job(client, db):
    """Issue #9: `alls-green-playwright` fails with its flaky Job, so it is explained."""
    proj = await make_project(db, "acme/app", "backend")
    pipe = await make_pipeline(db, "acme/app", ".github/workflows/playwright.yml")
    tests = await make_job(db, pipe, name="test-playwright")
    summary_job = await make_job(db, pipe, name="alls-green-playwright", summary=True)
    await make_job_execution(db, tests, "failed", ci_job_id="J-T", ci_run_id="38973797537", ci_run_attempt=1)
    await make_job_execution(db, summary_job, "failed", ci_job_id="J-G", ci_run_id="38973797537", ci_run_attempt=1)
    tc = await make_test_case(db, proj, name="Squad screen controls renders stat tile values unbroken")
    run = await make_run(db, proj, ci_run_id="38973797537", ci_run_attempt=1, ci_job_id="J-T")
    await make_execution(db, tc, run, status="failed")
    await db.commit()

    body = (await client.get(_url("38973797537"))).json()
    assert [(j["name"], j["outcome"], j["summary"]) for j in body["failed_jobs"]] == [
        ("alls-green-playwright", "explained", True),
        ("test-playwright", "explained", False),
    ]
    assert [f["name"] for f in body["failures"]] == ["Squad screen controls renders stat tile values unbroken"]


async def test_summary_job_is_unexplained_without_a_failed_sibling(client, db):
    """A Summary Job that fails on its own (no failed Job in the attempt) is still unexplained."""
    pipe = await make_pipeline(db, "acme/app", ".github/workflows/ci.yml")
    summary_job = await make_job(db, pipe, name="alls-green", summary=True)
    await make_job_execution(db, summary_job, "failed", ci_job_id="J-G", ci_run_id="100", ci_run_attempt=1)
    lint = await make_job(db, pipe, name="lint")
    await make_job_execution(db, lint, "passed", ci_job_id="J-L", ci_run_id="100", ci_run_attempt=1)
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert [(j["name"], j["outcome"], j["summary"]) for j in body["failed_jobs"]] == [
        ("alls-green", "unexplained", True)
    ]


async def test_failed_job_issue_url_null_for_non_github(client, db):
    pipe = await make_pipeline(db, "acme/app", "ci.yml", provider="gitlab")
    job = await make_job(db, pipe, name="e2e", github_issue_number=4)
    await make_job_execution(db, job, "failed", ci_job_id="J-E", ci_run_id="100", ci_run_attempt=1)
    await db.commit()

    body = (await client.get(_url("100"))).json()
    assert body["failed_jobs"][0]["github_issue_number"] == 4
    assert body["failed_jobs"][0]["github_issue_url"] is None


async def test_failed_jobs_of_other_attempts_are_ignored(client, db):
    pipe = await make_pipeline(db, "acme/app", ".github/workflows/ci.yml")
    e2e = await make_job(db, pipe, name="e2e")
    await make_job_execution(db, e2e, "failed", ci_job_id="J-1", ci_run_id="100", ci_run_attempt=1)
    await make_job_execution(db, e2e, "passed", ci_job_id="J-2", ci_run_id="100", ci_run_attempt=2)
    await db.commit()

    assert (await client.get(_url("100"))).json()["failed_jobs"] == []
    body = (await client.get(_url("100", ci_run_attempt=1))).json()
    assert [(j["name"], j["outcome"]) for j in body["failed_jobs"]] == [("e2e", "unexplained")]


async def test_nextest_flaky_fail_is_a_proven_flake_in_its_ci_run(client, db, session_factory):
    """A nextest flaky-fail retry explains its Job and is a proven flake, not a failure."""
    proj = await make_project(db, "acme/app", "default")
    await make_report(
        db,
        proj,
        (FIXTURES / "nextest_flaky_fail.xml").read_bytes(),
        commit_sha="777022bf",
        ci_run_id="38994487107",
        ci_job_id="J1",
    )
    await make_pipeline_report(
        db,
        "acme/app",
        {
            "repo": "acme/app",
            "provider": "github",
            "pipeline": ".github/workflows/ci.yml",
            "commit_sha": "777022bf",
            "branch": "main",
            "default_branch": "main",
            "ci_run_id": "38994487107",
            "ci_run_attempt": 1,
            "jobs": [
                {
                    "ci_job_id": "J1",
                    "name": "test",
                    "status": "failed",
                    "url": "",
                    "runner_name": "",
                    "runner_labels": [],
                }
            ],
        },
    )
    await db.commit()
    await process_next(session_factory)  # the junit Report
    await process_next(session_factory)  # the pipeline Report

    assert await explained_ci_job_ids(db, proj.repo_id, ["J1"]) == {"J1"}

    body = (await client.get(_url("38994487107"))).json()
    assert body["reports"] == {"pending": 0, "processed": 2, "failed": 0}
    (entry,) = [
        f for f in body["failures"] if f["name"].endswith("invalidate_priority_readiness_drops_from_each_driver")
    ]
    assert entry["status"] == "passed"
    assert entry["failed_attempts"] == 1
    assert entry["message"].startswith("thread 'embedding::reindexer")
    assert entry["confirmed_flake_count"] == 1
    assert entry["ci_job_id"] == "J1"
    assert len(body["failures"]) == 1  # the passing Test is not listed
    (job,) = body["failed_jobs"]
    assert (job["outcome"], job["ci_job_id"]) == ("explained", "J1")
