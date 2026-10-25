"""Job attribution: linked JUnit reports explain Job failures; only unexplained count."""

from app.attribution import explained_ci_job_ids, unexplained_failure_counts
from app.config import Settings
from app.models import Job, Repo, TestRun
from app.processing import _job_history, process_next, rescore_jobs
from sqlalchemy import select

from tests.conftest import make_junit
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


def _job(ci_job_id: str, name: str = "test", status: str = "passed", **kw) -> dict:
    return {
        "ci_job_id": ci_job_id,
        "name": name,
        "status": status,
        "url": "",
        "runner_name": "",
        "runner_labels": [],
        **kw,
    }


def _payload(jobs, sha="s", branch="main", default_branch="main", ci_run_id="1", ci_run_attempt=1) -> dict:
    return {
        "repo": "acme/app",
        "provider": "github",
        "pipeline": ".github/workflows/ci.yml",
        "commit_sha": sha,
        "branch": branch,
        "default_branch": default_branch,
        "ci_run_id": ci_run_id,
        "ci_run_attempt": ci_run_attempt,
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


async def _job_named(db, name: str) -> Job:
    return (await db.execute(select(Job).where(Job.name == name))).scalar_one()


async def _repo(db) -> Repo:
    return (await db.execute(select(Repo).where(Repo.name == "acme/app"))).scalar_one()


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
    repo = await _repo(db)
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


async def test_unexplained_failure_counts_over_the_newest_window(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    await _junit(db, proj, [("t1", "failed")], ci_job_id="J1", sha="s")
    await process_next(session_factory)
    # Oldest to newest: explained failure, unexplained failure, pass, unexplained failure.
    for ci_job_id, status in [("J1", "failed"), ("J2", "failed"), ("J3", "passed"), ("J4", "failed")]:
        await _pipeline(db, [_job(ci_job_id, status=status)], sha="s")
        await process_next(session_factory)

    job = await _job_row(db)
    assert await unexplained_failure_counts(db, [job.id], window=10) == {job.id: 2}
    assert await unexplained_failure_counts(db, [job.id], window=2) == {job.id: 1}
    assert await unexplained_failure_counts(db, [], window=10) == {}


async def test_explained_failure_does_not_break_clean_streak(db):
    proj = await make_project(db, "acme/app", "backend")
    pipe = await make_pipeline(db, "acme/app")
    job = await make_job(db, pipe, name="e2e")
    for ci, status in [("J1", "passed"), ("J2", "failed"), ("J3", "passed")]:
        await make_job_execution(db, job, ci_job_id=ci, status=status, branch="main")
    # Explain J2: a failing Test in the same repo with the same ci_job_id.
    tc = await make_test_case(db, proj)
    run = await make_run(db, proj, ci_job_id="J2")
    await make_execution(db, tc, run, status="failed")
    await db.commit()

    await rescore_jobs(db, [job.id])
    await db.commit()
    await db.refresh(job)
    assert job.clean_streak == 2


async def test_unexplained_failure_breaks_clean_streak(db):
    pipe = await make_pipeline(db, "acme/app")
    job = await make_job(db, pipe, name="e2e")
    for ci, status in [("J1", "passed"), ("J2", "failed"), ("J3", "passed")]:
        await make_job_execution(db, job, ci_job_id=ci, status=status, branch="main")
    await db.commit()

    await rescore_jobs(db, [job.id])
    await db.commit()
    await db.refresh(job)
    assert job.clean_streak == 1


async def test_old_explained_failures_beyond_the_window_do_not_change_the_score(db, monkeypatch):
    monkeypatch.setattr("app.processing.get_settings", lambda: Settings(score_window=3))
    proj = await make_project(db, "acme/app", "backend")
    pipe = await make_pipeline(db, "acme/app")
    twin = await make_job(db, pipe, name="recent-only")
    full = await make_job(db, pipe, name="with-old-explained")
    # 6 old explained failures on `full`: a failing Test run with the same ci_job_id explains each.
    tc = await make_test_case(db, proj)
    for i in range(6):
        await make_job_execution(db, full, status="failed", ci_job_id=f"OLD{i}", branch="main", commit_sha=f"o{i}")
        run = await make_run(db, proj, ci_job_id=f"OLD{i}")
        await make_execution(db, tc, run, status="failed")
    # Both Jobs then share the same newest 3 (unexplained) rows: failed, passed, failed.
    for job in (twin, full):
        for i, status in enumerate(["failed", "passed", "failed"]):
            await make_job_execution(
                db, job, status=status, ci_job_id=f"NEW{job.id}-{i}", branch="main", commit_sha=f"n{i}"
            )
    await db.commit()

    await rescore_jobs(db, [twin.id, full.id])
    await db.commit()
    await db.refresh(twin)
    await db.refresh(full)
    assert full.flakiness_score == twin.flakiness_score > 0.0
    assert full.confirmed_flake_count == twin.confirmed_flake_count
    assert full.clean_streak == twin.clean_streak
    assert await unexplained_failure_counts(db, [twin.id, full.id], window=3) == {twin.id: 2, full.id: 2}


async def test_job_default_branch_window_survives_newer_feature_rows(db, monkeypatch):
    monkeypatch.setattr("app.processing.get_settings", lambda: Settings(score_window=3))
    pipe = await make_pipeline(db, "acme/app")
    repo = (await db.execute(select(Repo))).scalar_one()
    repo.default_branch = "main"
    await db.flush()
    job = await make_job(db, pipe, name="e2e")
    for i, status in enumerate(["failed", "passed", "failed"]):
        await make_job_execution(db, job, status=status, ci_job_id=f"M{i}", branch="main", commit_sha=f"m{i}")
    for i in range(10):
        await make_job_execution(db, job, status="passed", ci_job_id=f"F{i}", branch="feat", commit_sha=f"f{i}")
    await db.commit()

    await rescore_jobs(db, [job.id])
    await db.commit()
    await db.refresh(job)
    # The 3 newest main flips still score despite 10 newer feat rows.
    assert job.flakiness_score > 0.0


# --- summary Jobs (issue #9) --------------------------------------------


async def test_summary_job_explained_by_a_failed_sibling(db, session_factory):
    """A summary Job fails because a non-summary Job failed; that is not its own failure."""
    await _pipeline(
        db,
        [
            _job("J1", name="test", status="failed"),
            _job("J2", name="alls-green", status="failed", summary=True),
        ],
    )
    await process_next(session_factory)

    repo = await _repo(db)
    # The non-summary Job has no Run, so it stays unexplained; the summary Job is explained.
    assert await explained_ci_job_ids(db, repo.id, ["J1", "J2"]) == {"J2"}

    summary_job = await _job_named(db, "alls-green")
    _, execs = (await _job_history(db, [summary_job.id]))[summary_job.id]
    assert [status for _, _, status in execs] == ["skipped"]


async def test_summary_job_without_a_failed_sibling_is_unexplained(db, session_factory):
    """A summary Job whose own step broke has no failed sibling to blame."""
    await _pipeline(db, [_job("J1", name="alls-green", status="failed", summary=True)])
    await process_next(session_factory)

    repo = await _repo(db)
    assert await explained_ci_job_ids(db, repo.id, ["J1"]) == set()

    job = await _job_row(db)
    _, execs = (await _job_history(db, [job.id]))[job.id]
    assert [status for _, _, status in execs] == ["failed"]


async def test_summary_jobs_do_not_explain_each_other(db, session_factory):
    """Only a failed non-summary Job explains a summary Job."""
    await _pipeline(
        db,
        [
            _job("J1", name="alls-green-a", status="failed", summary=True),
            _job("J2", name="alls-green-b", status="failed", summary=True),
        ],
    )
    await process_next(session_factory)

    repo = await _repo(db)
    assert await explained_ci_job_ids(db, repo.id, ["J1", "J2"]) == set()


async def test_summary_job_explained_when_the_sibling_failure_is_a_flaky_test(db, session_factory):
    """The issue #9 setup: the non-summary Job's failure is a test; both Jobs are explained."""
    proj = await make_project(db, "acme/app", "backend")
    await _pipeline(
        db,
        [
            _job("J1", name="test-playwright", status="failed"),
            _job("J2", name="alls-green-playwright", status="failed", summary=True),
        ],
    )
    await process_next(session_factory)
    await _junit(db, proj, [("t1", "failed")], ci_job_id="J1")
    await process_next(session_factory)

    repo = await _repo(db)
    assert await explained_ci_job_ids(db, repo.id, ["J1", "J2"]) == {"J1", "J2"}


async def test_summary_job_is_not_explained_by_another_attempt(db, session_factory):
    """Attempt 1 failed a non-summary Job; in attempt 2 only the summary Job failed."""
    await _pipeline(
        db,
        [
            _job("J1", name="test", status="failed"),
            _job("J2", name="alls-green", status="failed", summary=True),
        ],
        ci_run_id="100",
        ci_run_attempt=1,
    )
    await process_next(session_factory)
    await _pipeline(
        db,
        [
            _job("J3", name="test", status="passed"),
            _job("J4", name="alls-green", status="failed", summary=True),
        ],
        ci_run_id="100",
        ci_run_attempt=2,
    )
    await process_next(session_factory)

    repo = await _repo(db)
    assert await explained_ci_job_ids(db, repo.id, ["J2", "J4"]) == {"J2"}


async def test_summary_job_failure_does_not_move_its_score(db, session_factory):
    """Same SHA failed and passed: a proven flake for a non-summary Job, nothing for a Summary Job."""
    await _pipeline(
        db,
        [
            _job("J1", name="test", status="failed"),
            _job("J2", name="alls-green", status="failed", summary=True),
        ],
        sha="s",
    )
    await process_next(session_factory)
    await _pipeline(
        db,
        [
            _job("J3", name="test", status="passed"),
            _job("J4", name="alls-green", status="passed", summary=True),
        ],
        sha="s",
    )
    await process_next(session_factory)

    summary_job = await _job_named(db, "alls-green")
    assert summary_job.flakiness_score == 0.0
    assert summary_job.confirmed_flake_count == 0
    assert summary_job.clean_streak == 1


async def test_unexplained_failure_counts_excludes_summary_explained(db, session_factory):
    """Issue filing must not count a summary Job's explained failure."""
    await _pipeline(
        db,
        [
            _job("J1", name="test", status="failed"),
            _job("J2", name="alls-green", status="failed", summary=True),
        ],
    )
    await process_next(session_factory)

    summary_job = await _job_named(db, "alls-green")
    other = await _job_named(db, "test")
    assert await unexplained_failure_counts(db, [summary_job.id], window=10) == {}
    assert await unexplained_failure_counts(db, [other.id], window=10) == {other.id: 1}


async def test_summary_job_with_an_empty_ci_run_id_is_unexplained(db, session_factory):
    """A reporter that omits ci_run_id must not let another run explain a summary Job.

    Every report would collapse onto ci_run_id="" and attempt 1, so without the
    guard an unrelated older failure explains the summary Job forever.
    """
    await _pipeline(db, [_job("J1", name="test", status="failed")], ci_run_id="")
    await process_next(session_factory)
    await _pipeline(db, [_job("J2", name="alls-green", status="failed", summary=True)], ci_run_id="")
    await process_next(session_factory)

    repo = await _repo(db)
    assert await explained_ci_job_ids(db, repo.id, ["J2"]) == set()
    summary_job = await _job_named(db, "alls-green")
    _, execs = (await _job_history(db, [summary_job.id]))[summary_job.id]
    assert [status for _, _, status in execs] == ["failed"]


async def test_summary_flag_flip_rescores_without_a_new_execution(db, session_factory):
    """Marking an existing Job as a summary Job corrects its stored score at once."""
    await _pipeline(
        db,
        [_job("J1", name="test", status="passed"), _job("J2", name="alls-green", status="passed")],
        sha="a",
    )
    await process_next(session_factory)
    await _pipeline(
        db,
        [_job("J3", name="test", status="failed"), _job("J4", name="alls-green", status="failed")],
        sha="b",
    )
    await process_next(session_factory)

    summary_job = await _job_named(db, "alls-green")
    assert summary_job.flakiness_score > 0  # not yet a summary Job, so the failure counts

    # The same executions are re-reported, now with the marker: no new execution
    # is inserted, so only the flag change can trigger the correcting rescore.
    await _pipeline(
        db,
        [
            _job("J3", name="test", status="failed"),
            _job("J4", name="alls-green", status="failed", summary=True),
        ],
        sha="b",
    )
    await process_next(session_factory)

    await db.refresh(summary_job)
    assert summary_job.summary is True
    assert summary_job.flakiness_score == 0.0
    assert summary_job.confirmed_flake_count == 0
