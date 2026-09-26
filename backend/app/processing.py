"""Turn one pending Report into a Run: persist Executions, rescore Tests.

Called only by the single elected processor (ADR 0002), one Report at a time
in upload order — scoring depends on Execution order, so never parallelize.
Every statement is batched (chunks of CHUNK rows) to stay far below
asyncpg's 32,767 bind-parameter limit and avoid per-test round-trips.
"""

import asyncio
import logging
from collections import defaultdict
from dataclasses import dataclass
from typing import Any

from sqlalchemy import func, insert, or_, select, update
from sqlalchemy.dialects.postgresql import insert as pg_insert
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from . import scoring
from .config import get_settings
from .models import (
    REPORT_FAILED,
    REPORT_PENDING,
    REPORT_PROCESSED,
    Project,
    Repo,
    Report,
    TestCase,
    TestExecution,
    TestRun,
    utcnow,
)
from .parsing import ParsedCase, fingerprint, parse_junit_xml

logger = logging.getLogger("flakeradar.processing")

CHUNK = 1000
ERROR_MAX = 2000


@dataclass(frozen=True)
class ProcessOutcome:
    report_id: int
    status: str  # REPORT_PROCESSED | REPORT_FAILED
    run_id: int | None
    counts: dict[str, int] | None
    touched_test_ids: list[int]
    error: str | None


def _chunks(items: list, size: int = CHUNK):
    for i in range(0, len(items), size):
        yield items[i : i + size]


async def claim_next_report(db: AsyncSession) -> Report | None:
    """Lock the oldest pending Report for this transaction (SKIP LOCKED)."""
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
    for chunk in _chunks(rows):
        await db.execute(
            pg_insert(TestCase).values(chunk).on_conflict_do_nothing(constraint="uq_test_cases_project_fingerprint")
        )
    ids: dict[str, int] = {}
    for chunk in _chunks(list(by_fp)):
        result = await db.execute(
            select(TestCase.fingerprint, TestCase.id).where(
                TestCase.project_id == project_id, TestCase.fingerprint.in_(chunk)
            )
        )
        ids.update(dict(result.all()))
    return ids


async def rescore(db: AsyncSession, test_case_ids: list[int]) -> None:
    """Recompute flakiness for the given Tests from their last `window` Executions.

    Flip scoring looks only at Executions on the Repo's Default branch, while
    Proven flakes (same-SHA flips) still count on every branch. A Repo whose
    Default branch is unknown (NULL) scores exactly as before.
    """
    settings = get_settings()
    on_default = or_(Repo.default_branch.is_(None), TestRun.branch == Repo.default_branch)
    rn_all = (
        func.row_number()
        .over(partition_by=TestExecution.test_case_id, order_by=TestExecution.id.desc())
        .label("rn_all")
    )
    rn_def = (
        func.row_number()
        .over(partition_by=(TestExecution.test_case_id, on_default), order_by=TestExecution.id.desc())
        .label("rn_def")
    )
    history: dict[int, list[tuple[str, str, str]]] = defaultdict(list)  # id -> [(sha, branch, status)]
    default_branch: dict[int, str | None] = {}  # id -> branch (constant per Test/Repo)
    for chunk in _chunks(test_case_ids):
        ranked = (
            select(
                TestExecution.id,
                TestExecution.test_case_id,
                TestExecution.status,
                TestRun.commit_sha,
                TestRun.branch,
                Repo.default_branch,
                rn_all,
                rn_def,
            )
            .join(TestRun, TestRun.id == TestExecution.test_run_id)
            .join(Project, Project.id == TestRun.project_id)
            .join(Repo, Repo.id == Project.repo_id)
            .where(TestExecution.test_case_id.in_(chunk))
            .subquery()
        )
        rows = await db.execute(
            select(
                ranked.c.test_case_id, ranked.c.commit_sha, ranked.c.branch, ranked.c.status, ranked.c.default_branch
            )
            .where((ranked.c.rn_all <= settings.score_window) | (ranked.c.rn_def <= settings.score_window))
            .order_by(ranked.c.test_case_id, ranked.c.id.desc())
        )
        for tc_id, sha, branch, status, def_branch in rows.all():
            history[tc_id].append((sha, branch, status))
            default_branch[tc_id] = def_branch

    updates: list[dict[str, Any]] = []
    for tc_id in test_case_ids:
        execs = history.get(tc_id, [])
        score, confirmed = scoring.branch_scoped_score(
            execs,
            default_branch.get(tc_id),
            settings.score_decay,
            settings.score_window,
        )
        updates.append({"id": tc_id, "flakiness_score": score, "confirmed_flake_count": confirmed})
    for chunk in _chunks(updates):
        await db.execute(update(TestCase), chunk)


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


