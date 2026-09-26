"""Dashboard API: Jobs leaderboard, summary, and Job history."""

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy.ext.asyncio import AsyncSession

from .. import queries, schemas
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
    sort: queries.JobSortKey = Query(default="score"),
    page: int = Query(default=1, ge=1),
    page_size: int = Query(default=50, ge=1, le=100),
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
