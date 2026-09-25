# 08 — Repos, leaderboard and summary read API

## Tracer-Bullet Outcome
The dashboard (and later the MCP server) can ask:
- `GET /api/repos` — which Repos and Projects exist.
- `GET /api/tests?repo=…&project=…&sort=…&page=…` — the paginated, sortable leaderboard, which by default shows only Flaky and Suspect Tests. Each row carries its `repo`, `project`, `tier` and Location.
- `GET /api/summary` — the same counts, scoped to the same filters.

## User Story
As someone viewing the dashboard, I want to filter the leaderboard to one Repo or Repo/Project, page through it, and see which Repo/Project each test belongs to, so that I can find the flaky tests that matter to me.

## Description
Create `backend/app/queries.py`: the read services shared by REST and MCP (task 10). Create `backend/app/routers/tests.py` with three endpoints, and append the response models to `backend/app/schemas.py`. `/api/projects` is gone (replaced by `/api/repos`). The test-detail, history and quarantine endpoints are task 09.

## Context Pack
- Source decisions (`00-shared-context.md`):
  - The leaderboard shows score > 0 by default, and `include_stable=true` shows all.
  - `sort` is one of `score|last_seen|proven`. Pagination uses `page` (≥1) and `page_size` (1–100, default 50).
  - `project` requires `repo` (422 otherwise). Repo and project names are normalized with the same rules as ingest.
  - The summary gains `suspect_tests`. Tiers are flaky ≥ threshold, suspect > 0, stable = 0. An optional `file` substring filter (used by MCP too).
- Repo facts:
  - Normalizers (task 03, `app/identity.py`): `normalize_repo(raw: str) -> str` and `normalize_project(raw: str | None) -> str`, both raising `ValueError` with a readable message.
  - `get_settings().flake_threshold: float = 0.30`.
  - Models (task 01): `Repo(name)`, `Project(repo_id, name, root)`, `TestCase(project_id, fingerprint, suite, classname, name, file, line, flakiness_score, confirmed_flake_count, last_status, last_seen_at, quarantined, quarantined_at, github_issue_number)`, `TestRun(project_id)`, `TestExecution(test_case_id)`.
  - Factories (task 01): `make_project(db, repo, project, root="")` and `make_test_case(db, project, name="t1", classname="tests.test_mod", suite="unit", **fields)`. `**fields` sets any `TestCase` column, e.g. `flakiness_score=0.8, last_seen_at=...`.
  - `backend/app/schemas.py` currently holds only `IngestAccepted`, `ReportOut` and `ReportSummaryOut` (task 03), with `from typing import Any`.
  - The pre-fork leaderboard ordering was `flakiness_score DESC, last_seen_at DESC`. Keep that as `sort=score`, plus `id` as a final tiebreak.
- Verified external contracts: `func.count()` over `stmt.subquery()` for totals; `ilike(pattern, escape="\\")` for the literal substring match (verified by `test_file_filter_is_literal_substring`); `queries.Scope` returned from a FastAPI dependency (`Depends(scope_from_query)`) (verified).
- Non-goals: the history/detail endpoint, quarantine endpoints and permalinks (09); free-text search (10); any frontend change (11).

## Delivery Strategy
- Shape: Wide refactor: Migrate (integration branch). The frontend still calls the removed `/api/projects` and the old array-shaped `/api/tests` until task 11, so it stays broken.
- Valid-state scope: Named integration branch `feat/repo-split`.

## Implementation Contract
- Expected files:
  - Create `backend/app/queries.py`, `backend/app/routers/tests.py` and `backend/tests/test_tests_api.py`.
  - Edit `backend/app/schemas.py` (change `from typing import Any` to `from typing import Any, Literal` and append the block below) and `backend/app/main.py` (router include).
- Append to `backend/app/schemas.py`:
```python
# --- Tests, Repos, summary (task 08) ------------------------------------

Tier = Literal["flaky", "suspect", "stable"]


class ProjectOut(BaseModel):
    name: str
    root: str


class RepoOut(BaseModel):
    name: str
    projects: list[ProjectOut]


class TestOut(BaseModel):
    __test__ = False  # not a pytest class

    id: int
    repo: str
    project: str
    fingerprint: str
    suite: str
    classname: str
    name: str
    file: str | None
    line: int | None
    flakiness_score: float
    tier: Tier
    confirmed_flake_count: int
    last_status: str
    last_seen_at: datetime
    quarantined: bool
    quarantined_at: datetime | None
    github_issue_number: int | None


class TestPage(BaseModel):
    __test__ = False

    items: list[TestOut]
    total: int
    page: int
    page_size: int


class SummaryOut(BaseModel):
    total_tests: int
    flaky_tests: int
    suspect_tests: int
    confirmed_flaky_tests: int
    total_runs: int
    total_executions: int
    flake_threshold: float
```
  (`__test__ = False` stops pytest from trying to collect these `Test*` classes when a test module imports them.)
