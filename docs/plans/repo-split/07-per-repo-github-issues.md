# 07 — Per-repo GitHub issues, async

## Tracer-Bullet Outcome
When a processed Report pushes a Test over the flake threshold, FlakeRadar opens a GitHub issue **in that Test's own Repo** (e.g. `andrewthetechie/writers-app`). The issue carries the repo/project, the Location, the score, the proven-flake count, the last 10 Executions and a sample of the Failure details. There is one issue per Test, ever. A single `FLAKERADAR_GITHUB_TOKEN` configures it; `FLAKERADAR_GITHUB_REPO` is gone.

## User Story
As a developer on one of several repos, I want flaky-test issues filed in my repo, not in a single global one, so that they land with the code they are about.

## Description
Rewrite `backend/app/github_integration.py` for async SQLAlchemy and `httpx.AsyncClient`. It resolves each candidate Test's Repo through `Project → Repo` and posts to `/repos/{repo}/issues`. Add an `on_report_processed` hook, which `main.py` passes to `ReportWorker(on_processed=...)`, so filing runs after processing and never on the upload path. Remove the `github_repo` setting. Replace the old sync tests with the async tests below.

## Context Pack
- Source decisions: issues go in the Test's own Repo; one global token; `FLAKERADAR_GITHUB_REPO` removed (`00-shared-context.md` → GitHub issues).
- Repo facts:
  - The current file is sync (`httpx.Client`, `Session`, `db.execute(...).scalars()`) and posts to the global `s.github_repo`. Keep its behavior contract: no-op when unconfigured; skip Tests that already have `github_issue_number`; only Tests with `flakiness_score >= flake_threshold`; 201 → store `resp.json()["number"]` and commit; 403/429 → log `GitHub rate limit / forbidden (remaining=%s); stopping batch` and stop; other statuses → log and continue; `httpx.HTTPError` → log and return; title `[FlakeRadar] Flaky test: {classname}::{name}`; label `flakeradar`; headers `Authorization: Bearer <token>`, `Accept: application/vnd.github+json`, `X-GitHub-Api-Version: 2022-11-28`.
  - `ProcessOutcome` (task 04) has `status` and `touched_test_ids`, and `REPORT_PROCESSED = "processed"` (task 01).
  - `ReportWorker(..., on_processed: Callable[[ProcessOutcome], Awaitable[None]] | None)` from task 05. It logs and swallows exceptions from the hook.
- Verified external contracts: `httpx.AsyncClient(base_url=..., headers=..., timeout=15, transport=...)` accepts `transport=None` (the default network transport) or `httpx.MockTransport(handler)`, where `handler(request: httpx.Request) -> httpx.Response` (httpx 0.28.1; used by the tests below).
- Non-goals: closing issues automatically; per-repo tokens; filing on the upload path; a UI for issues beyond the existing `#number` badge.

## Delivery Strategy
- Shape: Wide refactor: Migrate (integration branch).
- Valid-state scope: Named integration branch `feat/repo-split`.

## Implementation Contract
- Expected files:
  - Replace `backend/app/github_integration.py`.
  - Create `backend/tests/test_github.py` (task 01 deleted the old one).
  - Edit `backend/app/config.py`, `backend/app/main.py` and `.env.example`.
