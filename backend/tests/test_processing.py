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
