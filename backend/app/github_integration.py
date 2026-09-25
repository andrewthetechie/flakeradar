"""GitHub issue automation, filed in each Test's own Repo.

When a Test crosses the *filing gate*, file an issue in that Test's Repo
(``owner/name``) with the evidence. Runs after the processor finishes a Report
(never on the upload path), so a slow or unreachable GitHub API never delays
CI.

Filing gate (all configured minimums must be met; 0 disables a signal):
  - ``github_issue_min_score``             — flakiness score (0..1)
  - ``github_issue_min_proven_flakes``     — same-commit fail+pass count
  - ``github_issue_min_failures``          — failures in the recent window
Deduplication works because a filed issue's number is stored on the Test; an
issue is only ever re-filed after GitHub reports it closed (``sync_closed_issues``
clears the stored number, and the next Report that touches the Test re-files it
if it still meets the gate).

Error handling: never raises.
- No token configured            -> silent no-op (self-host without GitHub).
- 403/429 (rate limit/forbidden) -> stop this batch, log a warning.
- Any other failure (404, network) -> log, go on.
"""

import logging

import httpx
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from .config import get_settings
from .models import REPORT_PROCESSED, Project, Repo, TestCase, TestExecution, TestRun
from .processing import CHUNK, ProcessOutcome
from .scoring import FAILING

logger = logging.getLogger("flakeradar.github")

API_BASE = "https://api.github.com"


def configured() -> bool:
    return bool(get_settings().github_token)


def _headers(token: str) -> dict[str, str]:
    return {
        "Authorization": f"Bearer {token}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
    }


def _repo_path(root: str, file: str) -> str:
    return f"{root}/{file}" if root else file


async def _issue_body(db: AsyncSession, tc: TestCase, repo: str, project: str, root: str) -> str:
    """Render a self-contained issue body an engineer/agent can act on."""
    s = get_settings()
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
        path = _repo_path(root, tc.file)
        location = f"`{path}`" + (f" line {tc.line}" if tc.line is not None else "")
    else:
        path = None
        location = "(not reported by the test runner)"
    last_fail_sha = last_fail_branch = None
    sample_failure = ""
    for execution, run in recent:
        if execution.status in FAILING:
            if last_fail_sha is None:
                last_fail_sha, last_fail_branch = run.commit_sha, run.branch
            if not sample_failure:
                sample_failure = execution.details or execution.message
    permalink = None
    if tc.file and last_fail_sha:
        permalink = f"https://github.com/{repo}/blob/{last_fail_sha}/{path}"
        if tc.line is not None and tc.line >= 1:
            permalink += f"#L{tc.line}"
    run_hint = "pytest"
    if path:
        run_hint += f" {path}" + (f"::{tc.name}" if tc.name else "")
    lines = [
        f"FlakeRadar detected a flaky test: `{tc.classname}::{tc.name}`",
        "",
        f"- **Repo / project:** `{repo}` / `{project}`",
        f"- **Location:** {location}",
    ]
    if permalink:
        lines.append(f"- **Permalink (last failing commit):** {permalink}")
    lines += [
        f"- **Flakiness score:** {tc.flakiness_score:.2f} (filed at ≥ {s.github_issue_min_score:.2f})",
        f"- **Proven flakes (same commit failed *and* passed):** {tc.confirmed_flake_count}",
        f"- **Suite / classname:** `{tc.suite or '(none)'}` / `{tc.classname}`",
    ]
    if last_fail_sha:
        lines.append(f"- **Last failing commit:** `{last_fail_sha}`" + (f" on branch `{last_fail_branch}`" if last_fail_branch else ""))
    lines += [
        "",
        "FlakeRadar flags this test because it reports nondeterministic results (a failure gives way to a",
        "pass on identical code). To reproduce it locally:",
        "",
        f"```\n{run_hint}\n```",
        "",
        "Then look in the traceback below for a shared resource the test depends on: timing, execution",
        "order, shared/global state, or the network. Re-run the test several times to confirm.",
        "",
        "### Last 10 executions",
        "",
        "| Status | Commit | Branch | When (UTC) |",
        "|---|---|---|---|",
    ]
    for execution, run in recent:
        lines.append(
            f"| {execution.status} | `{run.commit_sha[:10]}` | {run.branch} | {execution.created_at:%Y-%m-%d %H:%M} |"
        )
    if sample_failure:
        lines += ["", "### Sample failure", "", "```", sample_failure[:1500], "```"]
    lines += ["", f"_Fingerprint: `{tc.fingerprint}`_"]
    return "\n".join(lines)


async def _window_failures(db: AsyncSession, test_case_ids: list[int], window: int) -> dict[int, int]:
    """Count failed/error executions per Test over its most recent `window` runs."""
    rn = func.row_number().over(partition_by=TestExecution.test_case_id, order_by=TestExecution.id.desc()).label("rn")
    counts: dict[int, int] = {}
    for i in range(0, len(test_case_ids), CHUNK):
        chunk = test_case_ids[i : i + CHUNK]
        if not chunk:
            continue
        ranked = (
            select(TestExecution.test_case_id, TestExecution.status, rn)
            .where(TestExecution.test_case_id.in_(chunk))
            .subquery()
        )
        rows = await db.execute(select(ranked.c.test_case_id, ranked.c.status).where(ranked.c.rn <= window))
        for tid, status in rows.all():
            if status in FAILING:
                counts[tid] = counts.get(tid, 0) + 1
    return counts


