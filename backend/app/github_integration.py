"""GitHub issue automation, filed in each Test's (and Job's) own Repo.

When a Test or a Job crosses the *filing gate*, file an issue in that entity's
Repo (``owner/name``) with the evidence. Runs after the processor finishes a
Report (never on the upload path), so a slow or unreachable GitHub API never
delays CI.

Filing gate (all configured minimums must be met; 0 disables a signal):
  - ``github_issue_min_score``             — flakiness score (0..1)
  - ``github_issue_min_proven_flakes``     — same-commit fail+pass count
  - ``github_issue_min_failures``          — failures in the recent window
    (for Jobs: **unexplained** failures only — explained failures are Tests')
Deduplication works because a filed issue's number is stored on the Test/Job;
an issue is only ever re-filed after GitHub reports it closed
(``sync_closed_issues`` clears the stored number, and the next Report that
touches the entity re-files it if it still meets the gate).

Error handling: never raises.
- No token configured            -> silent no-op (self-host without GitHub).
- 403/429 (rate limit/forbidden) -> stop this batch, log a warning.
- Any other failure (404, network) -> log, go on.
"""

import logging
from collections.abc import Iterator
from dataclasses import dataclass

import httpx
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from . import queries
from .attribution import unexplained_failure_counts
from .config import Settings, get_settings
from .models import (
    REPORT_PROCESSED,
    Job,
    Pipeline,
    Project,
    Repo,
    TestCase,
    TestExecution,
    TestRun,
)
from .processing import CHUNK, ProcessOutcome
from .scoring import FAILING

logger = logging.getLogger("flakeradar.github")

API_BASE = "https://api.github.com"


def configured() -> bool:
    return bool(get_settings().github_token)


@dataclass(frozen=True)
class _FilingGate:
    """The three-signal issue-filing gate (see module docstring), as one unit."""

    min_score: float
    min_proven_flakes: int
    min_failures: int

    @classmethod
    def from_settings(cls, s: Settings) -> "_FilingGate":
        return cls(s.github_issue_min_score, s.github_issue_min_proven_flakes, s.github_issue_min_failures)


def _headers(token: str) -> dict[str, str]:
    return {
        "Authorization": f"Bearer {token}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
    }


def _chunks(ids: list[int]) -> Iterator[list[int]]:
    """Split `ids` into batches of `CHUNK`, staying under the DB driver's bind-param limit."""
    for i in range(0, len(ids), CHUNK):
        chunk = ids[i : i + CHUNK]
        if chunk:
            yield chunk


_RUN_HINT_TEMPLATES: dict[str, str] = {
    "py": "pytest {path}::{name}",
    "js": 'npx vitest run {path} -t "{name}"',
    "jsx": 'npx vitest run {path} -t "{name}"',
    "mjs": 'npx vitest run {path} -t "{name}"',
    "cjs": 'npx vitest run {path} -t "{name}"',
    "ts": 'npx vitest run {path} -t "{name}"',
    "tsx": 'npx vitest run {path} -t "{name}"',
    "go": "go test ./... -run {name}",
    "java": "mvn test -Dtest={classname}#{name}",
    "kt": "mvn test -Dtest={classname}#{name}",
    "rb": 'bundle exec rspec {path} -e "{name}"',
}


def _run_hint(path: str | None, classname: str, name: str) -> str:
    """Best-effort re-run command, inferred from the test file's extension.

    FlakeRadar ingests JUnit XML from any runner, so the framework is never
    reported directly; a wrong guess (e.g. always "pytest") would send an
    agent to run a Python command against a JS/TS/Java/Go test.
    """
    if not path:
        return f"# Re-run `{classname}::{name}` via your project's test runner (no file path reported)."
    ext = path.rsplit(".", 1)[-1].lower() if "." in path else ""
    template = _RUN_HINT_TEMPLATES.get(ext)
    if template is None:
        return f"# Re-run `{classname}::{name}` via your project's test runner ({path})."
    return template.format(path=path, name=name, classname=classname)


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
        path = f"{root}/{tc.file}" if root else tc.file
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
    run_hint = _run_hint(path, tc.classname, tc.name)
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
        lines.append(
            f"- **Last failing commit:** `{last_fail_sha}`"
            + (f" on branch `{last_fail_branch}`" if last_fail_branch else "")
        )
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
    for chunk in _chunks(test_case_ids):
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
    gate = _FilingGate.from_settings(s)
    candidates: list[tuple[TestCase, str, str, str]] = []
    for chunk in _chunks(test_case_ids):
        candidates += (
            await db.execute(
                select(TestCase, Repo.name, Project.name, Project.root)
                .join(Project, TestCase.project_id == Project.id)
                .join(Repo, Project.repo_id == Repo.id)
                .where(
                    TestCase.id.in_(chunk),
                    TestCase.flakiness_score >= gate.min_score,
                    TestCase.confirmed_flake_count >= gate.min_proven_flakes,
                    TestCase.github_issue_number.is_(None),
                )
                .order_by(TestCase.id)
            )
        ).all()
    if gate.min_failures > 0:
        failures = await _window_failures(db, [r[0].id for r in candidates], s.score_window)
        candidates = [r for r in candidates if failures.get(r[0].id, 0) >= gate.min_failures]
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


