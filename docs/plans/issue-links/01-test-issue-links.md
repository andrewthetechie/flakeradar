# 01 — Issue links for Tests: the service module and `PUT`/`DELETE /api/tests/{id}/issue`

## Tracer-Bullet Outcome
A caller with the API key can link a GitHub issue to a Test with `PUT /api/tests/{test_id}/issue` and remove the link with `DELETE`. The dashboard and `/api/tests/{id}/history` then show `github_issue_number` and `github_issue_url`. A different issue already linked gives `409` unless `?replace=true`. A `url` for another Repo gives `422`. The new module `backend/app/issue_links.py` holds every rule, ready for Jobs (task 02) and MCP (task 04).

## User Story
As an external filer (fabro's scout, a triage bot, a person), I want to record the issue I filed for a flaky Test, so that FlakeRadar links it and does not file a duplicate.

## Description
New service module, one request schema, two routes in `routers/tests.py`, and a new API test file. No migration, no change to `queries.py`, the filer or the frontend.

## Context Pack
- Source decisions: `00-shared-context.md`, "Storage", "REST: `PUT …`", "REST: `DELETE …`" and "The service module". ADR `docs/adr/0008-issue-links-from-other-filers.md`.
- Repo facts:
  - `queries.set_quarantine` (in `backend/app/queries.py`) is the write to copy:
    ```python
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
    ```
    The session factory has `expire_on_commit=False`, so the ORM object is readable after `commit()`.
  - `queries.issue_url(repo, number)` returns `f"https://github.com/{repo}/issues/{number}"`. Repo names are stored lowercase.
  - `routers/tests.py`, the quarantine route to sit next to:
    ```python
    @router.post("/api/tests/{test_id}/quarantine", response_model=schemas.TestOut)
    async def set_quarantine(
        test_id: int,
        body: schemas.QuarantineIn,
        db: AsyncSession = Depends(get_db),
    ):
    ```
    The file imports `from .. import queries, schemas` and `from ..auth import require_token`. `GET /api/quarantine` uses `dependencies=[Depends(require_token)]`.
  - The `CiRunNotFound` pattern (a service exception whose `str()` is the `detail`, mapped to `HTTPException` in the router) is in `queries.py` and `routers/ci_runs.py`.
  - `GET /api/tests/{id}/history` returns `HistoryOut`, whose `test` field is a `TestOut`.
- Non-goals: Jobs (task 02); MCP (task 04); README (task 05); storing the URL; any change to `github_integration.py`.

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `feat/issue-links` (create it from `main` in this task)

## Implementation Contract
- Expected files: `backend/app/issue_links.py` (new), `backend/app/schemas.py`, `backend/app/routers/tests.py`, `backend/tests/test_issue_links_api.py` (new).
- Interfaces and names:
  - `schemas.py`: new section at the end of the file:
    ```python
    # --- Issue links (issue #6) ---

    MAX_ISSUE_NUMBER = 2_147_483_647  # github_issue_number is a PostgreSQL INTEGER


    class IssueLinkIn(BaseModel):
        number: int = Field(ge=1, le=MAX_ISSUE_NUMBER)
        url: str | None = Field(default=None, max_length=2048)  # optional; must be this issue in the entity's own Repo
    ```
  - `issue_links.py`, complete except for the Job functions (task 02 adds `link_job_issue` / `unlink_job_issue` in the same file):
    ```python
    """Issue links: record a GitHub issue filed elsewhere on a Test or Job (issue #6).

    FlakeRadar's own filer (github_integration) sets github_issue_number when it
    files an issue. These functions let any other filer set or clear it, through
    REST and MCP. A linked entity is then treated exactly like a filed one: the
    filer skips it, and sync_closed_issues clears it when the issue closes. The
    issue must be in the entity's own Repo, because the sync looks it up there
    (ADR 0008).
    """

    import logging

    from sqlalchemy.ext.asyncio import AsyncSession

    from . import schemas
    from .models import TestCase
    from .queries import issue_url, tests_select, to_test_out
    from .schemas import MAX_ISSUE_NUMBER

    logger = logging.getLogger("flakeradar.issues")


    class IssueLinkError(Exception):
        """A request the Issue link rules refuse; str(exc) is the error detail."""

        status_code = 422


    class IssueLinkTargetNotFound(IssueLinkError):
        status_code = 404


    class IssueAlreadyLinked(IssueLinkError):
        status_code = 409


    class IssueNotLinkable(IssueLinkError):
        status_code = 422


    def _check_link(kind: str, current: int | None, number: int, *, repo: str, url: str | None, replace: bool) -> None:
        """The number, url and conflict rules of a link; `kind` is "Test" or "Job"."""
        if not 1 <= number <= MAX_ISSUE_NUMBER:
            raise IssueNotLinkable(f"number must be between 1 and {MAX_ISSUE_NUMBER}")
        expected = issue_url(repo, number)
        if url is not None and url.strip().rstrip("/").lower() != expected:
            raise IssueNotLinkable(f"url must be {expected}: the issue must be in the {kind}'s own Repo")
        if current is not None and current != number and not replace:
            raise IssueAlreadyLinked(f"{kind} already links issue #{current}; pass replace=true to replace it")


    async def link_test_issue(
        db: AsyncSession, test_id: int, number: int, *, url: str | None = None, replace: bool = False, threshold: float
    ) -> schemas.TestOut: ...


    async def unlink_test_issue(db: AsyncSession, test_id: int, *, threshold: float) -> schemas.TestOut: ...
    ```
    Leave the `{kind}'s` f-string as written: it yields `Test's` / `Job's`.
  - `routers/tests.py`: change the import to `from .. import issue_links, queries, schemas`, and add two routes right after `set_quarantine`:
    ```python
    @router.put("/api/tests/{test_id}/issue", response_model=schemas.TestOut, dependencies=[Depends(require_token)])
    async def link_issue(
        test_id: int,
        body: schemas.IssueLinkIn,
        replace: bool = Query(default=False),
        db: AsyncSession = Depends(get_db),
    ):
        """Link a GitHub issue filed elsewhere; FlakeRadar then files none for this Test."""


    @router.delete("/api/tests/{test_id}/issue", response_model=schemas.TestOut, dependencies=[Depends(require_token)])
    async def unlink_issue(test_id: int, db: AsyncSession = Depends(get_db)):
        """Remove the Test's Issue link."""
    ```
    Both call the service with `threshold=get_settings().flake_threshold` and map errors with
    `except issue_links.IssueLinkError as exc: raise HTTPException(status_code=exc.status_code, detail=str(exc)) from exc`.
- Behavior rules:
  1. `link_test_issue`:
     - `row = (await db.execute(tests_select().where(TestCase.id == test_id).with_for_update(of=TestCase))).first()`.
     - `None` → `raise IssueLinkTargetNotFound("Test not found")`.
     - `tc, project, repo = row`, then `_check_link("Test", tc.github_issue_number, number, repo=repo, url=url, replace=replace)`.
     - `previous = tc.github_issue_number`, `tc.github_issue_number = number`, `await db.commit()`.
     - `logger.info("Linked issue %s#%s to test %s (was %s)", repo, number, tc.id, previous)`.
     - `return to_test_out(tc, project, repo, threshold)`.
  2. `unlink_test_issue`: same load (with the lock), same `404`. `previous = tc.github_issue_number`, set it to `None`, commit. When `previous is not None`, `logger.info("Unlinked issue %s#%s from test %s", repo, previous, tc.id)`. Return `to_test_out(...)`.
  3. A refused request does not commit. The session closes at the end of the request, which rolls back and releases the row lock.
- Error and security rules: both routes need `X-API-Key` (`401` from `require_token`). Messages are the exact strings in `00-shared-context.md`. The `422` for a wrong `url` names the expected URL, which only holds the Test's own Repo name and the caller's number.

## Steps
1. `git switch -c feat/issue-links main`.
2. Write the tests below (red).
3. Add the schema, the service module and the routes.
4. Run the Validator.

## Acceptance Criteria
- [ ] Every test below passes.
- [ ] `/docs` (OpenAPI) lists `PUT` and `DELETE /api/tests/{test_id}/issue`, with the `IssueLinkIn` body.
- [ ] No migration file is added, and `test_db.py` is unchanged and green.

## Test Expectations
New file `backend/tests/test_issue_links_api.py`, module docstring `"""Issue links: link a GitHub issue filed elsewhere to a Test or Job (issue #6)."""`. Imports: `from app import issue_links`, `from tests.conftest import AUTH`, and the factories. Seed with `proj = await make_project(db, "acme/app", "backend")` and `tc = await make_test_case(db, proj, name="t1", **fields)`, then `await db.commit()`. After a request, check the database with `await db.refresh(tc)`.
1. `test_link_test_issue`: `PUT /api/tests/{tc.id}/issue` with `json={"number": 123}` and `headers=AUTH` → `200`, `body["id"] == tc.id`, `body["github_issue_number"] == 123`, `body["github_issue_url"] == "https://github.com/acme/app/issues/123"`. The database has `123`. `GET /api/tests/{tc.id}/history` → `json()["test"]["github_issue_number"] == 123`.
2. `test_issue_link_needs_the_token`: `PUT` with a valid body and no headers → `401`. With `{"X-API-Key": "wrong"}` → `401`. `DELETE` with no headers → `401`. The database still has `None`.
3. `test_unknown_test_is_404`: `PUT /api/tests/99999/issue` (valid body, `AUTH`) → `404`, `detail == "Test not found"`. `DELETE` the same path → `404`, same detail.
4. `test_same_number_again_is_a_no_op`: Test with `github_issue_number=5`. `PUT {"number": 5}` → `200`, `5`.
5. `test_different_number_is_409_unless_replace`: Test with `github_issue_number=5`. `PUT {"number": 6}` → `409`, `detail == "Test already links issue #5; pass replace=true to replace it"`, the database still has `5`. `PUT /api/tests/{id}/issue?replace=true` with `{"number": 6}` → `200`, `6`.
6. `test_url_must_be_the_issue_in_the_tests_repo`:
   - `{"number": 7, "url": "https://github.com/ACME/App/issues/7/"}` → `200` (case and a trailing slash are tolerated).
   - `?replace=true` with `{"number": 8, "url": "https://github.com/other/repo/issues/8"}` → `422`, `detail == "url must be https://github.com/acme/app/issues/8: the issue must be in the Test's own Repo"`. The database still has `7`.
   - `?replace=true` with `{"number": 8, "url": "https://github.com/acme/app/issues/9"}` → `422`.
7. `test_invalid_body_is_422`: each of `{"number": 0}`, `{"number": -1}`, `{"number": 2147483648}`, `{"number": "abc"}`, `{}`, `{"number": 1, "url": "x" * 2049}` → `422`.
8. `test_unlink_test_issue`: Test with `github_issue_number=5`. `DELETE` → `200`, `github_issue_number is None`, `github_issue_url is None`, the database has `None`. `DELETE` again → `200`, still `None`.
9. `test_service_rules` (calls the service directly, no HTTP):
   - `await issue_links.link_test_issue(db, tc.id, 0, threshold=0.3)` raises `issue_links.IssueNotLinkable`, `str(exc) == "number must be between 1 and 2147483647"`, `exc.status_code == 422` (use `pytest.raises(...) as info`). Then `await db.rollback()`: the refused call still holds the `FOR UPDATE` row lock in the open transaction, and a later request from another session on that row would wait for it.
   - `IssueLinkTargetNotFound.status_code == 404` and `IssueAlreadyLinked.status_code == 409`.
   - `await issue_links.link_test_issue(db, tc.id, 11, threshold=0.3)` returns a `TestOut` with `github_issue_number == 11`.

## Validator
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```

## Dependencies
- Blocked by: None.
- Blocks: 02, 03, 04, 05.

## Labels
`feature`, `backend`, `priority:high`
