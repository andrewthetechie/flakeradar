"""Report intake (ADR 0002, accept-then-process): building the pending row.

Free functions over `models`/`schemas`, in the shape of `score_history.py`: this
module owns how a pending Report row looks per kind (body and its digest), and
the JUnit duplicate identity. It never adds, flushes or commits — the caller
owns the transaction.
"""

import hashlib

from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession

from .models import (
    REPORT_KIND_JUNIT,
    REPORT_KIND_PIPELINE,
    REPORT_PENDING,
    Project,
    Report,
)
from .schemas import PipelineReportIn


def body_digest(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def pipeline_body(payload: PipelineReportIn) -> bytes:
    """The stored body of a Pipeline Report: the validated payload, canonically
    serialized. PipelineReportIn already normalized it on the way in."""
    return payload.model_dump_json().encode()


def build_junit_report(
    *,
    project: Project,
    body: bytes,
    commit_sha: str,
    branch: str = "main",
    ci_run_id: str = "",
    root: str | None = None,
    line_base: int | None = None,
    ci_job_id: str | None = None,
    ci_run_attempt: int | None = None,
    pipeline: str | None = None,
    default_branch: str | None = None,
    status: str = REPORT_PENDING,
) -> Report:
    """An un-added pending JUnit Report for `project`, carrying the raw XML and
    its digest. The digest is computed here so the duplicate check and the row
    can never disagree."""
    return Report(
        kind=REPORT_KIND_JUNIT,
        project_id=project.id,
        repo_id=project.repo_id,
        commit_sha=commit_sha,
        branch=branch,
        ci_run_id=ci_run_id,
        root=root,
        line_base=line_base,
        body=body,
        body_sha256=body_digest(body),
        status=status,
        default_branch=default_branch,
        ci_job_id=ci_job_id,
        ci_run_attempt=ci_run_attempt,
        pipeline=pipeline,
    )


def build_pipeline_report(
    *,
    repo_id: int,
    body: bytes,
    commit_sha: str,
    branch: str = "main",
    ci_run_id: str = "",
    ci_run_attempt: int | None = None,
    pipeline: str | None = None,
    default_branch: str | None = None,
    status: str = REPORT_PENDING,
) -> Report:
    """An un-added pending Pipeline Report for a Repo: no Project (a Pipeline
    run is not tied to one) and no body digest (only JUnit rows are
    duplicate-checked, so no Pipeline row needs one)."""
    return Report(
        kind=REPORT_KIND_PIPELINE,
        repo_id=repo_id,
        project_id=None,
        commit_sha=commit_sha,
        branch=branch,
        ci_run_id=ci_run_id,
        ci_run_attempt=ci_run_attempt,
        pipeline=pipeline,
        default_branch=default_branch,
        body=body,
        status=status,
    )


async def find_junit_duplicate(db: AsyncSession, candidate: Report) -> Report | None:
    """The stored JUnit Report with the same identity as `candidate`, if any.

    `candidate` is the un-added row, so the body is digested once. A CI retry
    whose response was lost uploads the same bytes again; identical bytes under
    the same identity mean that retry. Best effort — no unique constraint, so
    two parallel identical uploads can both insert.
    """
    return (
        await db.execute(
            select(Report)
            .where(
                Report.kind == REPORT_KIND_JUNIT,
                Report.project_id == candidate.project_id,
                Report.commit_sha == candidate.commit_sha,
                Report.ci_run_id == candidate.ci_run_id,
                Report.ci_run_attempt == candidate.ci_run_attempt,
                Report.ci_job_id == candidate.ci_job_id,
                # A retry repeats this Report's own line_base, so an upload with a
                # different one is a corrected re-upload and must be queued. A
                # stored NULL predates the parameter and is read as 1 (models.Report).
                func.coalesce(Report.line_base, 1) == candidate.line_base,
                Report.body_sha256 == candidate.body_sha256,
            )
            .order_by(Report.id.desc())
            .limit(1)
        )
    ).scalar_one_or_none()
