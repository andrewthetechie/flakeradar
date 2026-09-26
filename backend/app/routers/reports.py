"""Ingest (accept-then-process, ADR 0002) and the Report status endpoints."""

import asyncio

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from sqlalchemy import Select, func, select
from sqlalchemy.ext.asyncio import AsyncSession
from starlette.datastructures import UploadFile

from .. import schemas
from ..auth import require_token
from ..db import get_db
from ..identity import (
    DEFAULT_PROJECT,
    get_or_create_project,
    normalize_project,
    normalize_repo,
    normalize_root,
)
from ..models import (
    REPORT_FAILED,
    REPORT_KIND_JUNIT,
    REPORT_PENDING,
    REPORT_STATUSES,
    Project,
    Repo,
    Report,
)
from ..parsing import ParseError, parse_junit_xml

router = APIRouter()

MAX_REPORT_BYTES = 20 * 1024 * 1024


@router.post(
    "/api/ingest",
    status_code=202,
    response_model=schemas.IngestAccepted,
    dependencies=[Depends(require_token)],
)
async def ingest_endpoint(
    request: Request,
    repo: str = Query(..., max_length=255),
    project: str = Query(default=DEFAULT_PROJECT, max_length=100),
    root: str | None = Query(default=None, max_length=1024),
    commit_sha: str = Query(..., min_length=1, max_length=64),
    branch: str = Query(default="main", max_length=255),
    ci_run_id: str = Query(default="", max_length=255),
    default_branch: str | None = Query(default=None, max_length=255),
    db: AsyncSession = Depends(get_db),
):
    """Accept a JUnit XML report as multipart upload (`report`) or raw body.

    Only validation happens here; the Report is queued and processed later.
    """
    content = await _read_report(request)
    if not content:
        raise HTTPException(status_code=400, detail="Empty report body")
    if len(content) > MAX_REPORT_BYTES:
        raise HTTPException(status_code=413, detail="Report exceeds 20 MB limit")
    try:
        repo_name = normalize_repo(repo)
        project_name = normalize_project(project)
        root_value = normalize_root(root)
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
    try:
        # Parsing is CPU-bound: keep it off the event loop.
        await asyncio.to_thread(parse_junit_xml, content)
    except ParseError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc

    proj = await get_or_create_project(db, repo_name, project_name)
    db_default_branch = (default_branch or "").strip() or None
    row = Report(
        kind=REPORT_KIND_JUNIT,
        project_id=proj.id,
        repo_id=proj.repo_id,
        commit_sha=commit_sha,
        branch=branch,
        ci_run_id=ci_run_id,
        root=root_value,
        body=content,
        status=REPORT_PENDING,
        default_branch=db_default_branch,
    )
    db.add(row)
    await db.commit()
    return schemas.IngestAccepted(report_id=row.id, status=REPORT_PENDING)


async def _read_report(request: Request) -> bytes:
    """Multipart field `report`, else the raw body — whatever the Content-Type.

    Not a FastAPI File() parameter: that makes FastAPI parse urlencoded
    bodies (curl --data-binary's default Content-Type) as a form and consume
    the stream before the handler runs.
    """
    if request.headers.get("content-type", "").startswith("multipart/form-data"):
        form = await request.form()
        upload = form.get("report")
        return await upload.read() if isinstance(upload, UploadFile) else b""
    return await request.body()


def _report_query() -> Select:
    return (
        select(Report, Project.name, Repo.name)
        .join(Project, Report.project_id == Project.id)
        .join(Repo, Project.repo_id == Repo.id)
    )


def _report_out(report: Report, project: str, repo: str) -> schemas.ReportOut:
    return schemas.ReportOut(
        id=report.id,
        repo=repo,
        project=project,
        commit_sha=report.commit_sha,
        branch=report.branch,
        ci_run_id=report.ci_run_id,
        status=report.status,
        error=report.error,
        counts=report.counts,
        run_id=report.run_id,
        created_at=report.created_at,
        processed_at=report.processed_at,
    )


# Declared before /api/reports/{report_id} so "summary" is not parsed as an id.
@router.get("/api/reports/summary", response_model=schemas.ReportSummaryOut)
async def report_summary(db: AsyncSession = Depends(get_db)):
    rows = (
        await db.execute(
            select(Report.status, func.count())
            .where(Report.status.in_([REPORT_PENDING, REPORT_FAILED]))
            .group_by(Report.status)
        )
    ).all()
    counts = dict(rows)
    return schemas.ReportSummaryOut(pending=counts.get(REPORT_PENDING, 0), failed=counts.get(REPORT_FAILED, 0))


@router.get("/api/reports", response_model=list[schemas.ReportOut])
async def list_reports(
    status: str | None = Query(default=None),
    limit: int = Query(default=50, ge=1, le=200),
    db: AsyncSession = Depends(get_db),
):
    if status is not None and status not in REPORT_STATUSES:
        raise HTTPException(
            status_code=422,
            detail=f"status must be one of {', '.join(REPORT_STATUSES)}",
        )
    stmt = _report_query().order_by(Report.id.desc()).limit(limit)
    if status is not None:
        stmt = stmt.where(Report.status == status)
    return [_report_out(*row) for row in (await db.execute(stmt)).all()]


@router.get("/api/reports/{report_id}", response_model=schemas.ReportOut)
async def get_report(report_id: int, db: AsyncSession = Depends(get_db)):
    row = (await db.execute(_report_query().where(Report.id == report_id))).first()
    if row is None:
        raise HTTPException(status_code=404, detail="Report not found")
    return _report_out(*row)


@router.post(
    "/api/reports/{report_id}/retry",
    response_model=schemas.ReportOut,
    dependencies=[Depends(require_token)],
)
async def retry_report(report_id: int, db: AsyncSession = Depends(get_db)):
    row = (await db.execute(_report_query().where(Report.id == report_id))).first()
    if row is None:
        raise HTTPException(status_code=404, detail="Report not found")
    report, project, repo = row
    if report.status != REPORT_FAILED:
        raise HTTPException(status_code=409, detail="Only failed reports can be retried")
    report.status = REPORT_PENDING
    report.error = None
    report.processed_at = None
    await db.commit()
    return _report_out(report, project, repo)
