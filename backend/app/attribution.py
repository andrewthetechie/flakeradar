"""Job attribution: which failed Job executions are explained, and by what.

A failed Job execution is *explained* when:

1. a Run with the same ci_job_id, in the same Repo, has a failed or errored
   Test execution; or
2. the Job is a **summary** Job (it runs no tests and only reports whether the
   Jobs it ``needs`` succeeded, the alls-green pattern) and another failed
   non-summary Job exists in the same CI run attempt.

Scoring, the read API and issue filing must agree on those rules, so they live
here once. Attribution is recomputed on every read and never stored, because
job results and JUnit reports arrive in either order.
"""

from collections import defaultdict

from sqlalchemy import ColumnElement, Exists, and_, exists, func, or_, select
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import aliased

from .batching import chunks
from .models import JOB_FAILED, Job, JobExecution, Pipeline, Project, TestExecution, TestRun
from .scoring import FAILING


def explained_clause(ci_job_id: ColumnElement[str], repo_id: ColumnElement[int]) -> Exists:
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


def summary_explained_clause(
    ci_run_id: ColumnElement[str],
    ci_run_attempt: ColumnElement[int],
    repo_id: ColumnElement[int],
    summary_flag: ColumnElement[bool],
) -> ColumnElement[bool]:
    """True when this execution is a summary Job whose attempt also failed a non-summary Job.

    A summary Job (alls-green) fails whenever any Job it ``needs`` fails, so on
    its own its failure says nothing about the summary Job. The sibling tables
    are aliased so this EXISTS never correlates with the Job execution, Job and
    Pipeline already joined by the caller's query.

    ``ci_run_id`` must be non-empty. A reporter that omits it would collapse
    every run into one attempt, so an unrelated older failure would explain the
    summary Job forever. Such a reporter gets no summary attribution.
    """
    sibling = aliased(JobExecution)
    sibling_job = aliased(Job)
    sibling_pipeline = aliased(Pipeline)
    return and_(
        ci_run_id != "",
        summary_flag.is_(True),
        exists(
            select(1)
            .select_from(sibling)
            .join(sibling_job, sibling_job.id == sibling.job_id)
            .join(sibling_pipeline, sibling_pipeline.id == sibling_job.pipeline_id)
            .where(
                sibling.ci_run_id == ci_run_id,
                sibling.ci_run_attempt == ci_run_attempt,
                sibling.status == JOB_FAILED,
                sibling_job.summary.is_(False),
                sibling_pipeline.repo_id == repo_id,
            )
        ),
    )


def job_failure_explained_clause(
    ci_job_id: ColumnElement[str],
    ci_run_id: ColumnElement[str],
    ci_run_attempt: ColumnElement[int],
    repo_id: ColumnElement[int],
    summary_flag: ColumnElement[bool],
) -> ColumnElement[bool]:
    """The whole attribution rule: a failing Test explains it, or a failed sibling does.

    Composing the rule here keeps scoring, the read API and issue filing from
    drifting apart. `summary_explained_clause` owns the summary half.
    """
    return or_(
        explained_clause(ci_job_id, repo_id),
        summary_explained_clause(ci_run_id, ci_run_attempt, repo_id, summary_flag),
    )


def job_execution_explained() -> ColumnElement[bool]:
    """`job_failure_explained_clause` for a query that joins JobExecution, Job and Pipeline."""
    return job_failure_explained_clause(
        JobExecution.ci_job_id,
        JobExecution.ci_run_id,
        JobExecution.ci_run_attempt,
        Pipeline.repo_id,
        Job.summary,
    )


async def explained_ci_job_ids(db: AsyncSession, repo_id: int, ci_job_ids: list[str]) -> set[str]:
    """The subset of ci_job_ids explained by a failing Test or a failed sibling Job."""
    wanted = sorted({c for c in ci_job_ids if c})
    found: set[str] = set()
    for chunk in chunks(wanted):
        rows = await db.execute(
            select(TestRun.ci_job_id)
            .join(Project, Project.id == TestRun.project_id)
            .join(TestExecution, TestExecution.test_run_id == TestRun.id)
            .where(
                TestRun.ci_job_id.in_(chunk),
                Project.repo_id == repo_id,
                TestExecution.status.in_(FAILING),
            )
        )
        found.update(rows.scalars())
    # Summary Jobs: explained by another failed non-summary Job in the same attempt.
    for chunk in chunks(wanted):
        rows = await db.execute(
            select(JobExecution.ci_job_id)
            .distinct()
            .join(Job, Job.id == JobExecution.job_id)
            .join(Pipeline, Pipeline.id == Job.pipeline_id)
            .where(
                JobExecution.ci_job_id.in_(chunk),
                JobExecution.status == JOB_FAILED,
                Pipeline.repo_id == repo_id,
                summary_explained_clause(
                    JobExecution.ci_run_id, JobExecution.ci_run_attempt, Pipeline.repo_id, Job.summary
                ),
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
    for chunk in chunks(job_ids):
        ranked = (
            select(
                JobExecution.job_id,
                JobExecution.status,
                job_execution_explained().label("explained"),
                rn,
            )
            .join(Job, Job.id == JobExecution.job_id)
            .join(Pipeline, Pipeline.id == Job.pipeline_id)
            .where(JobExecution.job_id.in_(chunk))
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
