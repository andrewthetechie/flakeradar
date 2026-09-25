# 04 — Report processor

## Tracer-Bullet Outcome
Calling `await process_next(session_factory)` takes the oldest `pending` Report and turns it into a Run in one transaction. It creates any new Tests, records every Execution with its Failure message and details, and updates each Test's last status and Location. It also rescores every touched Test and marks the Report `processed` with its counts. A Report that cannot be processed is marked `failed` with its error, and the queue moves on. A 2,500-test Report is processed in well under a second, with batched statements.

Nothing calls `process_next` automatically yet. That is task 05.

## User Story
As the maintainer, I want each queued Report processed correctly and efficiently so that scores reflect every CI run and one bad upload never blocks the queue.

## Description
Create `backend/app/processing.py`. It contains the persistence and rescoring half of the deleted `app/ingest.py`, rewritten for async Postgres and **batched**. The old code ran one SELECT per test for get-or-create and one per test for rescoring, about 5,000 queries for a 2,500-test report.

The steps for one Report:
1. Parse its body (in a thread).
2. Apply the Project root if the Report sent one.
3. Insert the Run.
4. Bulk-upsert the Tests (`ON CONFLICT DO NOTHING`), then read back their ids.
5. Bulk-insert the Executions.
6. Bulk-update each Test's `last_status`, `last_seen_at` and Location.
7. Rescore all touched Tests with **one** window-function query per 1,000 Tests.
8. Mark the Report processed.

`process_next` wraps all of this in a transaction that claims the Report with `FOR UPDATE SKIP LOCKED`, with the work itself in a SAVEPOINT.

## Context Pack
- Source decisions:
  - One processor, in upload order, because scoring depends on Execution order (ADR 0002).
  - Location: the latest report wins, but a report without a file never erases one.
  - The root is set only when a Report sent one.
  - Failed Reports keep their error text, capped at 2,000 chars.
  - Same-SHA proof must still work, including when one report contains the same test twice (e.g. in-run retries).
- Repo facts:
  - Scoring contract (unchanged, from `backend/app/scoring.py`):
    `combined_score(statuses_newest_first: list[str], executions_with_sha: list[tuple[str, str]], decay: float, window: int) -> tuple[float, int]`. Here `executions_with_sha` is a list of `(commit_sha, status)` tuples. The pre-fork code called it with the last `score_window` Executions of a Test, newest first by `TestExecution.id`.
  - Settings (from `app/config.py`): `get_settings().score_window: int = 50` and `score_decay: float = 0.85`.
  - From task 02: `parse_junit_xml(content: bytes) -> list[ParsedCase]`, `fingerprint(suite, classname, name) -> str`, and `ParsedCase(suite, classname, name, status, duration, message, details, file, line)`.
  - From task 01: the models `Report`, `Project`, `TestCase`, `TestRun`, `TestExecution`, `utcnow()`, the `REPORT_*` constants, and the unique constraint `uq_test_cases_project_fingerprint`. The factories `make_project(db, repo, project, root="")` and `make_report(db, project, body, commit_sha="sha1", branch="main", ci_run_id="", root=None, status="pending")`.
- Verified external contracts (see `00-shared-context.md`):
  - `with_for_update(skip_locked=True)` claims distinct rows across sessions.
  - `pg_insert(...).on_conflict_do_nothing(constraint=...)`.
  - Heterogeneous bulk `update(TestCase)` with a list of dicts keyed by `id`, where a missing key leaves that column unchanged. This is how "never erase Location" works.
  - Bulk `insert(TestExecution)` with a list of dicts.
  - Keep each statement to ≤1,000 rows (the asyncpg limit is 32,767 bind params).
- Non-goals: the loop, sleeping or leader election (05); GitHub issues (07); retention (06); any HTTP endpoint.

## Delivery Strategy
- Shape: Wide refactor: Migrate (integration branch).
- Valid-state scope: Named integration branch `feat/repo-split`.

