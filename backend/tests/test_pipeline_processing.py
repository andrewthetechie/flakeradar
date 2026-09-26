"""Pipeline report processor: Jobs, Job executions, scoring, deduction, retention hook."""

from app.models import Job, JobExecution, Pipeline, Report, TestCase
from app.processing import process_next
from sqlalchemy import func, select

from tests.conftest import make_junit
from tests.factories import make_pipeline_report, make_project, make_report


def _job(ci_job_id: str, name: str, status: str = "passed", **kw) -> dict:
    d = {
        "ci_job_id": ci_job_id,
        "name": name,
        "status": status,
        "url": "",
        "runner_name": "",
        "runner_labels": [],
    }
    d.update(kw)
    return d


def _payload(
    jobs,
    sha="sha1",
    branch="main",
    default_branch="main",
    ci_run_attempt=1,
    ci_run_id="1",
    pipeline=".github/workflows/ci.yml",
    provider="github",
    repo="acme/app",
) -> dict:
    return {
        "repo": repo,
        "provider": provider,
        "pipeline": pipeline,
        "commit_sha": sha,
        "branch": branch,
        "default_branch": default_branch,
        "ci_run_id": ci_run_id,
        "ci_run_attempt": ci_run_attempt,
        "jobs": jobs,
    }


async def _queue_pipeline(db, payload, repo="acme/app") -> Report:
    rep = await make_pipeline_report(db, repo, payload)
    await db.commit()
    return rep


async def _count(db, model) -> int:
    return (await db.execute(select(func.count(model.id)))).scalar()


async def test_one_pipeline_creates_pipeline_jobs_executions(db, session_factory):
    payload = _payload([_job("1", "lint", "passed"), _job("2", "test (ubuntu, 3.12)", "failed")])
    rep = await _queue_pipeline(db, payload)
    outcome = await process_next(session_factory)
    assert outcome.status == "processed"
    assert outcome.counts == {"passed": 1, "failed": 1, "skipped": 0, "duplicate": 0}

    assert await _count(db, Pipeline) == 1
    assert await _count(db, Job) == 2
    assert await _count(db, JobExecution) == 2
    await db.refresh(rep)
    assert rep.status == "processed" and rep.run_id is None

    names = sorted((await db.execute(select(Job.name))).scalars())
    assert names == ["lint", "test (ubuntu, 3.12)"]
    assert outcome.touched_test_ids == []
    job_ids = sorted(j.id for j in (await db.execute(select(Job))).scalars())
    assert sorted(outcome.touched_job_ids) == job_ids


async def test_rerun_same_sha_is_proven_flake(db, session_factory):
    jobs1 = [_job("100", "test", "failed")]
    jobs2 = [_job("101", "test", "passed")]  # new ci_job_id, same SHA
    await _queue_pipeline(db, _payload(jobs1, sha="dead"))
    await _queue_pipeline(db, _payload(jobs2, sha="dead"))
    await process_next(session_factory)
    await process_next(session_factory)
    (job,) = (await db.execute(select(Job))).scalars()
    assert job.confirmed_flake_count == 1
    assert job.flakiness_score >= 0.6


async def test_duplicate_payload_adds_no_rows(db, session_factory):
    payload = _payload([_job("1", "test", "passed")])
    await _queue_pipeline(db, payload)
    await process_next(session_factory)
    (job,) = (await db.execute(select(Job))).scalars()
    before = (job.flakiness_score, job.confirmed_flake_count)

    await _queue_pipeline(db, payload)
    outcome = await process_next(session_factory)
    assert outcome.counts == {"passed": 0, "failed": 0, "skipped": 0, "duplicate": 1}
    assert outcome.touched_job_ids == []
    assert await _count(db, JobExecution) == 1
    await db.refresh(job)
    assert (job.flakiness_score, job.confirmed_flake_count) == before


