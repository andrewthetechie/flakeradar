"""Issue links: record a GitHub issue filed elsewhere on a Test or Job (issue #6).

FlakeRadar's own filer (github_integration) sets github_issue_number when it
files an issue. These functions let any other filer set or clear it, through
REST and MCP. A linked entity is then treated exactly like a filed one: the
filer skips it, and sync_closed_issues clears it when the issue closes. The
issue must be in the entity's own Repo, because the sync looks it up there
(ADR 0008).
"""

import logging
from typing import Literal

from sqlalchemy.ext.asyncio import AsyncSession

from . import schemas
from .models import Job, TestCase
from .queries import issue_url, jobs_select, tests_select, to_job_out, to_test_out
from .schemas import MAX_ISSUE_NUMBER

logger = logging.getLogger("flakeradar.issues")

LinkTarget = Literal["Test", "Job"]


class IssueLinkError(Exception):
    """A request the Issue link rules refuse; str(exc) is the error detail."""

    status_code = 422


class IssueLinkTargetNotFound(IssueLinkError):
    status_code = 404


class IssueAlreadyLinked(IssueLinkError):
    status_code = 409


class IssueNotLinkable(IssueLinkError):
    status_code = 422


def _check_link(
    kind: LinkTarget, current: int | None, number: int, *, repo: str, url: str | None, replace: bool
) -> None:
    """The number, url and conflict rules of a link."""
    if not 1 <= number <= MAX_ISSUE_NUMBER:
        raise IssueNotLinkable(f"number must be between 1 and {MAX_ISSUE_NUMBER}")
    expected = issue_url(repo, number)
    if url is not None and url.strip().rstrip("/").lower() != expected:
        raise IssueNotLinkable(f"url must be {expected}: the issue must be in the {kind}'s own Repo")
    if current is not None and current != number and not replace:
        raise IssueAlreadyLinked(f"{kind} already links issue #{current}; pass replace=true to replace it")


async def link_test_issue(
    db: AsyncSession, test_id: int, number: int, *, url: str | None = None, replace: bool = False, threshold: float
) -> schemas.TestOut:
    row = (await db.execute(tests_select().where(TestCase.id == test_id).with_for_update(of=TestCase))).first()
    if row is None:
        raise IssueLinkTargetNotFound("Test not found")
    tc, project, repo = row
    _check_link("Test", tc.github_issue_number, number, repo=repo, url=url, replace=replace)
    previous = tc.github_issue_number
    tc.github_issue_number = number
    tc.github_issue_filed = False  # a human linked this one; FlakeRadar must not close it
    if previous != number:
        tc.github_issue_checked_at = None  # a new link is checked first
    await db.commit()
    logger.info("Linked issue %s#%s to test %s (was %s)", repo, number, tc.id, previous)
    return to_test_out(tc, project, repo, threshold)


async def unlink_test_issue(db: AsyncSession, test_id: int, *, threshold: float) -> schemas.TestOut:
    row = (await db.execute(tests_select().where(TestCase.id == test_id).with_for_update(of=TestCase))).first()
    if row is None:
        raise IssueLinkTargetNotFound("Test not found")
    tc, project, repo = row
    previous = tc.github_issue_number
    tc.github_issue_number = None
    tc.github_issue_filed = None
    if previous is not None:
        tc.github_issue_checked_at = None
    await db.commit()
    if previous is not None:
        logger.info("Unlinked issue %s#%s from test %s", repo, previous, tc.id)
    return to_test_out(tc, project, repo, threshold)


async def link_job_issue(
    db: AsyncSession, job_id: int, number: int, *, url: str | None = None, replace: bool = False, threshold: float
) -> schemas.JobOut:
    row = (await db.execute(jobs_select().where(Job.id == job_id).with_for_update(of=Job))).first()
    if row is None:
        raise IssueLinkTargetNotFound("Job not found")
    job, pipeline, repo = row
    if pipeline.provider != "github":
        raise IssueNotLinkable("Only Jobs of GitHub Pipelines can link a GitHub issue")
    _check_link("Job", job.github_issue_number, number, repo=repo.name, url=url, replace=replace)
    previous = job.github_issue_number
    job.github_issue_number = number
    job.github_issue_filed = False  # a human linked this one; FlakeRadar must not close it
    if previous != number:
        job.github_issue_checked_at = None  # a new link is checked first
    await db.commit()
    logger.info("Linked issue %s#%s to job %s (was %s)", repo.name, number, job.id, previous)
    return to_job_out(job, pipeline, repo, threshold)


async def unlink_job_issue(db: AsyncSession, job_id: int, *, threshold: float) -> schemas.JobOut:
    row = (await db.execute(jobs_select().where(Job.id == job_id).with_for_update(of=Job))).first()
    if row is None:
        raise IssueLinkTargetNotFound("Job not found")
    job, pipeline, repo = row
    previous = job.github_issue_number
    job.github_issue_number = None
    job.github_issue_filed = None
    if previous is not None:
        job.github_issue_checked_at = None
    await db.commit()
    if previous is not None:
        logger.info("Unlinked issue %s#%s from job %s", repo.name, previous, job.id)
    return to_job_out(job, pipeline, repo, threshold)