async def _select_candidates(db: AsyncSession, test_case_ids: list[int]) -> list[tuple[TestCase, str, str, str]]:
    """Tests that meet the configured filing gate and still have no open issue."""
    s = get_settings()
    candidates: list[tuple[TestCase, str, str, str]] = []
    for i in range(0, len(test_case_ids), CHUNK):
        chunk = test_case_ids[i : i + CHUNK]
        if not chunk:
            continue
        candidates += (
            await db.execute(
                select(TestCase, Repo.name, Project.name, Project.root)
                .join(Project, TestCase.project_id == Project.id)
                .join(Repo, Project.repo_id == Repo.id)
                .where(
                    TestCase.id.in_(chunk),
                    TestCase.flakiness_score >= s.github_issue_min_score,
                    TestCase.confirmed_flake_count >= s.github_issue_min_proven_flakes,
                    TestCase.github_issue_number.is_(None),
                )
                .order_by(TestCase.id)
            )
        ).all()
    if s.github_issue_min_failures > 0:
        failures = await _window_failures(db, [r[0].id for r in candidates], s.score_window)
        candidates = [r for r in candidates if failures.get(r[0].id, 0) >= s.github_issue_min_failures]
    return candidates


async def file_issues_for(
    db: AsyncSession,
    test_case_ids: list[int],
    *,
    transport: httpx.AsyncBaseTransport | None = None,
) -> None:
    """File issues for Tests that now meet the filing gate. Never raises.

    `transport` exists for tests (httpx.MockTransport); production passes None.
    """
    if not configured() or not test_case_ids:
        return
    s = get_settings()
    candidates = await _select_candidates(db, test_case_ids)
    if not candidates:
        return
    try:
        async with httpx.AsyncClient(
            base_url=API_BASE, headers=_headers(s.github_token), timeout=15, transport=transport
        ) as client:
            for tc, repo, project, root in candidates:
                resp = await client.post(
                    f"/repos/{repo}/issues",
                    json={
                        "title": f"[FlakeRadar] Flaky test: {tc.classname}::{tc.name}",
                        "body": await _issue_body(db, tc, repo, project, root),
                        "labels": [s.github_issue_label],
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


async def sync_closed_issues(
    session_factory: async_sessionmaker[AsyncSession], *, transport: httpx.AsyncBaseTransport | None = None
) -> None:
    """Re-arm deduplication for Tests whose GitHub issue has been closed.

    A filed issue's number is stored on the Test and blocks re-filing. When
    GitHub reports that issue closed (or gone), clear the stored number so the
    next Report that touches the Test can re-file a fresh issue if it still
    meets the filing gate. Leader-only maintenance, batched, never raises.

    `transport` exists for tests (httpx.MockTransport); production passes None.
    """
    if not configured():
        return
    s = get_settings()
    async with session_factory() as db:
        rows = (
            await db.execute(
                select(TestCase, Repo.name)
                .join(Project, TestCase.project_id == Project.id)
                .join(Repo, Project.repo_id == Repo.id)
                .where(TestCase.github_issue_number.is_not(None))
                .order_by(TestCase.id)
                .limit(2000)
            )
        ).all()
        if not rows:
            return
        changed = False
        try:
            async with httpx.AsyncClient(
                base_url=API_BASE, headers=_headers(s.github_token), timeout=15, transport=transport
            ) as client:
                for tc, repo in rows:
                    resp = await client.get(f"/repos/{repo}/issues/{tc.github_issue_number}")
                    if resp.status_code in (403, 429):
                        logger.warning("GitHub rate limit / forbidden in sync; stopping")
                        break
                    if resp.status_code == 200 and resp.json().get("state") == "closed":
                        logger.info("Issue closed %s#%s; re-arming test %s", repo, tc.github_issue_number, tc.id)
                        tc.github_issue_number = None
                        changed = True
                    elif resp.status_code == 404:
                        # Issue deleted or token can no longer see it — do not hold dedup forever.
                        logger.info("Issue gone %s#%s; re-arming test %s", repo, tc.github_issue_number, tc.id)
                        tc.github_issue_number = None
                        changed = True
        except httpx.HTTPError as exc:
            logger.warning("GitHub unreachable, skipping issue-state sync: %s", exc)
        if changed:
            await db.commit()


async def on_report_processed(session_factory: async_sessionmaker[AsyncSession], outcome: ProcessOutcome) -> None:
    """ReportWorker hook: file issues for the Tests a processed Report touched."""
    if outcome.status != REPORT_PROCESSED or not configured():
        return
    async with session_factory() as db:
        await file_issues_for(db, outcome.touched_test_ids)
