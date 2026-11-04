"""Report processor: Runs, Executions, Tests, Location, scoring, failures."""

import asyncio
from pathlib import Path

import pytest
from app import scoring
from app.config import Settings
from app.models import Job, Repo, Report, TestCase, TestExecution, TestRun, TestScoreHistory, utcnow
from app.processing import claim_next_report, process_next, rescore
from sqlalchemy import event as sa_event
from sqlalchemy import func, select
from sqlalchemy.exc import IntegrityError, OperationalError

from tests.conftest import make_junit
from tests.factories import (
    make_execution,
    make_job,
    make_pipeline,
    make_project,
    make_report,
    make_run,
    make_test_case,
)
from tests.test_pipeline_processing import _job, _payload, _queue_pipeline

FIXTURES = Path(__file__).parent / "fixtures"


async def _queue(db, body: bytes, project=None, **kw) -> Report:
    project = project or await make_project(db, "acme/app", "backend")
    rep = await make_report(db, project, body, **kw)
    await db.commit()
    return rep


async def _tests(db) -> list[TestCase]:
    return list((await db.execute(select(TestCase).order_by(TestCase.name))).scalars())


async def _jobs(db) -> list[Job]:
    return list((await db.execute(select(Job).order_by(Job.name))).scalars())


async def test_empty_queue_returns_none(session_factory):
    assert await process_next(session_factory) is None


async def test_processes_report_into_run(db, session_factory):
    rep = await _queue(
        db, make_junit([("t1", "passed"), ("t2", "failed")]), commit_sha="abc", branch="feat", ci_run_id="9-1"
    )
    outcome = await process_next(session_factory)
    assert outcome.status == "processed" and outcome.report_id == rep.id
    assert outcome.counts == {"passed": 1, "failed": 1, "error": 0, "skipped": 0, "retried": 0}

    await db.refresh(rep)
    assert rep.status == "processed" and rep.run_id == outcome.run_id
    assert rep.counts == outcome.counts and rep.processed_at is not None
    run = (await db.execute(select(TestRun))).scalar_one()
    assert (run.commit_sha, run.branch, run.ci_run_id) == ("abc", "feat", "9-1")
    tests = await _tests(db)
    assert [(t.name, t.last_status) for t in tests] == [("t1", "passed"), ("t2", "failed")]
    assert sorted(outcome.touched_test_ids) == [t.id for t in tests]
    msg = (await db.execute(select(TestExecution.message).where(TestExecution.status == "failed"))).scalar_one()
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
    xml = (
        b'<testsuite name="unit">'
        b'<testcase classname="tests.test_mod" name="t"><failure message="x"/></testcase>'
        b'<testcase classname="tests.test_mod" name="t"/></testsuite>'
    )
    await _queue(db, xml)
    outcome = await process_next(session_factory)
    (tc,) = await _tests(db)
    assert tc.last_status == "passed"  # last occurrence wins
    assert tc.confirmed_flake_count == 1  # fail + pass on one sha
    assert outcome.counts["failed"] == 1 and outcome.counts["passed"] == 1
    n = (await db.execute(select(func.count(TestExecution.id)))).scalar()
    assert n == 2
    atts = (await db.execute(select(TestExecution.attempt).order_by(TestExecution.id))).scalars().all()
    assert atts == [0, 1]


async def test_flaky_retry_is_proven_flake_from_one_report(db, session_factory):
    xml = (
        b'<testsuite name="unit"><testcase classname="tests.test_mod" name="t">'
        b'<flakyFailure message="Timeout 30000ms exceeded" type="FAILURE" time="30.1">'
        b"<stackTrace>Error: boom</stackTrace></flakyFailure></testcase>"
        b'<testcase classname="c" name="plain"/></testsuite>'
    )
    await _queue(db, xml)
    outcome = await process_next(session_factory)
    assert outcome.counts == {"passed": 2, "failed": 1, "error": 0, "skipped": 0, "retried": 1}
    (flaky,) = (await db.execute(select(TestCase).where(TestCase.name == "t"))).scalars()
    assert flaky.confirmed_flake_count == 1 and flaky.flakiness_score >= 0.6
    rows = (
        await db.execute(
            select(TestExecution.status, TestExecution.attempt)
            .where(TestExecution.test_case_id == flaky.id)
            .order_by(TestExecution.id)
        )
    ).all()
    assert [(s, a) for s, a in rows] == [("failed", 0), ("passed", 1)]


