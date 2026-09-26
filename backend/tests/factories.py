"""Async seed helpers: build rows directly, bypassing ingest.

Each helper flushes (so ids are assigned) but does not commit; call
``await db.commit()`` when another session must see the rows.
"""

import itertools
from datetime import datetime

from app.identity import get_or_create_repo
from app.models import (
    JOB_STATUSES,
    REPORT_PENDING,
    Job,
    JobExecution,
    Pipeline,
    Project,
    Repo,
    Report,
    TestCase,
    TestExecution,
    TestRun,
    utcnow,
)
from app.schemas import PipelineReportIn
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

# Monotonic ci_job_id for factory-created job executions (no real provider id).
_ci_job_counter = itertools.count(1)


async def make_project(db: AsyncSession, repo: str = "acme/app", project: str = "default", root: str = "") -> Project:
    repo_row = (await db.execute(select(Repo).where(Repo.name == repo))).scalar_one_or_none()
    if repo_row is None:
        repo_row = Repo(name=repo)
        db.add(repo_row)
        await db.flush()
    proj = Project(repo_id=repo_row.id, name=project, root=root)
    db.add(proj)
    await db.flush()
    return proj


async def make_test_case(
    db: AsyncSession,
    project: Project,
    name: str = "t1",
    classname: str = "tests.test_mod",
    suite: str = "unit",
    **fields,
) -> TestCase:
    from app.parsing import fingerprint  # created in task 02

    tc = TestCase(
        project_id=project.id,
        fingerprint=fingerprint(suite, classname, name),
        suite=suite,
        classname=classname,
        name=name,
        **fields,
    )
    db.add(tc)
    await db.flush()
    return tc


async def make_run(
    db: AsyncSession,
    project: Project,
    commit_sha: str = "sha1",
    branch: str = "main",
    ci_run_id: str = "",
    created_at: datetime | None = None,
    ci_job_id: str | None = None,
    ci_run_attempt: int | None = None,
    pipeline: str | None = None,
) -> TestRun:
    run = TestRun(
        project_id=project.id,
        commit_sha=commit_sha,
        branch=branch,
        ci_run_id=ci_run_id,
        created_at=created_at or utcnow(),
        ci_job_id=ci_job_id,
        ci_run_attempt=ci_run_attempt,
        pipeline=pipeline,
    )
    db.add(run)
    await db.flush()
    return run


async def make_execution(
    db: AsyncSession,
    test_case: TestCase,
    run: TestRun,
    status: str = "passed",
    message: str = "",
    details: str = "",
    attempt: int = 0,
    failure_category: str | None = None,
    created_at: datetime | None = None,
) -> TestExecution:
    ex = TestExecution(
        test_case_id=test_case.id,
        test_run_id=run.id,
        status=status,
        message=message,
        details=details,
        attempt=attempt,
        failure_category=failure_category,
        created_at=created_at or utcnow(),
    )
    db.add(ex)
    await db.flush()
    return ex


async def make_pipeline_report(
    db: AsyncSession,
    repo: str = "acme/app",
    report_json: dict | None = None,
) -> Report:
    """Seed a pending `pipeline` Report for a Repo (creating the Repo if needed).

    `report_json` is the raw JSON dict; it is validated, normalized and
    serialized the same way the ingest endpoint does it.
    """
    if report_json is None:
        report_json = {
            "repo": repo,
            "provider": "github",
            "pipeline": ".github/workflows/ci.yml",
            "commit_sha": "sha1",
            "branch": "main",
            "default_branch": "main",
            "ci_run_id": "1",
            "ci_run_attempt": 1,
            "jobs": [{"ci_job_id": "1", "name": "test", "status": "passed"}],
        }
    report_in = PipelineReportIn(**{"repo": repo, **report_json})
    repo_row = await get_or_create_repo(db, report_in.repo)
    rep = Report(
        kind="pipeline",
        repo_id=repo_row.id,
        project_id=None,
        commit_sha=report_in.commit_sha,
        branch=report_in.branch,
        ci_run_id=report_in.ci_run_id,
        ci_run_attempt=report_in.ci_run_attempt,
        pipeline=report_in.pipeline,
        default_branch=report_in.default_branch,
        body=report_in.model_dump_json().encode(),
        status=REPORT_PENDING,
    )
    db.add(rep)
    await db.flush()
    return rep


async def make_report(
    db: AsyncSession,
    project: Project,
    body: bytes,
    commit_sha: str = "sha1",
    branch: str = "main",
    ci_run_id: str = "",
    root: str | None = None,
    status: str = REPORT_PENDING,
    kind: str = "junit",
    ci_job_id: str | None = None,
    ci_run_attempt: int | None = None,
    pipeline: str | None = None,
    default_branch: str | None = None,
) -> Report:
    rep = Report(
        project_id=None if kind == "pipeline" else project.id,
        kind=kind,
        repo_id=project.repo_id,
        commit_sha=commit_sha,
        branch=branch,
        ci_run_id=ci_run_id,
        root=root,
        body=body,
        status=status,
        ci_job_id=ci_job_id,
        ci_run_attempt=ci_run_attempt,
        pipeline=pipeline,
        default_branch=default_branch,
    )
    db.add(rep)
    await db.flush()
    return rep


async def make_pipeline(
    db: AsyncSession,
    repo: str = "acme/app",
    name: str = ".github/workflows/ci.yml",
    provider: str = "github",
) -> Pipeline:
    repo_row = (await db.execute(select(Repo).where(Repo.name == repo))).scalar_one_or_none()
    if repo_row is None:
        repo_row = Repo(name=repo)
        db.add(repo_row)
        await db.flush()
    pipe = Pipeline(repo_id=repo_row.id, provider=provider, name=name)
    db.add(pipe)
    await db.flush()
    return pipe


async def make_job(db: AsyncSession, pipeline: Pipeline, name: str = "test", **fields) -> Job:
    job = Job(pipeline_id=pipeline.id, name=name, **fields)
    db.add(job)
    await db.flush()
    return job


async def make_job_execution(
    db: AsyncSession,
    job: Job,
    status: str = "passed",
    *,
    ci_job_id: str | None = None,
    commit_sha: str = "sha1",
    branch: str = "main",
    ci_run_id: str = "1",
    ci_run_attempt: int = 1,
    created_at: datetime | None = None,
    **fields,
) -> JobExecution:
    # status must be one of JOB_STATUSES to satisfy callers that assume the DB contract.
    if status not in JOB_STATUSES:
        raise ValueError(f"bad job status: {status!r}")
    ex = JobExecution(
        job_id=job.id,
        ci_job_id=ci_job_id or f"{next(_ci_job_counter)}",
        commit_sha=commit_sha,
        branch=branch,
        ci_run_id=ci_run_id,
        ci_run_attempt=ci_run_attempt,
        status=status,
        created_at=created_at or utcnow(),
        **fields,
    )
    db.add(ex)
    await db.flush()
    return ex
