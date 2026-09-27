"""Repo / Project / Project-root naming rules and get-or-create.

A Repo is a lowercase ``owner/name``; a Project is a lowercase name unique
within its Repo (``default`` when omitted). See CONTEXT.md and ADR 0001.
"""

import re

from sqlalchemy import select
from sqlalchemy.dialects.postgresql import insert as pg_insert
from sqlalchemy.ext.asyncio import AsyncSession

from .models import Project, Repo

REPO_RE = re.compile(r"^[a-z0-9_.-]+/[a-z0-9_.-]+$")
PROJECT_RE = re.compile(r"^[a-z0-9_.-]{1,100}$")
PROVIDER_RE = re.compile(r"^[a-z0-9_-]{1,32}$")
DEFAULT_PROJECT = "default"


def normalize_repo(raw: str) -> str:
    value = raw.strip().lower()
    if len(value) > 255 or not REPO_RE.match(value):
        raise ValueError(f"repo must look like 'owner/name', got {raw!r}")
    return value


def normalize_project(raw: str | None) -> str:
    value = (raw or "").strip().lower() or DEFAULT_PROJECT
    if not PROJECT_RE.match(value):
        raise ValueError(f"project must be 1-100 chars of a-z, 0-9, '.', '_' or '-', got {raw!r}")
    return value


def normalize_root(raw: str | None) -> str | None:
    """None = not sent (leave the Project's root alone); "" = repo root."""
    if raw is None:
        return None
    value = raw.strip()
    while value.startswith("./"):
        value = value[2:]
    value = value.strip("/")
    if ".." in value.split("/"):
        raise ValueError(f"root must not contain '..', got {raw!r}")
    return value


def normalize_provider(raw: str) -> str:
    value = raw.strip().lower()
    if not PROVIDER_RE.match(value):
        raise ValueError(f"provider must be 1-32 chars of a-z, 0-9, '_' or '-', got {raw!r}")
    return value


async def get_or_create_repo(db: AsyncSession, repo: str) -> Repo:
    """Race-safe get-or-create for a normalized Repo name.

    SELECT first so the hot path (the Repo already exists) is a pure read: it
    never takes a write lock or waits on a concurrent writer's lock on the row.
    The INSERT fallback is still race-safe under two concurrent creates (the
    loser's ON CONFLICT DO NOTHING no-ops, then both re-SELECT the row).
    """
    existing = (await db.execute(select(Repo).where(Repo.name == repo))).scalar_one_or_none()
    if existing is not None:
        return existing
    await db.execute(pg_insert(Repo).values(name=repo).on_conflict_do_nothing(index_elements=["name"]))
    return (await db.execute(select(Repo).where(Repo.name == repo))).scalar_one()


async def get_or_create_project(db: AsyncSession, repo: str, project: str) -> Project:
    """Race-safe get-or-create for normalized names.

    SELECT first (see get_or_create_repo): ingest reads the existing Project
    without taking a write lock, so it never waits on another session's lock
    on the Project row (e.g. the worker rewriting projects.root).
    """
    repo_row = await get_or_create_repo(db, repo)
    existing = (
        await db.execute(select(Project).where(Project.repo_id == repo_row.id, Project.name == project))
    ).scalar_one_or_none()
    if existing is not None:
        return existing
    await db.execute(
        pg_insert(Project)
        .values(repo_id=repo_row.id, name=project)
        .on_conflict_do_nothing(constraint="uq_projects_repo_name")
    )
    return (
        await db.execute(select(Project).where(Project.repo_id == repo_row.id, Project.name == project))
    ).scalar_one()
