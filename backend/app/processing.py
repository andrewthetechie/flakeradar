"""Turn one pending Report into a Run: persist Executions, rescore Tests.

Called only by the single elected processor (ADR 0002), one Report at a time
in upload order — scoring depends on Execution order, so never parallelize.
Every statement is batched (chunks of CHUNK rows) to stay far below
asyncpg's 32,767 bind-parameter limit and avoid per-test round-trips.
"""

import asyncio
import logging
from collections import defaultdict
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from typing import Any

from sqlalchemy import ColumnElement, Row, Select, insert, select, update
from sqlalchemy.dialects.postgresql import insert as pg_insert
from sqlalchemy.exc import InterfaceError, OperationalError
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from . import report_intake, scoring
from .attribution import job_execution_explained, job_execution_window_ids, test_execution_window_ids
from .batching import chunks
from .classify import classify_failure, dominant_category
from .config import Settings, get_settings
from .models import (
    JOB_FAILED,
    JOB_SKIPPED,
    JOB_STATUSES,
    REPORT_FAILED,
    REPORT_KIND_PIPELINE,
    REPORT_PENDING,
    REPORT_PROCESSED,
    Job,
    JobExecution,
    JobScoreHistory,
    Pipeline,
    Project,
    Repo,
    Report,
    TestCase,
    TestExecution,
    TestRun,
    TestScoreHistory,
    utcnow,
)
from .notify import TierChange
from .parsing import ParsedCase, fingerprint, parse_junit_xml
from .queries import is_broken, tier_for
from .score_history import write_today_history

logger = logging.getLogger("flakeradar.processing")

ERROR_MAX = 2000


def _parse_and_classify(body: bytes, line_base: int = 1) -> list[tuple[ParsedCase, str | None]]:
    """Parse a JUnit body and classify each failing case (CPU work: runs in a thread)."""
    return [
        (pc, classify_failure(pc.message, pc.details) if pc.status in scoring.FAILING else None)
        for pc in parse_junit_xml(body, line_base)
    ]


@dataclass(frozen=True)
class ProcessOutcome:
    report_id: int
    status: str  # REPORT_PROCESSED | REPORT_FAILED | REPORT_PENDING
    run_id: int | None
    counts: dict[str, int] | None
    touched_test_ids: list[int]
    error: str | None
    touched_job_ids: list[int] = field(default_factory=list)
    # Tests and Jobs whose tier crossed the Flake threshold in this Report, in
    # the direction it crossed. Empty when the Report changed the Default branch:
    # that rescore must stay silent (ADR 0009).
    tier_changes: list[TierChange] = field(default_factory=list)
    # Retries already spent on this Report: the worker backs off before the next
    # claim, so it needs the value the requeue just bumped.
    retry_count: int = 0


async def claim_next_report(db: AsyncSession) -> Report | None:
    """Lock the oldest pending Report for this transaction (SKIP LOCKED).

    Only this query skips a locked row: an admin delete of the Report's Repo or
    Project (routers/repos.py) waits for this transaction, but the rest of it does
    not skip — the TestRun/TestCase inserts take FOR KEY SHARE on the parent row
    through their foreign keys, which conflicts with that delete's FOR UPDATE. The
    pair can therefore deadlock, and the delete side handles it with a retryable 503.
    """
    return (
        await db.execute(
            select(Report)
            .where(Report.status == REPORT_PENDING)
            .order_by(Report.id)
            .limit(1)
            .with_for_update(skip_locked=True)
        )
    ).scalar_one_or_none()


async def _upsert_test_cases(db: AsyncSession, project_id: int, parsed: list[ParsedCase]) -> dict[str, int]:
    """Insert unseen Tests; return {fingerprint: test_case_id} for all of them."""
    by_fp: dict[str, ParsedCase] = {}
    for pc in parsed:
        by_fp.setdefault(fingerprint(pc.suite, pc.classname, pc.name), pc)
    rows = [
        {"project_id": project_id, "fingerprint": fp, "suite": pc.suite, "classname": pc.classname, "name": pc.name}
        for fp, pc in by_fp.items()
    ]
    for chunk in chunks(rows):
        await db.execute(
            pg_insert(TestCase).values(chunk).on_conflict_do_nothing(constraint="uq_test_cases_project_fingerprint")
        )
    ids: dict[str, int] = {}
    for chunk in chunks(list(by_fp)):
        result = await db.execute(
            select(TestCase.fingerprint, TestCase.id).where(
                TestCase.project_id == project_id, TestCase.fingerprint.in_(chunk)
            )
        )
        ids.update(dict(result.all()))
    return ids


