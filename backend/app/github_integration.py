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
touches the entity re-files it if it still meets the gate) — unless it was
closed as ``not_planned``, which keeps the number and flags the row, so nothing
is filed again until a person unlinks it.

Error handling: never raises.
- No token configured            -> silent no-op (self-host without GitHub).
- Rate limit on create (429, or a 403 that is one: `x-ratelimit-remaining: 0`,
  a `retry-after` header, or a body message containing "rate limit") -> stop
  this batch, log a warning.
- Any other 403 on create (a permission error: a token without access to that
  Repo, an archived Repo), or 404/410 (issues disabled) -> skip that Repo for
  the rest of the batch, log one warning per Repo, go on with the other Repos.
- In the sync (`sync_closed_issues`), a definitive Repo-wide client error
  (a permission 403, bad credentials 401, a gone Repo 410) leaves the Issue
  link as it is, skips that Repo for the rest of the run, and stamps the rows
  of that Repo so they rotate; 404 still re-arms.
- After an admin delete (`close_issues_for_deleted_repo`), any failure but a
  rate limit costs only that one Issue: the other numbers of the deleted scope
  are still commented on and closed.
- An admin delete that commits between a filing POST and the filer's commit
  (a StaleDataError on the vanished row) costs only that filing: the
  just-created issue is commented on and closed best-effort, and the rest of
  the batch goes on.
- The fixed-note step (`note_fixed_issues_for`, after filing) follows the same
  policy for the comment POST and the optional close PATCH. It stamps
  `github_issue_fixed_noted_at` the moment the comment lands, so a later close
  failure never causes a second comment on the same fix.
- A response body FlakeRadar cannot use (a 201 without an issue number, a
  sync 200 that is not a JSON object with a `state`) -> log a warning, go on;
  no issue number is stored and no link is cleared (the sync still stamps the
  row's check time, so an unreadable link keeps rotating instead of pinning
  the front of the queue).