## Implementation Contract
- Expected files: create `backend/app/processing.py` and `backend/tests/test_processing.py`. Touch nothing else.
- Interfaces and names: this reference implementation passes the tests below. Copy it; the public names and signatures are required.
```python
"""Turn one pending Report into a Run: persist Executions, rescore Tests.

Called only by the single elected processor (ADR 0002), one Report at a time
in upload order — scoring depends on Execution order, so never parallelize.
Every statement is batched (chunks of CHUNK rows) to stay far below
asyncpg's 32,767 bind-parameter limit and avoid per-test round-trips.
"""
import asyncio
import logging
from collections import defaultdict
from dataclasses import dataclass
from typing import Any

from sqlalchemy import func, insert, select, update
from sqlalchemy.dialects.postgresql import insert as pg_insert
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from . import scoring
from .config import get_settings
from .models import (
    REPORT_FAILED, REPORT_PENDING, REPORT_PROCESSED, Project, Report, TestCase,
    TestExecution, TestRun, utcnow,
)
from .parsing import ParsedCase, fingerprint, parse_junit_xml

logger = logging.getLogger("flakeradar.processing")

CHUNK = 1000
ERROR_MAX = 2000


@dataclass(frozen=True)
class ProcessOutcome:
    report_id: int
    status: str                      # REPORT_PROCESSED | REPORT_FAILED
    run_id: int | None
    counts: dict[str, int] | None
    touched_test_ids: list[int]
    error: str | None


def _chunks(items: list, size: int = CHUNK):
    for i in range(0, len(items), size):
        yield items[i:i + size]


async def claim_next_report(db: AsyncSession) -> Report | None:
    """Lock the oldest pending Report for this transaction (SKIP LOCKED)."""
    return (await db.execute(
        select(Report)
        .where(Report.status == REPORT_PENDING)
        .order_by(Report.id)
        .limit(1)
        .with_for_update(skip_locked=True)
    )).scalar_one_or_none()


async def _upsert_test_cases(
    db: AsyncSession, project_id: int, parsed: list[ParsedCase]
) -> dict[str, int]:
    """Insert unseen Tests; return {fingerprint: test_case_id} for all of them."""
    by_fp: dict[str, ParsedCase] = {}
    for pc in parsed:
        by_fp.setdefault(fingerprint(pc.suite, pc.classname, pc.name), pc)
    rows = [
        {"project_id": project_id, "fingerprint": fp, "suite": pc.suite,
         "classname": pc.classname, "name": pc.name}
        for fp, pc in by_fp.items()
    ]
    for chunk in _chunks(rows):
        await db.execute(
            pg_insert(TestCase).values(chunk).on_conflict_do_nothing(
                constraint="uq_test_cases_project_fingerprint")
        )
    ids: dict[str, int] = {}
    for chunk in _chunks(list(by_fp)):
        result = await db.execute(
            select(TestCase.fingerprint, TestCase.id).where(
                TestCase.project_id == project_id, TestCase.fingerprint.in_(chunk))
        )
        ids.update(dict(result.all()))
    return ids


async def rescore(db: AsyncSession, test_case_ids: list[int]) -> None:
    """Recompute flakiness for the given Tests from their last `window` Executions."""
    settings = get_settings()
    rn = func.row_number().over(
        partition_by=TestExecution.test_case_id, order_by=TestExecution.id.desc()
    ).label("rn")
    history: dict[int, list[tuple[str, str]]] = defaultdict(list)  # id -> [(sha, status)] newest first
    for chunk in _chunks(test_case_ids):
        ranked = (
            select(TestExecution.test_case_id, TestExecution.status, TestRun.commit_sha, rn)
            .join(TestRun, TestRun.id == TestExecution.test_run_id)
            .where(TestExecution.test_case_id.in_(chunk))
            .subquery()
        )
        rows = await db.execute(
            select(ranked.c.test_case_id, ranked.c.status, ranked.c.commit_sha)
            .where(ranked.c.rn <= settings.score_window)
            .order_by(ranked.c.test_case_id, ranked.c.rn)
        )
        for tc_id, status, sha in rows.all():
            history[tc_id].append((sha, status))

    updates: list[dict[str, Any]] = []
    for tc_id in test_case_ids:
        execs = history.get(tc_id, [])
        score, confirmed = scoring.combined_score(
            [status for _, status in execs], execs,
            settings.score_decay, settings.score_window,
        )
        updates.append({"id": tc_id, "flakiness_score": score,
                        "confirmed_flake_count": confirmed})
    for chunk in _chunks(updates):
        await db.execute(update(TestCase), chunk)


async def process_report(db: AsyncSession, report: Report) -> ProcessOutcome:
    """Persist one Report as a Run. Caller owns the transaction (no commit here)."""
    parsed = await asyncio.to_thread(parse_junit_xml, report.body)
    now = utcnow()

    if report.root is not None:
        await db.execute(
            update(Project).where(Project.id == report.project_id).values(root=report.root)
        )

    run = TestRun(project_id=report.project_id, commit_sha=report.commit_sha,
                  branch=report.branch, ci_run_id=report.ci_run_id, created_at=now)
    db.add(run)
    await db.flush()

    ids = await _upsert_test_cases(db, report.project_id, parsed)

    counts = {"passed": 0, "failed": 0, "error": 0, "skipped": 0}
    executions: list[dict[str, Any]] = []
    latest: dict[int, dict[str, Any]] = {}  # per Test: last occurrence in the report wins
    for pc in parsed:
        tc_id = ids[fingerprint(pc.suite, pc.classname, pc.name)]
        counts[pc.status] += 1
        executions.append({
            "test_case_id": tc_id, "test_run_id": run.id, "status": pc.status,
            "duration": pc.duration, "message": pc.message, "details": pc.details,
            "created_at": now,
        })
        row = latest.setdefault(tc_id, {"id": tc_id})
        row["last_status"] = pc.status
        row["last_seen_at"] = now
        if pc.file is not None:  # a report without Location never erases one
            row["file"] = pc.file
            row["line"] = pc.line

    for chunk in _chunks(executions):
        await db.execute(insert(TestExecution), chunk)
    for chunk in _chunks(list(latest.values())):
        await db.execute(update(TestCase), chunk)

    touched = sorted(latest)
    await rescore(db, touched)

    report.status = REPORT_PROCESSED
    report.counts = counts
    report.run_id = run.id
    report.error = None
    report.processed_at = now
    return ProcessOutcome(report_id=report.id, status=REPORT_PROCESSED, run_id=run.id,
                          counts=counts, touched_test_ids=touched, error=None)


async def process_next(
    session_factory: async_sessionmaker[AsyncSession],
) -> ProcessOutcome | None:
    """Claim and process the oldest pending Report. None when the queue is empty.

    Work happens in a SAVEPOINT: on any error it is rolled back, and the
    Report (still row-locked) is marked failed with the error in the same
    transaction, so a bad Report can never block the queue.
    """
    async with session_factory() as db:
        async with db.begin():
            report = await claim_next_report(db)
            if report is None:
                return None
            try:
                async with db.begin_nested():
                    return await process_report(db, report)
            except Exception as exc:
                logger.exception("Report %s failed to process", report.id)
                error = f"{type(exc).__name__}: {exc}"[:ERROR_MAX]
                report.status = REPORT_FAILED
                report.error = error
                report.processed_at = utcnow()
                return ProcessOutcome(report_id=report.id, status=REPORT_FAILED,
                                      run_id=None, counts=None, touched_test_ids=[],
                                      error=error)
```
- Behavior rules:
  - `counts` always has exactly the keys `passed`, `failed`, `error` and `skipped`, and counts **Executions**, not unique Tests.
  - `touched_test_ids` is sorted ascending.
  - Duplicate test in one report: one Test, one Execution per occurrence, and the last occurrence sets `last_status`.
  - On failure, `error` is `f"{type(exc).__name__}: {exc}"` truncated to 2,000 chars. No Run or Executions survive (the SAVEPOINT rolls back), but the Report row is updated to `failed` and committed.
  - `process_next` returns `None` only when no pending Report is claimable.