async def _rows_by_id(db: AsyncSession, stmt: Select, id_column: Any, ids: list[int]) -> dict[int, Row]:
    """The rows `stmt` selects for `ids` (chunked), keyed by id, in id order.

    `stmt` must select `id_column` under the name `id`. A Test or Job deleted
    before this read has no row: it names nothing, so it sends nothing.
    """
    rows: dict[int, Row] = {}
    for chunk in chunks(ids):
        rows.update({row.id: row for row in (await db.execute(stmt.where(id_column.in_(chunk)))).all()})
    return dict(sorted(rows.items()))


async def _old_scores(db: AsyncSession, model: type[TestCase] | type[Job], ids: list[int]) -> dict[int, float]:
    """The scores a rescore of `model` is about to replace, read before the UPDATE."""
    rows = await _rows_by_id(db, select(model.id, model.flakiness_score), model.id, ids)
    return {row.id: row.flakiness_score for row in rows.values()}


def _crossed(updates: list[dict[str, Any]], old: dict[int, float], threshold: float) -> list[dict[str, Any]]:
    """The updates that crossed the Flake threshold, in either direction.

    Only the flaky boundary counts. A move from Stable to Suspect (or back)
    changes the tier but crosses nothing, so it sends no Notification.
    """
    return [u for u in updates if (u["flakiness_score"] >= threshold) != (old.get(u["id"], 0.0) >= threshold)]


@dataclass(frozen=True)
class RescoreWindow:
    """What one rescore target's fetch found for one row: its Default branch and its Executions.

    `execs` holds `(commit_sha, branch, status)` triples newest first; `categories` holds the
    matching Executions' failure categories, newest first, and stays empty for a target with no
    category. A row with no Executions in the window has an empty list and no Default branch.
    """

    default_branch: str | None = None
    execs: list[tuple[str, str, str]] = field(default_factory=list)
    categories: list[str | None] = field(default_factory=list)


async def _test_window(db: AsyncSession, test_case_ids: list[int], settings: Settings) -> dict[int, RescoreWindow]:
    """The Executions `rescore` scores each Test from, with their categories and Default branch.

    The windowed read itself (`attribution.test_execution_window_ids`) and its accepted worst case
    are documented on `rescore`, the public function this fetch serves; one Test's rows come back
    newest first, in the order `scoring.branch_scoped_*` reads them.
    """
    history: dict[int, list[tuple[str, str, str]]] = defaultdict(list)  # id -> [(sha, branch, status)]
    categories: dict[int, list[str | None]] = defaultdict(list)  # id -> [category, ...] newest first
    default_branch: dict[int, str | None] = {}  # id -> branch (constant per Test/Repo)
    for chunk in chunks(test_case_ids):
        windowed = test_execution_window_ids(chunk, settings.score_window, include_default_branch=True).subquery()
        rows = await db.execute(
            select(
                TestExecution.test_case_id,
                TestRun.commit_sha,
                TestRun.branch,
                TestExecution.status,
                TestExecution.failure_category,
                Repo.default_branch,
            )
            .join(TestRun, TestRun.id == TestExecution.test_run_id)
            .join(Project, Project.id == TestRun.project_id)
            .join(Repo, Repo.id == Project.repo_id)
            .where(TestExecution.id.in_(select(windowed.c.id)))
            .order_by(TestExecution.test_case_id, TestExecution.id.desc())
        )
        for tc_id, sha, branch, status, category, def_branch in rows.all():
            history[tc_id].append((sha, branch, status))
            categories[tc_id].append(category)
            default_branch[tc_id] = def_branch
    return {
        tc_id: RescoreWindow(default_branch.get(tc_id), history[tc_id], categories[tc_id]) for tc_id in test_case_ids
    }


def _test_update_row(tc_id: int, window: RescoreWindow, settings: Settings) -> dict[str, Any]:
    """The cached columns `rescore` writes for one Test, from the window the fetch collected."""
    score, confirmed = scoring.branch_scoped_score(
        window.execs,
        window.default_branch,
        settings.score_decay,
        settings.score_window,
    )
    streak = scoring.branch_scoped_streak(window.execs, window.default_branch)
    values: dict[str, Any] = {
        "id": tc_id,
        "flakiness_score": score,
        "confirmed_flake_count": confirmed,
        "failure_category": dominant_category(window.categories[: settings.score_window]),
        "clean_streak": streak,
        "fail_streak": scoring.branch_scoped_fail_streak(window.execs, window.default_branch),
    }
    if streak == 0:
        # Issue #33: the Test failed again, so the "the fix appears to hold" note is stale
        # and the next clean streak must be able to post it again. Any other streak leaves
        # the key out of the bulk update, which is what preserves an existing stamp.
        values["github_issue_fixed_noted_at"] = None
    return values


