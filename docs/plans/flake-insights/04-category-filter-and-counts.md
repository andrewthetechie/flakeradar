# 04 — Filter and count by Failure category (REST + MCP)

## Tracer-Bullet Outcome
`GET /api/tests?category=timing` lists only the Tests whose Failure category is `timing`. `GET /api/summary` returns `category_counts`, the number of flaky + suspect Tests per category. The MCP tool `top_flaky_tests` takes `category`, and the MCP instructions explain what it means. An agent can ask "show me the network flakes in `acme/app`" and get only those.

## User Story
As a maintainer (or an agent working for one), I want to list the flaky tests with one likely cause, so that I can fix a batch with one change.

## Description
Read side only. Add a filter argument to `queries.list_tests`, a grouped count to `queries.summary`, a query parameter to the REST route, and a parameter plus instructions to the MCP tool.

## Context Pack
- Source decisions: `00-shared-context.md`, "Failure categories". ADR 0006 ("The leaderboard, the API and the MCP server can filter and group by it").
- Repo facts:
  - Task 03 created `backend/app/classify.py` with:
    ```python
    FailureCategory = Literal["network", "environment", "timing", "assertion", "other"]
    CATEGORIES: tuple[FailureCategory, ...] = ("network", "environment", "timing", "assertion", "other")
    ```
    and the column `TestCase.failure_category: Mapped[str | None]` (NULL when the Test has no failure in its window).
  - Current `queries.list_tests` signature and filter block (`backend/app/queries.py`):
    ```python
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
    ```
  - Current `queries.summary` builds counts with a local helper:
    ```python
    async def summary(db: AsyncSession, scope: Scope, *, threshold: float) -> schemas.SummaryOut:
        def tests_count(*conditions) -> Select:
            return _scoped(
                select(func.count(TestCase.id))
                .join(Project, TestCase.project_id == Project.id)
                .join(Repo, Project.repo_id == Repo.id)
                .where(*conditions),
                scope,
            )
        ...
        return schemas.SummaryOut(
            total_tests=total, flaky_tests=flaky, suspect_tests=suspect, confirmed_flaky_tests=confirmed,
            total_runs=runs, total_executions=executions, flake_threshold=threshold,
        )
    ```
    `_scoped(stmt, scope)` adds `Repo.name == scope.repo` / `Project.name == scope.project` to a statement that already joins `Project` and `Repo`.
  - Current `schemas.SummaryOut`:
    ```python
    class SummaryOut(BaseModel):
        total_tests: int
        flaky_tests: int
        suspect_tests: int
        confirmed_flaky_tests: int
        total_runs: int
        total_executions: int
        flake_threshold: float
    ```
  - Current REST route (`backend/app/routers/tests.py`):
    ```python
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
            db, scope, threshold=get_settings().flake_threshold, include_stable=include_stable,
            sort=sort, page=page, page_size=page_size, file=file,
        )
    ```
    `queries.SortKey` is a `Literal`, and FastAPI already returns 422 for a value outside it. A `Literal`-typed query parameter behaves the same way.
  - Current MCP tool (`backend/app/mcp_server.py`):
    ```python
    @mcp.tool
    async def top_flaky_tests(
        repo: str,
        project: str | None = None,
        limit: int = 20,
        include_suspect: bool = True,
        file: str | None = None,
    ) -> list[dict[str, Any]]:
        """Worst tests first in a Repo (optionally one Project).

        include_suspect=False returns only 'flaky' tier tests. `file` keeps
        tests whose reported file contains that text (e.g. 'tests/test_cron.py').
        """
        _check_limit(limit)
        scope = _scope(repo, project)
        async with session_factory() as db:
            page = await queries.list_tests(
                db, scope, threshold=get_settings().flake_threshold,
                flaky_only=not include_suspect, page_size=limit, file=file,
            )
        return [t.model_dump(mode="json") for t in page.items]
    ```
    `ToolError` comes from `fastmcp.exceptions`. The module-level `INSTRUCTIONS` string's first paragraph ends with: `Location (file/line/GitHub permalink at the last failing commit) and the\nfailure traceback.`
  - Existing tests: `backend/tests/test_tests_api.py` (`_seed(db)` makes Tests with `make_test_case(db, project, name=..., flakiness_score=..., ...)`) and `backend/tests/test_mcp.py` (`mcp` fixture, `Client(mcp)`, `await c.call_tool("top_flaky_tests", {...})`). Copy how the existing tests read results.
- Non-goals: the UI (task 05); a category filter for `search_tests` or Jobs; changing sort keys.

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `feat/flake-insights` after this draft

