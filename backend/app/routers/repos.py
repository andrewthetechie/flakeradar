"""Admin API: delete a Repo or a single Project (issue #39). No UI, no MCP tool."""

from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy import func, select
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import AsyncSession

from .. import schemas
from ..auth import require_token
from ..db import get_db
from ..github_integration import close_issues_for_deleted_repo
from ..identity import normalize_project, normalize_repo
from ..models import (
    Job,
    Pipeline,
    Project,
    Repo,
    Report,
    TestCase,
    TestExecution,
)

router = APIRouter()


async def _scalar(db: AsyncSession, stmt) -> int:
    return (await db.execute(stmt)).scalar_one()


async def _count_project_deletions(db: AsyncSession, project_id: int) -> schemas.DeletedCountsOut:
    tests = select(func.count()).select_from(TestCase).where(TestCase.project_id == project_id)
    executions = (
        select(func.count())
        .select_from(TestExecution)
        .join(TestCase, TestExecution.test_case_id == TestCase.id)
        .where(TestCase.project_id == project_id)
    )
    # Jobs hang off Pipelines, which belong to the Repo, so a Project delete removes none.
    reports = select(func.count()).select_from(Report).where(Report.project_id == project_id)
    return schemas.DeletedCountsOut(
        tests=await _scalar(db, tests),
        executions=await _scalar(db, executions),
        jobs=0,
        reports=await _scalar(db, reports),
    )


async def _count_repo_deletions(db: AsyncSession, repo_id: int) -> schemas.DeletedCountsOut:
    tests = (
        select(func.count())
        .select_from(TestCase)
        .join(Project, TestCase.project_id == Project.id)
        .where(Project.repo_id == repo_id)
    )
    executions = (
        select(func.count())
        .select_from(TestExecution)
        .join(TestCase, TestExecution.test_case_id == TestCase.id)
        .join(Project, TestCase.project_id == Project.id)
        .where(Project.repo_id == repo_id)
    )
    jobs = (
        select(func.count())
        .select_from(Job)
        .join(Pipeline, Job.pipeline_id == Pipeline.id)
        .where(Pipeline.repo_id == repo_id)
    )
    reports = select(func.count()).select_from(Report).where(Report.repo_id == repo_id)
    return schemas.DeletedCountsOut(
        tests=await _scalar(db, tests),
        executions=await _scalar(db, executions),
        jobs=await _scalar(db, jobs),
        reports=await _scalar(db, reports),
    )


async def _issue_numbers(db: AsyncSession, *stmts) -> list[int]:
    """Sorted, deduplicated issue numbers of the rows the given queries select.

    The rows still have to exist, so this runs before the cascade: once the
    Test/Job row is gone, `sync_closed_issues` no longer sees the issue and it
    would stay open forever pointing at a dead permalink.
    """
    numbers: set[int] = set()
    for stmt in stmts:
        for number in (await db.execute(stmt)).scalars().all():
            if number is not None:
                numbers.add(number)
    return sorted(numbers)


async def _project_issue_numbers(db: AsyncSession, project_id: int) -> tuple[list[int], list[int]]:
    """Every stored issue number of the Project, and the FlakeRadar-filed subset.

    A human-linked issue (flag False, or NULL from before the marker existed) is
    reported but never auto-closed: FlakeRadar does not own it.
    """
    # Jobs hang off Pipelines, which belong to the Repo, so a Project delete strands no Job issue.
    tests = select(TestCase.github_issue_number).where(
        TestCase.project_id == project_id, TestCase.github_issue_number.is_not(None)
    )
    filed = tests.where(TestCase.github_issue_filed.is_(True))
    return await _issue_numbers(db, tests), await _issue_numbers(db, filed)


async def _repo_issue_numbers(db: AsyncSession, repo_id: int) -> tuple[list[int], list[int]]:
    """Every stored issue number of the Repo, and the FlakeRadar-filed subset."""
    tests = (
        select(TestCase.github_issue_number)
        .join(Project, TestCase.project_id == Project.id)
        .where(Project.repo_id == repo_id, TestCase.github_issue_number.is_not(None))
    )
    jobs = (
        select(Job.github_issue_number)
        .join(Pipeline, Job.pipeline_id == Pipeline.id)
        .where(Pipeline.repo_id == repo_id, Job.github_issue_number.is_not(None))
    )
    issues = await _issue_numbers(db, tests, jobs)
    filed = await _issue_numbers(
        db, tests.where(TestCase.github_issue_filed.is_(True)), jobs.where(Job.github_issue_filed.is_(True))
    )
    return issues, filed