async def _test_tier_snapshot(
    db: AsyncSession, updates: list[dict[str, Any]], old: dict[int, float], settings: Settings
) -> list[TierChange]:
    """Notifications for the Tests whose tier this rescore moves (ADR 0009).

    A Test that leaves the flaky tier because it is now broken has not
    recovered (CONTEXT.md), so it sends no event.
    """
    threshold = settings.flake_threshold
    crossed = [
        u
        for u in _crossed(updates, old, threshold)
        if tier_for(u["flakiness_score"], threshold) == "flaky"
        or not is_broken(u["fail_streak"], settings.broken_streak)
    ]
    if not crossed:
        return []
    stmt = (
        select(TestCase.id, TestCase.classname, TestCase.name, Project.name.label("project"), Repo.name.label("repo"))
        .join(Project, Project.id == TestCase.project_id)
        .join(Repo, Repo.id == Project.repo_id)
    )
    rows = await _rows_by_id(db, stmt, TestCase.id, [u["id"] for u in crossed])
    by_id = {u["id"]: u for u in crossed}
    changes: list[TierChange] = []
    for row in rows.values():
        u = by_id[row.id]
        changes.append(
            TierChange(
                kind="test",
                id=row.id,
                repo=row.repo,
                project=row.project,
                name=row.name,
                classname=row.classname,
                flakiness_score=u["flakiness_score"],
                tier=tier_for(u["flakiness_score"], threshold),
                confirmed_flake_count=u["confirmed_flake_count"],
                failure_category=u["failure_category"],
            )
        )
    return changes


@dataclass(frozen=True)
class RescoreTarget:
    """Everything the rescore engine below needs to know about one scored entity (issue #129).

    Follows `LeaderboardEntity` (queries.py): pure data plus per-entity callables, one instance per
    entity, consumed by the engine. Nothing in here may encode a shared rule — the Settings read,
    the old-score read, the chunked bulk update and the Score-history write all live in
    `_rescore_engine` only, and an intended per-entity difference (the Test's failure category,
    Fail streak and broken filter, the Job's Pipeline and absent Project, the history owner column)
    is descriptor data, as it is on TEST and JOB.
    `fetch` returns one entry per id, an empty `RescoreWindow` for a row with no Executions, which
    is what lets the engine read the update rows in the order the bulk update relies on.
    """

    model: type[TestCase] | type[Job]
    history_model: type
    owner_col: ColumnElement[int]
    fetch: Callable[[AsyncSession, list[int], Settings], Awaitable[dict[int, RescoreWindow]]]
    update_row: Callable[[int, RescoreWindow, Settings], dict[str, Any]]
    tier_snapshot: Callable[
        [AsyncSession, list[dict[str, Any]], dict[int, float], Settings], Awaitable[list[TierChange]]
    ]


TEST = RescoreTarget(
    model=TestCase,
    history_model=TestScoreHistory,
    owner_col=TestScoreHistory.test_case_id,
    fetch=_test_window,
    update_row=_test_update_row,
    # A Test that becomes broken has not recovered, so it is not a Tier change worth sending:
    # the filter is Tests-only (a Job has no Fail streak to be broken by) and lives here, as
    # descriptor data, and nowhere else in the engine.
    tier_snapshot=_test_tier_snapshot,
)


async def _rescore_engine(
    db: AsyncSession, ids: list[int], target: RescoreTarget, *, day_counts: dict[int, tuple[int, int]] | None = None
) -> list[TierChange]:
    """Recompute the cached scoring columns of `ids`, then write the day's Score history.

    The sequence every rescore shares, stated once: read Settings, fetch each row's windowed
    Executions, build one update row per id in the given order (the bulk update relies on that
    order), read the scores the UPDATE is about to replace, snapshot the Tier changes it causes,
    apply the chunked bulk update, and upsert today's Score history. `day_counts` maps an id to
    (executions, failures) added today, so the same rescore writes that day's history row.

    Returns the Tier changes for the caller to notify about. Nothing is emitted here: rescore_repo
    and the retention prune are callers that must stay silent, so they log a count and pass none on
    (ADR 0009).
    """
    settings = get_settings()
    windows = await target.fetch(db, ids, settings)
    updates = [target.update_row(row_id, windows[row_id], settings) for row_id in ids]
    changes = await target.tier_snapshot(db, updates, await _old_scores(db, target.model, ids), settings)
    for chunk in chunks(updates):
        await db.execute(update(target.model), chunk)
    if updates:
        await write_today_history(db, target.history_model, target.owner_col, updates, day_counts=day_counts)
    return changes


