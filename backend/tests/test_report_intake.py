"""Report intake: the per-kind pending-row builders and the JUnit duplicate identity.

The builders return un-added rows, so the builder cases need no session beyond the
Project. The duplicate cases run against the shared Postgres `db` fixture, seeding
the stored row exactly the way ingest would queue it.
"""

import hashlib

import pytest
from app.models import Report
from app.report_intake import (
    body_digest,
    build_junit_report,
    build_pipeline_report,
    find_junit_duplicate,
    pipeline_body,
)
from app.schemas import PipelineReportIn
from sqlalchemy import select
from sqlalchemy.orm import object_session

from tests.conftest import make_junit
from tests.factories import make_project

_PIPELINE_JSON = {
    "repo": "acme/app",
    "provider": "github",
    "pipeline": ".github/workflows/ci.yml",
    "commit_sha": "sha1",
    "branch": "main",
    "default_branch": "main",
    "ci_run_id": "1",
    "ci_run_attempt": 1,
    "jobs": [{"ci_job_id": "1", "name": "test", "status": "passed"}],
}

_BODY = make_junit([("t1", "failed"), ("t2", "passed")])
_IDENTITY = {
    "commit_sha": "sha1",
    "branch": "main",
    "ci_run_id": "7",
    "root": "pkg",
    "line_base": 1,
    "ci_job_id": "J1",
    "ci_run_attempt": 2,
    "pipeline": ".github/workflows/ci.yml",
    "default_branch": "main",
}


async def _seed(db, row) -> None:
    db.add(row)
    await db.flush()


async def _report_ids(db) -> list[int]:
    """Ids of every stored Report, re-read from the DB (the session's identity
    map would otherwise hand back the instances the test already built)."""
    rows = await db.execute(select(Report.id).order_by(Report.id))
    return list(rows.scalars().all())


def test_body_digest_is_the_sha256_of_the_bytes():
    assert body_digest(_BODY) == hashlib.sha256(_BODY).hexdigest()


async def test_build_junit_report_carries_the_body_and_its_digest(db):
    proj = await make_project(db)
    row = build_junit_report(project=proj, body=_BODY, **_IDENTITY)

    assert row.kind == "junit"
    assert (row.project_id, row.repo_id) == (proj.id, proj.repo_id)
    assert row.body == _BODY
    assert row.body_sha256 == hashlib.sha256(_BODY).hexdigest()
    assert row.status == "pending"
    for field, value in _IDENTITY.items():
        assert getattr(row, field) == value
    # Built, not written: the caller owns add/commit.
    assert object_session(row) is None
    assert await _report_ids(db) == []


async def test_build_junit_report_defaults(db):
    proj = await make_project(db)
    row = build_junit_report(project=proj, body=_BODY, commit_sha="sha1")

    assert (row.branch, row.ci_run_id, row.status) == ("main", "", "pending")
    assert (row.root, row.line_base, row.ci_job_id, row.ci_run_attempt) == (None, None, None, None)
    assert row.body_sha256 == body_digest(_BODY)


async def test_build_pipeline_report_has_no_project_and_no_digest(db):
    proj = await make_project(db)
    body = pipeline_body(PipelineReportIn(**_PIPELINE_JSON))
    row = build_pipeline_report(
        repo_id=proj.repo_id,
        body=body,
        commit_sha="sha1",
        ci_run_id="1",
        ci_run_attempt=1,
        pipeline=_PIPELINE_JSON["pipeline"],
        default_branch="main",
    )

    assert row.kind == "pipeline"
    assert row.project_id is None
    assert row.repo_id == proj.repo_id
    # Only the JUnit duplicate check reads a digest; Pipeline rows never get one.
    assert row.body_sha256 is None
    assert row.body == body
    assert row.status == "pending"
    assert object_session(row) is None
    assert await _report_ids(db) == []


def test_pipeline_body_is_the_canonical_json_of_the_payload():
    payload = PipelineReportIn(**_PIPELINE_JSON)
    body = pipeline_body(payload)

    assert body == payload.model_dump_json().encode()
    assert PipelineReportIn.model_validate_json(body) == payload


async def test_find_junit_duplicate_returns_the_stored_report(db):
    proj = await make_project(db)
    stored = build_junit_report(project=proj, body=_BODY, **_IDENTITY)
    await _seed(db, stored)
    candidate = build_junit_report(project=proj, body=_BODY, **_IDENTITY)

    assert await find_junit_duplicate(db, candidate) is stored
    # The lookup reads only: no second row, and the candidate stays unwritten.
    assert object_session(candidate) is None
    assert await _report_ids(db) == [stored.id]


async def test_find_junit_duplicate_finds_nothing_when_the_table_is_empty(db):
    proj = await make_project(db)
    candidate = build_junit_report(project=proj, body=_BODY, **_IDENTITY)

    assert await find_junit_duplicate(db, candidate) is None


@pytest.mark.parametrize(
    "field, value",
    [
        ("commit_sha", "other-sha"),
        ("ci_run_id", "8"),
        ("ci_run_attempt", 3),
        ("ci_job_id", "J2"),
        ("line_base", 0),
    ],
)
async def test_find_junit_duplicate_keys_on_each_identity_column(db, field, value):
    proj = await make_project(db)
    stored = build_junit_report(project=proj, body=_BODY, **_IDENTITY)
    await _seed(db, stored)
    candidate = build_junit_report(project=proj, body=_BODY, **{**_IDENTITY, field: value})

    assert await find_junit_duplicate(db, candidate) is None


async def test_find_junit_duplicate_keys_on_the_project(db):
    proj = await make_project(db)
    other = await make_project(db, project="other")
    stored = build_junit_report(project=proj, body=_BODY, **_IDENTITY)
    await _seed(db, stored)
    candidate = build_junit_report(project=other, body=_BODY, **_IDENTITY)

    assert await find_junit_duplicate(db, candidate) is None


async def test_find_junit_duplicate_keys_on_the_body_bytes(db):
    proj = await make_project(db)
    stored = build_junit_report(project=proj, body=_BODY, **_IDENTITY)
    await _seed(db, stored)
    candidate = build_junit_report(project=proj, body=make_junit([("t1", "passed")]), **_IDENTITY)

    assert await find_junit_duplicate(db, candidate) is None


async def test_stored_null_line_base_reads_as_one(db):
    proj = await make_project(db)
    stored = build_junit_report(project=proj, body=_BODY, **{**_IDENTITY, "line_base": None})
    await _seed(db, stored)

    assert await find_junit_duplicate(db, build_junit_report(project=proj, body=_BODY, **_IDENTITY)) is stored
    assert (
        await find_junit_duplicate(db, build_junit_report(project=proj, body=_BODY, **{**_IDENTITY, "line_base": 0}))
        is None
    )


async def test_find_junit_duplicate_returns_the_newest_stored_report(db):
    proj = await make_project(db)
    older = build_junit_report(project=proj, body=_BODY, **_IDENTITY)
    await _seed(db, older)
    newer = build_junit_report(project=proj, body=_BODY, **_IDENTITY)
    await _seed(db, newer)
    assert newer.id > older.id

    assert await find_junit_duplicate(db, build_junit_report(project=proj, body=_BODY, **_IDENTITY)) is newer
