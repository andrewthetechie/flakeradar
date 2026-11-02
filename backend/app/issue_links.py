"""Issue links: record a GitHub issue filed elsewhere on a Test or Job (issue #6).

FlakeRadar's own filer (github_integration) sets github_issue_number when it
files an issue. These functions let any other filer set or clear it, through
REST and MCP. A linked entity is then treated exactly like a filed one: the
filer skips it, and sync_closed_issues clears it when the issue closes. The
issue must be in the entity's own Repo, because the sync looks it up there
(ADR 0008).
"""

import logging
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any, Literal

from sqlalchemy import Select
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


LinkRow = tuple[Any, ...]  # whatever the target's locked select returns: strings for Tests, ORM rows for Jobs


def _no_provider(row: LinkRow) -> str | None:
    """Tests have no CI provider, so the provider gate below never applies to them."""
    return None


@dataclass(frozen=True)
class IssueLinkTarget:
    """Everything the shared link/unlink engines need to know about one linkable entity.

    Follows `LeaderboardEntity` (queries.py): pure data plus per-target callables, one instance
    per target, consumed by the engines below. The rules — the 404, the provider gate, the
    number/url/conflict check and the write invariant — all live in the engines; an intended
    per-target difference is descriptor data (see `github_provider_only` on JOB). The callables
    exist because the two selects genuinely return different row shapes: `tests_select()` gives
    `(TestCase, project: str, repo: str)` and `jobs_select()` gives `(Job, Pipeline, Repo)`.
    """

    kind: LinkTarget
    locked_select: Callable[[int], Select]
    entity_of: Callable[[LinkRow], Any]  # the row that carries the (number, checked_at) pair
    repo_name_of: Callable[[LinkRow], str]
    to_out: Callable[[LinkRow, float], Any]
    github_provider_only: bool = False
    provider_of: Callable[[LinkRow], str | None] = _no_provider


def _job_provider(row: LinkRow) -> str | None:
    return row[1].provider  # the Pipeline half of the jobs_select() row


_TEST = IssueLinkTarget(
    kind="Test",
    locked_select=lambda test_id: tests_select().where(TestCase.id == test_id).with_for_update(of=TestCase),
    entity_of=lambda row: row[0],
    repo_name_of=lambda row: row[2],
    to_out=lambda row, threshold: to_test_out(row[0], row[1], row[2], threshold),
    # Tests reach their Repo through a Project, so they have no CI provider: the Job-only rule
    # in the engines is descriptor data, and it is off here.
    github_provider_only=False,
)

_JOB = IssueLinkTarget(
    kind="Job",
    locked_select=lambda job_id: jobs_select().where(Job.id == job_id).with_for_update(of=Job),
    entity_of=lambda row: row[0],
    repo_name_of=lambda row: row[2].name,
    to_out=lambda row, threshold: to_job_out(row[0], row[1], row[2], threshold),
    # Only a Job whose Pipeline talks to GitHub can hold an Issue link (ADR 0008): the sync can
    # only look the issue up in the Pipeline's provider. Same standing as `keeps_stable_rows`.
    github_provider_only=True,
    provider_of=_job_provider,
)


async def _link(
    target: IssueLinkTarget,
    db: AsyncSession,
    target_id: int,
    number: int,
    *,
    url: str | None,
    replace: bool,
    threshold: float,
) -> Any:
    """The one link engine: locked row, rules, the pair write, commit, log, out."""
    row = (await db.execute(target.locked_select(target_id))).first()
    if row is None:
        raise IssueLinkTargetNotFound(f"{target.kind} not found")
    obj = target.entity_of(row)
    repo = target.repo_name_of(row)
    if target.github_provider_only and target.provider_of(row) != "github":
        raise IssueNotLinkable("Only Jobs of GitHub Pipelines can link a GitHub issue")
    _check_link(target.kind, obj.github_issue_number, number, repo=repo, url=url, replace=replace)
    previous = obj.github_issue_number
    obj.github_issue_number = number
    if previous != number:
        obj.github_issue_checked_at = None  # a new link is checked first
    await db.commit()
    logger.info("Linked issue %s#%s to %s %s (was %s)", repo, number, target.kind.lower(), obj.id, previous)
    return target.to_out(row, threshold)


async def _unlink(target: IssueLinkTarget, db: AsyncSession, target_id: int, *, threshold: float) -> Any:
    """The one unlink engine: unlinking clears both halves of the pair."""
    row = (await db.execute(target.locked_select(target_id))).first()
    if row is None:
        raise IssueLinkTargetNotFound(f"{target.kind} not found")
    obj = target.entity_of(row)
    repo = target.repo_name_of(row)
    previous = obj.github_issue_number
    obj.github_issue_number = None
    if previous is not None:
        obj.github_issue_checked_at = None
    await db.commit()
    if previous is not None:
        logger.info("Unlinked issue %s#%s from %s %s", repo, previous, target.kind.lower(), obj.id)
    return target.to_out(row, threshold)


async def link_test_issue(
    db: AsyncSession, test_id: int, number: int, *, url: str | None = None, replace: bool = False, threshold: float
) -> schemas.TestOut:
    return await _link(_TEST, db, test_id, number, url=url, replace=replace, threshold=threshold)


async def unlink_test_issue(db: AsyncSession, test_id: int, *, threshold: float) -> schemas.TestOut:
    return await _unlink(_TEST, db, test_id, threshold=threshold)


async def link_job_issue(
    db: AsyncSession, job_id: int, number: int, *, url: str | None = None, replace: bool = False, threshold: float
) -> schemas.JobOut:
    return await _link(_JOB, db, job_id, number, url=url, replace=replace, threshold=threshold)


async def unlink_job_issue(db: AsyncSession, job_id: int, *, threshold: float) -> schemas.JobOut:
    return await _unlink(_JOB, db, job_id, threshold=threshold)