async def rescore(
    db: AsyncSession, test_case_ids: list[int], *, day_counts: dict[int, tuple[int, int]] | None = None
) -> list[TierChange]:
    """Recompute flakiness for the given Tests from their last `window` Executions.

    Flip scoring looks only at Executions on the Repo's Default branch, while
    Proven flakes (same-SHA flips) still count on every branch. A Repo whose
    Default branch is unknown (NULL) scores exactly as before. `day_counts`
    maps a test id to (executions, failures) added today, so each rescore also
    writes that day's Score history row. A rescore whose Clean streak comes out
    at 0 also clears `github_issue_fixed_noted_at`, so a Test that failed again
    can be noted again on GitHub when the next fix holds (issue #33).

    The Executions read per Test are the newest `window` overall plus the newest
    `window` on the Repo's Default branch (attribution.test_execution_window_ids),
    not every retained Execution. Accepted worst case: whether a Run is on the
    Default branch lives in `test_runs`/`repos`, so no index can seek Default rows
    directly — the Default-branch fetch walks the Test's Executions newest first
    and joins each one until it has `window` Default rows or runs out. A Test with
    fewer than `window` Default-branch Executions in retention (for example a Repo
    whose Default branch rarely runs CI) therefore still reads all of its
    Executions in that arm (measured before and after, both shapes:
    docs/perf/20-test-rescore-explain.md).

    Returns the Tier changes this rescore caused, for the caller to notify
    about. Nothing is emitted here: rescore_repo and the retention prune are
    callers that must stay silent, so they log a count of them and pass none on.

    The work itself is the shared `_rescore_engine` run with the TEST descriptor.
    """
    return await _rescore_engine(db, test_case_ids, TEST, day_counts=day_counts)


async def apply_default_branch(db: AsyncSession, repo_id: int, value: str | None) -> bool:
    """Store a reported Default branch on the Repo. True when it changed (None never changes it)."""
    if value is None:
        return False
    repo = (await db.execute(select(Repo).where(Repo.id == repo_id))).scalar_one_or_none()
    if repo is None or repo.default_branch == value:
        return False
    repo.default_branch = value
    await db.flush()  # the rescore in the same transaction must read the new value
    return True


async def rescore_repo(
    db: AsyncSession, repo_id: int, *, report_tier_changes: list[TierChange]
) -> tuple[list[int], list[int]]:
    """Rescore every Test and every Job in the Repo (chunked).

    Returns (test_ids, job_ids), each sorted. The Tier changes this causes are
    dropped on purpose: a Default branch change rescores the whole Repo at once,
    so notifying about it would send hundreds of events for one Report (ADR 0009).
    The caller's own rescore already ran under the new branch, so its
    `report_tier_changes` have the same cause: the caller drops them and they
    only count towards the logged total here.
    """
    test_ids = list(
        (
            await db.execute(
                select(TestCase.id).join(Project, Project.id == TestCase.project_id).where(Project.repo_id == repo_id)
            )
        ).scalars()
    )
    job_ids = list(
        (
            await db.execute(
                select(Job.id).join(Pipeline, Pipeline.id == Job.pipeline_id).where(Pipeline.repo_id == repo_id)
            )
        ).scalars()
    )
    test_ids, job_ids = sorted(test_ids), sorted(job_ids)
    dropped = report_tier_changes + await rescore(db, test_ids)
    dropped += await rescore_jobs(db, job_ids)
    if dropped:
        # One event per changed Test would flood the channel, so only the count
        # is reported (ADR 0009).
        logger.info("Rescore of Repo %s dropped %d Notification(s)", repo_id, len(dropped))
    return test_ids, job_ids


