"""Issue links: record a GitHub issue filed elsewhere on a Test or Job (issue #6).

This module owns the (github_issue_number, github_issue_checked_at) pair for both
targets: REST and MCP link and unlink it, FlakeRadar's own filer (github_integration)
stores a number it just filed through `store_filed_link`, and `sync_closed_issues`
takes its rotation list from `linked_for_check`. A linked entity is treated exactly
like a filed one: the filer skips it, and the sync clears it when the issue closes.
The issue must be in the entity's own Repo, because the sync looks it up there
(ADR 0008).
"""

import logging
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any, Literal

from fastapi import HTTPException
from sqlalchemy import Select, select
from sqlalchemy.ext.asyncio import AsyncSession

from . import schemas
from .models import Job, Pipeline, Project, Repo, TestCase
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
    """Everything the shared link, unlink and rotation engines need to know about one linkable entity.

    Follows `LeaderboardEntity` (queries.py): pure data plus per-target callables, one instance
    per target, consumed by the engines below. The rules — the 404, the provider gate, the
    number/url/conflict check and the write invariant — all live in the engines; an intended
    per-target difference is descriptor data (see `github_provider_only` on JOB). The callables
    exist because the two selects genuinely return different row shapes: `tests_select()` gives
    `(TestCase, project: str, repo: str)` and `jobs_select()` gives `(Job, Pipeline, Repo)`.
    `linked_select` exists for the rotation read, whose row shape is the same for both targets.
    """

    kind: LinkTarget
    locked_select: Callable[[int], Select]
    entity_of: Callable[[LinkRow], Any]  # the row that carries the (number, checked_at) pair
    repo_name_of: Callable[[LinkRow], str]
    to_out: Callable[[LinkRow, float], Any]
    linked_select: Callable[[], Select]  # (linked row, Repo.name) over this target's path to its Repo
    model: type  # the linked model: the columns the rotation rule below filters and orders
    github_provider_only: bool = False
    provider_of: Callable[[LinkRow], str | None] = _no_provider


def _job_provider(row: LinkRow) -> str | None:
    return row[1].provider  # the Pipeline half of the jobs_select() row


def _test_linked_select() -> Select:
    """SELECT TestCase, Repo.name — the shape `sync_closed_issues` mutates and logs."""
    return (
        select(TestCase, Repo.name)
        .join(Project, TestCase.project_id == Project.id)
        .join(Repo, Project.repo_id == Repo.id)
    )


def _job_linked_select() -> Select:
    """SELECT Job, Repo.name — the same shape, over the Pipeline's path to its Repo."""
    return select(Job, Repo.name).join(Pipeline, Job.pipeline_id == Pipeline.id).join(Repo, Pipeline.repo_id == Repo.id)


TEST = IssueLinkTarget(
    kind="Test",
    locked_select=lambda test_id: tests_select().where(TestCase.id == test_id).with_for_update(of=TestCase),
    entity_of=lambda row: row[0],
    repo_name_of=lambda row: row[2],
    to_out=lambda row, threshold: to_test_out(row[0], row[1], row[2], threshold),
    linked_select=_test_linked_select,
    model=TestCase,
    # Tests reach their Repo through a Project, so they have no CI provider: the Job-only rule
    # in the engines is descriptor data, and it is off here.
    github_provider_only=False,
)

JOB = IssueLinkTarget(
    kind="Job",
    locked_select=lambda job_id: jobs_select().where(Job.id == job_id).with_for_update(of=Job),
    entity_of=lambda row: row[0],
    repo_name_of=lambda row: row[2].name,
    to_out=lambda row, threshold: to_job_out(row[0], row[1], row[2], threshold),
    linked_select=_job_linked_select,
    model=Job,
    # Only a Job whose Pipeline talks to GitHub can hold an Issue link (ADR 0008): the sync can
    # only look the issue up in the Pipeline's provider. Same standing as `keeps_stable_rows`.
    github_provider_only=True,
    provider_of=_job_provider,
)


def store_filed_link(obj: Job | TestCase, number: int) -> None:
    """Write a brand-new link: the one place the pair's invariant is stated.

    A new link is checked first, so the check stamp is cleared and the row goes to the
    front of `linked_for_check`. Both writers of a new link call this — the filer storing
    the number GitHub just created (`github_integration._file_batch`, which commits per
    issue so a later failure cannot undo the links before it) and `_link` below storing a
    number someone else filed, only when it differs from the number it replaces.
    Deliberately *not* the sync's re-arm, which stamps `utcnow()` and clears only the
    number (`github_integration._clear_if_closed`) because the issue it just read really
    was checked.
    """
    obj.github_issue_number = number
    obj.github_issue_checked_at = None  # a new link is checked first


