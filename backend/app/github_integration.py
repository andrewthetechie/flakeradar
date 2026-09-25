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
from .processing import CHUNK, ProcessOutcome

logger = logging.getLogger("flakeradar.github")

LABEL = "flakeradar"
API_BASE = "https://api.github.com"


def configured() -> bool:
    return bool(get_settings().github_token)


async def _issue_body(db: AsyncSession, tc: TestCase, repo: str, project: str, root: str) -> str:
    recent = (
        await db.execute(
            select(TestExecution, TestRun)
            .join(TestRun, TestExecution.test_run_id == TestRun.id)
            .where(TestExecution.test_case_id == tc.id)
            .order_by(TestExecution.id.desc())
            .limit(10)
        )
    ).all()
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
            f"| {execution.status} | `{run.commit_sha[:10]}` | {run.branch} | {execution.created_at:%Y-%m-%d %H:%M} |"
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
    candidates = []
    # Chunked IN lists: a Report can touch more Tests than asyncpg can bind.
    for i in range(0, len(test_case_ids), CHUNK):
        candidates += (
            await db.execute(
                select(TestCase, Repo.name, Project.name, Project.root)
                .join(Project, TestCase.project_id == Project.id)
                .join(Repo, Project.repo_id == Repo.id)
                .where(
                    TestCase.id.in_(test_case_ids[i : i + CHUNK]),
                    TestCase.flakiness_score >= s.flake_threshold,
                    TestCase.github_issue_number.is_(None),
                )
                .order_by(TestCase.id)
            )
        ).all()
    if not candidates:
        return

    headers = {
        "Authorization": f"Bearer {s.github_token}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    try:
        async with httpx.AsyncClient(base_url=API_BASE, headers=headers, timeout=15, transport=transport) as client:
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
                    logger.info("Filed issue %s#%s for test %s", repo, tc.github_issue_number, tc.id)
                elif resp.status_code in (403, 429):
                    logger.warning(
                        "GitHub rate limit / forbidden (remaining=%s); stopping batch",
                        resp.headers.get("x-ratelimit-remaining"),
                    )
                    return
                else:
                    logger.warning(
                        "GitHub issue creation failed for test %s in %s: %s %s",
                        tc.id,
                        repo,
                        resp.status_code,
                        resp.text[:300],
                    )
    except httpx.HTTPError as exc:
        logger.warning("GitHub unreachable, skipping issue filing: %s", exc)


async def on_report_processed(session_factory: async_sessionmaker[AsyncSession], outcome: ProcessOutcome) -> None:
    """ReportWorker hook: file issues for the Tests a processed Report touched."""
    if outcome.status != REPORT_PROCESSED or not configured():
        return
    async with session_factory() as db:
        await file_issues_for(db, outcome.touched_test_ids)