- Error and security rules: log failures with `logger.exception("Report %s failed to process", report.id)`. Never log the body.

## Acceptance Criteria
- [ ] Processing `make_junit([("t1","passed"),("t2","failed")])` creates 1 Run, 2 Tests and 2 Executions, and the Report becomes `processed` with `counts == {"passed": 1, "failed": 1, "error": 0, "skipped": 0}`.
- [ ] Two Reports on commit `dead` (fail, then pass) give `confirmed_flake_count == 1` and `flakiness_score >= 0.6`.
- [ ] Location `tests/a.py:3`, then a report without a file, then `tests/b.py:9` → `(tests/a.py, 3)` after the second report and `(tests/b.py, 9)` after the third.
- [ ] Body `b"not xml"` → outcome `failed`, with an error starting `ParseError: Not a valid JUnit XML report`. The next good Report still processes, and only 1 Run exists.
- [ ] Two open transactions claiming at the same time get two different Reports.
- [ ] A 2,500-case report is processed within 30 s (about 0.5 s measured).

## Test Expectations
- Framework: pytest + pytest-asyncio with a real Postgres (`db`, `session_factory` fixtures).
- `backend/tests/test_processing.py` (verified, 10 passing):
```python
"""Report processor: Runs, Executions, Tests, Location, scoring, failures."""
import asyncio

from sqlalchemy import func, select

from app.models import Project, Report, TestCase, TestExecution, TestRun
from app.processing import claim_next_report, process_next
from tests.conftest import make_junit
from tests.factories import make_project, make_report


async def _queue(db, body: bytes, project=None, **kw) -> Report:
    project = project or await make_project(db, "acme/app", "backend")
    rep = await make_report(db, project, body, **kw)
    await db.commit()
    return rep


async def _tests(db) -> list[TestCase]:
    return list((await db.execute(select(TestCase).order_by(TestCase.name))).scalars())


async def test_empty_queue_returns_none(session_factory):
    assert await process_next(session_factory) is None


async def test_processes_report_into_run(db, session_factory):
    rep = await _queue(db, make_junit([("t1", "passed"), ("t2", "failed")]),
                       commit_sha="abc", branch="feat", ci_run_id="9-1")
    outcome = await process_next(session_factory)
    assert outcome.status == "processed" and outcome.report_id == rep.id
    assert outcome.counts == {"passed": 1, "failed": 1, "error": 0, "skipped": 0}

    await db.refresh(rep)
    assert rep.status == "processed" and rep.run_id == outcome.run_id
    assert rep.counts == outcome.counts and rep.processed_at is not None
    run = (await db.execute(select(TestRun))).scalar_one()
    assert (run.commit_sha, run.branch, run.ci_run_id) == ("abc", "feat", "9-1")
    tests = await _tests(db)
    assert [(t.name, t.last_status) for t in tests] == [("t1", "passed"), ("t2", "failed")]
    assert sorted(outcome.touched_test_ids) == [t.id for t in tests]
    msg = (await db.execute(select(TestExecution.message).where(
        TestExecution.status == "failed"))).scalar_one()
    assert msg == "assert 1 == 2"


async def test_same_sha_retry_is_proven_flake(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    await _queue(db, make_junit([("t_flaky", "failed")]), project=proj, commit_sha="dead")
    await _queue(db, make_junit([("t_flaky", "passed")]), project=proj, commit_sha="dead")
    await process_next(session_factory)
    await process_next(session_factory)
    (tc,) = await _tests(db)
    assert tc.confirmed_flake_count == 1 and tc.flakiness_score >= 0.6


async def test_same_test_twice_in_one_report(db, session_factory):
    xml = (b'<testsuite name="unit">'
           b'<testcase classname="tests.test_mod" name="t"><failure message="x"/></testcase>'
           b'<testcase classname="tests.test_mod" name="t"/></testsuite>')
    await _queue(db, xml)
    outcome = await process_next(session_factory)
    (tc,) = await _tests(db)
    assert tc.last_status == "passed"  # last occurrence wins
    assert tc.confirmed_flake_count == 1  # fail + pass on one sha
    assert outcome.counts["failed"] == 1 and outcome.counts["passed"] == 1
    n = (await db.execute(select(func.count(TestExecution.id)))).scalar()
    assert n == 2


async def test_same_name_in_two_projects_is_two_tests(db, session_factory):
    a = await make_project(db, "acme/app", "backend")
    b = await make_project(db, "acme/app", "frontend")
    await _queue(db, make_junit([("test_login", "passed")]), project=a)
    await _queue(db, make_junit([("test_login", "passed")]), project=b)
    await process_next(session_factory)
    await process_next(session_factory)
    tests = await _tests(db)
    assert sorted(t.project_id for t in tests) == [a.id, b.id]


async def test_location_latest_wins_but_is_never_erased(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    with_loc = (b'<testsuite name="s"><testcase classname="c" name="t" '
                b'file="tests/a.py" line="3"/></testsuite>')
    moved = (b'<testsuite name="s"><testcase classname="c" name="t" '
             b'file="tests/b.py" line="9"/></testsuite>')
    no_loc = b'<testsuite name="s"><testcase classname="c" name="t"/></testsuite>'
    await _queue(db, with_loc, project=proj)
    await process_next(session_factory)
    await _queue(db, no_loc, project=proj)
    await process_next(session_factory)
    (tc,) = await _tests(db)
    assert (tc.file, tc.line) == ("tests/a.py", 3)
    await _queue(db, moved, project=proj)
    await process_next(session_factory)
    await db.refresh(tc)
    assert (tc.file, tc.line) == ("tests/b.py", 9)


async def test_root_is_set_only_when_sent(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    await _queue(db, make_junit([("t", "passed")]), project=proj, root="server")
    await process_next(session_factory)
    await _queue(db, make_junit([("t", "passed")]), project=proj, root=None)
    await process_next(session_factory)
    await db.refresh(proj)
    assert proj.root == "server"


async def test_bad_report_is_marked_failed_and_queue_moves_on(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    bad = await _queue(db, b"not xml", project=proj)
    good = await _queue(db, make_junit([("t", "passed")]), project=proj)
    first = await process_next(session_factory)
    assert first.status == "failed" and first.report_id == bad.id
    assert first.error.startswith("ParseError: Not a valid JUnit XML report")
    second = await process_next(session_factory)
    assert second.status == "processed" and second.report_id == good.id
    await db.refresh(bad)
    assert bad.status == "failed" and bad.error == first.error
    assert (await db.execute(select(func.count(TestRun.id)))).scalar() == 1


async def test_claim_skips_locked_rows(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    r1 = await _queue(db, make_junit([("t", "passed")]), project=proj)
    r2 = await _queue(db, make_junit([("t", "passed")]), project=proj)
    async with session_factory() as a, session_factory() as b:
        async with a.begin(), b.begin():
            got_a = await claim_next_report(a)
            got_b = await claim_next_report(b)
            assert (got_a.id, got_b.id) == (r1.id, r2.id)


async def test_large_report_is_batched(db, session_factory):
    cases = [(f"t{i}", "failed" if i % 50 == 0 else "passed") for i in range(2500)]
    await _queue(db, make_junit(cases))
    outcome = await asyncio.wait_for(process_next(session_factory), timeout=30)
    assert outcome.status == "processed"
    assert len(outcome.touched_test_ids) == 2500
```

## Dependencies
- Blocked by: 01 — Postgres + async foundation; 02 — JUnit parsing
- Why blocked: 01 supplies the schema, fixtures and factories. 02 supplies `parse_junit_xml`, `fingerprint` and `ParsedCase`.
- Blocks: 05 (the worker loop calls `process_next`), 07 (GitHub filing uses `ProcessOutcome.touched_test_ids`)

## Labels
`feature`, `backend`, `ingest`, `performance`, `priority:high`

## Estimate
Medium

## Risk
3 - Scoring correctness and data integrity. Contained by the tests, including the same-SHA and failure paths.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q tests/test_processing.py tests/test_ingest_api.py tests/test_reports_api.py tests/test_identity.py tests/test_parsing.py tests/test_db.py tests/test_scoring.py
# expect: 58 passed
```
