# 09 — Test detail, permalink and quarantine API

## Tracer-Bullet Outcome
`GET /api/tests/{id}/history` returns everything needed to investigate one Test:
- the Test with its `repo`, `project` and `tier`;
- its **Location**: a Repo-relative `path` and `line`, plus a **GitHub permalink** pinned to the last failing commit;
- `last_failing_sha` and `last_failing_branch`;
- recent Executions, newest first, each with its Failure message **and Failure details**.

The quarantine toggle works on the new model. Test runners fetch `GET /api/quarantine?repo=…&project=…` scoped to one Repo + Project.

## User Story
As a developer (or an agent) looking at a flaky test, I want a single call that tells me where the test lives, links to the code as it was when it failed, and shows the full traceback, so that I can start fixing it right away.

## Description
Extend `backend/app/queries.py` with `repo_path`, `permalink`, `get_test`, `set_quarantine` and `quarantine_list`. Extend `backend/app/schemas.py` with the detail and quarantine DTOs. Add three routes to `backend/app/routers/tests.py`. All the code below is verified.

## Context Pack
- Source decisions (`00-shared-context.md`):
  - The permalink is `https://github.com/{repo}/blob/{last_failing_sha}/{path}#L{line}`, where `path` is `root/file` (or `file` if the root is empty). `#L` is added only when `line >= 1`. There is no URL without a failing Execution.
  - The quarantine list is keyed by `repo` (required) and `project` (default `default`), and needs the token. The toggle stays open.
  - The history keeps its path, `/api/tests/{id}/history`.
  - pytest reports 0-based lines, so `line=0` gives no `#L` anchor.
- Repo facts:
  - From task 08 (`app/queries.py`): `tests_select() -> Select` (rows are `(TestCase, project_name, repo_name)`), `to_test_out(tc, project, repo, threshold) -> schemas.TestOut`, and `schemas.TestOut`.
  - From task 03: `app/identity.py` provides `DEFAULT_PROJECT = "default"`, `normalize_repo` and `normalize_project`. From task 01: `app/auth.py` provides `require_token`.
  - Pre-fork behavior to keep:
    - The quarantine toggle sets `quarantined_at = utcnow()` when turning on and `None` when turning off, and a missing Test → 404 `"Test not found"`.
    - The history limit is 1–500.
    - The runner list is ordered by name. It previously returned `suite, classname, name, fingerprint, quarantined_at`; it now also returns `file` and `line`.
  - `Select.add_columns(Project.root)` extends `tests_select()` rows to `(TestCase, project, repo, root)` (verified).
- Non-goals: MCP (10); UI (11–13); editing Locations by hand; authenticating the toggle (the dashboard is internal only).

## Delivery Strategy
- Shape: Wide refactor: Migrate (integration branch).
- Valid-state scope: Named integration branch `feat/repo-split`.

## Implementation Contract
- Expected files:
  - Edit `backend/app/schemas.py`, `backend/app/queries.py` and `backend/app/routers/tests.py`.
  - Create `backend/tests/test_test_detail_api.py`.
