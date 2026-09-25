"""Dashboard read API: Repos, the Test leaderboard and summary tiles."""
from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy.ext.asyncio import AsyncSession

from .. import queries, schemas
from ..config import get_settings
from ..db import get_db
from ..identity import normalize_project, normalize_repo

router = APIRouter()


def scope_from_query(
    repo: str | None = Query(default=None, max_length=255),
    project: str | None = Query(default=None, max_length=100),
) -> queries.Scope:
    """?repo=&project= → Scope. 422 for bad names or project without repo."""
    try:
        return queries.Scope(
            repo=normalize_repo(repo) if repo is not None else None,
            project=normalize_project(project) if project is not None else None,
        )
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc


@router.get("/api/repos", response_model=list[schemas.RepoOut])
async def list_repos(db: AsyncSession = Depends(get_db)):
    return await queries.list_repos(db)


@router.get("/api/tests", response_model=schemas.TestPage)
async def list_tests(
    scope: queries.Scope = Depends(scope_from_query),
    include_stable: bool = Query(default=False),
    sort: queries.SortKey = Query(default="score"),
    page: int = Query(default=1, ge=1),
    page_size: int = Query(default=50, ge=1, le=100),
    file: str | None = Query(default=None, max_length=1024),
    db: AsyncSession = Depends(get_db),
):
    return await queries.list_tests(
        db, scope, threshold=get_settings().flake_threshold,
        include_stable=include_stable, sort=sort, page=page,
        page_size=page_size, file=file,
    )


@router.get("/api/summary", response_model=schemas.SummaryOut)
async def summary(
    scope: queries.Scope = Depends(scope_from_query),
    db: AsyncSession = Depends(get_db),
):
    return await queries.summary(db, scope, threshold=get_settings().flake_threshold)