async def test_nextest_flaky_fail_is_proven_flake_from_one_report(db, session_factory):
    await _queue(db, (FIXTURES / "nextest_flaky_fail.xml").read_bytes(), commit_sha="777022bf")
    outcome = await process_next(session_factory)
    assert outcome.counts == {"passed": 2, "failed": 1, "error": 0, "skipped": 0, "retried": 1}
    name = "embedding::reindexer::worker::tests::invalidate_priority_readiness_drops_from_each_driver"
    (flaky,) = (await db.execute(select(TestCase).where(TestCase.name == name))).scalars()
    assert flaky.confirmed_flake_count == 1 and flaky.flakiness_score >= 0.6
    assert flaky.last_status == "passed"
    rows = (
        await db.execute(
            select(TestExecution.status, TestExecution.attempt, TestExecution.failure_category)
            .where(TestExecution.test_case_id == flaky.id)
            .order_by(TestExecution.id)
        )
    ).all()
    assert [(status, attempt) for status, attempt, _ in rows] == [("failed", 0), ("passed", 1)]
    assert rows[0].failure_category is not None  # the failed try is still classified
    assert rows[1].failure_category is None  # the converted pass is not


async def test_failures_are_classified_and_test_gets_dominant_category(db, session_factory):
    xml = (
        b'<testsuite name="s"><testcase classname="c" name="t">'
        b'<failure message="Test timeout of 30000ms exceeded.">trace</failure></testcase>'
        b'<testcase classname="c" name="ok"/></testsuite>'
    )
    await _queue(db, xml)
    await process_next(session_factory)
    by_name = {
        name: (status, category)
        for name, status, category in (
            await db.execute(
                select(TestCase.name, TestExecution.status, TestExecution.failure_category).join(
                    TestCase, TestCase.id == TestExecution.test_case_id
                )
            )
        ).all()
    }
    assert by_name["t"] == ("failed", "timing")  # the failing attempt is categorized
    assert by_name["ok"] == ("passed", None)  # passing rows stay NULL
    (t,) = (await db.execute(select(TestCase).where(TestCase.name == "t"))).scalars()
    assert t.failure_category == "timing"  # cached dominant category


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
    with_loc = b'<testsuite name="s"><testcase classname="c" name="t" file="tests/a.py" line="3"/></testsuite>'
    moved = b'<testsuite name="s"><testcase classname="c" name="t" file="tests/b.py" line="9"/></testsuite>'
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


