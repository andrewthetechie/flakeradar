"""Async seed helpers: build rows directly, bypassing ingest.

Each helper flushes (so ids are assigned) but does not commit; call
``await db.commit()`` when another session must see the rows.
"""
from datetime import datetime

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models import (
    REPORT_PENDING, Project, Repo, Report, TestCase, TestExecution, TestRun, utcnow,
)


async def make_project(
    db: AsyncSession, repo: str = "acme/app", project: str = "default", root: str = ""
) -> Project:
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
        suite=suite, classname=classname, name=name,
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
) -> TestRun:
    run = TestRun(project_id=project.id, commit_sha=commit_sha, branch=branch,
                  ci_run_id=ci_run_id, created_at=created_at or utcnow())
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
    created_at: datetime | None = None,
) -> TestExecution:
    ex = TestExecution(test_case_id=test_case.id, test_run_id=run.id, status=status,
                       message=message, details=details,
                       created_at=created_at or utcnow())
    db.add(ex)
    await db.flush()
    return ex


async def make_report(
    db: AsyncSession,
    project: Project,
    body: bytes,
    commit_sha: str = "sha1",
    branch: str = "main",
    ci_run_id: str = "",
    root: str | None = None,
    status: str = REPORT_PENDING,
) -> Report:
    rep = Report(project_id=project.id, commit_sha=commit_sha, branch=branch,
                 ci_run_id=ci_run_id, root=root, body=body, status=status)
    db.add(rep)
    await db.flush()
    return rep