- `backend/app/queries.py` (verified):
```python
"""Read services shared by the REST routers and the MCP server.

Every function takes an AsyncSession and returns schemas.* models, so both
front doors (HTTP and MCP) expose exactly the same data.
"""
from dataclasses import dataclass
from typing import Literal

from sqlalchemy import Select, func, select
from sqlalchemy.ext.asyncio import AsyncSession

from . import schemas
from .models import Project, Repo, TestCase, TestExecution, TestRun

SortKey = Literal["score", "last_seen", "proven"]


@dataclass(frozen=True)
class Scope:
    """Which Tests a query covers. `project` requires `repo` (names overlap across Repos)."""
    repo: str | None = None
    project: str | None = None

    def __post_init__(self) -> None:
        if self.project is not None and self.repo is None:
            raise ValueError("project filter requires repo")


def tier_for(score: float, threshold: float) -> schemas.Tier:
    if score >= threshold:
        return "flaky"
    if score > 0:
        return "suspect"
    return "stable"


def escape_like(text: str) -> str:
    """Escape LIKE wildcards so user text matches literally (used with escape='\\\\')."""
    return text.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")


def to_test_out(tc: TestCase, project: str, repo: str, threshold: float) -> schemas.TestOut:
    return schemas.TestOut(
        id=tc.id, repo=repo, project=project, fingerprint=tc.fingerprint,
        suite=tc.suite, classname=tc.classname, name=tc.name,
        file=tc.file, line=tc.line, flakiness_score=tc.flakiness_score,
        tier=tier_for(tc.flakiness_score, threshold),
        confirmed_flake_count=tc.confirmed_flake_count, last_status=tc.last_status,
        last_seen_at=tc.last_seen_at, quarantined=tc.quarantined,
        quarantined_at=tc.quarantined_at, github_issue_number=tc.github_issue_number,
    )


def tests_select() -> Select:
    """SELECT TestCase, Project.name, Repo.name — the row shape to_test_out() takes."""
    return (
        select(TestCase, Project.name, Repo.name)
        .join(Project, TestCase.project_id == Project.id)
        .join(Repo, Project.repo_id == Repo.id)
    )


def _scoped(stmt: Select, scope: Scope) -> Select:
    """Filter a statement that already joins Project and Repo."""
    if scope.repo is not None:
        stmt = stmt.where(Repo.name == scope.repo)
    if scope.project is not None:
        stmt = stmt.where(Project.name == scope.project)
    return stmt


async def list_repos(db: AsyncSession) -> list[schemas.RepoOut]:
    rows = (await db.execute(
        select(Repo.name, Project.name, Project.root)
        .join(Project, Project.repo_id == Repo.id)
        .order_by(Repo.name, Project.name)
    )).all()
    repos: dict[str, schemas.RepoOut] = {}
    for repo, project, root in rows:
        repos.setdefault(repo, schemas.RepoOut(name=repo, projects=[]))
        repos[repo].projects.append(schemas.ProjectOut(name=project, root=root))
    return list(repos.values())


async def list_tests(
    db: AsyncSession,
    scope: Scope,
    *,
    threshold: float,
    include_stable: bool = False,
    flaky_only: bool = False,
    sort: SortKey = "score",
    page: int = 1,
    page_size: int = 50,
    file: str | None = None,
) -> schemas.TestPage:
    stmt = _scoped(tests_select(), scope)
    if flaky_only:
        stmt = stmt.where(TestCase.flakiness_score >= threshold)
    elif not include_stable:
        stmt = stmt.where(TestCase.flakiness_score > 0)
    if file:
        stmt = stmt.where(TestCase.file.ilike(f"%{escape_like(file)}%", escape="\\"))

    total = (await db.execute(
        select(func.count()).select_from(stmt.subquery())
    )).scalar_one()

    order = {
        "score": (TestCase.flakiness_score.desc(), TestCase.last_seen_at.desc(), TestCase.id),
        "last_seen": (TestCase.last_seen_at.desc(), TestCase.id.desc()),
        "proven": (TestCase.confirmed_flake_count.desc(),
                   TestCase.flakiness_score.desc(), TestCase.id),
    }[sort]
    rows = (await db.execute(
        stmt.order_by(*order).offset((page - 1) * page_size).limit(page_size)
    )).all()
    return schemas.TestPage(
        items=[to_test_out(tc, project, repo, threshold) for tc, project, repo in rows],
        total=total, page=page, page_size=page_size,
    )


async def summary(db: AsyncSession, scope: Scope, *, threshold: float) -> schemas.SummaryOut:
    def tests_count(*conditions) -> Select:
        return _scoped(
            select(func.count(TestCase.id))
            .join(Project, TestCase.project_id == Project.id)
            .join(Repo, Project.repo_id == Repo.id)
            .where(*conditions),
            scope,
        )

    total = (await db.execute(tests_count())).scalar_one()
    flaky = (await db.execute(tests_count(TestCase.flakiness_score >= threshold))).scalar_one()
    suspect = (await db.execute(tests_count(
        TestCase.flakiness_score > 0, TestCase.flakiness_score < threshold))).scalar_one()
    confirmed = (await db.execute(tests_count(TestCase.confirmed_flake_count > 0))).scalar_one()
    runs = (await db.execute(_scoped(
        select(func.count(TestRun.id))
        .join(Project, TestRun.project_id == Project.id)
        .join(Repo, Project.repo_id == Repo.id),
        scope,
    ))).scalar_one()
    executions = (await db.execute(_scoped(
        select(func.count(TestExecution.id))
        .join(TestCase, TestExecution.test_case_id == TestCase.id)
        .join(Project, TestCase.project_id == Project.id)
        .join(Repo, Project.repo_id == Repo.id),
        scope,
    ))).scalar_one()
    return schemas.SummaryOut(
        total_tests=total, flaky_tests=flaky, suspect_tests=suspect,
        confirmed_flaky_tests=confirmed, total_runs=runs,
        total_executions=executions, flake_threshold=threshold,
    )
```
- `backend/app/routers/tests.py` (verified):
```python
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
```
- `backend/app/main.py`:
  - Change `from .routers import reports` to `from .routers import reports, tests`.
  - Add `app.include_router(tests.router)` directly under `app.include_router(reports.router)`.
