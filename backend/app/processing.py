"""Turn one pending Report into a Run: persist Executions, rescore Tests.

Called only by the single elected processor (ADR 0002), one Report at a time
in upload order — scoring depends on Execution order, so never parallelize.
Every statement is batched (chunks of CHUNK rows) to stay far below
asyncpg's 32,767 bind-parameter limit and avoid per-test round-trips.
"""

import asyncio
import logging
from collections import defaultdict
from dataclasses import dataclass, field
from typing import Any

from sqlalchemy import func, insert, or_, select, update
from sqlalchemy.dialects.postgresql import insert as pg_insert
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from . import scoring
from .attribution import explained_clause
from .batching import chunks
from .config import get_settings
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
    Pipeline,
    Project,
    Repo,
    Report,
    TestCase,
    TestExecution,
    TestRun,
    utcnow,
)
from .parsing import ParsedCase, fingerprint, parse_junit_xml
from .schemas import PipelineReportIn

logger = logging.getLogger("flakeradar.processing")

ERROR_MAX = 2000


@dataclass(frozen=True)
class ProcessOutcome:
    report_id: int
    status: str  # REPORT_PROCESSED | REPORT_FAILED
    run_id: int | None
    counts: dict[str, int] | None
    touched_test_ids: list[int]
    error: str | None
    touched_job_ids: list[int] = field(default_factory=list)


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
    for chunk in chunks(test_case_ids):
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
    for chunk in chunks(updates):
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


async def rescore_repo(db: AsyncSession, repo_id: int) -> tuple[list[int], list[int]]:
    """Rescore every Test and every Job in the Repo (chunked).

    Returns (test_ids, job_ids), each sorted.
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
    await rescore(db, test_ids)
    await rescore_jobs(db, job_ids)
    return test_ids, job_ids


async def _job_history(
    db: AsyncSession, job_ids: list[int]
) -> dict[int, tuple[str | None, list[tuple[str, str, str]]]]:
    """Newest-first Job-execution history for each Job, plus its Repo's Default branch.

    Returns {job_id: (default_branch, [(commit_sha, branch, status), ...])}.
    """
    settings = get_settings()
    on_default = or_(Repo.default_branch.is_(None), JobExecution.branch == Repo.default_branch)
    rn_all = func.row_number().over(partition_by=JobExecution.job_id, order_by=JobExecution.id.desc()).label("rn_all")
    rn_def = (
        func.row_number()
        .over(partition_by=(JobExecution.job_id, on_default), order_by=JobExecution.id.desc())
        .label("rn_def")
    )
    history: dict[int, list[tuple[str, str, str]]] = defaultdict(list)
    branches: dict[int, str | None] = {}
    for chunk in chunks(job_ids):
        ranked = (
            select(
                JobExecution.id,
                JobExecution.job_id,
                JobExecution.commit_sha,
                JobExecution.branch,
                JobExecution.status,
                explained_clause(JobExecution.ci_job_id, Pipeline.repo_id).label("explained"),
                Repo.default_branch,
                rn_all,
                rn_def,
            )
            .join(Job, Job.id == JobExecution.job_id)
            .join(Pipeline, Pipeline.id == Job.pipeline_id)
            .join(Repo, Repo.id == Pipeline.repo_id)
            .where(JobExecution.job_id.in_(chunk))
            .subquery()
        )
        rows = await db.execute(
            select(
                ranked.c.job_id,
                ranked.c.commit_sha,
                ranked.c.branch,
                ranked.c.status,
                ranked.c.explained,
                ranked.c.default_branch,
            )
            .where((ranked.c.rn_all <= settings.score_window) | (ranked.c.rn_def <= settings.score_window))
            .order_by(ranked.c.job_id, ranked.c.id.desc())
        )
        for job_id, sha, branch, status, is_explained, def_branch in rows.all():
            if is_explained and status == JOB_FAILED:
                status = JOB_SKIPPED
            history[job_id].append((sha, branch, status))
            branches.setdefault(job_id, def_branch)
    return {jid: (branches.get(jid), history[jid]) for jid in job_ids}


async def rescore_jobs(db: AsyncSession, job_ids: list[int]) -> None:
    """Recompute flakiness for Jobs with the Default-branch rule (see rescore)."""
    settings = get_settings()
    history = await _job_history(db, job_ids)
    updates: list[dict[str, Any]] = []
    for job_id in job_ids:
        def_branch, execs = history.get(job_id, (None, []))
        score, confirmed = scoring.branch_scoped_score(
            execs,
            def_branch,
            settings.score_decay,
            settings.score_window,
        )
        updates.append({"id": job_id, "flakiness_score": score, "confirmed_flake_count": confirmed})
    for chunk in chunks(updates):
        await db.execute(update(Job), chunk)


async def _rescore_jobs_for_ci_job_id(db: AsyncSession, repo_id: int, ci_job_id: str) -> list[int]:
    """Rescore every Job that carries this ci_job_id in the Repo; return their ids."""
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
        await rescore_jobs(db, job_ids)
    return job_ids


async def process_pipeline_report(db: AsyncSession, report: Report) -> ProcessOutcome:
    """Persist one Pipeline report as Job executions. Caller owns the transaction."""
    pipeline_report = PipelineReportIn.model_validate_json(report.body)
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
    for chunk in chunks(job_names):
        await db.execute(
            pg_insert(Job)
            .values([{"pipeline_id": pipeline_id, "name": n, "last_seen_at": now} for n in chunk])
            .on_conflict_do_nothing(constraint="uq_jobs_pipeline_name")
        )
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

    touched = sorted(jobs_with_new_exec)
    await rescore_jobs(db, touched)
    if branch_changed:
        await rescore_repo(db, report.repo_id)

    report.status = REPORT_PROCESSED
    report.counts = {**new_counts, "duplicate": len(pipeline_report.jobs) - sum(new_counts.values())}
    report.run_id = None
    report.error = None
    report.processed_at = now
    return ProcessOutcome(
        report_id=report.id,
        status=REPORT_PROCESSED,
        run_id=None,
        counts=report.counts,
        touched_test_ids=[],
        error=None,
        touched_job_ids=touched,
    )


async def process_report(db: AsyncSession, report: Report) -> ProcessOutcome:
    """Persist one Report. Caller owns the transaction (no commit here)."""
    if report.kind == REPORT_KIND_PIPELINE:
        return await process_pipeline_report(db, report)
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
    for pc in parsed:
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
    await rescore(db, touched)
    if branch_changed:
        await rescore_repo(db, report.repo_id)

    # A linked JUnit report explains (or un-explains) Job failures: rescore the
    # Jobs that carry this ci_job_id, so order of arrival does not matter.
    touched_jobs: list[int] = []
    if report.ci_job_id is not None:
        touched_jobs = await _rescore_jobs_for_ci_job_id(db, report.repo_id, report.ci_job_id)

    report.status = REPORT_PROCESSED
    report.counts = counts
    report.run_id = run.id
    report.error = None
    report.processed_at = now
    return ProcessOutcome(
        report_id=report.id,
        status=REPORT_PROCESSED,
        run_id=run.id,
        counts=counts,
        touched_test_ids=touched,
        error=None,
        touched_job_ids=touched_jobs,
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
