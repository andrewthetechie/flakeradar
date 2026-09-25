# 03 — Queued ingest and the Report API

## Tracer-Bullet Outcome
CI can `POST /api/ingest?repo=owner/name&project=backend&commit_sha=…` and get `202 {"report_id": 7, "status": "pending"}` almost immediately. The raw Report is stored durably, and the Repo and Project are created if they are new. Anyone can then check the Report with `GET /api/reports/7`, list reports, and see the queue counts at `GET /api/reports/summary`. A token holder can retry a failed Report.

Nothing processes Reports yet (task 04/05). After this task, uploaded Reports stay `pending`.

## User Story
As a CI pipeline, I want my upload accepted in milliseconds even when the server is busy, so that no test results are lost.

## Description
Add the Repo/Project/root naming rules and a race-safe get-or-create (`app/identity.py`). Replace `app/schemas.py` with the Report DTOs. Add an APIRouter with the ingest endpoint and four Report endpoints (`app/routers/reports.py`), and include it in `main.py`. The ingest endpoint validates the query parameters and checks that the XML parses (in a worker thread, off the event loop). Then it stores a `pending` `Report` row and returns 202.

## Context Pack
- Source decisions (from `00-shared-context.md` → Decisions; ADR 0001 and ADR 0002):
  - `repo` is required, lowercased, and matches `^[a-z0-9_.-]+/[a-z0-9_.-]+$` (≤255 chars); otherwise 422.
  - `project` is lowercased, matches `^[a-z0-9_.-]{1,100}$`, and defaults to `default`.
  - `root` is optional; it is normalized, and a `..` segment → 422.
  - Repos and Projects are auto-created.
  - Empty body → 400. Body > 20 MB → 413. Unparseable XML → 422.
  - The Report read endpoints need no token; retry needs the token.
- Repo facts:
  - `app/auth.py` (task 01) provides `def require_token(x_api_key: str = Header(default="")) -> None`, which raises `HTTPException(401, "Invalid or missing X-API-Key")`.
  - `app/db.py` provides `async def get_db() -> AsyncIterator[AsyncSession]`.
  - `app/models.py` provides `Repo`, `Project`, `Report` and the constants `REPORT_PENDING`, `REPORT_PROCESSED`, `REPORT_FAILED` and `REPORT_STATUSES`.
  - `app/parsing.py` (task 02) provides `parse_junit_xml(content: bytes) -> list[ParsedCase]`, which raises `ParseError(ValueError)`.
  - The old (pre-fork) ingest handler accepted **either** a multipart field named `report` **or** the raw body, through a FastAPI `report: UploadFile | None = File(default=None)` parameter. **Do not keep that parameter.** It was verified to break on a live server (starlette 1.7): `curl --data-binary @junit.xml` without `-H Content-Type` sends `application/x-www-form-urlencoded`, FastAPI consumes the stream as a form, and `request.body()` raises `RuntimeError: Stream consumed` (HTTP 500). Use the `_read_report(request)` helper below, which reads the form only for `multipart/form-data`.
  - `backend/app/schemas.py` still contains the old pre-fork DTOs, which nothing imports any more. **Replace the whole file.**
  - In `backend/app/main.py`, routers must be included at the marked spot, **above** the `StaticFiles` mount.
- Non-goals: processing Reports (04); the background worker (05); any test/leaderboard read endpoints (08+); a UI for Reports (13); deleting Reports.

## Delivery Strategy
- Shape: Wide refactor: Migrate (integration branch). The slice is complete end-to-end for "upload → queued → visible".
- Valid-state scope: Named integration branch `feat/repo-split`.

## Implementation Contract
- Expected files:
  - Create `backend/app/identity.py` and `backend/app/routers/reports.py`.
  - Replace `backend/app/schemas.py`.
  - Edit `backend/app/main.py`: two lines, shown below.
  - Create the tests `backend/tests/test_identity.py`, `backend/tests/test_ingest_api.py` and `backend/tests/test_reports_api.py`.