async def _job_history(
    db: AsyncSession, job_ids: list[int]
) -> dict[int, tuple[str | None, list[tuple[str, str, str]]]]:
    """Newest-first Job-execution history for each Job, plus its Repo's Default branch.

    Returns {job_id: (default_branch, [(commit_sha, branch, status), ...])}.
    """
    settings = get_settings()
    history: dict[int, list[tuple[str, str, str]]] = defaultdict(list)
    branches: dict[int, str | None] = {}
    for chunk in chunks(job_ids):
        windowed = job_execution_window_ids(chunk, settings.score_window, include_default_branch=True).subquery()
        rows = await db.execute(
            select(
                JobExecution.job_id,
                JobExecution.commit_sha,
                JobExecution.branch,
                JobExecution.status,
                job_execution_explained().label("explained"),
                Repo.default_branch,
            )
            .join(Job, Job.id == JobExecution.job_id)
            .join(Pipeline, Pipeline.id == Job.pipeline_id)
            .join(Repo, Repo.id == Pipeline.repo_id)
            .where(JobExecution.id.in_(select(windowed.c.id)))
            .order_by(JobExecution.job_id, JobExecution.id.desc())
        )
        for job_id, sha, branch, status, is_explained, def_branch in rows.all():
            if is_explained and status == JOB_FAILED:
                status = JOB_SKIPPED
            history[job_id].append((sha, branch, status))
            branches.setdefault(job_id, def_branch)
    return {jid: (branches.get(jid), history[jid]) for jid in job_ids}


async def _job_window(db: AsyncSession, job_ids: list[int], settings: Settings) -> dict[int, RescoreWindow]:
    """The Executions `rescore_jobs` scores each Job from, with its Repo's Default branch.

    The fetch is `_job_history` above, whose explained-`failed` to `JOB_SKIPPED` mapping is the
    Job's row-to-status mapping and therefore travels with this descriptor (issue #129); it stays a
    function of its own because `tests/test_attribution.py` reads it directly. A Job has no failure
    category, so its windows carry none, and `settings` goes unused here: `_job_history` reads the
    window size itself.
    """
    history = await _job_history(db, job_ids)
    windows: dict[int, RescoreWindow] = {}
    for job_id in job_ids:
        def_branch, execs = history[job_id]
        windows[job_id] = RescoreWindow(def_branch, execs)
    return windows


def _job_update_row(job_id: int, window: RescoreWindow, settings: Settings) -> dict[str, Any]:
    """The cached columns `rescore_jobs` writes for one Job, from the window the fetch collected.

    A Job has no failure category and no Fail streak column (`models.py`), so this row stops at
    `clean_streak` where a Test's row goes on.
    """
    score, confirmed = scoring.branch_scoped_score(
        window.execs,
        window.default_branch,
        settings.score_decay,
        settings.score_window,
    )
    streak = scoring.branch_scoped_streak(window.execs, window.default_branch)
    values: dict[str, Any] = {
        "id": job_id,
        "flakiness_score": score,
        "confirmed_flake_count": confirmed,
        "clean_streak": streak,
    }
    if streak == 0:
        # Issue #33: the Job failed again, so the "the fix appears to hold" note is stale — the
        # same reset `_test_update_row` states for Tests. Any other streak leaves the key out of
        # the bulk update, which is what preserves an existing stamp.
        values["github_issue_fixed_noted_at"] = None
    return values


async def _job_tier_snapshot(
    db: AsyncSession, updates: list[dict[str, Any]], old: dict[int, float], settings: Settings
) -> list[TierChange]:
    """Notifications for the Jobs whose tier this rescore moves (ADR 0009).

    Every crossing is a change worth sending: the broken-is-not-recovered filter `TEST` applies
    needs a Fail streak, which a Job does not have.
    """
    threshold = settings.flake_threshold
    crossed = _crossed(updates, old, threshold)
    if not crossed:
        return []
    stmt = (
        select(Job.id, Job.name, Pipeline.name.label("pipeline"), Repo.name.label("repo"))
        .join(Pipeline, Pipeline.id == Job.pipeline_id)
        .join(Repo, Repo.id == Pipeline.repo_id)
    )
    rows = await _rows_by_id(db, stmt, Job.id, [u["id"] for u in crossed])
    by_id = {u["id"]: u for u in crossed}
    changes: list[TierChange] = []
    for row in rows.values():
        u = by_id[row.id]
        changes.append(
            TierChange(
                kind="job",
                id=row.id,
                repo=row.repo,
                project=None,  # a Job belongs to a Repo, not a Project
                name=row.name,
                pipeline=row.pipeline,
                flakiness_score=u["flakiness_score"],
                tier=tier_for(u["flakiness_score"], threshold),
                confirmed_flake_count=u["confirmed_flake_count"],
            )
        )
    return changes