- Behavior rules:
  - `Scope(project=..., repo=None)` raises `ValueError`, which the router turns into 422.
  - An unknown (but valid) repo gives an empty page, not a 404.
  - `flaky_only=True` (used only by MCP in task 10) overrides `include_stable`.
  - The `file` filter is a case-insensitive **literal** substring of `TestCase.file`. `%` and `_` are escaped.
  - Summary counts use the same scope. `total_runs` counts Runs of Projects in scope, and `total_executions` counts Executions of Tests in scope.
- Error and security rules: these read endpoints are open (internal dashboard). There are no secrets in responses.

## Acceptance Criteria
- [ ] `GET /api/repos` returns Repos sorted by name, each with its Projects sorted by name, and `root` included.
- [ ] Default `GET /api/tests` excludes score-0 Tests, orders by score, and gives each item `repo`, `project`, `tier`, `file` and `line`.
- [ ] `?repo=AndrewTheTechie/Writers-App` matches `andrewthetechie/writers-app`. `?project=backend` alone → 422. `?repo=nope` → 422.
- [ ] `?page=2&page_size=3` over 4 Tests → 1 item with `total == 4`. `page_size=101` → 422. `sort=name` → 422.
- [ ] `?file=%25` (a literal `%`) matches nothing.
- [ ] `GET /api/summary` over the seed below → `{"total_tests": 5, "flaky_tests": 2, "suspect_tests": 2, "confirmed_flaky_tests": 1, "total_runs": 1, "total_executions": 1, "flake_threshold": 0.3}`.

