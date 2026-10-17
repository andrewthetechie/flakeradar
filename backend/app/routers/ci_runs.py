"""CI run read API: the failing Tests and failed Jobs of one CI run attempt."""

from fastapi import APIRouter, Depends, HTTPException, Path, Query
from sqlalchemy.ext.asyncio import AsyncSession

from .. import queries, schemas
from ..config import get_settings
from ..db import get_db
from ..identity import normalize_repo

router = APIRouter()


@router.get("/api/ci-runs/{ci_run_id}/failures", response_model=schemas.CiRunFailuresOut)
async def ci_run_failures(
    ci_run_id: str = Path(..., min_length=1, max_length=255),
    repo: str = Query(..., max_length=255),
    ci_run_attempt: int | None = Query(default=None, ge=1),
    db: AsyncSession = Depends(get_db),
):
    try:
        name = normalize_repo(repo)
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
    try:
        return await queries.ci_run_failures(
            db, name, ci_run_id, ci_run_attempt, threshold=get_settings().flake_threshold
        )
    except queries.CiRunNotFound as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