- Append to `backend/app/schemas.py`:
```python
# --- Test detail and quarantine (task 09) -------------------------------

class ExecutionOut(BaseModel):
    id: int
    status: str
    duration: float
    message: str   # Failure message
    details: str   # Failure details (traceback + captured output)
    created_at: datetime
    commit_sha: str
    branch: str
    ci_run_id: str


class LocationOut(BaseModel):
    path: str             # file joined onto the Project root, repo-relative
    line: int | None
    url: str | None       # GitHub permalink at the last failing SHA, when one exists


class HistoryOut(BaseModel):
    test: TestOut
    location: LocationOut | None       # None when the runner never reported a file
    last_failing_sha: str | None
    last_failing_branch: str | None
    executions: list[ExecutionOut]     # newest first


class QuarantineIn(BaseModel):
    quarantined: bool


class QuarantineItem(BaseModel):
    suite: str
    classname: str
    name: str
    fingerprint: str
    file: str | None
    line: int | None
    quarantined_at: datetime | None
```
- `backend/app/queries.py`: change the models import to `from .models import Project, Repo, TestCase, TestExecution, TestRun, utcnow`, then append:
```python
# --- Test detail and quarantine (task 09) -------------------------------

FAILING = ("failed", "error")


def repo_path(root: str, file: str) -> str:
    """A reported file path made relative to the Repo root."""
    return f"{root}/{file}" if root else file


def permalink(repo: str, sha: str, root: str, file: str, line: int | None) -> str:
    url = f"https://github.com/{repo}/blob/{sha}/{repo_path(root, file)}"
    return f"{url}#L{line}" if line is not None and line >= 1 else url


async def get_test(
    db: AsyncSession, test_id: int, *, threshold: float, executions_limit: int = 60
) -> schemas.HistoryOut | None:
    row = (await db.execute(
        tests_select().add_columns(Project.root).where(TestCase.id == test_id)
    )).first()
    if row is None:
        return None
    tc, project, repo, root = row

    rows = (await db.execute(
        select(TestExecution, TestRun)
        .join(TestRun, TestExecution.test_run_id == TestRun.id)
        .where(TestExecution.test_case_id == test_id)
        .order_by(TestExecution.id.desc())
        .limit(executions_limit)
    )).all()
    executions = [
        schemas.ExecutionOut(
            id=e.id, status=e.status, duration=e.duration, message=e.message,
            details=e.details, created_at=e.created_at, commit_sha=r.commit_sha,
            branch=r.branch, ci_run_id=r.ci_run_id,
        )
        for e, r in rows
    ]

    failing = (await db.execute(
        select(TestRun.commit_sha, TestRun.branch)
        .join(TestExecution, TestExecution.test_run_id == TestRun.id)
        .where(TestExecution.test_case_id == test_id, TestExecution.status.in_(FAILING))
        .order_by(TestExecution.id.desc())
        .limit(1)
    )).first()
    last_sha, last_branch = failing if failing is not None else (None, None)

    location = None
    if tc.file:
        location = schemas.LocationOut(
            path=repo_path(root, tc.file), line=tc.line,
            url=permalink(repo, last_sha, root, tc.file, tc.line) if last_sha else None,
        )
    return schemas.HistoryOut(
        test=to_test_out(tc, project, repo, threshold), location=location,
        last_failing_sha=last_sha, last_failing_branch=last_branch,
        executions=executions,
    )


async def set_quarantine(
    db: AsyncSession, test_id: int, quarantined: bool, *, threshold: float
) -> schemas.TestOut | None:
    row = (await db.execute(tests_select().where(TestCase.id == test_id))).first()
    if row is None:
        return None
    tc, project, repo = row
    tc.quarantined = quarantined
    tc.quarantined_at = utcnow() if quarantined else None
    await db.commit()
    return to_test_out(tc, project, repo, threshold)


async def quarantine_list(
    db: AsyncSession, repo: str, project: str
) -> list[schemas.QuarantineItem]:
    rows = (await db.execute(
        select(TestCase)
        .join(Project, TestCase.project_id == Project.id)
        .join(Repo, Project.repo_id == Repo.id)
        .where(Repo.name == repo, Project.name == project, TestCase.quarantined.is_(True))
        .order_by(TestCase.name, TestCase.id)
    )).scalars().all()
    return [
        schemas.QuarantineItem(
            suite=tc.suite, classname=tc.classname, name=tc.name,
            fingerprint=tc.fingerprint, file=tc.file, line=tc.line,
            quarantined_at=tc.quarantined_at,
        )
        for tc in rows
    ]
```
- `backend/app/routers/tests.py`:
  - Change the module docstring to `"""Dashboard API: Repos, leaderboard, summary, Test detail and quarantine."""`.
  - Add `from ..auth import require_token`.
  - Change the identity import to `from ..identity import DEFAULT_PROJECT, normalize_project, normalize_repo`.
  - Append:
```python
@router.get("/api/tests/{test_id}/history", response_model=schemas.HistoryOut)
async def test_history(
    test_id: int,
    limit: int = Query(default=60, ge=1, le=500),
    db: AsyncSession = Depends(get_db),
):
    history = await queries.get_test(
        db, test_id, threshold=get_settings().flake_threshold, executions_limit=limit
    )
    if history is None:
        raise HTTPException(status_code=404, detail="Test not found")
    return history


@router.post("/api/tests/{test_id}/quarantine", response_model=schemas.TestOut)
async def set_quarantine(
    test_id: int,
    body: schemas.QuarantineIn,
    db: AsyncSession = Depends(get_db),
):
    result = await queries.set_quarantine(
        db, test_id, body.quarantined, threshold=get_settings().flake_threshold
    )
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
```
- Behavior rules:
  - `location` is `None` when `file` is unknown.
  - `url` is `None` when the Test never failed. `path` and `line` are still present.
  - The executions are the newest `limit` (default 60).
  - An invalid `repo` or `project` on `/api/quarantine` → 422 with the normalizer's message.
- Error and security rules: `/api/quarantine` needs `X-API-Key`. The toggle and history are open.

## Acceptance Criteria
- [ ] For a Test in `andrewthetechie/writers-app` (project `frontend`, root `frontend`, file `src/app.test.ts`, line 12, last failure on `aaa111`), `location.url == "https://github.com/andrewthetechie/writers-app/blob/aaa111/frontend/src/app.test.ts#L12"`.
- [ ] With root `""` and line `0`, the URL has no `#L` and no root prefix.
- [ ] No file → `location is None`. Never failed → `location.url is None` and `last_failing_sha is None`.
- [ ] Failing Executions include `details` verbatim.
- [ ] The quarantine list for `(writers-app, frontend)` contains only that project's quarantined Test, even though `backend` has a quarantined Test with the same name. Without a token → 401. `project` without `repo` → 422.