async def test_line_base_zero_stores_one_based_location(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    pytest_like = b'<testsuite name="s"><testcase classname="c" name="t" file="tests/a.py" line="11"/></testsuite>'
    await _queue(db, pytest_like, project=proj, line_base=0)
    await process_next(session_factory)
    (tc,) = await _tests(db)
    assert (tc.file, tc.line) == ("tests/a.py", 12)
    # A later default report (line_base=1, or NULL from before the migration)
    # stores the reported line as is.
    await _queue(db, pytest_like.replace(b'line="11"', b'line="5"'), project=proj)
    await process_next(session_factory)
    await db.refresh(tc)
    assert (tc.file, tc.line) == ("tests/a.py", 5)


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
    assert bad.retry_count == 0  # a permanent data error never consumes retry budget
    assert (await db.execute(select(func.count(TestRun.id)))).scalar() == 1


async def test_project_root_written_only_when_changed(db, session_factory, engine):
    """projects.root is rewritten only when its value actually changes."""
    proj = await make_project(db, "acme/app", "backend", root="backend")
    await make_report(db, proj, make_junit([("t", "passed")]), root="backend")  # same value
    await db.commit()

    rows_rewritten = []

    def capture(conn, cursor, statement, parameters, context, executemany):
        if "UPDATE projects" in statement:
            rows_rewritten.append(cursor.rowcount)

    sa_event.listen(engine.sync_engine, "after_cursor_execute", capture)
    try:
        await process_next(session_factory)
    finally:
        sa_event.remove(engine.sync_engine, "after_cursor_execute", capture)
    assert rows_rewritten == [0], "same root must not rewrite any Project row"
    await db.refresh(proj)
    assert proj.root == "backend"

    rows_rewritten.clear()
    await make_report(db, proj, make_junit([("t", "passed")]), root="frontend")
    await db.commit()
    sa_event.listen(engine.sync_engine, "after_cursor_execute", capture)
    try:
        await process_next(session_factory)
    finally:
        sa_event.remove(engine.sync_engine, "after_cursor_execute", capture)
    assert any(n > 0 for n in rows_rewritten), "a different root must rewrite the Project row"
    await db.refresh(proj)
    assert proj.root == "frontend"


async def test_transient_failure_is_retried_then_capped(db, session_factory, monkeypatch):
    """A transient DB failure is re-queued up to the cap, then marked failed."""
    proj = await make_project(db, "acme/app", "backend")

    async def boom(db, report):
        raise OperationalError("stmt", {}, Exception("connection reset"))

    monkeypatch.setattr("app.processing.process_report", boom)
    monkeypatch.setattr("app.processing.get_settings", lambda: Settings(max_report_retries=3))
    rep = await make_report(db, proj, make_junit([("t", "passed")]))
    await db.commit()

    # max_report_retries=3 permits one initial attempt plus three retries.
    for expected_rc in (1, 2, 3):
        outcome = await process_next(session_factory)
        assert outcome.status == "pending" and outcome.report_id == rep.id
        await db.refresh(rep)
        assert (rep.status, rep.retry_count) == ("pending", expected_rc)

    outcome = await process_next(session_factory)
    assert outcome.status == "failed"
    await db.refresh(rep)
    assert (rep.status, rep.retry_count) == ("failed", 3)


async def test_permanent_db_error_not_retried(db, session_factory, monkeypatch):
    """A permanent DBAPIError (e.g. a constraint violation) fails on the first
    attempt and never consumes retry budget."""
    proj = await make_project(db, "acme/app", "backend")

    async def boom(db, report):
        raise IntegrityError("stmt", {}, Exception("duplicate key"))

    monkeypatch.setattr("app.processing.process_report", boom)
    rep = await make_report(db, proj, make_junit([("t", "passed")]))
    await db.commit()

    outcome = await process_next(session_factory)
    assert outcome.status == "failed"
    await db.refresh(rep)
    assert rep.status == "failed" and rep.retry_count == 0


async def test_claim_skips_locked_rows(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    r1 = await _queue(db, make_junit([("t", "passed")]), project=proj)
    r2 = await _queue(db, make_junit([("t", "passed")]), project=proj)
    async with session_factory() as a, session_factory() as b, a.begin(), b.begin():
        got_a = await claim_next_report(a)
        got_b = await claim_next_report(b)
        assert (got_a.id, got_b.id) == (r1.id, r2.id)


async def test_large_report_is_batched(db, session_factory):
    cases = [(f"t{i}", "failed" if i % 50 == 0 else "passed") for i in range(2500)]
    await _queue(db, make_junit(cases))
    outcome = await asyncio.wait_for(process_next(session_factory), timeout=30)
    assert outcome.status == "processed"
    assert len(outcome.touched_test_ids) == 2500


async def test_default_branch_ignores_feature_flips(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    # feat fail@a, feat pass@b, then main pass@c, main pass@d.
    await _queue(db, make_junit([("t", "failed")]), project=proj, branch="feat", commit_sha="a", default_branch="main")
    await _queue(db, make_junit([("t", "passed")]), project=proj, branch="feat", commit_sha="b")
    await _queue(db, make_junit([("t", "passed")]), project=proj, branch="main", commit_sha="c")
    await _queue(db, make_junit([("t", "passed")]), project=proj, branch="main", commit_sha="d")
    for _ in range(4):
        await process_next(session_factory)
    repo = (await db.execute(select(Repo))).scalar_one()
    assert repo.default_branch == "main"
    (tc,) = await _tests(db)
    assert tc.flakiness_score == 0.0
    assert tc.confirmed_flake_count == 0


async def test_same_sha_flip_on_feature_branch_still_proven(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    await _queue(db, make_junit([("t", "failed")]), project=proj, branch="feat", commit_sha="x", default_branch="main")
    await _queue(db, make_junit([("t", "passed")]), project=proj, branch="feat", commit_sha="x")
    await process_next(session_factory)
    await process_next(session_factory)
    (tc,) = await _tests(db)
    assert tc.confirmed_flake_count == 1
    assert tc.flakiness_score >= 0.6


async def test_branch_change_rescores_tests_not_in_report(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    # t1 and t2 each flicker on feat (different shas) -> both score high with no default.
    await _queue(db, make_junit([("t1", "failed")]), project=proj, branch="feat", commit_sha="a")
    await _queue(db, make_junit([("t1", "passed")]), project=proj, branch="feat", commit_sha="b")
    await _queue(db, make_junit([("t2", "failed")]), project=proj, branch="feat", commit_sha="c")
    await _queue(db, make_junit([("t2", "passed")]), project=proj, branch="feat", commit_sha="d")
    for _ in range(4):
        await process_next(session_factory)
    tests = {t.name: t for t in await _tests(db)}
    assert tests["t1"].flakiness_score > 0 and tests["t2"].flakiness_score > 0
    t2_before = tests["t2"].flakiness_score
    # A report touching ONLY t1 sets the Default branch -> t2 must be rescored too.
    await _queue(db, make_junit([("t1", "passed")]), project=proj, branch="main", commit_sha="e", default_branch="main")
    await process_next(session_factory)
    tests = {t.name: t for t in await _tests(db)}
    await db.refresh(tests["t2"])  # expire_on_commit=False: force a fresh read
    assert tests["t2"].flakiness_score == 0.0
    assert tests["t2"].flakiness_score < t2_before


async def test_default_branch_applied_only_at_processing(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    await _queue(db, make_junit([("t", "passed")]), project=proj, default_branch="main")
    repo = (await db.execute(select(Repo))).scalar_one()  # still pending: not applied
    assert repo.default_branch is None
    await process_next(session_factory)
    await db.refresh(repo)
    assert repo.default_branch == "main"


async def test_report_without_default_branch_never_clears(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    await _queue(db, make_junit([("t", "passed")]), project=proj, default_branch="main")
    await process_next(session_factory)
    await _queue(db, make_junit([("t", "passed")]), project=proj)  # no default_branch
    await process_next(session_factory)
    repo = (await db.execute(select(Repo))).scalar_one()
    assert repo.default_branch == "main"


async def test_default_branch_window_rescope(db, session_factory, monkeypatch):
    monkeypatch.setattr("app.processing.get_settings", lambda: Settings(score_window=3))
    proj = await make_project(db, "acme/app", "backend")
    # 5 main executions flickering (oldest first), then 10 newer feat passes.
    for i, status in enumerate(["failed", "passed", "failed", "passed", "failed"]):
        await _queue(
            db,
            make_junit([("t", status)]),
            project=proj,
            branch="main",
            commit_sha=f"m{i}",
            default_branch="main" if i == 0 else None,
        )
    for i in range(10):
        await _queue(db, make_junit([("t", "passed")]), project=proj, branch="feat", commit_sha=f"f{i}")
    for _ in range(15):
        await process_next(session_factory)
    (tc,) = await _tests(db)
    # The 3 newest main rows must still be flip-scored despite 10 newer feat rows.
    assert tc.flakiness_score > 0


async def test_rescore_matches_branch_scoped_score_over_all_rows(db, monkeypatch):
    monkeypatch.setattr("app.processing.get_settings", lambda: Settings(score_window=3))
    proj = await make_project(db, "acme/app", "backend")
    repo = (await db.execute(select(Repo))).scalar_one()
    repo.default_branch = "main"
    tc = await make_test_case(db, proj, name="t")
    # 5 flickering main executions (oldest first), then 10 newer feat passes.
    for i, status in enumerate(["failed", "passed", "failed", "passed", "failed"]):
        run = await make_run(db, proj, commit_sha=f"m{i}", branch="main")
        await make_execution(db, tc, run, status=status)
    for i in range(10):
        run = await make_run(db, proj, commit_sha=f"f{i}", branch="feat")
        await make_execution(db, tc, run, status="passed")
    await db.commit()

    await rescore(db, [tc.id])
    await db.commit()
    await db.refresh(tc)

    all_rows = (
        await db.execute(
            select(TestRun.commit_sha, TestRun.branch, TestExecution.status)
            .join(TestExecution, TestExecution.test_run_id == TestRun.id)
            .where(TestExecution.test_case_id == tc.id)
            .order_by(TestExecution.id.desc())
        )
    ).all()
    assert len(all_rows) == 15  # more than `window` off-Default rows

    s = Settings(score_window=3)
    expected_score, expected_confirmed = scoring.branch_scoped_score(all_rows, "main", s.score_decay, 3)
    assert expected_score > 0.0  # the newest main flips still count
    assert tc.flakiness_score == expected_score
    assert tc.confirmed_flake_count == expected_confirmed
    assert tc.clean_streak == scoring.branch_scoped_streak(all_rows, "main")
    assert tc.fail_streak == scoring.branch_scoped_fail_streak(all_rows, "main")


async def test_same_day_reports_share_one_history_row(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    await _queue(db, make_junit([("t", "failed")]), project=proj, commit_sha="s1", branch="main")
    await _queue(db, make_junit([("t", "passed")]), project=proj, commit_sha="s1", branch="main")
    await process_next(session_factory)
    await process_next(session_factory)
    (tc,) = await _tests(db)
    rows = (await db.execute(select(TestScoreHistory))).scalars().all()
    assert len(rows) == 1
    row = rows[0]
    assert (row.executions, row.failures) == (2, 1)
    assert row.day == utcnow().date()
    assert row.flakiness_score == tc.flakiness_score


async def test_clean_streak_counts_passes_since_last_failure(db, session_factory):
    proj = await make_project(db, "acme/app", "backend")
    for status in ("failed", "passed", "passed"):
        await _queue(db, make_junit([("t", status)]), project=proj, commit_sha="s1", branch="main")
    for _ in range(3):
        await process_next(session_factory)
    (tc,) = await _tests(db)
    assert tc.clean_streak == 2


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_a_failure_back_to_a_zero_streak_clears_the_fixed_note_stamp(db, session_factory, entity):
    """Issue #33: a Report that collapses the Clean streak clears the note stamp, for both entities.

    A stamp that survived a new failure would suppress the next "the fix appears to hold"
    comment forever, so `rescore`/`rescore_jobs` reset it exactly when `clean_streak` is 0. One
    Report carries both rows in the same bulk update: only the collapsed one may lose its stamp.
    """
    if entity == "test":
        proj = await make_project(db, "acme/app", "backend")
        # `held` first on purpose: `rescore` walks ascending ids, so the bulk update sees a
        # dict WITHOUT the reset key first and one WITH it second.
        held = await make_test_case(db, proj, name="t_keep", clean_streak=3, github_issue_fixed_noted_at=utcnow())
        broke = await make_test_case(db, proj, name="t_fail", clean_streak=3, github_issue_fixed_noted_at=utcnow())
        await db.commit()
        await _queue(
            db,
            make_junit([("t_fail", "failed"), ("t_keep", "passed")]),
            project=proj,
            branch="main",
            commit_sha="f9",
            default_branch="main",
        )
    else:
        pipe = await make_pipeline(db, "acme/app")
        held = await make_job(db, pipe, name="lint", clean_streak=3, github_issue_fixed_noted_at=utcnow())
        broke = await make_job(db, pipe, name="e2e", clean_streak=3, github_issue_fixed_noted_at=utcnow())
        await db.commit()
        await _queue_pipeline(db, _payload([_job("1", "e2e", "failed"), _job("2", "lint", "passed")], sha="f9"))
    await process_next(session_factory)
    await db.refresh(broke)
    await db.refresh(held)
    assert broke.clean_streak == 0
    assert broke.github_issue_fixed_noted_at is None
    assert held.clean_streak > 0
    assert held.github_issue_fixed_noted_at is not None  # the sibling keeps the note it earned


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_a_rescore_with_a_clean_streak_keeps_the_fixed_note_stamp(db, session_factory, entity):
    """The reset is keyed on the computed streak, not on any rescore: a streak above 0 keeps it."""
    if entity == "test":
        proj = await make_project(db, "acme/app", "backend")
        await make_test_case(db, proj, name="t", clean_streak=3, github_issue_fixed_noted_at=utcnow())
        await db.commit()
        await _queue(
            db,
            make_junit([("t", "passed")]),
            project=proj,
            branch="main",
            commit_sha="f9",
            default_branch="main",
        )
    else:
        pipe = await make_pipeline(db, "acme/app")
        await make_job(db, pipe, name="test", clean_streak=3, github_issue_fixed_noted_at=utcnow())
        await db.commit()
        await _queue_pipeline(db, _payload([_job("1", "test", "passed")], sha="f9"))
    await process_next(session_factory)
    db.expire_all()
    rows = await _tests(db) if entity == "test" else await _jobs(db)
    (row,) = rows
    assert row.clean_streak > 0
    assert row.github_issue_fixed_noted_at is not None  # the note this row already earned survives


async def test_fail_streak_marks_broken_after_five_default_branch_failures(db, session_factory, monkeypatch):
    monkeypatch.setattr("app.processing.get_settings", lambda: Settings(score_window=50, broken_streak=5))
    proj = await make_project(db, "acme/app", "backend")
    for i in range(5):
        await _queue(
            db,
            make_junit([("t", "failed")]),
            project=proj,
            branch="main",
            commit_sha=f"m{i}",
            default_branch="main" if i == 0 else None,
        )
    for _ in range(5):
        await process_next(session_factory)
    (tc,) = await _tests(db)
    assert tc.flakiness_score == 0.0  # broken, not flaky
    assert tc.fail_streak == 5
    # One pass resets the fail streak.
    await _queue(db, make_junit([("t", "passed")]), project=proj, branch="main", commit_sha="m5")
    await process_next(session_factory)
    db.expire_all()  # the db-session identity map still holds the stale TestCase
    (tc,) = await _tests(db)
    assert tc.fail_streak == 0


# --- Notifications: tier crossings (ADR 0009) -----------------------------


def _notify_settings(**kw) -> Settings:
    """Decay 0 makes the score depend on one transition, so crossings are exact."""
    base = {"flake_threshold": 0.05, "score_decay": 0.0, "score_window": 50, "broken_streak": 5}
    base.update(kw)
    return Settings(**base)


async def test_rescore_reports_an_upward_tier_crossing(db, session_factory, monkeypatch):
    monkeypatch.setattr("app.processing.get_settings", lambda: _notify_settings())
    proj = await make_project(db, "acme/app", "backend")
    await _queue(db, make_junit([("t", "failed")]), project=proj, commit_sha="a")
    first = await process_next(session_factory)
    assert first.tier_changes == []  # a Test that only ever failed scores 0: broken, not flaky
    await _queue(db, make_junit([("t", "passed")]), project=proj, commit_sha="b")
    second = await process_next(session_factory)
    (tc,) = await _tests(db)
    (change,) = second.tier_changes
    assert (change.kind, change.event) == ("test", "test.flaky")
    assert change.id == tc.id
    assert (change.repo, change.project) == ("acme/app", "backend")
    assert (change.classname, change.name) == ("tests.test_mod", "t")
    assert change.flakiness_score == tc.flakiness_score
    assert change.tier == "flaky"
    assert change.confirmed_flake_count == tc.confirmed_flake_count
    assert change.pipeline is None


async def test_rescore_reports_a_downward_tier_crossing(db, session_factory, monkeypatch):
    monkeypatch.setattr("app.processing.get_settings", lambda: _notify_settings())
    proj = await make_project(db, "acme/app", "backend")
    await _queue(db, make_junit([("t", "failed")]), project=proj, commit_sha="a")
    await process_next(session_factory)
    await _queue(db, make_junit([("t", "passed")]), project=proj, commit_sha="b")
    (flaky,) = (await process_next(session_factory)).tier_changes
    assert flaky.event == "test.flaky"
    await _queue(db, make_junit([("t", "failed")]), project=proj, commit_sha="c")
    await process_next(session_factory)  # one failure still flips against the pass
    await _queue(db, make_junit([("t", "failed")]), project=proj, commit_sha="d")
    (change,) = (await process_next(session_factory)).tier_changes
    assert change.event == "test.recovered"
    assert change.tier != "flaky"
    assert change.flakiness_score == 0.0


async def test_staying_flaky_reports_no_second_event(db, session_factory, monkeypatch):
    monkeypatch.setattr("app.processing.get_settings", lambda: _notify_settings())
    proj = await make_project(db, "acme/app", "backend")
    await _queue(db, make_junit([("t", "failed")]), project=proj, commit_sha="a")
    await process_next(session_factory)
    await _queue(db, make_junit([("t", "passed")]), project=proj, commit_sha="b")
    assert (await process_next(session_factory)).tier_changes
    await _queue(db, make_junit([("t", "failed")]), project=proj, commit_sha="a")
    assert (await process_next(session_factory)).tier_changes == []


async def test_a_broken_test_reports_no_recovery(db, session_factory, monkeypatch):
    """Leaving the flaky tier because the Test is now broken is not a recovery."""
    monkeypatch.setattr("app.processing.get_settings", lambda: _notify_settings(broken_streak=2))
    proj = await make_project(db, "acme/app", "backend")
    await _queue(db, make_junit([("t", "failed")]), project=proj, commit_sha="a")
    await process_next(session_factory)
    await _queue(db, make_junit([("t", "passed")]), project=proj, commit_sha="b")
    assert (await process_next(session_factory)).tier_changes
    await _queue(db, make_junit([("t", "failed")]), project=proj, commit_sha="c")
    await process_next(session_factory)
    await _queue(db, make_junit([("t", "failed")]), project=proj, commit_sha="d")
    outcome = await process_next(session_factory)
    db.expire_all()
    (tc,) = await _tests(db)
    assert (tc.flakiness_score, tc.fail_streak) == (0.0, 2)  # the tier did cross, downward
    assert outcome.tier_changes == []


async def test_a_new_test_above_the_threshold_is_reported_flaky(db, session_factory, monkeypatch):
    monkeypatch.setattr("app.processing.get_settings", lambda: _notify_settings())
    proj = await make_project(db, "acme/app", "backend")
    # Same SHA, fail then pass: a Proven flake on the Test's very first Report.
    await _queue(db, make_junit([("t", "failed"), ("t", "passed")]), project=proj, commit_sha="a")
    (change,) = (await process_next(session_factory)).tier_changes
    assert change.event == "test.flaky"
    assert change.confirmed_flake_count == 1


async def test_rescore_repo_reports_no_crossings(db, session_factory, monkeypatch, caplog):
    """A Default branch change rescores the whole Repo; those events are dropped."""
    monkeypatch.setattr("app.processing.get_settings", lambda: _notify_settings())
    proj = await make_project(db, "acme/app", "backend")
    await _queue(db, make_junit([("t2", "failed")]), project=proj, branch="feat", commit_sha="a")
    await process_next(session_factory)
    await _queue(db, make_junit([("t2", "passed")]), project=proj, branch="feat", commit_sha="b")
    assert (await process_next(session_factory)).tier_changes  # t2 became flaky on 'feat'
    # A Report that only touches t1 on the Default branch rescores all of t2's history.
    await _queue(db, make_junit([("t1", "passed")]), project=proj, branch="main", commit_sha="c", default_branch="main")
    with caplog.at_level("INFO", logger="flakeradar.processing"):
        outcome = await process_next(session_factory)
    db.expire_all()
    tests = {t.name: t for t in await _tests(db)}
    assert tests["t2"].flakiness_score == 0.0  # its flaky tier was left...
    assert outcome.tier_changes == []  # ...but rescore_repo sends no event
    assert "dropped 1 Notification(s)" in caplog.text  # the count is all it reports


async def test_a_move_into_suspect_reports_nothing(db, session_factory, monkeypatch):
    """Only the flaky boundary counts: stable -> suspect is not an event."""
    monkeypatch.setattr("app.processing.get_settings", lambda: _notify_settings(flake_threshold=0.5))
    proj = await make_project(db, "acme/app", "backend")
    await _queue(db, make_junit([("t", "failed")]), project=proj, commit_sha="a")
    await process_next(session_factory)
    await _queue(db, make_junit([("t", "passed")]), project=proj, commit_sha="b")
    outcome = await process_next(session_factory)
    db.expire_all()
    (tc,) = await _tests(db)
    assert 0.0 < tc.flakiness_score < 0.5  # a Suspect test, not a Flaky one
    assert outcome.tier_changes == []