async def _job_candidates(db: AsyncSession, job_ids: list[int]) -> list[tuple[Job, str, str]]:
    """github-provider Jobs that meet the gate and still have no open issue.

    Returns [(Job, pipeline_name, repo_name)]. `min_failures`, when configured,
    requires that many UNEXPLAINED failures in the newest `score_window` Job
    executions.
    """
    s = get_settings()
    gate = _FilingGate.from_settings(s)
    candidates: list[tuple[Job, str, str]] = []
    for chunk in _chunks(job_ids):
        candidates += (
            await db.execute(
                select(Job, Pipeline.name, Repo.name)
                .join(Pipeline, Job.pipeline_id == Pipeline.id)
                .join(Repo, Pipeline.repo_id == Repo.id)
                .where(
                    Job.id.in_(chunk),
                    Pipeline.provider == "github",
                    Job.flakiness_score >= gate.min_score,
                    Job.confirmed_flake_count >= gate.min_proven_flakes,
                    Job.github_issue_number.is_(None),
                )
                .order_by(Job.id)
            )
        ).all()
    if gate.min_failures > 0:
        failures = await unexplained_failure_counts(db, [r[0].id for r in candidates], s.score_window)
        candidates = [r for r in candidates if failures.get(r[0].id, 0) >= gate.min_failures]
    return candidates


async def _job_issue_body(db: AsyncSession, job: Job, pipeline: str, repo: str) -> str:
    """Render a self-contained issue body for a flaky CI Job."""
    s = get_settings()
    hist = await queries.get_job(db, job.id, threshold=s.flake_threshold, executions_limit=10)
    lines = [
        f"FlakeRadar detected a flaky CI job: `{pipeline}` / `{job.name}`",
        "",
        f"- **Repo:** `{repo}`",
        f"- **Pipeline:** `{pipeline}`",
        f"- **Job:** `{job.name}`",
        f"- **Flakiness score:** {job.flakiness_score:.2f} (filed at ≥ {s.github_issue_min_score:.2f})",
        f"- **Proven flakes (same commit failed *and* passed):** {job.confirmed_flake_count}",
    ]
    if hist is not None:
        lines += [
            f"- **Unexplained failures:** {hist.unexplained_failures} (failures with no failing test in the job)",
            f"- **Explained by tests:** {hist.explained_failures}",
        ]
    lines += [
        "",
        "Job scores count only **unexplained** failures — a failure that a failing Test in the same job",
        "explains is not counted. An unexplained failure points at setup, network, runner capacity,",
        "timeouts, or a crash before the tests run. If the failures cluster on one runner, that runner",
        "may be the cause. Open the job and fix the failing step, then re-run.",
        "",
        "### Last 10 executions",
        "",
        "| Outcome | Commit | Branch | Attempt | Runner | Job |",
        "|---|---|---|---|---|---|",
    ]
    if hist is not None:
        for e in hist.executions:
            link = f"[job]({e.url})" if e.url else "—"
            runner = e.runner_name or "—"
            lines.append(
                f"| {e.outcome} | `{e.commit_sha[:10]}` | {e.branch} | {e.ci_run_attempt} | {runner} | {link} |"
            )
    return "\n".join(lines)