async def test_same_job_name_two_pipelines_is_two_jobs(db, session_factory):
    p1 = _payload([_job("1", "build", "passed")], pipeline=".github/workflows/ci.yml")
    p2 = _payload([_job("2", "build", "passed")], pipeline=".github/workflows/release.yml")
    await _queue_pipeline(db, p1)
    await _queue_pipeline(db, p2)
    await process_next(session_factory)
    await process_next(session_factory)
    assert await _count(db, Job) == 2
    assert len(set((await db.execute(select(Job.pipeline_id))).scalars())) == 2


async def test_same_pipeline_name_two_providers_is_two_pipelines(db, session_factory):
    p1 = _payload([_job("1", "build", "passed")], provider="github")
    p2 = _payload([_job("2", "build", "passed")], provider="gitlab")
    await _queue_pipeline(db, p1)
    await _queue_pipeline(db, p2)
    await process_next(session_factory)
    await process_next(session_factory)
    assert await _count(db, Pipeline) == 2
    assert await _count(db, Job) == 2


async def test_default_branch_rule_on_pr_branch_scores_zero(db, session_factory):
    # Branch `feat`, Default `main`: feature flips never count against the Job.
    await _queue_pipeline(db, _payload([_job("1", "test", "failed")], sha="a", branch="feat"))
    await _queue_pipeline(db, _payload([_job("2", "test", "passed")], sha="b", branch="feat"))
    await process_next(session_factory)
    await process_next(session_factory)
    (job,) = (await db.execute(select(Job))).scalars()
    assert job.flakiness_score == 0.0
    assert job.confirmed_flake_count == 0


async def test_default_branch_rule_on_main_scores_positive(db, session_factory):
    await _queue_pipeline(db, _payload([_job("1", "test", "failed")], sha="a", branch="main"))
    await _queue_pipeline(db, _payload([_job("2", "test", "passed")], sha="b", branch="main"))
    await process_next(session_factory)
    await process_next(session_factory)
    (job,) = (await db.execute(select(Job))).scalars()
    assert job.flakiness_score > 0.0


async def test_default_branch_change_rescores_tests_and_jobs(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    # A Test and a Job both flicker on `feat` (different shas) with no Default known.
    await _queue_pipeline(db, _payload([_job("1", "job-a", "failed")], sha="a", branch="feat", default_branch=None))
    await _queue_pipeline(db, _payload([_job("2", "job-a", "passed")], sha="b", branch="feat", default_branch=None))
    for _ in range(2):
        await process_next(session_factory)
    (job,) = (await db.execute(select(Job))).scalars()
    assert job.flakiness_score > 0

    await make_report(db, proj, make_junit([("t1", "failed")]), commit_sha="c", branch="feat")
    await make_report(db, proj, make_junit([("t1", "passed")]), commit_sha="d", branch="feat")
    await db.commit()
    await process_next(session_factory)
    await process_next(session_factory)
    (tc,) = (await db.execute(select(TestCase))).scalars()
    assert tc.flakiness_score > 0

    # A pipeline report that sets the Default branch to `main` rescopes both.
    await _queue_pipeline(db, _payload([_job("3", "lint", "passed")], sha="e", branch="main", default_branch="main"))
    await process_next(session_factory)
    await db.refresh(job)
    await db.refresh(tc)
    assert job.flakiness_score == 0.0
    assert tc.flakiness_score == 0.0


async def test_malformed_pipeline_body_fails_and_queue_moves_on(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    bad = await make_pipeline_report(db, "acme/app", _payload([_job("1", "x", "passed")]))
    bad.body = b"{}"
    await db.commit()
    good = await make_report(db, proj, make_junit([("t", "passed")]))
    await db.commit()

    first = await process_next(session_factory)
    assert first.status == "failed" and first.report_id == bad.id
    assert "ValidationError" in first.error
    second = await process_next(session_factory)
    assert second.status == "processed" and second.report_id == good.id
    await db.refresh(bad)
    assert bad.status == "failed"
