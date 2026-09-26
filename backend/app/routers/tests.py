"""Dashboard API: Repos, leaderboard, summary, Test detail and quarantine."""

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy.ext.asyncio import AsyncSession

from .. import queries, schemas
from ..auth import require_token
from ..classify import FailureCategory
from ..config import get_settings
from ..db import get_db
from ..identity import DEFAULT_PROJECT, normalize_project, normalize_repo

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
    category: FailureCategory | None = Query(default=None),
    db: AsyncSession = Depends(get_db),
):
    return await queries.list_tests(
        db,
        scope,
        threshold=get_settings().flake_threshold,
        include_stable=include_stable,
        sort=sort,
        page=page,
        page_size=page_size,
        file=file,
        category=category,
    )


@router.get("/api/summary", response_model=schemas.SummaryOut)
async def summary(
    scope: queries.Scope = Depends(scope_from_query),
    db: AsyncSession = Depends(get_db),
):
    return await queries.summary(db, scope, threshold=get_settings().flake_threshold)


@router.get("/api/tests/{test_id}/history", response_model=schemas.HistoryOut)
async def test_history(
    test_id: int,
    limit: int = Query(default=60, ge=1, le=500),
    db: AsyncSession = Depends(get_db),
):
    history = await queries.get_test(db, test_id, threshold=get_settings().flake_threshold, executions_limit=limit)
    if history is None:
        raise HTTPException(status_code=404, detail="Test not found")
    return history


@router.post("/api/tests/{test_id}/quarantine", response_model=schemas.TestOut)
async def set_quarantine(
    test_id: int,
    body: schemas.QuarantineIn,
    db: AsyncSession = Depends(get_db),
):
    result = await queries.set_quarantine(db, test_id, body.quarantined, threshold=get_settings().flake_threshold)
    if result is None:
        raise HTTPException(status_code=404, detail="Test not found")
    return result


@router.get(
    "/api/quarantine",
    response_model=list[schemas.QuarantineItem],
    dependencies=[Depends(require_token)],
)
async def quarantine_list(
    repo: str = Query(..., max_length=255),
    project: str = Query(default=DEFAULT_PROJECT, max_length=100),
    db: AsyncSession = Depends(get_db),
):
    """For test runners: the Tests to skip in one Repo + Project."""
    try:
        repo_name, project_name = normalize_repo(repo), normalize_project(project)
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
    return await queries.quarantine_list(db, repo_name, project_name)