## Test Expectations
- Framework: pytest + pytest-asyncio with the `client` and `db` fixtures (real Postgres). Seed data comes from `tests/factories.py`.
- `backend/tests/test_tests_api.py` (verified, 6 passing):
```python
"""Leaderboard, Repos and summary: scoping, tiers, sorting, pagination."""
from datetime import timedelta

from app.models import utcnow
from tests.factories import (
    make_execution, make_project, make_run, make_test_case,
)


async def _seed(db):
    """writers-app/backend: flaky 0.8 (2 proofs), suspect 0.1, stable 0.
    writers-app/frontend: suspect 0.2 in src/app.test.ts.
    fantasy/backend: flaky 0.5 with the SAME test name as writers-app's flaky one."""
    now = utcnow()
    wb = await make_project(db, "andrewthetechie/writers-app", "backend", root="server")
    wf = await make_project(db, "andrewthetechie/writers-app", "frontend")
    fb = await make_project(db, "andrewthetechie/fantasy", "backend")
    await make_test_case(db, wb, name="test_flaky", flakiness_score=0.8,
                         confirmed_flake_count=2, last_seen_at=now - timedelta(hours=3))
    await make_test_case(db, wb, name="test_suspect", flakiness_score=0.1,
                         last_seen_at=now - timedelta(hours=2))
    stable = await make_test_case(db, wb, name="test_stable", flakiness_score=0.0,
                                  last_seen_at=now)
    await make_test_case(db, wf, name="renders", classname="src/app.test.ts",
                         file="src/app.test.ts", line=4, flakiness_score=0.2,
                         last_seen_at=now - timedelta(hours=1))
    await make_test_case(db, fb, name="test_flaky", flakiness_score=0.5,
                         confirmed_flake_count=0, last_seen_at=now - timedelta(hours=5))
    run = await make_run(db, wb)
    await make_execution(db, stable, run)
    await db.commit()


def _names(page) -> list[tuple[str, str, str]]:
    return [(t["repo"], t["project"], t["name"]) for t in page["items"]]


async def test_repos_endpoint(client, db):
    await _seed(db)
    repos = (await client.get("/api/repos")).json()
    assert repos == [
        {"name": "andrewthetechie/fantasy", "projects": [{"name": "backend", "root": ""}]},
        {"name": "andrewthetechie/writers-app", "projects": [
            {"name": "backend", "root": "server"}, {"name": "frontend", "root": ""}]},
    ]


async def test_default_leaderboard_hides_stable_and_sorts_by_score(client, db):
    await _seed(db)
    page = (await client.get("/api/tests")).json()
    assert page["total"] == 4 and page["page"] == 1 and page["page_size"] == 50
    assert _names(page) == [
        ("andrewthetechie/writers-app", "backend", "test_flaky"),
        ("andrewthetechie/fantasy", "backend", "test_flaky"),
        ("andrewthetechie/writers-app", "frontend", "renders"),
        ("andrewthetechie/writers-app", "backend", "test_suspect"),
    ]
    assert [t["tier"] for t in page["items"]] == ["flaky", "flaky", "suspect", "suspect"]
    assert page["items"][2]["file"] == "src/app.test.ts" and page["items"][2]["line"] == 4


async def test_scope_repo_and_project(client, db):
    await _seed(db)
    repo_page = (await client.get("/api/tests?repo=AndrewTheTechie/Writers-App")).json()
    assert {t["repo"] for t in repo_page["items"]} == {"andrewthetechie/writers-app"}
    assert repo_page["total"] == 3
    proj = (await client.get(
        "/api/tests?repo=andrewthetechie/writers-app&project=backend&include_stable=true")).json()
    assert [t["name"] for t in proj["items"]] == ["test_flaky", "test_suspect", "test_stable"]
    assert (await client.get("/api/tests?project=backend")).status_code == 422
    assert (await client.get("/api/tests?repo=nope")).status_code == 422
    unknown = (await client.get("/api/tests?repo=someone/else")).json()
    assert unknown == {"items": [], "total": 0, "page": 1, "page_size": 50}


async def test_sorting_and_pagination(client, db):
    await _seed(db)
    proven = (await client.get("/api/tests?sort=proven")).json()
    assert proven["items"][0]["confirmed_flake_count"] == 2
    seen = (await client.get("/api/tests?sort=last_seen")).json()
    assert [t["name"] for t in seen["items"]] == ["renders", "test_suspect", "test_flaky", "test_flaky"]
    p2 = (await client.get("/api/tests?page=2&page_size=3")).json()
    assert p2["total"] == 4 and len(p2["items"]) == 1
    assert (await client.get("/api/tests?page_size=101")).status_code == 422
    assert (await client.get("/api/tests?sort=name")).status_code == 422


async def test_file_filter_is_literal_substring(client, db):
    await _seed(db)
    page = (await client.get("/api/tests?file=app.test")).json()
    assert [t["name"] for t in page["items"]] == ["renders"]
    assert (await client.get("/api/tests?file=%25")).json()["total"] == 0  # "%" is literal


async def test_summary_scoped(client, db):
    await _seed(db)
    everything = (await client.get("/api/summary")).json()
    assert everything == {
        "total_tests": 5, "flaky_tests": 2, "suspect_tests": 2,
        "confirmed_flaky_tests": 1, "total_runs": 1, "total_executions": 1,
        "flake_threshold": 0.3,
    }
    one = (await client.get("/api/summary?repo=andrewthetechie/fantasy")).json()
    assert (one["total_tests"], one["flaky_tests"], one["total_runs"]) == (1, 1, 0)
```

## Dependencies
- Blocked by: 01 — Postgres + async foundation; 03 — Queued ingest (for `app/identity.py` normalizers and `schemas.py`)
- Why blocked: the schema and fixtures come from 01. The name normalizers and the existing `schemas.py` come from 03.
- Blocks: 09 (reuses `queries.py`, `TestOut`, `tests_select` and `to_test_out`), 10 (MCP), 11 (UI leaderboard)

## Labels
`feature`, `backend`, `api`, `priority:high`

## Estimate
Medium

## Risk
2 - Read-only. The main risk is the API shape, which the frontend (11) and MCP (10) depend on.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q   # expect: 75 passed
```