- Any other failure (network) -> log, go on.
"""

import logging
from dataclasses import dataclass

import httpx
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker
from sqlalchemy.orm.exc import StaleDataError

from . import issue_links, queries
from .attribution import unexplained_failure_counts
from .batching import chunks
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
    utcnow,
)
from .processing import ProcessOutcome
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


def _json_object(resp: httpx.Response) -> dict | None:
    """The body parsed as a JSON object, or None when it is not one. Never raises."""
    try:
        payload = resp.json()
    except ValueError:  # non-JSON body (a proxy error page, an empty response)
        return None
    return payload if isinstance(payload, dict) else None


def _issue_number(resp: httpx.Response) -> int | None:
    """The number of a created issue, or None when the 201 body does not carry one."""
    body = _json_object(resp)
    number = body.get("number") if body is not None else None
    return number if isinstance(number, int) and not isinstance(number, bool) else None


def _issue_state(resp: httpx.Response) -> str | None:
    """The state of a fetched issue, or None when the 200 body does not report one."""
    body = _json_object(resp)
    state = body.get("state") if body is not None else None
    return state if isinstance(state, str) else None


def _issue_state_reason(resp: httpx.Response) -> str | None:
    """Why a fetched issue changed state, or None when the 200 body does not report one.

    GitHub fills this in only for closed issues (`completed`, `not_planned`) and
    reopened ones (`reopened`); an issue closed before the field existed, or a body
    this cannot read, simply has no reason and is treated as a plain close.
    """
    body = _json_object(resp)
    reason = body.get("state_reason") if body is not None else None
    return reason if isinstance(reason, str) else None


def _rate_limited(resp: httpx.Response) -> bool:
    """True when GitHub is throttling: 429, or a 403 that is a (primary or secondary) rate limit.

    A 403 also means *permission* ("Resource not accessible by personal access
    token", a token without access to one repo, an archived repo); treating
    those as rate limits would stop the batch for every other Repo. Never
    raises: an unparseable body simply is not a rate limit.
    """
    if resp.status_code == 429:
        return True
    if resp.status_code != 403:
        return False
    if resp.headers.get("x-ratelimit-remaining") == "0" or resp.headers.get("retry-after"):
        return True
    body = _json_object(resp)
    message = body.get("message", "") if body is not None else ""
    return "rate limit" in str(message).lower()


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
    for chunk in chunks(test_case_ids):
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
    for chunk in chunks(test_case_ids):
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


async def _file_batch(
    db: AsyncSession,
    candidates: list[tuple[Job | TestCase, str, str, str]],
    *,
    entity: str,
    transport: httpx.AsyncBaseTransport | None = None,
) -> None:
    """File one batch of rendered issues, whichever entity they describe.

    Each candidate is `(obj, repo, title, body)`; `entity` is the noun ("test"
    or "job") the log lines name. This is the module docstring's filing policy
    in one place: the client lifecycle, a 201 that stores its number on the
    entity and commits per issue, a 201 whose body has no number (log, go on),
    a rate limit (stop this batch), any other 403/404/410 (skip that Repo for
    the rest of the batch), any other status (log, go on), and a network error
    (log, never raise).

    `transport` exists for tests (httpx.MockTransport); production passes None.
    """
    s = get_settings()
    try:
        async with httpx.AsyncClient(
            base_url=API_BASE, headers=_headers(s.github_token), timeout=15, transport=transport
        ) as client:
            skipped: set[str] = set()
            for obj, repo, title, body in candidates:
                if repo in skipped:
                    continue
                resp = await client.post(
                    f"/repos/{repo}/issues",
                    json={"title": title, "body": body, "labels": [s.github_issue_label]},
                )
                if resp.status_code == 201:
                    number = _issue_number(resp)
                    if number is None:
                        logger.warning(
                            "GitHub issue creation in %s returned 201 with no issue number: %s", repo, resp.text[:300]
                        )
                    else:
                        obj_id = obj.id  # the object is expired after the savepoint rollback
                        # SAVEPOINT, so a row deleted under us (an admin delete
                        # winning the race) costs only this filing, not the batch.
                        nested = await db.begin_nested()
                        issue_links.store_filed_link(obj, number)  # also marks the link as ours
                        try:
                            await nested.commit()
                        except StaleDataError:
                            # The row vanished mid-filing: the number is referenced
                            # nowhere, so close the issue we just created, best-effort.
                            await nested.rollback()
                            await close_issues_for_deleted_repo(repo, [number], transport=transport)
                            logger.warning(
                                "%s %s deleted (admin delete) while filing; closed issue %s#%s",
                                entity.capitalize(),
                                obj_id,
                                repo,
                                number,
                            )
                            continue
                        await db.commit()
                        logger.info("Filed issue %s#%s for %s %s", repo, obj.github_issue_number, entity, obj.id)
                elif _rate_limited(resp):
                    logger.warning(
                        "GitHub rate limit (remaining=%s); stopping batch",
                        resp.headers.get("x-ratelimit-remaining"),
                    )
                    return
                elif resp.status_code in (403, 404, 410):
                    # Permission error, or issues disabled on this Repo: it costs
                    # this Repo only, the rest of the batch goes on.
                    logger.warning(
                        "GitHub issue creation skipped for repo %s: %s %s",
                        repo,
                        resp.status_code,
                        resp.text[:300],
                    )
                    skipped.add(repo)
                else:
                    logger.warning(
                        "GitHub issue creation failed for %s %s in %s: %s %s",
                        entity,
                        obj.id,
                        repo,
                        resp.status_code,
                        resp.text[:300],
                    )
    except httpx.HTTPError as exc:
        logger.warning("GitHub unreachable, skipping %s issue filing: %s", entity, exc)


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
    candidates = await _select_candidates(db, test_case_ids)
    if not candidates:
        return
    batch = [
        (
            tc,
            repo,
            f"[FlakeRadar] Flaky test: {tc.classname}::{tc.name}",
            await _issue_body(db, tc, repo, project, root),
        )
        for tc, repo, project, root in candidates
    ]
    await _file_batch(db, batch, entity="test", transport=transport)


async def _job_candidates(db: AsyncSession, job_ids: list[int]) -> list[tuple[Job, str, str]]:
    """github-provider Jobs that meet the gate and still have no open issue.

    Returns [(Job, pipeline_name, repo_name)]. `min_failures`, when configured,
    requires that many UNEXPLAINED failures in the newest `score_window` Job
    executions.
    """
    s = get_settings()
    gate = _FilingGate.from_settings(s)
    candidates: list[tuple[Job, str, str]] = []
    for chunk in chunks(job_ids):
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
        top_runners = [r for r in hist.runners if r.unexplained_failures > 0][:3]
        if top_runners:
            lines.append("- **Failures by runner:**")
            for r in top_runners:
                label = r.runner_name or ", ".join(r.labels) or "(no labels)"
                lines.append(f"  - `{label}`: {r.unexplained_failures} unexplained of {r.executions} executions")
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
    candidates = await _job_candidates(db, job_ids)
    if not candidates:
        return
    batch = [
        (
            job,
            repo,
            f"[FlakeRadar] Flaky CI job: {pipeline} / {job.name}",
            await _job_issue_body(db, job, pipeline, repo),
        )
        for job, pipeline, repo in candidates
    ]
    await _file_batch(db, batch, entity="job", transport=transport)


async def sync_closed_issues(
    session_factory: async_sessionmaker[AsyncSession], *, transport: httpx.AsyncBaseTransport | None = None
) -> None:
    """Re-arm deduplication for Tests and Jobs whose GitHub issue has been closed.

    A filed issue's number is stored on the entity and blocks re-filing. When
    GitHub reports that issue closed (or gone), clear the stored number so the
    next Report that touches the entity can re-file a fresh issue if it still
    meets the filing gate — except a close as `not_planned`, which is the team
    accepting the flake: the link stays and `github_issue_closed` records it, so
    nothing is filed again until someone unlinks the issue or GitHub reopens it.
    Leader-only maintenance; checks each open issue with
    its own sequential GET (capped at the `FLAKERADAR_GITHUB_SYNC_BATCH` most
    stale rows per run for each of Tests and Jobs, oldest check first
    (`github_issue_checked_at` NULLS FIRST) — the GitHub REST API has no bulk
    lookup by arbitrary issue numbers). Never raises.

    `transport` exists for tests (httpx.MockTransport); production passes None.
    """
    if not configured():
        return
    async with session_factory() as db:
        batch = get_settings().github_sync_batch
        test_rows = await issue_links.linked_for_check(db, issue_links.TEST, batch)
        job_rows = await issue_links.linked_for_check(db, issue_links.JOB, batch)
        if not test_rows and not job_rows:
            return
        rows = [(tc, repo, "test") for tc, repo in test_rows] + [(job, repo, "job") for job, repo in job_rows]
        skipped: set[str] = set()
        try:
            async with httpx.AsyncClient(
                base_url=API_BASE, headers=_headers(get_settings().github_token), timeout=15, transport=transport
            ) as client:
                for obj, repo, entity in rows:
                    if repo in skipped:
                        obj.github_issue_checked_at = utcnow()  # unreadable repo this run; rotate it like a checked row
                        continue
                    if not await _clear_if_closed(client, obj, repo, entity, skipped):
                        break
        except httpx.HTTPError as exc:
            logger.warning("GitHub unreachable, skipping issue-state sync: %s", exc)
        await db.commit()  # keeps whatever was cleared before a stop or a network error


async def _clear_if_closed(
    client: httpx.AsyncClient, obj: Job | TestCase, repo: str, entity: str, skipped: set[str]
) -> bool:
    """Check one issue; clear its number on the ORM object when closed/gone.

    A close carries a `state_reason`, and it decides what "closed" means: `not_planned`
    is the team saying "known, accepted, will not fix", so the link is KEPT and
    `github_issue_closed` is set — the filer skips rows that carry a number, so no fresh
    issue is filed for a Test the team already declined. Any other reason (or none at
    all, e.g. a close predating the field) re-arms as before: number and filed flag go,
    and the closed flag goes with them. `open` clears the flag, which is what keeps
    these links in the rotation: a declined issue that is reopened re-arms filing. A 404
    (deleted, or invisible to the token) clears link and flag exactly like a close.

    Returns False on a rate limit, when the batch must stop; a definitive
    Repo-wide client error (a permission 403, bad credentials 401, a gone
    Repo 410) adds `repo` to `skipped` and leaves the link as it is.

    Every row this got an answer for is stamped with `utcnow()` — open,
    closed, gone, a Repo-wide client error, an unexpected status, and even a
    body this cannot read — and so is a row skipped because its Repo is
    already in `skipped`. `github_issue_checked_at` is what the sync's
    ordering rotates on: an unstamped row sorts NULLS FIRST, so a link whose
    answer keeps arriving unusable would take a batch slot on every single
    run and starve the links behind it whenever the cap is below the linked
    set. Only a rate-limit stop and a transport error leave a row unstamped,
    so those are retried first.
    """
    resp = await client.get(f"/repos/{repo}/issues/{obj.github_issue_number}")
    if _rate_limited(resp):
        logger.warning("GitHub rate limit in sync; stopping")
        return False
    # The request was answered, whatever it said: this row has been checked.
    obj.github_issue_checked_at = utcnow()
    if resp.status_code in (401, 403, 410):
        # Definitive, Repo-wide client error (bad credentials, a permission
        # error, a gone Repo): this Repo's links stay as they are; skip it for
        # the rest of the sync and go on with the other Repos.
        logger.warning(
            "GitHub issue-state sync skipped for repo %s: %s %s",
            repo,
            resp.status_code,
            resp.text[:300],
        )
        skipped.add(repo)
        return True
    if resp.status_code == 200:
        state = _issue_state(resp)
        if state is None:
            # An unreadable body says nothing about the issue: leave the link alone.
            logger.warning(
                "GitHub issue-state sync got an unusable 200 body for %s#%s: %s",
                repo,
                obj.github_issue_number,
                resp.text[:300],
            )
        elif state == "closed":
            reason = _issue_state_reason(resp)
            if reason == "not_planned":
                # Known, accepted, will not fix: keep the link so the filer (which skips
                # any row that carries a number) never files this again, and say so.
                logger.info(
                    "Issue closed as not planned %s#%s; keeping the link for %s %s",
                    repo,
                    obj.github_issue_number,
                    entity,
                    obj.id,
                )
                obj.github_issue_closed = True
            else:
                logger.info(
                    "Issue closed %s %s#%s; re-arming %s %s",
                    reason or "without a reason",
                    repo,
                    obj.github_issue_number,
                    entity,
                    obj.id,
                )
                obj.github_issue_number = None
                obj.github_issue_filed = None
                obj.github_issue_closed = False
        elif state == "open":
            # Possibly reopened after a close as not planned: filing is live again.
            obj.github_issue_closed = False
    elif resp.status_code == 404:
        # Issue deleted or token can no longer see it — do not hold dedup forever.
        logger.info("Issue gone %s#%s; re-arming %s %s", repo, obj.github_issue_number, entity, obj.id)
        obj.github_issue_number = None
        obj.github_issue_filed = None
        obj.github_issue_closed = False
    return True


_CLOSE_COMMENT = (
    "FlakeRadar deleted the tracked flaky test or CI job this issue was filed for via the admin delete API, "
    "so the permalinks in this issue no longer resolve. Closing it automatically; if the test or job is "
    "uploaded again, FlakeRadar files a fresh issue."
)


async def close_issues_for_deleted_repo(
    repo: str, numbers: list[int], *, transport: httpx.AsyncBaseTransport | None = None
) -> None:
    """Best-effort close of the issues whose Test/Job rows an admin delete removed.

    An admin delete takes the tracked row with it, so no later Report re-files or
    syncs the issue and `sync_closed_issues` never sees it again: without this it
    would stay open forever, pointing at permalinks that now 404. No-op when
    GitHub is not configured or `numbers` is empty. For each number, post an
    explanatory comment, then PATCH the issue closed.

    Never raises. A rate limit logs a warning and stops the batch; any other
    failure (a transport error, a 5xx, a client error) costs that one issue and
    the rest of the list goes on — unlike filing, where one failure skips the
    whole Repo for the batch. `transport` exists for tests (httpx.MockTransport).
    """
    if not configured() or not numbers:
        return
    try:
        async with httpx.AsyncClient(
            base_url=API_BASE, headers=_headers(get_settings().github_token), timeout=15, transport=transport
        ) as client:
            for number in numbers:
                try:
                    resp = await client.post(f"/repos/{repo}/issues/{number}/comments", json={"body": _CLOSE_COMMENT})
                    if _rate_limited(resp):
                        logger.warning(
                            "GitHub rate limit (remaining=%s); stopping the post-delete close batch",
                            resp.headers.get("x-ratelimit-remaining"),
                        )
                        return
                    if resp.status_code in (404, 410):
                        logger.info("Issue %s#%s already gone after an admin delete", repo, number)
                        continue
                    if resp.status_code != 201:
                        logger.warning(
                            "GitHub comment on %s#%s failed: %s %s", repo, number, resp.status_code, resp.text[:300]
                        )
                        continue
                    resp = await client.patch(f"/repos/{repo}/issues/{number}", json={"state": "closed"})
                    if _rate_limited(resp):
                        logger.warning(
                            "GitHub rate limit (remaining=%s) on %s#%s; stopping the batch",
                            resp.headers.get("x-ratelimit-remaining"),
                            repo,
                            number,
                        )
                        return
                    if resp.status_code in (404, 410):
                        logger.info("Issue %s#%s already gone after an admin delete", repo, number)
                    elif resp.status_code == 200:
                        logger.info("Closed issue %s#%s after an admin delete", repo, number)
                    else:
                        logger.warning(
                            "GitHub close of %s#%s failed: %s %s", repo, number, resp.status_code, resp.text[:300]
                        )
                except httpx.HTTPError as exc:
                    # A transport failure on one number must not lose the others.
                    logger.warning("GitHub post-delete close skipped for issue %s#%s: %s", repo, number, exc)
    except httpx.HTTPError as exc:
        logger.warning("GitHub unreachable, skipping post-delete issue close: %s", exc)


def _fixed_comment(obj: Job | TestCase, default_branch: str | None, *, entity: str) -> str:
    """The one comment that says the fix appears to hold (issue #33).

    `default_branch` is the Repo's Default branch, the branch the Clean streak
    is counted on; when it is unknown (NULL) the streak spans every branch, so
    naming one would be a lie and the clause is dropped. With `public_url` set
    the comment carries a deep link to the entity's own dashboard view.
    """
    s = get_settings()
    where = f" on `{default_branch}`" if default_branch else ""
    body = (
        f"FlakeRadar: {obj.clean_streak} passing runs{where} since the last failure "
        f"(score now {obj.flakiness_score:.2f}). The fix appears to hold."
    )
    if s.public_url:
        base = s.public_url.rstrip("/")
        link = f"{base}/?view=jobs&job={obj.id}" if entity == "job" else f"{base}/?test={obj.id}"
        body = f"{body}\n\n{link}"
    return body


async def _fixed_note_test_candidates(
    db: AsyncSession, test_case_ids: list[int]
) -> list[tuple[TestCase, str, str | None]]:
    """Linked Tests at the note threshold whose fix has not been noted yet.

    Returns [(test, repo, default_branch)]. A row already stamped with
    `github_issue_fixed_noted_at` is never noted again, and a link flagged
    `github_issue_closed` (closed as not planned) is skipped: that issue is
    already closed, so there is nothing to note or close here.
    """
    s = get_settings()
    candidates: list[tuple[TestCase, str, str | None]] = []
    for chunk in chunks(test_case_ids):
        candidates += (
            await db.execute(
                select(TestCase, Repo.name, Repo.default_branch)
                .join(Project, TestCase.project_id == Project.id)
                .join(Repo, Project.repo_id == Repo.id)
                .where(
                    TestCase.id.in_(chunk),
                    TestCase.github_issue_number.is_not(None),
                    TestCase.github_issue_fixed_noted_at.is_(None),
                    TestCase.github_issue_closed.is_(False),
                    TestCase.clean_streak >= s.github_fixed_streak,
                )
                .order_by(TestCase.id)
            )
        ).all()
    return candidates


async def _fixed_note_job_candidates(db: AsyncSession, job_ids: list[int]) -> list[tuple[Job, str, str | None]]:
    """github-provider Jobs at the note threshold whose fix has not been noted yet.

    Returns [(job, repo, default_branch)]; the same rules as the Test select, and
    only a github Provider has an issue to note.
    """
    s = get_settings()
    candidates: list[tuple[Job, str, str | None]] = []
    for chunk in chunks(job_ids):
        candidates += (
            await db.execute(
                select(Job, Repo.name, Repo.default_branch)
                .join(Pipeline, Job.pipeline_id == Pipeline.id)
                .join(Repo, Pipeline.repo_id == Repo.id)
                .where(
                    Job.id.in_(chunk),
                    Pipeline.provider == "github",
                    Job.github_issue_number.is_not(None),
                    Job.github_issue_fixed_noted_at.is_(None),
                    Job.github_issue_closed.is_(False),
                    Job.clean_streak >= s.github_fixed_streak,
                )
                .order_by(Job.id)
            )
        ).all()
    return candidates


async def _note_batch(
    db: AsyncSession,
    rows: list[tuple[Job | TestCase, str, str]],
    *,
    entity: str,
    transport: httpx.AsyncBaseTransport | None = None,
) -> None:
    """Note (and maybe close) one batch of fixed issues, whichever entity they describe.

    Each row is `(obj, repo, body)` and `entity` is the noun the log lines name.
    Same lifecycle and #14 error policy as `_file_batch`: one client, a rate limit
    stops the batch, any other 403/404/410 skips that Repo, anything else logs and
    goes on, a network error logs and ends, and nothing raises.

    The stamp lands as soon as the comment POST succeeds — before the close, and
    kept even if the close fails — because the rule is *one* comment per fix, and
    re-posting after a failed close would break it.
    """
    s = get_settings()
    try:
        async with httpx.AsyncClient(
            base_url=API_BASE, headers=_headers(s.github_token), timeout=15, transport=transport
        ) as client:
            skipped: set[str] = set()
            for obj, repo, body in rows:
                if repo in skipped:
                    continue
                number = obj.github_issue_number
                resp = await client.post(f"/repos/{repo}/issues/{number}/comments", json={"body": body})
                if resp.status_code == 201:
                    obj_id = obj.id  # the object is expired after the savepoint rollback below
                    nested = await db.begin_nested()
                    obj.github_issue_fixed_noted_at = utcnow()
                    try:
                        await nested.commit()
                    except StaleDataError:
                        # An admin delete won the race: the comment is on an issue
                        # whose row is gone, which costs this note only.
                        await nested.rollback()
                        logger.warning(
                            "%s %s deleted (admin delete) while noting issue %s#%s",
                            entity.capitalize(),
                            obj_id,
                            repo,
                            number,
                        )
                        continue
                    await db.commit()
                    logger.info("Noted the fix appears to hold on issue %s#%s for %s %s", repo, number, entity, obj_id)
                    if s.github_close_fixed:
                        close = await client.patch(
                            f"/repos/{repo}/issues/{number}",
                            json={"state": "closed", "state_reason": "completed"},
                        )
                        if _rate_limited(close):
                            logger.warning(
                                "GitHub rate limit (remaining=%s) closing issue %s#%s; stopping the batch",
                                close.headers.get("x-ratelimit-remaining"),
                                repo,
                                number,
                            )
                            return
                        if close.status_code in (403, 404, 410):
                            logger.warning(
                                "GitHub issue close skipped for repo %s: %s %s",
                                repo,
                                close.status_code,
                                close.text[:300],
                            )
                            skipped.add(repo)
                        elif close.status_code == 200:
                            logger.info("Closed issue %s#%s after noting the fix", repo, number)
                        else:
                            logger.warning(
                                "GitHub close of %s#%s failed: %s %s", repo, number, close.status_code, close.text[:300]
                            )
                elif _rate_limited(resp):
                    logger.warning(
                        "GitHub rate limit (remaining=%s); stopping the note batch",
                        resp.headers.get("x-ratelimit-remaining"),
                    )
                    return
                elif resp.status_code in (403, 404, 410):
                    logger.warning(
                        "GitHub fixed-note skipped for repo %s: %s %s", repo, resp.status_code, resp.text[:300]
                    )
                    skipped.add(repo)
                else:
                    logger.warning(
                        "GitHub fixed-note comment failed for %s %s in %s: %s %s",
                        entity,
                        obj.id,
                        repo,
                        resp.status_code,
                        resp.text[:300],
                    )
    except httpx.HTTPError as exc:
        logger.warning("GitHub unreachable, skipping %s fixed-note comments: %s", entity, exc)


async def note_fixed_issues_for(
    db: AsyncSession,
    test_case_ids: list[int],
    *,
    transport: httpx.AsyncBaseTransport | None = None,
) -> None:
    """Comment once on the linked issue of each Test whose fix appears to hold. Never raises.

    Disabled while `github_fixed_streak` is 0. `transport` exists for tests
    (httpx.MockTransport); production passes None.
    """
    if not configured() or get_settings().github_fixed_streak <= 0 or not test_case_ids:
        return
    candidates = await _fixed_note_test_candidates(db, test_case_ids)
    if not candidates:
        return
    batch = [(tc, repo, _fixed_comment(tc, branch, entity="test")) for tc, repo, branch in candidates]
    await _note_batch(db, batch, entity="test", transport=transport)


async def note_fixed_job_issues_for(
    db: AsyncSession,
    job_ids: list[int],
    *,
    transport: httpx.AsyncBaseTransport | None = None,
) -> None:
    """Comment once on the linked issue of each Job whose fix appears to hold. Never raises.

    Disabled while `github_fixed_streak` is 0. `transport` exists for tests
    (httpx.MockTransport); production passes None.
    """
    if not configured() or get_settings().github_fixed_streak <= 0 or not job_ids:
        return
    candidates = await _fixed_note_job_candidates(db, job_ids)
    if not candidates:
        return
    batch = [(job, repo, _fixed_comment(job, branch, entity="job")) for job, repo, branch in candidates]
    await _note_batch(db, batch, entity="job", transport=transport)


async def on_report_processed(
    session_factory: async_sessionmaker[AsyncSession],
    outcome: ProcessOutcome,
    *,
    transport: httpx.AsyncBaseTransport | None = None,
) -> None:
    """ReportWorker hook: file issues for the Tests and Jobs a processed Report touched.

    Also notes the Tests and Jobs whose Clean streak now proves the fix held:
    filing first, so a newly-filed issue is never the one commented on.

    `transport` exists for tests (httpx.MockTransport) and for the GitHub task;
    production callers pass None.
    """
    if outcome.status != REPORT_PROCESSED or not configured():
        return
    async with session_factory() as db:
        await file_issues_for(db, outcome.touched_test_ids, transport=transport)
        await file_job_issues_for(db, outcome.touched_job_ids, transport=transport)
        await note_fixed_issues_for(db, outcome.touched_test_ids, transport=transport)
        await note_fixed_job_issues_for(db, outcome.touched_job_ids, transport=transport)
