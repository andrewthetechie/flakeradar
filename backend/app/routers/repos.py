"""Admin API: delete a Repo or a single Project (issue #39). No UI, no MCP tool."""

from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession

from .. import schemas
from ..auth import require_token
from ..db import get_db
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


# Deleting the Repo/Project row lets the ON DELETE CASCADE foreign keys remove the
# children in the same transaction. Lock order with the processor: claim_next_report
# (app/processing.py) locks the pending Report it is processing FOR UPDATE SKIP LOCKED
# for the whole transaction, so this DELETE blocks until that transaction commits or
# rolls back, while the processor itself never waits on us — it skips a locked row.
# Net effect: a delete may wait for one in-flight Report, and a pending Report counted
# here can still be processed first and is then deleted as already processed. Pending
# and failed Reports are deleted exactly like processed ones.


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
    await db.delete(repo)
    await db.commit()
    return schemas.RepoDeleteOut(repo=repo.name, deleted=deleted)


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
    await db.delete(proj)
    await db.commit()
    return schemas.ProjectDeleteOut(repo=repo.name, project=proj.name, deleted=deleted)
