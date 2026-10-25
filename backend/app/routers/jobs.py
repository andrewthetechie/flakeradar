"""Dashboard API: Jobs leaderboard, summary, and Job history."""

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy.ext.asyncio import AsyncSession

from .. import issue_links, queries, schemas
from ..auth import require_token
from ..config import get_settings
from ..db import get_db
from ..identity import normalize_repo

router = APIRouter()


def _repo(repo: str | None) -> str | None:
    """Normalize an optional ?repo= value; 422 for an invalid name."""
    if repo is None:
        return None
    try:
        return normalize_repo(repo)
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc


@router.get("/api/jobs", response_model=schemas.JobPage)
async def list_jobs(
    repo: str | None = Query(default=None, max_length=255),
    include_stable: bool = Query(default=False),
    sort: queries.SortKey = Query(default="score"),
    page: int = Query(default=1, ge=1),
    page_size: int = Query(default=50, ge=1, le=100),
    q: str | None = Query(default=None, max_length=200),
    pipeline: str | None = Query(default=None, max_length=512),
    has_issue: bool | None = Query(default=None),
    new_within_days: int | None = Query(default=None, ge=1, le=90),
    db: AsyncSession = Depends(get_db),
):
    return await queries.list_jobs(
        db,
        _repo(repo),
        threshold=get_settings().flake_threshold,
        include_stable=include_stable,
        sort=sort,
        page=page,
        page_size=page_size,
        q=q,
        pipeline=pipeline,
        has_issue=has_issue,
        new_within_days=new_within_days,
    )


# Declared before /api/jobs/{job_id}/... so "summary" is not parsed as an id.
@router.get("/api/jobs/summary", response_model=schemas.JobSummaryOut)
async def summary(repo: str | None = Query(default=None, max_length=255), db: AsyncSession = Depends(get_db)):
    return await queries.job_summary(db, _repo(repo), threshold=get_settings().flake_threshold)


@router.get("/api/jobs/{job_id}/history", response_model=schemas.JobHistoryOut)
async def job_history(
    job_id: int,
    limit: int = Query(default=60, ge=1, le=500),
    db: AsyncSession = Depends(get_db),
):
    history = await queries.get_job(db, job_id, threshold=get_settings().flake_threshold, executions_limit=limit)
    if history is None:
        raise HTTPException(status_code=404, detail="Job not found")
    return history


@router.put("/api/jobs/{job_id}/issue", response_model=schemas.JobOut, dependencies=[Depends(require_token)])
async def link_issue(
    job_id: int,
    body: schemas.IssueLinkIn,
    replace: bool = Query(default=False),
    db: AsyncSession = Depends(get_db),
):
    """Link a GitHub issue filed elsewhere; FlakeRadar then files none for this Job."""
    try:
        return await issue_links.link_job_issue(
            db, job_id, body.number, url=body.url, replace=replace, threshold=get_settings().flake_threshold
        )
    except issue_links.IssueLinkError as exc:
        raise HTTPException(status_code=exc.status_code, detail=str(exc)) from exc


@router.delete("/api/jobs/{job_id}/issue", response_model=schemas.JobOut, dependencies=[Depends(require_token)])
async def unlink_issue(job_id: int, db: AsyncSession = Depends(get_db)):
    """Remove the Job's Issue link."""
    try:
        return await issue_links.unlink_job_issue(db, job_id, threshold=get_settings().flake_threshold)
    except issue_links.IssueLinkError as exc:
        raise HTTPException(status_code=exc.status_code, detail=str(exc)) from exc
