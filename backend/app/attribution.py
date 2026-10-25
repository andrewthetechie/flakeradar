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
from collections.abc import Iterable, Sequence

from sqlalchemy import (
    ARRAY,
    ColumnElement,
    Exists,
    Integer,
    Select,
    and_,
    bindparam,
    exists,
    func,
    or_,
    select,
    true,
    tuple_,
)
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import aliased

from .batching import chunks
from .models import JOB_FAILED, Job, JobExecution, Pipeline, Project, Repo, TestExecution, TestRun
from .scoring import FAILING

# Most explaining sibling Jobs listed per (ci_run_id, ci_run_attempt) pair.
SIBLING_MAX = 20


def _sibling_conditions(
    status: ColumnElement[str],
    summary_flag: ColumnElement[bool],
    sibling_repo_id: ColumnElement[int],
    repo_id: ColumnElement[int] | int,
) -> list[ColumnElement[bool]]:
    """What makes a Job execution an *explaining sibling*: a failed execution of a
    non-summary Job in the given Repo.

    Takes the sibling columns (aliased or plain) so it is shared by
    `summary_explained_clause`, which adds the same-attempt correlation and the
    empty-run-id guard, and `summary_explaining_siblings`, which selects the
    siblings of whole (ci_run_id, ci_run_attempt) pairs — so the rule cannot drift
    between scoring, the read API and issue filing.
    """
    return [
        status == JOB_FAILED,
        summary_flag.is_(False),
        sibling_repo_id == repo_id,
    ]


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
                *_sibling_conditions(sibling.status, sibling_job.summary, sibling_pipeline.repo_id, repo_id),
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


async def summary_explaining_siblings(
    db: AsyncSession, repo_id: int, pairs: Iterable[tuple[str, int]]
) -> dict[tuple[str, int], list[tuple[int, str, str, str]]]:
    """The failed non-summary Jobs that explain each (ci_run_id, ci_run_attempt) pair.

    The sibling half of `summary_explained_clause`, stated once here: the read API
    labels a summary Job's execution ``explained`` with that clause and lists these
    Jobs beside it, so both must answer to one rule. Pairs with an empty ci_run_id
    are dropped (see `summary_explained_clause`). One query covers every pair; each
    pair gets at most SIBLING_MAX Jobs, oldest first, as rows
    (job_id, pipeline, name, url).
    """
    wanted = sorted({pair for pair in pairs if pair[0] != ""})
    if not wanted:
        return {}
    rows = (
        await db.execute(
            select(
                JobExecution.ci_run_id,
                JobExecution.ci_run_attempt,
                Job.id,
                Pipeline.name,
                Job.name,
                JobExecution.url,
            )
            .join(Job, Job.id == JobExecution.job_id)
            .join(Pipeline, Pipeline.id == Job.pipeline_id)
            .where(
                tuple_(JobExecution.ci_run_id, JobExecution.ci_run_attempt).in_(wanted),
                *_sibling_conditions(JobExecution.status, Job.summary, Pipeline.repo_id, repo_id),
            )
            .order_by(JobExecution.id)
        )
    ).all()
    by_pair: dict[tuple[str, int], list[tuple[int, str, str, str]]] = {}
    for run_id, attempt, job_id, pipeline, name, url in rows:
        siblings = by_pair.setdefault((run_id, attempt), [])
        if len(siblings) < SIBLING_MAX:
            siblings.append((job_id, pipeline, name, url))
    return by_pair


def _window_ids(job_ids: Sequence[int], window: int, key: str, *, on_default: bool) -> Select:
    """(job_id, id) of the newest `window` executions per Job, via LATERAL ... LIMIT."""
    src = func.unnest(bindparam(key, list(job_ids), type_=ARRAY(Integer()))).table_valued("job_id").render_derived()
    lat = select(JobExecution.id).where(JobExecution.job_id == src.c.job_id)
    if on_default:
        lat = (
            lat.join(Job, Job.id == JobExecution.job_id)
            .join(Pipeline, Pipeline.id == Job.pipeline_id)
            .join(Repo, Repo.id == Pipeline.repo_id)
            .where(or_(Repo.default_branch.is_(None), JobExecution.branch == Repo.default_branch))
        )
    lat = lat.order_by(JobExecution.id.desc()).limit(window).lateral()
    return select(src.c.job_id, lat.c.id).join_from(src, lat, true())


def job_execution_window_ids(job_ids: Sequence[int], window: int, *, include_default_branch: bool = False) -> Select:
    """The newest `window` Job-execution ids per Job: unnest + LATERAL ... LIMIT.

    Each lateral stops after `window` rows per Job on ix_job_exec_job_id, so the
    cost tracks the window, not the Job's retained history. With
    `include_default_branch`, also the newest `window` per Job on its Repo's
    Default branch (a NULL Default branch matches every branch), UNION-deduped:
    the id set the old (rn_all <= window) OR (rn_def <= window) filter selected.
    Rows are (job_id, id); `job_ids` must be non-empty and within one chunk.
    """
    windowed = _window_ids(job_ids, window, key="je_window_overall", on_default=False)
    if not include_default_branch:
        return windowed
    return windowed.union(_window_ids(job_ids, window, key="je_window_default", on_default=True))


async def unexplained_failure_counts(db: AsyncSession, job_ids: list[int], window: int) -> dict[int, int]:
    """Unexplained failures per Job over its newest `window` Job executions (any branch).

    Jobs with none are left out of the result.
    """
    counts: dict[int, int] = defaultdict(int)
    for chunk in chunks(job_ids):
        windowed = job_execution_window_ids(chunk, window).subquery()
        rows = await db.execute(
            select(JobExecution.job_id)
            .join(Job, Job.id == JobExecution.job_id)
            .join(Pipeline, Pipeline.id == Job.pipeline_id)
            .where(
                JobExecution.id.in_(select(windowed.c.id)),
                JobExecution.status == JOB_FAILED,
                job_execution_explained().is_(False),
            )
        )
        for job_id in rows.scalars():
            counts[job_id] += 1
    return dict(counts)