JOB = RescoreTarget(
    model=Job,
    history_model=JobScoreHistory,
    owner_col=JobScoreHistory.job_id,
    fetch=_job_window,
    update_row=_job_update_row,
    # A Job is scored with no broken filter, names no Project and no failure category, and names
    # its Pipeline instead: all of that is descriptor data, as on TEST, and nowhere else.
    tier_snapshot=_job_tier_snapshot,
)


async def rescore_jobs(
    db: AsyncSession, job_ids: list[int], *, day_counts: dict[int, tuple[int, int]] | None = None
) -> list[TierChange]:
    """Recompute flakiness for Jobs with the Default-branch rule (see rescore).

    `day_counts` maps a job id to (executions, failures) added today, so each
    rescore also writes that day's Score history row. Explained failures are
    already `JOB_SKIPPED` in `_job_history`, so they never break the streak.
    Returns the Tier changes, as `rescore` does.

    The work itself is the shared `_rescore_engine` run with the JOB descriptor.
    """
    return await _rescore_engine(db, job_ids, JOB, day_counts=day_counts)


async def _rescore_jobs_for_ci_job_id(
    db: AsyncSession, repo_id: int, ci_job_id: str
) -> tuple[list[int], list[TierChange]]:
    """Rescore every Job that carries this ci_job_id in the Repo.

    Returns (job_ids, tier_changes).
    """
    job_ids = sorted(
        set(
            (
                await db.execute(
                    select(JobExecution.job_id)
                    .join(Job, Job.id == JobExecution.job_id)
                    .join(Pipeline, Pipeline.id == Job.pipeline_id)
                    .where(JobExecution.ci_job_id == ci_job_id, Pipeline.repo_id == repo_id)
                )
            ).scalars()
        )
    )
    if job_ids:
        return job_ids, await rescore_jobs(db, job_ids)
    return job_ids, []


async def process_pipeline_report(db: AsyncSession, report: Report) -> ProcessOutcome:
    """Persist one Pipeline report as Job executions. Caller owns the transaction."""
    pipeline_report = report_intake.stored_pipeline_payload(report)
    now = utcnow()

    branch_changed = await apply_default_branch(db, report.repo_id, pipeline_report.default_branch)

    # Upsert the Pipeline, then the Jobs, then insert Job executions (dedupe by
    # ci_job_id). Only brand-new executions count toward scoring.
    await db.execute(
        pg_insert(Pipeline)
        .values(repo_id=report.repo_id, provider=pipeline_report.provider, name=pipeline_report.pipeline)
        .on_conflict_do_nothing(constraint="uq_pipelines_repo_provider_name")
    )
    pipeline_id = (
        await db.execute(
            select(Pipeline.id).where(
                Pipeline.repo_id == report.repo_id,
                Pipeline.provider == pipeline_report.provider,
                Pipeline.name == pipeline_report.pipeline,
            )
        )
    ).scalar_one()

    job_names = sorted({j.name for j in pipeline_report.jobs})
    # A single report names each Job once, so the last `summary` flag wins if a
    # caller ever repeats a name.
    summary_by_name = {j.name: j.summary for j in pipeline_report.jobs}
    flag_changed: set[int] = set()
    for chunk in chunks(job_names):
        stmt = pg_insert(Job).values(
            [{"pipeline_id": pipeline_id, "name": n, "summary": summary_by_name[n], "last_seen_at": now} for n in chunk]
        )
        # last-write-wins on `summary`, so a workflow that starts marking a summary
        # Job is picked up on the next report. RETURNING yields the rows that were
        # inserted or whose flag actually changed, so a flag flip rescores even when
        # the report added no new executions.
        changed = (
            await db.execute(
                stmt.on_conflict_do_update(
                    constraint="uq_jobs_pipeline_name",
                    set_={"summary": stmt.excluded.summary},
                    where=Job.summary.is_distinct_from(stmt.excluded.summary),
                ).returning(Job.id)
            )
        ).scalars()
        flag_changed.update(changed)
    job_ids: dict[str, int] = {}
    for chunk in chunks(job_names):
        rows = await db.execute(select(Job.name, Job.id).where(Job.pipeline_id == pipeline_id, Job.name.in_(chunk)))
        job_ids.update(dict(rows.all()))

    exec_rows = []
    for j in pipeline_report.jobs:
        exec_rows.append(
            {
                "job_id": job_ids[j.name],
                "ci_job_id": j.ci_job_id,
                "ci_run_id": pipeline_report.ci_run_id,
                "ci_run_attempt": pipeline_report.ci_run_attempt,
                "commit_sha": pipeline_report.commit_sha,
                "branch": pipeline_report.branch,
                "status": j.status,
                "url": j.url,
                "runner_name": j.runner_name,
                "runner_labels": j.runner_labels,
                "started_at": j.started_at,
                "completed_at": j.completed_at,
                "created_at": now,
            }
        )

    jobs_with_new_exec: set[int] = set()
    new_counts = dict.fromkeys(JOB_STATUSES, 0)
    day_counts: dict[int, tuple[int, int]] = {}
    for chunk in chunks(exec_rows):
        inserted = (
            await db.execute(
                pg_insert(JobExecution)
                .values(chunk)
                .on_conflict_do_nothing(constraint="uq_job_executions_job_ci_job_id")
                .returning(JobExecution.job_id, JobExecution.status)
            )
        ).all()
        for job_id, status in inserted:
            jobs_with_new_exec.add(job_id)
            new_counts[status] += 1
            n, f = day_counts.get(job_id, (0, 0))
            day_counts[job_id] = (n + 1, f + (status == JOB_FAILED))

    # Update last status for every Job that got a new execution.
    status_by_job: dict[int, str] = {}
    for j in pipeline_report.jobs:
        if job_ids[j.name] in jobs_with_new_exec:
            status_by_job[job_ids[j.name]] = j.status
    if status_by_job:
        for chunk in chunks(list(status_by_job.items())):
            await db.execute(
                update(Job),
                [{"id": jid, "last_status": s, "last_seen_at": now} for jid, s in chunk],
            )

    touched = sorted(jobs_with_new_exec | flag_changed)
    tier_changes = await rescore_jobs(db, touched, day_counts=day_counts)
    if branch_changed:
        await rescore_repo(db, report.repo_id, report_tier_changes=tier_changes)
        tier_changes = []  # whole-Repo rescore: no Notifications

    counts_out = {**new_counts, "duplicate": len(pipeline_report.jobs) - sum(new_counts.values())}
    report_intake.complete_report(report, now=now, run_id=None, counts=counts_out)
    return ProcessOutcome(
        report_id=report.id,
        status=REPORT_PROCESSED,
        run_id=None,
        counts=counts_out,
        touched_test_ids=[],
        error=None,
        touched_job_ids=touched,
        tier_changes=tier_changes,
    )


