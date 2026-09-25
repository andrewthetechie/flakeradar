"""Repo/Project/root naming rules and race-safe get-or-create."""

import pytest
from app.identity import (
    get_or_create_project,
    normalize_project,
    normalize_repo,
    normalize_root,
)
from app.models import Project, Repo
from sqlalchemy import func, select


def test_normalize_repo():
    assert normalize_repo("  AndrewTheTechie/Writers-App ") == "andrewthetechie/writers-app"
    for bad in ["", "writers-app", "a/b/c", "a b/c", "owner/"]:
        with pytest.raises(ValueError):
            normalize_repo(bad)


def test_normalize_project():
    assert normalize_project(None) == "default"
    assert normalize_project("") == "default"
    assert normalize_project(" Backend ") == "backend"
    for bad in ["front end", "a/b", "x" * 101]:
        with pytest.raises(ValueError):
            normalize_project(bad)


def test_normalize_root():
    assert normalize_root(None) is None
    assert normalize_root("") == ""
    assert normalize_root("./frontend/") == "frontend"
    assert normalize_root("/apps/web") == "apps/web"
    with pytest.raises(ValueError):
        normalize_root("../secrets")


async def test_get_or_create_project_is_idempotent(db):
    a = await get_or_create_project(db, "acme/app", "backend")
    b = await get_or_create_project(db, "acme/app", "backend")
    c = await get_or_create_project(db, "other/app", "backend")
    await db.commit()
    assert a.id == b.id != c.id
    assert (await db.execute(select(func.count(Repo.id)))).scalar() == 2
    assert (await db.execute(select(func.count(Project.id)))).scalar() == 2