def clear_link(obj: Job | TestCase) -> None:
    """Remove a link: the other write of the pair, the counterpart of `store_filed_link`.

    Unlink clears BOTH halves, unconditionally: with no number there is nothing for
    `linked_for_check` to look up, so a stamp left on an unlinked row only misleads. Keying this
    write on the previous number would be wrong — the sync's re-arm (`_clear_if_closed`) clears
    the number while deliberately keeping its `utcnow()` stamp, so a row can reach unlink with
    a number that is already None and a stamp that is not.
    """
    obj.github_issue_number = None
    obj.github_issue_checked_at = None


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
    if previous != number:
        store_filed_link(obj, number)  # the same number keeps its check stamp: it was just checked
    await db.commit()
    logger.info("Linked issue %s#%s to %s %s (was %s)", repo, number, target.kind.lower(), obj.id, previous)
    return target.to_out(row, threshold)


async def _unlink(target: IssueLinkTarget, db: AsyncSession, target_id: int, *, threshold: float) -> Any:
    """The one unlink engine: locked row, `clear_link`, commit, log, out."""
    row = (await db.execute(target.locked_select(target_id))).first()
    if row is None:
        raise IssueLinkTargetNotFound(f"{target.kind} not found")
    obj = target.entity_of(row)
    repo = target.repo_name_of(row)
    previous = obj.github_issue_number
    clear_link(obj)  # both halves, whatever they held: see `clear_link`
    await db.commit()
    if previous is not None:
        logger.info("Unlinked issue %s#%s from %s %s", repo, previous, target.kind.lower(), obj.id)
    return target.to_out(row, threshold)


async def link_test_issue(
    db: AsyncSession, test_id: int, number: int, *, url: str | None = None, replace: bool = False, threshold: float
) -> schemas.TestOut:
    return await _link(TEST, db, test_id, number, url=url, replace=replace, threshold=threshold)


async def unlink_test_issue(db: AsyncSession, test_id: int, *, threshold: float) -> schemas.TestOut:
    return await _unlink(TEST, db, test_id, threshold=threshold)


async def link_job_issue(
    db: AsyncSession, job_id: int, number: int, *, url: str | None = None, replace: bool = False, threshold: float
) -> schemas.JobOut:
    return await _link(JOB, db, job_id, number, url=url, replace=replace, threshold=threshold)


async def unlink_job_issue(db: AsyncSession, job_id: int, *, threshold: float) -> schemas.JobOut:
    return await _unlink(JOB, db, job_id, threshold=threshold)


_TARGETS: dict[LinkTarget, IssueLinkTarget] = {TEST.kind: TEST, JOB.kind: JOB}


async def link_issue(
    db: AsyncSession,
    target: LinkTarget,
    entity_id: int,
    number: int,
    *,
    url: str | None = None,
    replace: bool = False,
    threshold: float,
) -> schemas.TestOut | schemas.JobOut:
    """Link an Issue to the entity `entity_id` of `target`: the front doors' entry point.

    A caller that knows only a target noun and an id — a REST route body, an MCP tool —
    calls this instead of picking between the per-target functions above, so no front door
    repeats the Test-vs-Job dispatch. The per-target functions stay for callers that are
    already per-target and want the precise return type.
    """
    return await _link(_TARGETS[target], db, entity_id, number, url=url, replace=replace, threshold=threshold)


async def unlink_issue(
    db: AsyncSession, target: LinkTarget, entity_id: int, *, threshold: float
) -> schemas.TestOut | schemas.JobOut:
    """Unlink the Issue of the entity `entity_id` of `target`. See `link_issue`.

    Unlinking clears both halves of the pair (see `clear_link`), which is what makes it distinct
    from the sync's re-arm: that one stamps `utcnow()` and clears only the number, because the
    issue it just read really was checked.
    """
    return await _unlink(_TARGETS[target], db, entity_id, threshold=threshold)


def http_error(exc: IssueLinkError) -> HTTPException:
    """The one mapping from a refused Issue-link operation to an HTTP response.

    Routers keep the exception handling local to their own body — `except
    issue_links.IssueLinkError as exc: raise issue_links.http_error(exc) from exc` — so the
    status code travels with the error class instead of being re-stated per route.
    """
    return HTTPException(status_code=exc.status_code, detail=str(exc))


async def linked_for_check(db: AsyncSession, target: IssueLinkTarget, batch: int) -> list[tuple[Any, str]]:
    """This target's linked rows in check order, with their Repo names: the sync's rotation list.

    One statement of the rule the two partial indexes also encode — `ix_test_cases_issue_check`
    and `ix_jobs_issue_check` (models.py) index `github_issue_checked_at NULLS FIRST, id` where
    `github_issue_number IS NOT NULL` — so the rows are exactly what those indexes order: never
    checked (or just re-linked, see `store_filed_link`) first, then least recently checked. The
    entities are the ORM rows themselves, which `sync_closed_issues` stamps and clears before
    committing once; no row lock, because the sync is leader-only maintenance.
    """
    model = target.model
    stmt = (
        target.linked_select()
        .where(model.github_issue_number.is_not(None))
        .order_by(model.github_issue_checked_at.asc().nulls_first(), model.id)
        .limit(batch)
    )
    return [(entity, repo) for entity, repo in (await db.execute(stmt)).all()]