# Postgres SQLSTATEs an admin retry can resolve: deadlock_detected, serialization_failure.
_DEADLOCK_SQLSTATES = frozenset({"40P01", "40001"})


def _is_deadlock(exc: DBAPIError) -> bool:
    """True when Postgres aborted us for a lock cycle the caller may retry."""
    return getattr(exc.orig, "sqlstate", None) in _DEADLOCK_SQLSTATES


# Deleting the Repo/Project row lets the ON DELETE CASCADE foreign keys remove the
# children in the same transaction. The lock order with the processor is a two-way wait:
# claim_next_report (app/processing.py) locks the pending Report it is processing FOR UPDATE
# SKIP LOCKED for the whole transaction, so this DELETE blocks until that transaction commits
# or rolls back; and in the other direction, the processor's later writes — the TestRun and
# TestCase inserts (app/processing.py) — take FOR KEY SHARE on the parent Repo/Project row
# through their foreign keys, which conflicts with this endpoint's FOR UPDATE. That cycle is
# a real deadlock, and Postgres may abort either transaction with it (SQLSTATE 40P01); this
# endpoint answers being chosen with a rollback and a retryable 503. Net effect: a delete may
# wait for one in-flight Report, and a pending Report counted here can still be processed
# first and is then deleted as already processed. Pending and failed Reports are deleted
# exactly like processed ones.


@router.delete(
    "/api/repos/{owner}/{name}",
    response_model=schemas.RepoDeleteOut,
    dependencies=[Depends(require_token)],
)
async def delete_repo(owner: str, name: str, db: AsyncSession = Depends(get_db)):
    """Delete a Repo and, by cascade, everything in it."""
    try:
        repo_name = normalize_repo(f"{owner}/{name}")
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc

    repo = (await db.execute(select(Repo).where(Repo.name == repo_name).with_for_update())).scalar_one_or_none()
    if repo is None:
        raise HTTPException(status_code=404, detail="Repo not found")

    deleted = await _count_repo_deletions(db, repo.id)
    issues, filed = await _repo_issue_numbers(db, repo.id)
    try:
        await db.delete(repo)
        await db.commit()
    except DBAPIError as exc:
        await db.rollback()
        if _is_deadlock(exc):
            raise HTTPException(status_code=503, detail="Delete raced with the report processor; retry.") from exc
        raise
    # Best-effort close of the stranded issues, after the commit: a slow or
    # failing GitHub API neither holds the DB transaction nor changes the
    # response. Only issues FlakeRadar filed: a human-linked one is reported for
    # manual cleanup and must never be commented on or closed by us.
    await close_issues_for_deleted_repo(repo.name, filed)
    return schemas.RepoDeleteOut(repo=repo.name, deleted=deleted, github_issue_numbers=issues)


@router.delete(
    "/api/repos/{owner}/{name}/projects/{project}",
    response_model=schemas.ProjectDeleteOut,
    dependencies=[Depends(require_token)],
)
async def delete_project(owner: str, name: str, project: str, db: AsyncSession = Depends(get_db)):
    """Delete one Project of a Repo and, by cascade, its Tests, Runs, Executions and Reports."""
    try:
        repo_name = normalize_repo(f"{owner}/{name}")
        project_name = normalize_project(project)
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc

    repo = (await db.execute(select(Repo).where(Repo.name == repo_name))).scalar_one_or_none()
    if repo is None:
        raise HTTPException(status_code=404, detail="Repo not found")

    proj = (
        await db.execute(
            select(Project).where(Project.repo_id == repo.id, Project.name == project_name).with_for_update()
        )
    ).scalar_one_or_none()
    if proj is None:
        raise HTTPException(status_code=404, detail="Project not found")

    deleted = await _count_project_deletions(db, proj.id)
    issues, filed = await _project_issue_numbers(db, proj.id)
    try:
        await db.delete(proj)
        await db.commit()
    except DBAPIError as exc:
        await db.rollback()
        if _is_deadlock(exc):
            raise HTTPException(status_code=503, detail="Delete raced with the report processor; retry.") from exc
        raise
    # Same as the Repo delete: only after the commit, a GitHub failure never
    # reaches the caller, and only FlakeRadar-filed issues are touched at all.
    await close_issues_for_deleted_repo(repo.name, filed)
    return schemas.ProjectDeleteOut(
        repo=repo.name, project=proj.name, deleted=deleted, github_issue_numbers=issues
    )