async def process_report(db: AsyncSession, report: Report) -> ProcessOutcome:
    """Persist one Report. Caller owns the transaction (no commit here)."""
    if report.kind == REPORT_KIND_PIPELINE:
        return await process_pipeline_report(db, report)
    body, line_base = report_intake.stored_junit_body(report)
    classified = await asyncio.to_thread(_parse_and_classify, body, line_base)
    parsed = [pc for pc, _ in classified]
    now = utcnow()

    # A reported Default branch changes the Repo; every Test in it is then
    # rescored because flip scoring suddenly filters by branch. Apply it before
    # inserting Executions so the rescore sees the new value.
    branch_changed = await apply_default_branch(db, report.repo_id, report.default_branch)

    if report.root is not None:
        # Write only when the value actually changes, so a run of reports with the
        # same root never rewrites (and briefly locks) the Project row.
        await db.execute(
            update(Project).where(Project.id == report.project_id, Project.root != report.root).values(root=report.root)
        )

    run = TestRun(
        project_id=report.project_id,
        commit_sha=report.commit_sha,
        branch=report.branch,
        ci_run_id=report.ci_run_id,
        created_at=now,
        ci_job_id=report.ci_job_id,
        ci_run_attempt=report.ci_run_attempt,
        pipeline=report.pipeline,
    )
    db.add(run)
    await db.flush()

    ids = await _upsert_test_cases(db, report.project_id, parsed)

    counts = {"passed": 0, "failed": 0, "error": 0, "skipped": 0}
    attempts: dict[int, int] = {}  # per Test: Executions seen so far in this report
    executions: list[dict[str, Any]] = []
    latest: dict[int, dict[str, Any]] = {}  # per Test: last occurrence in the report wins
    for pc, category in classified:
        tc_id = ids[fingerprint(pc.suite, pc.classname, pc.name)]
        attempt = attempts.get(tc_id, 0)
        attempts[tc_id] = attempt + 1
        counts[pc.status] += 1
        executions.append(
            {
                "test_case_id": tc_id,
                "test_run_id": run.id,
                "status": pc.status,
                "duration": pc.duration,
                "message": pc.message,
                "details": pc.details,
                "attempt": attempt,
                "failure_category": category,
                "created_at": now,
            }
        )
        row = latest.setdefault(tc_id, {"id": tc_id})
        row["last_status"] = pc.status
        row["last_seen_at"] = now
        if pc.file is not None:  # a report without Location never erases one
            row["file"] = pc.file
            row["line"] = pc.line

    counts["retried"] = sum(1 for n in attempts.values() if n > 1)

    for chunk in chunks(executions):
        await db.execute(insert(TestExecution), chunk)
    for chunk in chunks(list(latest.values())):
        await db.execute(update(TestCase), chunk)

    touched = sorted(latest)
    day_counts: dict[int, tuple[int, int]] = {}
    for ex in executions:
        n, f = day_counts.get(ex["test_case_id"], (0, 0))
        day_counts[ex["test_case_id"]] = (n + 1, f + (ex["status"] in scoring.FAILING))
    tier_changes = await rescore(db, touched, day_counts=day_counts)
    if branch_changed:
        await rescore_repo(db, report.repo_id, report_tier_changes=tier_changes)

    # A linked JUnit report explains (or un-explains) Job failures: rescore the
    # Jobs that carry this ci_job_id, so order of arrival does not matter.
    touched_jobs: list[int] = []
    if report.ci_job_id is not None:
        touched_jobs, job_changes = await _rescore_jobs_for_ci_job_id(db, report.repo_id, report.ci_job_id)
        tier_changes += job_changes

    report_intake.complete_report(report, now=now, run_id=run.id, counts=counts)
    return ProcessOutcome(
        report_id=report.id,
        status=REPORT_PROCESSED,
        run_id=run.id,
        counts=counts,
        touched_test_ids=touched,
        error=None,
        touched_job_ids=touched_jobs,
        tier_changes=[] if branch_changed else tier_changes,  # whole-Repo rescore: no Notifications
    )


