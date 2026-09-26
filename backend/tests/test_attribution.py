"""Job attribution: linked JUnit reports explain Job failures; only unexplained count."""

from app.models import Job, Repo, TestRun
from app.processing import _job_history, explained_ci_job_ids, process_next
from sqlalchemy import select

from tests.conftest import make_junit
from tests.factories import make_pipeline_report, make_project, make_report


def _job(ci_job_id: str, name: str = "test", status: str = "passed") -> dict:
    return {"ci_job_id": ci_job_id, "name": name, "status": status, "url": "", "runner_name": "", "runner_labels": []}


def _payload(jobs, sha="s", branch="main", default_branch="main", ci_run_id="1") -> dict:
    return {
        "repo": "acme/app",
        "provider": "github",
        "pipeline": ".github/workflows/ci.yml",
        "commit_sha": sha,
        "branch": branch,
        "default_branch": default_branch,
        "ci_run_id": ci_run_id,
        "ci_run_attempt": 1,
        "jobs": jobs,
    }


async def _pipeline(db, jobs, **kw):
    await make_pipeline_report(db, "acme/app", _payload(jobs, **kw))
    await db.commit()


async def _junit(db, project, cases, ci_job_id=None, sha="s", **kw):
    await make_report(db, project, make_junit(cases), commit_sha=sha, ci_job_id=ci_job_id, **kw)
    await db.commit()


async def _job_row(db):
    return (await db.execute(select(Job))).scalar_one()


async def test_pipeline_first_then_junit_explains(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    await _pipeline(db, [_job("J1", status="failed")], sha="s")
    await process_next(session_factory)
    await _junit(db, proj, [("t1", "failed")], ci_job_id="J1", sha="s")
    await process_next(session_factory)
    await _pipeline(db, [_job("J2", status="passed")], sha="s")
    await process_next(session_factory)
    job = await _job_row(db)
    # J1's failure is explained (failing t1 on the same ci_job_id), so the
    # history is [pass, skipped] -> no flip.
    assert job.confirmed_flake_count == 0
    assert job.flakiness_score == 0.0


async def test_junit_first_then_pipeline_explains(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    await _junit(db, proj, [("t1", "failed")], ci_job_id="J1", sha="s")
    await process_next(session_factory)
    await _pipeline(db, [_job("J1", status="failed")], sha="s")
    await process_next(session_factory)
    await _pipeline(db, [_job("J2", status="passed")], sha="s")
    await process_next(session_factory)
    job = await _job_row(db)
    assert job.flakiness_score == 0.0  # same outcome in the other arrival order


async def test_unexplained_failure_moves_score(db, session_factory):
    await _pipeline(db, [_job("J1", status="failed")], sha="s")
    await process_next(session_factory)
    await _pipeline(db, [_job("J2", status="passed")], sha="s")
    await process_next(session_factory)
    job = await _job_row(db)
    assert job.flakiness_score > 0.0  # the failure is NOT explained


async def test_cross_repo_run_does_not_explain(db, session_factory):
    await make_project(db, "acme/app", "backend")
    other = await make_project(db, "other/app", "backend")
    await _pipeline(db, [_job("J1", status="failed")], sha="s")
    await process_next(session_factory)
    await _pipeline(db, [_job("J2", status="passed")], sha="s")
    await process_next(session_factory)
    # A Run in ANOTHER repo with the same ci_job_id must not explain acme/app's J1.
    await make_report(db, other, make_junit([("t1", "failed")]), commit_sha="s", ci_job_id="J1")
    await db.commit()
    await process_next(session_factory)
    job = await _job_row(db)
    assert job.flakiness_score > 0.0


async def test_passing_tests_do_not_explain(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    await _pipeline(db, [_job("J1", status="failed")], sha="s")
    await process_next(session_factory)
    await _pipeline(db, [_job("J2", status="passed")], sha="s")
    await process_next(session_factory)
    await _junit(db, proj, [("t1", "passed")], ci_job_id="J1", sha="s")
    await process_next(session_factory)
    job = await _job_row(db)
    assert job.flakiness_score > 0.0  # only passing Tests -> still unexplained


async def test_attribution_params_round_trip_to_test_run(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    await make_report(
        db,
        proj,
        make_junit([("t1", "passed")]),
        commit_sha="s",
        ci_job_id="42",
        ci_run_attempt=3,
        pipeline=".github/workflows/ci.yml",
    )
    await db.commit()
    await process_next(session_factory)
    run = (await db.execute(select(TestRun))).scalar_one()
    assert (run.ci_job_id, run.ci_run_attempt, run.pipeline) == ("42", 3, ".github/workflows/ci.yml")


async def test_explained_ci_job_ids_agrees_with_scoring(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    await _pipeline(db, [_job("J1", status="failed")], sha="s")
    await process_next(session_factory)
    await _junit(db, proj, [("t1", "failed")], ci_job_id="J1", sha="s")
    await process_next(session_factory)

    job = await _job_row(db)
    repo = (await db.execute(select(Repo).where(Repo.name == "acme/app"))).scalar_one()
    assert await explained_ci_job_ids(db, repo.id, ["J1"]) == {"J1"}
    # The scoring history agrees: J1 is fed to scoring as skipped.
    history = await _job_history(db, [job.id])
    _, execs = history[job.id]
    assert [status for _, _, status in execs] == ["skipped"]

    # A ci_job_id with only passing Tests is not explained.
    await _junit(db, proj, [("t1", "passed")], ci_job_id="J2", sha="s")
    await _pipeline(db, [_job("J2", status="failed")], sha="s")
    await process_next(session_factory)
    await process_next(session_factory)
    assert await explained_ci_job_ids(db, repo.id, ["J2"]) == set()