async def file_job_issues_for(
    db: AsyncSession,
    job_ids: list[int],
    *,
    transport: httpx.AsyncBaseTransport | None = None,
) -> None:
    """File issues for github-provider Jobs that now meet the filing gate. Never raises.

    `transport` exists for tests (httpx.MockTransport); production passes None.
    """
    if not configured() or not job_ids:
        return
    s = get_settings()
    candidates = await _job_candidates(db, job_ids)
    if not candidates:
        return
    try:
        async with httpx.AsyncClient(
            base_url=API_BASE, headers=_headers(s.github_token), timeout=15, transport=transport
        ) as client:
            for job, pipeline, repo in candidates:
                resp = await client.post(
                    f"/repos/{repo}/issues",
                    json={
                        "title": f"[FlakeRadar] Flaky CI job: {pipeline} / {job.name}",
                        "body": await _job_issue_body(db, job, pipeline, repo),
                        "labels": [s.github_issue_label],
                    },
                )
                if resp.status_code == 201:
                    job.github_issue_number = resp.json()["number"]
                    await db.commit()
                    logger.info("Filed issue %s#%s for job %s", repo, job.github_issue_number, job.id)
                elif resp.status_code in (403, 429):
                    logger.warning(
                        "GitHub rate limit / forbidden (remaining=%s); stopping batch",
                        resp.headers.get("x-ratelimit-remaining"),
                    )
                    return
                else:
                    logger.warning(
                        "GitHub issue creation failed for job %s in %s: %s %s",
                        job.id,
                        repo,
                        resp.status_code,
                        resp.text[:300],
                    )
    except httpx.HTTPError as exc:
        logger.warning("GitHub unreachable, skipping job issue filing: %s", exc)


async def sync_closed_issues(
    session_factory: async_sessionmaker[AsyncSession], *, transport: httpx.AsyncBaseTransport | None = None
) -> None:
    """Re-arm deduplication for Tests and Jobs whose GitHub issue has been closed.

    A filed issue's number is stored on the entity and blocks re-filing. When
    GitHub reports that issue closed (or gone), clear the stored number so the
    next Report that touches the entity can re-file a fresh issue if it still
    meets the filing gate. Leader-only maintenance; checks each open issue with
    its own sequential GET (capped at the 2000 most stale rows per run for each
    of Tests and Jobs — the GitHub REST API has no bulk lookup by arbitrary
    issue numbers). Never raises.

    `transport` exists for tests (httpx.MockTransport); production passes None.
    """
    if not configured():
        return
    async with session_factory() as db:
        test_rows = (
            await db.execute(
                select(TestCase, Repo.name)
                .join(Project, TestCase.project_id == Project.id)
                .join(Repo, Project.repo_id == Repo.id)
                .where(TestCase.github_issue_number.is_not(None))
                .order_by(TestCase.id)
                .limit(2000)
            )
        ).all()
        job_rows = (
            await db.execute(
                select(Job, Repo.name)
                .join(Pipeline, Job.pipeline_id == Pipeline.id)
                .join(Repo, Pipeline.repo_id == Repo.id)
                .where(Job.github_issue_number.is_not(None))
                .order_by(Job.id)
                .limit(2000)
            )
        ).all()
        if not test_rows and not job_rows:
            return
        rows = [(tc, repo, "test") for tc, repo in test_rows] + [(job, repo, "job") for job, repo in job_rows]
        try:
            async with httpx.AsyncClient(
                base_url=API_BASE, headers=_headers(get_settings().github_token), timeout=15, transport=transport
            ) as client:
                for obj, repo, entity in rows:
                    if not await _clear_if_closed(client, obj, repo, entity):
                        break
        except httpx.HTTPError as exc:
            logger.warning("GitHub unreachable, skipping issue-state sync: %s", exc)
        await db.commit()  # keeps whatever was cleared before a stop or a network error


async def _clear_if_closed(client: httpx.AsyncClient, obj: Job | TestCase, repo: str, entity: str) -> bool:
    """Check one issue; clear its number on the ORM object when closed/gone.

    Returns False on 403/429, when the batch must stop.
    """
    resp = await client.get(f"/repos/{repo}/issues/{obj.github_issue_number}")
    if resp.status_code in (403, 429):
        logger.warning("GitHub rate limit / forbidden in sync; stopping")
        return False
    if resp.status_code == 200 and resp.json().get("state") == "closed":
        logger.info("Issue closed %s#%s; re-arming %s %s", repo, obj.github_issue_number, entity, obj.id)
        obj.github_issue_number = None
    elif resp.status_code == 404:
        # Issue deleted or token can no longer see it — do not hold dedup forever.
        logger.info("Issue gone %s#%s; re-arming %s %s", repo, obj.github_issue_number, entity, obj.id)
        obj.github_issue_number = None
    return True


async def on_report_processed(session_factory: async_sessionmaker[AsyncSession], outcome: ProcessOutcome) -> None:
    """ReportWorker hook: file issues for the Tests and Jobs a processed Report touched."""
    if outcome.status != REPORT_PROCESSED or not configured():
        return
    async with session_factory() as db:
        await file_issues_for(db, outcome.touched_test_ids)
        await file_job_issues_for(db, outcome.touched_job_ids)
