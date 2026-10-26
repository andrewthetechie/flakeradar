"""Admin delete of a Repo or a single Project, with cascade and deletion counts (issue #39)."""

from app.models import (
    Job,
    JobExecution,
    Pipeline,
    Project,
    Repo,
    Report,
    TestCase,
    TestExecution,
    TestRun,
)
import pytest
from sqlalchemy import func, select
from sqlalchemy.exc import DBAPIError, OperationalError

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


async def _count(db, model, *where) -> int:
    stmt = select(func.count()).select_from(model)
    for clause in where:
        stmt = stmt.where(clause)
    return (await db.execute(stmt)).scalar_one()


async def _seed_project(db, *, tests: int, runs: int = 1, executions: int = 0, reports: int = 0):
    """Seed one Project with the given number of Tests, Runs, Executions and pending Reports."""
    proj = await make_project(db, "acme/app", "backend")
    tcs = [await make_test_case(db, proj, name=f"t{i}") for i in range(tests)]
    run_rows = [await make_run(db, proj, commit_sha=f"sha{i}") for i in range(runs)]
    for i in range(executions):
        await make_execution(db, tcs[i % len(tcs)], run_rows[i % len(run_rows)], status="failed")
    for i in range(reports):
        await make_report(db, proj, make_junit([(f"t{i}", "passed")]))
    return proj, tcs, run_rows


async def test_delete_project_removes_only_its_own_rows(client, db):
    proj, tcs, _runs = await _seed_project(db, tests=2, runs=1, executions=2, reports=1)
    # A sibling project, with its own test, must survive.
    sibling = await make_project(db, "acme/app", "frontend")
    sibling_tc = await make_test_case(db, sibling, name="ui")
    # A pipeline report (project_id NULL) must survive a Project delete.
    pipeline_report = await make_pipeline_report(db, "acme/app")
    # So must the Repo's Pipeline/Job: Jobs belong to the Repo, not the Project.
    pipe = await make_pipeline(db, "acme/app")
    await make_job(db, pipe, name="test")
    await db.commit()

    resp = await client.delete("/api/repos/acme/app/projects/backend", headers=AUTH)
    assert resp.status_code == 200
    assert resp.json() == {
        "repo": "acme/app",
        "project": "backend",
        "deleted": {"tests": 2, "executions": 2, "jobs": 0, "reports": 1},
        "github_issue_numbers": [],
    }

    for model, clause in (
        (Project, Project.id == proj.id),
        (TestCase, TestCase.project_id == proj.id),
        (TestRun, TestRun.project_id == proj.id),
        (TestExecution, TestExecution.test_case_id.in_([tc.id for tc in tcs])),
        (Report, Report.project_id == proj.id),
    ):
        assert await _count(db, model, clause) == 0, model.__tablename__

    # The repo, the sibling project, the pipeline report and the Jobs are untouched.
    assert await _count(db, Repo) == 1
    assert await _count(db, Project, Project.id == sibling.id) == 1
    assert await _count(db, TestCase, TestCase.id == sibling_tc.id) == 1
    assert await _count(db, Report, Report.id == pipeline_report.id) == 1
    assert await _count(db, Pipeline) == 1
    assert await _count(db, Job) == 1

    # The pending report is gone from the report list too.
    listed = await client.get("/api/reports")
    assert [r["id"] for r in listed.json()] == [pipeline_report.id]

    repos = await client.get("/api/repos")
    assert [p["name"] for p in repos.json()[0]["projects"]] == ["frontend"]


async def test_delete_repo_removes_everything_in_one_transaction(client, db):
    proj, tcs, runs = await _seed_project(db, tests=2, runs=2, executions=2, reports=1)
    other = await make_project(db, "acme/app", "frontend")
    await make_test_case(db, other, name="ui")
    pipe = await make_pipeline(db, "acme/app")
    job = await make_job(db, pipe, name="test")
    await make_job_execution(db, job, status="failed")
    pipeline_report = await make_pipeline_report(db, "acme/app")
    assert proj.id and runs and tcs and pipeline_report and job.id
    await db.commit()

    resp = await client.delete("/api/repos/acme/app", headers=AUTH)
    assert resp.status_code == 200
    assert resp.json() == {
        "repo": "acme/app",
        "deleted": {"tests": 3, "executions": 2, "jobs": 1, "reports": 2},
        "github_issue_numbers": [],
    }

    for model in (Repo, Project, TestCase, TestRun, TestExecution, Pipeline, Job, JobExecution, Report):
        assert await _count(db, model) == 0, model.__tablename__
    assert (await client.get("/api/repos")).json() == []


async def test_delete_project_reports_the_issue_numbers_of_its_tests(client, db):
    """A deleted Test's auto-filed issue is stranded: the response names it so a human can close it."""
    proj = await make_project(db, "acme/app", "backend")
    await make_test_case(db, proj, name="t0", github_issue_number=123)
    # Jobs belong to the Repo and survive a Project delete, so 456 is not stranded by it.
    pipe = await make_pipeline(db, "acme/app")
    job = await make_job(db, pipe, name="test", github_issue_number=456)
    await db.commit()

    resp = await client.delete("/api/repos/acme/app/projects/backend", headers=AUTH)
    assert resp.status_code == 200
    assert resp.json()["github_issue_numbers"] == [123]
    assert await _count(db, Job, Job.id == job.id) == 1