- Interfaces and names — target code (verified: the tests below pass against it):

`backend/app/identity.py`
```python
"""Repo / Project / Project-root naming rules and get-or-create.

A Repo is a lowercase ``owner/name``; a Project is a lowercase name unique
within its Repo (``default`` when omitted). See CONTEXT.md and ADR 0001.
"""
import re

from sqlalchemy import select
from sqlalchemy.dialects.postgresql import insert as pg_insert
from sqlalchemy.ext.asyncio import AsyncSession

from .models import Project, Repo

REPO_RE = re.compile(r"^[a-z0-9_.-]+/[a-z0-9_.-]+$")
PROJECT_RE = re.compile(r"^[a-z0-9_.-]{1,100}$")
DEFAULT_PROJECT = "default"


def normalize_repo(raw: str) -> str:
    value = raw.strip().lower()
    if len(value) > 255 or not REPO_RE.match(value):
        raise ValueError(f"repo must look like 'owner/name', got {raw!r}")
    return value


def normalize_project(raw: str | None) -> str:
    value = (raw or "").strip().lower() or DEFAULT_PROJECT
    if not PROJECT_RE.match(value):
        raise ValueError(
            f"project must be 1-100 chars of a-z, 0-9, '.', '_' or '-', got {raw!r}"
        )
    return value


def normalize_root(raw: str | None) -> str | None:
    """None = not sent (leave the Project's root alone); "" = repo root."""
    if raw is None:
        return None
    value = raw.strip()
    while value.startswith("./"):
        value = value[2:]
    value = value.strip("/")
    if ".." in value.split("/"):
        raise ValueError(f"root must not contain '..', got {raw!r}")
    return value


async def get_or_create_project(db: AsyncSession, repo: str, project: str) -> Project:
    """Race-safe get-or-create for normalized names (ON CONFLICT DO NOTHING)."""
    await db.execute(pg_insert(Repo).values(name=repo).on_conflict_do_nothing(
        index_elements=["name"]))
    repo_id = (await db.execute(select(Repo.id).where(Repo.name == repo))).scalar_one()
    await db.execute(pg_insert(Project).values(repo_id=repo_id, name=project)
                     .on_conflict_do_nothing(constraint="uq_projects_repo_name"))
    return (await db.execute(
        select(Project).where(Project.repo_id == repo_id, Project.name == project)
    )).scalar_one()
```

`backend/app/schemas.py` (replace the whole file. Tasks 08 and 09 will append more models to it)
```python
"""Pydantic response models — the typed contract the frontend and MCP consume."""
from datetime import datetime
from typing import Any

from pydantic import BaseModel


class IngestAccepted(BaseModel):
    report_id: int
    status: str  # always "pending"


class ReportOut(BaseModel):
    id: int
    repo: str
    project: str
    commit_sha: str
    branch: str
    ci_run_id: str
    status: str  # pending | processed | failed
    error: str | None
    counts: dict[str, Any] | None
    run_id: int | None
    created_at: datetime
    processed_at: datetime | None


class ReportSummaryOut(BaseModel):
    pending: int
    failed: int
```

