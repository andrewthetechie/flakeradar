"""Job attribution: which failed Job executions a failing Test explains.

A failed Job execution is *explained* when a Run with the same ci_job_id, in
the same Repo, has a failed or errored Test execution. Scoring, the read API
and issue filing must agree on that rule, so it lives here once. Attribution
is recomputed on every read and never stored, because job results and JUnit
reports arrive in either order.
"""

from collections import defaultdict

from sqlalchemy import ColumnElement, Exists, exists, func, select
from sqlalchemy.ext.asyncio import AsyncSession

from .models import JOB_FAILED, Job, JobExecution, Pipeline, Project, TestExecution, TestRun
from .scoring import FAILING

CHUNK = 1000


def explained(ci_job_id: ColumnElement[str], repo_id: ColumnElement[int]) -> Exists:
    """EXISTS a failing Test execution from a Run with this ci_job_id in this Repo."""
    return exists(
        select(1)
        .select_from(TestRun)
        .join(Project, Project.id == TestRun.project_id)
        .join(TestExecution, TestExecution.test_run_id == TestRun.id)
        .where(
            TestRun.ci_job_id == ci_job_id,
            Project.repo_id == repo_id,
            TestExecution.status.in_(FAILING),
        )
    )


async def explained_ci_job_ids(db: AsyncSession, repo_id: int, ci_job_ids: list[str]) -> set[str]:
    """The subset of ci_job_ids that a failing Test execution in this Repo explains."""
    wanted = sorted({c for c in ci_job_ids if c})
    found: set[str] = set()
    for i in range(0, len(wanted), CHUNK):
        rows = await db.execute(
            select(TestRun.ci_job_id)
            .join(Project, Project.id == TestRun.project_id)
            .join(TestExecution, TestExecution.test_run_id == TestRun.id)
            .where(
                TestRun.ci_job_id.in_(wanted[i : i + CHUNK]),
                Project.repo_id == repo_id,
                TestExecution.status.in_(FAILING),
            )
        )
        found.update(rows.scalars())
    return found


async def unexplained_failure_counts(db: AsyncSession, job_ids: list[int], window: int) -> dict[int, int]:
    """Unexplained failures per Job over its newest `window` Job executions (any branch).

    Jobs with none are left out of the result.
    """
    rn = func.row_number().over(partition_by=JobExecution.job_id, order_by=JobExecution.id.desc()).label("rn")
    counts: dict[int, int] = defaultdict(int)
    for i in range(0, len(job_ids), CHUNK):
        ranked = (
            select(
                JobExecution.job_id,
                JobExecution.status,
                explained(JobExecution.ci_job_id, Pipeline.repo_id).label("explained"),
                rn,
            )
            .join(Job, Job.id == JobExecution.job_id)
            .join(Pipeline, Pipeline.id == Job.pipeline_id)
            .where(JobExecution.job_id.in_(job_ids[i : i + CHUNK]))
            .subquery()
        )
        rows = await db.execute(
            select(ranked.c.job_id).where(
                ranked.c.rn <= window, ranked.c.status == JOB_FAILED, ranked.c.explained.is_(False)
            )
        )
        for job_id in rows.scalars():
            counts[job_id] += 1
    return dict(counts)