async def test_delete_repo_reports_the_issue_numbers_of_its_tests_and_jobs(client, db):
    """Repo delete reports both scopes, sorted and deduplicated; the counts stay unchanged."""
    proj = await make_project(db, "acme/app", "backend")
    await make_test_case(db, proj, name="t0", github_issue_number=123)
    sibling = await make_project(db, "acme/app", "frontend")
    await make_test_case(db, sibling, name="ui", github_issue_number=123)
    await make_test_case(db, sibling, name="ui2")
    pipe = await make_pipeline(db, "acme/app")
    await make_job(db, pipe, name="test", github_issue_number=456)
    await db.commit()

    resp = await client.delete("/api/repos/acme/app", headers=AUTH)
    assert resp.status_code == 200
    body = resp.json()
    assert body["github_issue_numbers"] == [123, 456]
    assert body["deleted"] == {"tests": 3, "executions": 0, "jobs": 1, "reports": 0}


async def test_delete_requires_the_api_token(client, db):
    await _seed_project(db, tests=1)
    await db.commit()

    for path in ("/api/repos/acme/app", "/api/repos/acme/app/projects/backend"):
        assert (await client.delete(path)).status_code == 401
        assert (await client.delete(path, headers={"X-API-Key": "wrong"})).status_code == 401
    assert await _count(db, Repo) == 1


async def test_delete_reports_unknown_names_as_404(client, db):
    await _seed_project(db, tests=1)
    await db.commit()

    resp = await client.delete("/api/repos/acme/none", headers=AUTH)
    assert resp.status_code == 404
    assert resp.json()["detail"] == "Repo not found"

    resp = await client.delete("/api/repos/acme/app/projects/none", headers=AUTH)
    assert resp.status_code == 404
    assert resp.json()["detail"] == "Project not found"
    assert await _count(db, Repo) == 1


async def test_delete_rejects_malformed_names(client, db):
    await _seed_project(db, tests=1)
    await db.commit()

    resp = await client.delete("/api/repos/ac!me/app", headers=AUTH)
    assert resp.status_code == 422
    assert "owner/name" in resp.json()["detail"]

    resp = await client.delete("/api/repos/acme/app/projects/bad*project", headers=AUTH)
    assert resp.status_code == 422
    assert await _count(db, Repo) == 1


async def test_delete_normalizes_the_names(client, db):
    await _seed_project(db, tests=1)
    await db.commit()

    resp = await client.delete("/api/repos/ACME/App", headers=AUTH)
    assert resp.status_code == 200
    assert resp.json()["repo"] == "acme/app"
    assert await _count(db, Repo) == 0


class _FakeDeadlock(Exception):
    """Stands in for the asyncpg driver exception, which carries a SQLSTATE."""

    def __init__(self, sqlstate: str) -> None:
        super().__init__("deadlock detected")
        self.sqlstate = sqlstate


def _patch_session_method(monkeypatch, method: str, sqlstate: str | None) -> None:
    """Make every AsyncSession.<method> raise a DBAPIError wrapping this driver SQLSTATE.

    `method` is "delete" or "commit", i.e. the two statements of the endpoint's
    transaction that can lose a deadlock race with the report processor. A None
    sqlstate mimics a driver error without one, so a non-deadlock failure.
    """

    def orig() -> Exception:
        return _FakeDeadlock(sqlstate) if sqlstate else Exception("no state")

    if method == "delete":

        async def boom(self, obj):  # AsyncSession.delete(self, obj)
            raise OperationalError("DELETE FROM repos ...", {}, orig())

    else:

        async def boom(self):  # AsyncSession.commit(self)
            raise OperationalError("DELETE FROM repos ...", {}, orig())

    monkeypatch.setattr(f"sqlalchemy.ext.asyncio.AsyncSession.{method}", boom)


@pytest.mark.parametrize("sqlstate", ["40P01", "40001"])
@pytest.mark.parametrize("path", ["/api/repos/acme/app", "/api/repos/acme/app/projects/backend"])
@pytest.mark.parametrize("method", ["delete", "commit"])
async def test_delete_deadlocking_with_the_processor_gets_a_retryable_503(
    client, db, monkeypatch, method, path, sqlstate
):
    """A deadlock abort from the delete/processor race is a 503, not an unhandled 500."""
    await _seed_project(db, tests=1)
    # The seed is committed before the patch, so only the endpoint's own statement fails.
    await db.commit()
    _patch_session_method(monkeypatch, method, sqlstate)

    resp = await client.delete(path, headers=AUTH)

    assert resp.status_code == 503
    assert resp.json()["detail"] == "Delete raced with the report processor; retry."
    # The aborted transaction left the whole Repo/Project behind for a retry.
    assert await _count(db, Repo) == 1
    assert await _count(db, Project) == 1


@pytest.mark.parametrize("path", ["/api/repos/acme/app", "/api/repos/acme/app/projects/backend"])
async def test_other_database_errors_still_propagate(client, db, monkeypatch, path):
    """Only the retryable SQLSTATEs are turned into a 503; anything else is a 500."""
    await _seed_project(db, tests=1)
    await db.commit()
    _patch_session_method(monkeypatch, "commit", None)

    with pytest.raises(DBAPIError):
        await client.delete(path, headers=AUTH)