async def rescore_repo(db: AsyncSession, repo_id: int) -> list[int]:
    """Rescore every Test in the Repo (chunked). Returns their ids, sorted."""
    ids = list(
        (
            await db.execute(
                select(TestCase.id).join(Project, Project.id == TestCase.project_id).where(Project.repo_id == repo_id)
            )
        ).scalars()
    )
    ids = sorted(ids)
    await rescore(db, ids)
    return ids


async def process_report(db: AsyncSession, report: Report) -> ProcessOutcome:
    """Persist one Report as a Run. Caller owns the transaction (no commit here)."""
    parsed = await asyncio.to_thread(parse_junit_xml, report.body)
    now = utcnow()

    # A reported Default branch changes the Repo; every Test in it is then
    # rescored because flip scoring suddenly filters by branch. Apply it before
    # inserting Executions so the rescore sees the new value.
    branch_changed = await apply_default_branch(db, report.repo_id, report.default_branch)

    if report.root is not None:
        await db.execute(update(Project).where(Project.id == report.project_id).values(root=report.root))

    run = TestRun(
        project_id=report.project_id,
        commit_sha=report.commit_sha,
        branch=report.branch,
        ci_run_id=report.ci_run_id,
        created_at=now,
    )
    db.add(run)
    await db.flush()

    ids = await _upsert_test_cases(db, report.project_id, parsed)

    counts = {"passed": 0, "failed": 0, "error": 0, "skipped": 0}
    executions: list[dict[str, Any]] = []
    latest: dict[int, dict[str, Any]] = {}  # per Test: last occurrence in the report wins
    for pc in parsed:
        tc_id = ids[fingerprint(pc.suite, pc.classname, pc.name)]
        counts[pc.status] += 1
        executions.append(
            {
                "test_case_id": tc_id,
                "test_run_id": run.id,
                "status": pc.status,
                "duration": pc.duration,
                "message": pc.message,
                "details": pc.details,
                "created_at": now,
            }
        )
        row = latest.setdefault(tc_id, {"id": tc_id})
        row["last_status"] = pc.status
        row["last_seen_at"] = now
        if pc.file is not None:  # a report without Location never erases one
            row["file"] = pc.file
            row["line"] = pc.line

    for chunk in _chunks(executions):
        await db.execute(insert(TestExecution), chunk)
    for chunk in _chunks(list(latest.values())):
        await db.execute(update(TestCase), chunk)

    touched = sorted(latest)
    await rescore(db, touched)
    if branch_changed:
        await rescore_repo(db, report.repo_id)

    report.status = REPORT_PROCESSED
    report.counts = counts
    report.run_id = run.id
    report.error = None
    report.processed_at = now
    return ProcessOutcome(
        report_id=report.id, status=REPORT_PROCESSED, run_id=run.id, counts=counts, touched_test_ids=touched, error=None
    )


async def process_next(
    session_factory: async_sessionmaker[AsyncSession],
) -> ProcessOutcome | None:
    """Claim and process the oldest pending Report. None when the queue is empty.

    Work happens in a SAVEPOINT: on any error it is rolled back, and the
    Report (still row-locked) is marked failed with the error in the same
    transaction, so a bad Report can never block the queue.
    """
    async with session_factory() as db, db.begin():
        report = await claim_next_report(db)
        if report is None:
            return None
        try:
            async with db.begin_nested():
                return await process_report(db, report)
        except Exception as exc:
            logger.exception("Report %s failed to process", report.id)
            error = f"{type(exc).__name__}: {exc}"[:ERROR_MAX]
            report.status = REPORT_FAILED
            report.error = error
            report.processed_at = utcnow()
            return ProcessOutcome(
                report_id=report.id, status=REPORT_FAILED, run_id=None, counts=None, touched_test_ids=[], error=error
            )