- `backend/app/github_integration.py` (verified):
```python
"""GitHub issue automation, filed in each Test's own Repo.

When a Test's flakiness score crosses the threshold, file an issue in that
Test's Repo (``owner/name``) with the evidence. Runs after the processor
finishes a Report (never on the upload path), so a slow or unreachable
GitHub API never delays CI.

Behavior:
- No token configured            -> silent no-op (self-host without GitHub).
- Issue already filed for test   -> no-op (issue number stored on the row).
- 403/429 (rate limit/forbidden) -> stop this batch, log a warning.
- Any other failure (404: token cannot see that repo, network) -> log, go on.
Never raises.
"""
import logging

import httpx
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from .config import get_settings
from .models import REPORT_PROCESSED, Project, Repo, TestCase, TestExecution, TestRun
from .processing import ProcessOutcome

logger = logging.getLogger("flakeradar.github")

LABEL = "flakeradar"
API_BASE = "https://api.github.com"


def configured() -> bool:
    return bool(get_settings().github_token)


async def _issue_body(
    db: AsyncSession, tc: TestCase, repo: str, project: str, root: str
) -> str:
    recent = (await db.execute(
        select(TestExecution, TestRun)
        .join(TestRun, TestExecution.test_run_id == TestRun.id)
        .where(TestExecution.test_case_id == tc.id)
        .order_by(TestExecution.id.desc())
        .limit(10)
    )).all()
    if tc.file:
        path = f"{root}/{tc.file}" if root else tc.file
        location = f"`{path}`" + (f" line {tc.line}" if tc.line is not None else "")
    else:
        location = "(not reported by the test runner)"
    lines = [
        f"FlakeRadar detected a flaky test: `{tc.classname}::{tc.name}`",
        "",
        f"- **Repo / project:** {repo} / {project}",
        f"- **Location:** {location}",
        f"- **Flakiness score:** {tc.flakiness_score:.2f}",
        f"- **Proven flakes (same-commit fail + pass):** {tc.confirmed_flake_count}",
        f"- **Suite:** {tc.suite or '(none)'}",
        "",
        "### Last 10 executions",
        "",
        "| Status | Commit | Branch | When (UTC) |",
        "|---|---|---|---|",
    ]
    sample_failure = ""
    for execution, run in recent:
        lines.append(
            f"| {execution.status} | `{run.commit_sha[:10]}` | {run.branch} "
            f"| {execution.created_at:%Y-%m-%d %H:%M} |"
        )
        if not sample_failure and execution.status in ("failed", "error"):
            sample_failure = execution.details or execution.message
    if sample_failure:
        lines += ["", "### Sample failure", "", "```", sample_failure[:1500], "```"]
    lines += ["", f"_Fingerprint: `{tc.fingerprint}`_"]
    return "\n".join(lines)


async def file_issues_for(
    db: AsyncSession,
    test_case_ids: list[int],
    *,
    transport: httpx.AsyncBaseTransport | None = None,
) -> None:
    """File issues for newly-over-threshold Tests. Never raises.

    `transport` exists for tests (httpx.MockTransport); production passes None.
    """
    if not configured() or not test_case_ids:
        return
    s = get_settings()
    candidates = (await db.execute(
        select(TestCase, Repo.name, Project.name, Project.root)
        .join(Project, TestCase.project_id == Project.id)
        .join(Repo, Project.repo_id == Repo.id)
        .where(
            TestCase.id.in_(test_case_ids),
            TestCase.flakiness_score >= s.flake_threshold,
            TestCase.github_issue_number.is_(None),
        )
        .order_by(TestCase.id)
    )).all()
    if not candidates:
        return

    headers = {
        "Authorization": f"Bearer {s.github_token}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    try:
        async with httpx.AsyncClient(base_url=API_BASE, headers=headers, timeout=15,
                                     transport=transport) as client:
            for tc, repo, project, root in candidates:
                resp = await client.post(
                    f"/repos/{repo}/issues",
                    json={
                        "title": f"[FlakeRadar] Flaky test: {tc.classname}::{tc.name}",
                        "body": await _issue_body(db, tc, repo, project, root),
                        "labels": [LABEL],
                    },
                )
                if resp.status_code == 201:
                    tc.github_issue_number = resp.json()["number"]
                    await db.commit()
                    logger.info("Filed issue %s#%s for test %s",
                                repo, tc.github_issue_number, tc.id)
                elif resp.status_code in (403, 429):
                    logger.warning(
                        "GitHub rate limit / forbidden (remaining=%s); stopping batch",
                        resp.headers.get("x-ratelimit-remaining"),
                    )
                    return
                else:
                    logger.warning(
                        "GitHub issue creation failed for test %s in %s: %s %s",
                        tc.id, repo, resp.status_code, resp.text[:300],
                    )
    except httpx.HTTPError as exc:
        logger.warning("GitHub unreachable, skipping issue filing: %s", exc)


async def on_report_processed(
    session_factory: async_sessionmaker[AsyncSession], outcome: ProcessOutcome
) -> None:
    """ReportWorker hook: file issues for the Tests a processed Report touched."""
    if outcome.status != REPORT_PROCESSED or not configured():
        return
    async with session_factory() as db:
        await file_issues_for(db, outcome.touched_test_ids)
```
- `backend/app/config.py`: replace the GitHub block with
```python
    # GitHub integration. Leave the token empty to disable (graceful no-op).
    # Issues are filed in each Test's own Repo, so the token needs
    # Issues: write on every Repo you ingest.
    github_token: str = ""
```
  (That deletes `github_repo`. `extra="ignore"` means an old `FLAKERADAR_GITHUB_REPO` in someone's `.env` is ignored rather than causing a crash.)
- `.env.example`: replace the GitHub section with
```bash
# --- Optional: GitHub issue automation (leave blank to disable) ---
# Fine-grained PAT with "Issues: write" on every repo you ingest; issues are
# filed in each flaky test's own repo.
FLAKERADAR_GITHUB_TOKEN=
```
- `backend/app/main.py`: add `from . import github_integration` to the imports. The lifespan becomes:
```python
@asynccontextmanager
async def lifespan(app: FastAPI):
    settings = get_settings()
    assert_secure_token(settings.api_token)
    if settings.api_token == DEFAULT_INSECURE_TOKEN:
        logger.warning(
            "FLAKERADAR_API_TOKEN is the default 'changeme' — set a real token."
        )
    await run_migrations(engine)
    worker = ReportWorker(
        engine, SessionLocal,
        poll_seconds=settings.worker_poll_seconds,
        on_processed=lambda outcome: github_integration.on_report_processed(SessionLocal, outcome),
        prune=lambda: run_prune(SessionLocal),
        prune_interval_seconds=settings.prune_interval_seconds,
    )
    await worker.start()
    try:
        yield
    finally:
        await worker.stop()
        await engine.dispose()
```
- Behavior rules:
  - Candidates are ordered by `TestCase.id`.
  - The Location line in the issue body is `` `root/file` line N `` (or `` `file` `` when the root is empty; the ` line N` suffix only when `line` is not None). With no file, it reads `(not reported by the test runner)`.
  - The sample failure is the newest failing Execution's `details`, falling back to `message`, cut to 1,500 chars.
  - `on_report_processed` does nothing for failed outcomes or when unconfigured.
- Error and security rules: never log the token. Response bodies are logged only when truncated to 300 chars.

## Acceptance Criteria
- [ ] A Test in `andrewthetechie/writers-app` with score 0.6 gives exactly one POST to `/repos/andrewthetechie/writers-app/issues`, and `github_issue_number` is stored as 77.
- [ ] The issue body contains `andrewthetechie/writers-app / backend`, `` `frontend/src/a.test.ts` line 12 `` and the Failure details text.
- [ ] Tests with an existing issue, or scoring below the threshold, → no request.
- [ ] 403 → one request, then stop. 404 → both requests made, nothing stored.
- [ ] `httpx.ConnectError` → no exception.
- [ ] `Settings()` no longer has a `github_repo` attribute.

## Test Expectations
- Framework: pytest + pytest-asyncio against a real Postgres. GitHub is faked with `httpx.MockTransport`; no network access.
- `backend/tests/test_github.py` (verified, 6 passing):
```python
"""GitHub issues are filed in each Test's own Repo; failures never raise."""
import json

import httpx
import pytest

from app import github_integration
from app.config import Settings
from app.models import TestCase
from app.processing import ProcessOutcome
from tests.factories import make_execution, make_project, make_run, make_test_case


@pytest.fixture()
def gh_settings(monkeypatch):
    settings = Settings(github_token="tok", flake_threshold=0.3)
    monkeypatch.setattr("app.github_integration.get_settings", lambda: settings)
    return settings


async def _flaky_test(db, repo="acme/app", project="backend", name="t_flaky", **fields):
    proj = await make_project(db, repo, project, root=fields.pop("root", ""))
    tc = await make_test_case(db, proj, name=name, flakiness_score=0.6,
                              confirmed_flake_count=1, **fields)
    run = await make_run(db, proj, commit_sha="deadbeef")
    await make_execution(db, tc, run, status="failed", message="assert 1 == 2",
                         details="Traceback: boom")
    await make_execution(db, tc, run, status="passed")
    await db.commit()
    return tc


def _recorder(status=201, number=77, headers=None):
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        return httpx.Response(status, json={"number": number}, headers=headers or {})

    return calls, httpx.MockTransport(handler)


async def test_unconfigured_is_noop(db):
    tc = await _flaky_test(db)
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [tc.id], transport=transport)
    assert calls == []


async def test_files_issue_in_the_tests_own_repo(db, gh_settings):
    tc = await _flaky_test(db, repo="andrewthetechie/writers-app", file="src/a.test.ts",
                           line=12, root="frontend")
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [tc.id], transport=transport)

    (req,) = calls
    assert req.url.path == "/repos/andrewthetechie/writers-app/issues"
    assert req.headers["Authorization"] == "Bearer tok"
    body = json.loads(req.content)
    assert body["title"] == "[FlakeRadar] Flaky test: tests.test_mod::t_flaky"
    assert body["labels"] == ["flakeradar"]
    assert "andrewthetechie/writers-app / backend" in body["body"]
    assert "`frontend/src/a.test.ts` line 12" in body["body"]
    assert "Traceback: boom" in body["body"]
    await db.refresh(tc)
    assert tc.github_issue_number == 77


async def test_does_not_refile_or_file_below_threshold(db, gh_settings):
    filed = await _flaky_test(db, name="filed", github_issue_number=5)
    proj = await make_project(db, "acme/other")
    calm = await make_test_case(db, proj, name="calm", flakiness_score=0.1)
    await db.commit()
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [filed.id, calm.id], transport=transport)
    assert calls == []


async def test_rate_limit_stops_batch_and_404_continues(db, gh_settings):
    a = await _flaky_test(db, repo="acme/a")
    b = await _flaky_test(db, repo="acme/b")
    calls, transport = _recorder(status=403, headers={"x-ratelimit-remaining": "0"})
    await github_integration.file_issues_for(db, [a.id, b.id], transport=transport)
    assert len(calls) == 1

    calls, transport = _recorder(status=404)
    await github_integration.file_issues_for(db, [a.id, b.id], transport=transport)
    assert len(calls) == 2
    await db.refresh(a)
    assert a.github_issue_number is None


async def test_network_error_never_raises(db, gh_settings):
    tc = await _flaky_test(db)

    def boom(request):
        raise httpx.ConnectError("offline")

    await github_integration.file_issues_for(db, [tc.id], transport=httpx.MockTransport(boom))
    await db.refresh(tc)
    assert tc.github_issue_number is None


async def test_hook_ignores_failed_reports(session_factory, gh_settings, monkeypatch):
    called = []

    async def fake_file(db, ids, **kw):
        called.append(ids)

    monkeypatch.setattr(github_integration, "file_issues_for", fake_file)
    failed = ProcessOutcome(report_id=1, status="failed", run_id=None, counts=None,
                            touched_test_ids=[], error="x")
    done = ProcessOutcome(report_id=2, status="processed", run_id=1, counts={},
                          touched_test_ids=[3, 4], error=None)
    await github_integration.on_report_processed(session_factory, failed)
    await github_integration.on_report_processed(session_factory, done)
    assert called == [[3, 4]]
```

## Dependencies
- Blocked by: 05 — Background processor with leader lock
- Why blocked: the issues are filed from the worker's `on_processed` hook.
- Blocks: 14 (docs for the new GitHub setup)

## Labels
`feature`, `backend`, `integration`, `priority:medium`

## Estimate
Small

## Risk
2 - External side effects (issues in real repos). This is the same contract as before, with the repo now taken from the Test.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q   # expect: 69 passed
```