## Implementation Contract
- Expected files: `backend/app/queries.py`, `backend/app/schemas.py`, `backend/app/routers/tests.py`, `backend/app/mcp_server.py`, `backend/tests/test_tests_api.py`, `backend/tests/test_mcp.py`.
- Interfaces and names:
  - `queries.list_tests` gains the keyword `category: FailureCategory | None = None` (import from `.classify`). When it is set: `stmt = stmt.where(TestCase.failure_category == category)`. It applies **together with** the tier filter (by default, stable Tests stay hidden).
  - `schemas.SummaryOut` gains `category_counts: dict[str, int]` (last field).
  - `queries.summary` computes
    ```python
    rows = await db.execute(
        _scoped(
            select(TestCase.failure_category, func.count(TestCase.id))
            .join(Project, TestCase.project_id == Project.id)
            .join(Repo, Project.repo_id == Repo.id)
            .where(TestCase.flakiness_score > 0, TestCase.failure_category.is_not(None))
            .group_by(TestCase.failure_category),
            scope,
        )
    )
    category_counts = {c: 0 for c in CATEGORIES} | {cat: n for cat, n in rows.all() if cat in CATEGORIES}
    ```
    Every one of the five keys is always present, in `CATEGORIES` order.
  - REST `GET /api/tests` gains `category: FailureCategory | None = Query(default=None)` and passes it through.
  - MCP `top_flaky_tests` gains `category: str | None = None`. When it is not `None` and not in `CATEGORIES`, raise `ToolError("category must be one of: network, environment, timing, assertion, other")`. Pass it to `list_tests`. Add to the docstring: ``"`category` keeps tests whose likely cause (Failure category) is one of network, environment, timing, assertion, other."``
  - `INSTRUCTIONS`: append this paragraph (keep the rest):
    ```
    Each test has a failure_category: the LIKELY cause of its recent failures
    (network, environment, timing, assertion or other), from rules over the
    failure messages. It is a hint, not a diagnosis; read the failure details
    before you conclude. Filter with top_flaky_tests(category=...).
    ```
- Verified external contracts: FastAPI validates `Literal` query parameters and returns 422 for other values. The existing `sort: queries.SortKey` parameter already relies on this.
- Behavior rules: `category` does not change `total` semantics. `total` counts the filtered set. Tests with `failure_category IS NULL` never match a category filter and are not counted.
- Error and security rules: REST gives a 422 for an unknown category (FastAPI default). MCP raises the `ToolError` text above.

## Acceptance Criteria
- [ ] With Tests A (score 0.8, `timing`), B (0.2, `network`), C (0.5, `timing`) and D (0.0, `timing`) in one Repo: `GET /api/tests?repo=…&category=timing` returns `[A, C]` with `total == 2`, and adding `include_stable=true` returns `[A, C, D]`.
- [ ] `GET /api/tests?category=bogus` returns 422.
- [ ] `GET /api/summary?repo=…` returns `category_counts == {"network": 1, "environment": 0, "timing": 2, "assertion": 0, "other": 0}` for that seed (D is stable, so it is not counted).
- [ ] MCP `top_flaky_tests(repo=…, category="network")` returns only B. `category="nope"` raises a ToolError whose message contains `category must be one of`.

## Test Expectations
- Framework: pytest-asyncio, the httpx `client` fixture, and the fastmcp `Client`. Run: `cd backend && .venv/bin/python -m pytest -q`.
- `tests/test_tests_api.py`: add `test_category_filter_and_counts`, which seeds A–D above with `make_test_case(db, proj, name="A", flakiness_score=0.8, failure_category="timing")` and so on, then commits. Assert the three REST results from the Acceptance Criteria.
- `tests/test_mcp.py`: add `test_top_flaky_tests_category`, with the same seed style (repo `acme/app`). Use the file's existing patterns: `names = [t["name"] for t in (await c.call_tool("top_flaky_tests", {"repo": "acme/app", "category": "network"})).data]` gives `["B"]`. For the bad value: `bad = await c.call_tool("top_flaky_tests", {"repo": "acme/app", "category": "nope"}, raise_on_error=False)`, then `assert bad.is_error and "category must be one of" in bad.content[0].text`.

## Dependencies
- Blocked by: 03.
- Why blocked: 03 creates `classify.CATEGORIES`, `FailureCategory` and `TestCase.failure_category`.
- Blocks: 05.

## Labels
`feature`, `backend`, `priority:medium`

## Estimate
Small

## Risk
1 - Read-only filters.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```