`backend/app/routers/reports.py`
```python
"""Ingest (accept-then-process, ADR 0002) and the Report status endpoints."""
import asyncio

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from starlette.datastructures import UploadFile
from sqlalchemy import Select, func, select
from sqlalchemy.ext.asyncio import AsyncSession

from .. import schemas
from ..auth import require_token
from ..db import get_db
from ..identity import (
    DEFAULT_PROJECT, get_or_create_project, normalize_project, normalize_repo,
    normalize_root,
)
from ..models import (
    REPORT_FAILED, REPORT_PENDING, REPORT_STATUSES, Project, Repo, Report,
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
    row = Report(
        project_id=proj.id, commit_sha=commit_sha, branch=branch,
        ci_run_id=ci_run_id, root=root_value, body=content, status=REPORT_PENDING,
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
        id=report.id, repo=repo, project=project, commit_sha=report.commit_sha,
        branch=report.branch, ci_run_id=report.ci_run_id, status=report.status,
        error=report.error, counts=report.counts, run_id=report.run_id,
        created_at=report.created_at, processed_at=report.processed_at,
    )


# Declared before /api/reports/{report_id} so "summary" is not parsed as an id.
@router.get("/api/reports/summary", response_model=schemas.ReportSummaryOut)
async def report_summary(db: AsyncSession = Depends(get_db)):
    rows = (await db.execute(
        select(Report.status, func.count())
        .where(Report.status.in_([REPORT_PENDING, REPORT_FAILED]))
        .group_by(Report.status)
    )).all()
    counts = dict(rows)
    return schemas.ReportSummaryOut(
        pending=counts.get(REPORT_PENDING, 0), failed=counts.get(REPORT_FAILED, 0)
    )


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
```

`backend/app/main.py` edits:
```python
# with the other imports
from .routers import reports

# at the marked spot, directly under the comment
# "# A Mount("/") registered earlier would swallow every later route."
app.include_router(reports.router)
```

- Verified external contracts: `pg_insert(...).on_conflict_do_nothing(index_elements=["name"])` and `on_conflict_do_nothing(constraint="uq_projects_repo_name")` (SQLAlchemy 2.1 postgresql dialect, verified in the spike); `request.form()` → `form.get("report")` is a `starlette.datastructures.UploadFile` for multipart uploads (verified by `test_ingest_multipart_upload`); `request.body()` for any other Content-Type (verified by `test_ingest_raw_body_any_content_type` and a live `curl --data-binary` smoke run).
- Behavior rules:
  - Validation order: empty body → 400; size → 413; names → 422; XML parse → 422. Only after all of these pass are rows written, so a rejected upload creates **no** Repo, Project or Report rows.
  - `GET /api/reports` returns reports newest first (`id DESC`) and takes an optional `status` filter. `limit` defaults to 50 and must be between 1 and 200. An unknown status → 422 with `detail` `"status must be one of pending, processed, failed"`.
  - `/api/reports/summary` must be declared **before** `/api/reports/{report_id}`.
  - Retry: an unknown id → 404 `"Report not found"`. A non-failed Report → 409 `"Only failed reports can be retried"`. Otherwise, set `status=pending`, `error=None`, `processed_at=None` and return the updated `ReportOut`.
  - `ReportOut` never includes the raw `body`.
- Error and security rules: ingest and retry need `X-API-Key`; the reads are open (the instance is internal-only). Never log the body.