# DBAPIError subclasses that denote permanent data/schema problems: retrying
# them can never succeed, so they must not be treated as transient.
def _is_retryable(exc: BaseException) -> bool:
    """True for failures that may succeed on a later attempt.

    Only the connection/operational DBAPI family (a dropped connection, a
    cancelled/hung statement, a deadlock, a serialization failure) is treated
    as transient. Data errors (a malformed body, schema-validation failures)
    and other DBAPI errors are permanent and are never retried, so they fail
    immediately without consuming retry budget.
    """
    return isinstance(exc, (OperationalError, InterfaceError))


async def process_next(
    session_factory: async_sessionmaker[AsyncSession],
) -> ProcessOutcome | None:
    """Claim and process the oldest pending Report. None when the queue is empty.

    Work happens in a SAVEPOINT: on any error it is rolled back and, in the
    same transaction, the Report is either re-queued for retry or permanently
    marked failed with the error — so a bad Report can never block the queue.
    Transient (DB-level) failures are re-queued up to `max_report_retries`;
    data errors fail immediately. Each retry is a fresh session/connection, so
    a genuinely transient DB blip is given another chance.
    """
    retry_limit = get_settings().max_report_retries
    async with session_factory() as db, db.begin():
        report = await claim_next_report(db)
        if report is None:
            return None
        try:
            async with db.begin_nested():
                return await process_report(db, report)
        except Exception as exc:
            error = f"{type(exc).__name__}: {exc}"[:ERROR_MAX]
            # Only transient (DB-level) failures consume retry budget; a permanent
            # data error (malformed body, validation) never does. retry_count is
            # the number of retries already used, so a report gets one initial
            # attempt plus up to `retry_limit` retries before giving up.
            if _is_retryable(exc) and (report.retry_count or 0) < retry_limit:
                report_intake.requeue_report(report, error=error)
                logger.warning(
                    "Report %s transient failure (retry %d/%d): %s",
                    report.id,
                    report.retry_count,
                    retry_limit,
                    error,
                )
                # Rolled back by the SAVEPOINT; keep it pending so the next poll
                # retries it (bounded by the cap).
                return ProcessOutcome(
                    report_id=report.id,
                    status=REPORT_PENDING,
                    run_id=None,
                    counts=None,
                    touched_test_ids=[],
                    error=error,
                    retry_count=report.retry_count,
                )
            logger.exception("Report %s failed to process", report.id)
            report_intake.fail_report(report, now=utcnow(), error=error)
            return ProcessOutcome(
                report_id=report.id, status=REPORT_FAILED, run_id=None, counts=None, touched_test_ids=[], error=error
            )