## Test Expectations
- Framework: pytest + pytest-asyncio with the `client` and `db` fixtures.
- `backend/tests/test_test_detail_api.py` (verified, 6 passing):
```python
"""Test detail (history, Location, permalink) and quarantine."""
from tests.conftest import AUTH
from tests.factories import make_execution, make_project, make_run, make_test_case


async def _seed(db, *, file="src/app.test.ts", line=12, root="frontend"):
    proj = await make_project(db, "andrewthetechie/writers-app", "frontend", root=root)
    tc = await make_test_case(db, proj, name="renders", file=file, line=line,
                              flakiness_score=0.6, confirmed_flake_count=1)
    r1 = await make_run(db, proj, commit_sha="aaa111", branch="main", ci_run_id="1")
    await make_execution(db, tc, r1, status="failed", message="expected 3",
                         details="Traceback\n  at src/app.test.ts:14")
    r2 = await make_run(db, proj, commit_sha="bbb222", branch="feat", ci_run_id="2")
    await make_execution(db, tc, r2, status="passed")
    await db.commit()
    return tc


async def test_history_includes_location_permalink_and_details(client, db):
    tc = await _seed(db)
    body = (await client.get(f"/api/tests/{tc.id}/history")).json()
    assert body["test"]["repo"] == "andrewthetechie/writers-app"
    assert body["test"]["project"] == "frontend" and body["test"]["tier"] == "flaky"
    assert body["location"] == {
        "path": "frontend/src/app.test.ts", "line": 12,
        "url": "https://github.com/andrewthetechie/writers-app/blob/aaa111/frontend/src/app.test.ts#L12",
    }
    assert (body["last_failing_sha"], body["last_failing_branch"]) == ("aaa111", "main")
    assert [e["status"] for e in body["executions"]] == ["passed", "failed"]  # newest first
    assert body["executions"][1]["details"] == "Traceback\n  at src/app.test.ts:14"
    assert body["executions"][1]["message"] == "expected 3"


async def test_history_limit_and_404(client, db):
    tc = await _seed(db)
    body = (await client.get(f"/api/tests/{tc.id}/history?limit=1")).json()
    assert len(body["executions"]) == 1
    assert (await client.get("/api/tests/9999/history")).status_code == 404
    assert (await client.get(f"/api/tests/{tc.id}/history?limit=501")).status_code == 422


async def test_location_edge_cases(client, db):
    no_root_line0 = await _seed(db, root="", line=0)
    body = (await client.get(f"/api/tests/{no_root_line0.id}/history")).json()
    assert body["location"]["url"] == \
        "https://github.com/andrewthetechie/writers-app/blob/aaa111/src/app.test.ts"


async def test_no_file_means_no_location(client, db):
    tc = await _seed(db, file=None, line=None)
    body = (await client.get(f"/api/tests/{tc.id}/history")).json()
    assert body["location"] is None and body["last_failing_sha"] == "aaa111"


async def test_never_failed_has_path_but_no_url(client, db):
    proj = await make_project(db, "acme/app")
    tc = await make_test_case(db, proj, file="tests/test_a.py", line=3)
    run = await make_run(db, proj)
    await make_execution(db, tc, run, status="passed")
    await db.commit()
    body = (await client.get(f"/api/tests/{tc.id}/history")).json()
    assert body["location"] == {"path": "tests/test_a.py", "line": 3, "url": None}
    assert body["last_failing_sha"] is None


async def test_quarantine_toggle_and_scoped_list(client, db):
    tc = await _seed(db)
    other_proj = await make_project(db, "andrewthetechie/writers-app", "backend")
    await make_test_case(db, other_proj, name="renders", quarantined=True)
    await db.commit()

    on = await client.post(f"/api/tests/{tc.id}/quarantine", json={"quarantined": True})
    assert on.status_code == 200
    assert on.json()["quarantined"] is True and on.json()["quarantined_at"] is not None
    assert on.json()["repo"] == "andrewthetechie/writers-app"

    url = "/api/quarantine?repo=andrewthetechie/writers-app&project=frontend"
    assert (await client.get(url)).status_code == 401
    items = (await client.get(url, headers=AUTH)).json()
    assert [(i["name"], i["file"], i["line"]) for i in items] == \
        [("renders", "src/app.test.ts", 12)]
    default = (await client.get("/api/quarantine?repo=andrewthetechie/writers-app",
                                headers=AUTH)).json()
    assert default == []  # project defaults to "default", which has nothing
    assert (await client.get("/api/quarantine?project=frontend", headers=AUTH)).status_code == 422

    off = await client.post(f"/api/tests/{tc.id}/quarantine", json={"quarantined": False})
    assert off.json()["quarantined"] is False and off.json()["quarantined_at"] is None
    assert (await client.post("/api/tests/9999/quarantine",
                              json={"quarantined": True})).status_code == 404
```

## Dependencies
- Blocked by: 08 — Repos, leaderboard and summary read API
- Why blocked: reuses `tests_select`, `to_test_out`, `TestOut` and the router module that 08 creates.
- Blocks: 10 (MCP `get_test` wraps `queries.get_test`), 12 (the UI slide-over uses `HistoryOut`)

## Labels
`feature`, `backend`, `api`, `priority:high`

## Estimate
Medium

## Risk
2 - Read-mostly. The quarantine list is a CI-facing contract (`repo` is now required).

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q   # expect: 81 passed
```