## Acceptance Criteria
- [ ] A valid upload returns 202 with an integer `report_id` and `"status": "pending"`. The stored `Report` has the exact uploaded bytes, and its Repo and Project are lowercased.
- [ ] Missing `repo`, `repo=writers-app`, `project=front end` and `root=../x` each → 422.
- [ ] A raw body sent as `application/x-www-form-urlencoded`, `application/xml` or `text/plain` → 202 (curl's default Content-Type must work).
- [ ] An empty body → 400. `not xml at all` → 422 containing `Not a valid JUnit XML report`, with no `Report` row created.
- [ ] `GET /api/reports/summary` → `{"pending": 1, "failed": 1}` for the seed in `test_reports_api.py`.
- [ ] Retrying a failed Report returns 200 with `status == "pending"`. Retrying a pending one → 409. Retrying without the token → 401.

## Test Expectations
- Framework: pytest + pytest-asyncio, with the `client`/`db` fixtures from `tests/conftest.py` and seeds from `tests/factories.py` (task 01).
- `backend/tests/test_identity.py`:
```python
"""Repo/Project/root naming rules and race-safe get-or-create."""
import pytest
from sqlalchemy import func, select

from app.identity import (
    get_or_create_project, normalize_project, normalize_repo, normalize_root,
)
from app.models import Project, Repo


def test_normalize_repo():
    assert normalize_repo("  AndrewTheTechie/Writers-App ") == "andrewthetechie/writers-app"
    for bad in ["", "writers-app", "a/b/c", "a b/c", "owner/"]:
        with pytest.raises(ValueError):
            normalize_repo(bad)


def test_normalize_project():
    assert normalize_project(None) == "default"
    assert normalize_project("") == "default"
    assert normalize_project(" Backend ") == "backend"
    for bad in ["front end", "a/b", "x" * 101]:
        with pytest.raises(ValueError):
            normalize_project(bad)


def test_normalize_root():
    assert normalize_root(None) is None
    assert normalize_root("") == ""
    assert normalize_root("./frontend/") == "frontend"
    assert normalize_root("/apps/web") == "apps/web"
    with pytest.raises(ValueError):
        normalize_root("../secrets")


async def test_get_or_create_project_is_idempotent(db):
    a = await get_or_create_project(db, "acme/app", "backend")
    b = await get_or_create_project(db, "acme/app", "backend")
    c = await get_or_create_project(db, "other/app", "backend")
    await db.commit()
    assert a.id == b.id != c.id
    assert (await db.execute(select(func.count(Repo.id)))).scalar() == 2
    assert (await db.execute(select(func.count(Project.id)))).scalar() == 2
```
- `backend/tests/test_ingest_api.py`:
```python
"""Queued ingest: validate, store a pending Report, answer 202."""
from sqlalchemy import select

from app.models import Project, Repo, Report
from tests.conftest import AUTH, make_junit


def _url(**params) -> str:
    base = {"repo": "Acme/App", "commit_sha": "sha1"}
    base.update(params)
    return "/api/ingest?" + "&".join(f"{k}={v}" for k, v in base.items())


async def test_ingest_requires_token(client):
    resp = await client.post(_url(), content=make_junit([("t1", "passed")]))
    assert resp.status_code == 401


async def test_ingest_queues_report(client, db):
    resp = await client.post(
        _url(project="Backend", root="./server/", branch="feat", ci_run_id="42-1"),
        content=make_junit([("t1", "passed")]), headers=AUTH,
    )
    assert resp.status_code == 202, resp.text
    body = resp.json()
    assert body["status"] == "pending"

    report = (await db.execute(select(Report).where(Report.id == body["report_id"]))).scalar_one()
    assert (report.status, report.commit_sha, report.branch, report.ci_run_id, report.root) == \
        ("pending", "sha1", "feat", "42-1", "server")
    assert report.body == make_junit([("t1", "passed")])
    proj = (await db.execute(select(Project).where(Project.id == report.project_id))).scalar_one()
    repo = (await db.execute(select(Repo).where(Repo.id == proj.repo_id))).scalar_one()
    assert (repo.name, proj.name) == ("acme/app", "backend")


async def test_ingest_defaults_project_to_default(client, db):
    resp = await client.post(_url(), content=make_junit([("t1", "passed")]), headers=AUTH)
    assert resp.status_code == 202
    names = (await db.execute(select(Project.name))).scalars().all()
    assert names == ["default"]


async def test_ingest_multipart_upload(client):
    resp = await client.post(
        _url(), headers=AUTH,
        files={"report": ("junit.xml", make_junit([("t1", "passed")]), "text/xml")},
    )
    assert resp.status_code == 202


async def test_ingest_raw_body_any_content_type(client):
    # curl --data-binary @junit.xml sends application/x-www-form-urlencoded.
    for ctype in ("application/x-www-form-urlencoded", "application/xml", "text/plain"):
        resp = await client.post(_url(), content=make_junit([("t1", "passed")]),
                                 headers={**AUTH, "Content-Type": ctype})
        assert resp.status_code == 202, (ctype, resp.text)


async def test_ingest_rejects_missing_repo(client):
    resp = await client.post("/api/ingest?commit_sha=s", content=make_junit([("t", "passed")]),
                             headers=AUTH)
    assert resp.status_code == 422


async def test_ingest_rejects_bad_names(client):
    for url in (_url(repo="writers-app"), _url(project="front end"), _url(root="../x")):
        resp = await client.post(url, content=make_junit([("t", "passed")]), headers=AUTH)
        assert resp.status_code == 422, url


async def test_ingest_rejects_empty_and_garbage(client, db):
    assert (await client.post(_url(), headers=AUTH)).status_code == 400
    garbage = await client.post(_url(), content=b"not xml at all", headers=AUTH)
    assert garbage.status_code == 422
    assert "Not a valid JUnit XML report" in garbage.json()["detail"]
    assert (await db.execute(select(Report))).first() is None
```
- `backend/tests/test_reports_api.py`:
```python
"""Report status, listing, summary and retry."""
from app.models import REPORT_FAILED, REPORT_PROCESSED
from tests.conftest import AUTH
from tests.factories import make_project, make_report


async def _seed(db):
    proj = await make_project(db, "acme/app", "backend")
    pending = await make_report(db, proj, b"<x/>", commit_sha="a")
    failed = await make_report(db, proj, b"<x/>", commit_sha="b", status=REPORT_FAILED)
    failed.error = "ParseError: boom"
    done = await make_report(db, proj, b"<x/>", commit_sha="c", status=REPORT_PROCESSED)
    await db.commit()
    return pending, failed, done


async def test_get_report(client, db):
    pending, _, _ = await _seed(db)
    body = (await client.get(f"/api/reports/{pending.id}")).json()
    assert body["repo"] == "acme/app" and body["project"] == "backend"
    assert body["status"] == "pending" and body["counts"] is None
    assert "body" not in body
    assert (await client.get("/api/reports/9999")).status_code == 404


async def test_list_and_summary(client, db):
    pending, failed, done = await _seed(db)
    ids = [r["id"] for r in (await client.get("/api/reports")).json()]
    assert ids == [done.id, failed.id, pending.id]  # newest first
    only_failed = (await client.get("/api/reports?status=failed")).json()
    assert [r["error"] for r in only_failed] == ["ParseError: boom"]
    assert (await client.get("/api/reports?status=bogus")).status_code == 422
    assert (await client.get("/api/reports/summary")).json() == {"pending": 1, "failed": 1}


async def test_retry(client, db):
    pending, failed, _ = await _seed(db)
    assert (await client.post(f"/api/reports/{failed.id}/retry")).status_code == 401
    resp = await client.post(f"/api/reports/{failed.id}/retry", headers=AUTH)
    assert resp.status_code == 200
    assert resp.json()["status"] == "pending" and resp.json()["error"] is None
    conflict = await client.post(f"/api/reports/{pending.id}/retry", headers=AUTH)
    assert conflict.status_code == 409
    assert conflict.json()["detail"] == "Only failed reports can be retried"
```

## Dependencies
- Blocked by: 01 — Postgres + async foundation; 02 — JUnit parsing
- Why blocked: 01 supplies the `reports`, `repos` and `projects` tables, `get_db`, `require_token` and the fixtures. 02 supplies `parse_junit_xml`/`ParseError`, used for validation.
- Blocks: 04 (processes the Reports stored here), 13 (UI queue indicator), 14 (docs)

## Labels
`feature`, `backend`, `api`, `ingest`, `priority:high`

## Estimate
Medium

## Risk
3 - This is the CI-facing contract. The API break (required `repo`) is intended (ADR 0001).

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q tests/test_identity.py tests/test_ingest_api.py tests/test_reports_api.py tests/test_parsing.py tests/test_db.py tests/test_scoring.py
# expect: 48 passed
```
