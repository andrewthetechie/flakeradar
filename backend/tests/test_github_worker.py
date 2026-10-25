"""GitHubWorker: filing and issue-state sync run off the processor's hot path."""

import asyncio

import httpx
import pytest
from app import github_integration
from app.config import Settings
from app.db import WORKER_LOCK_KEY
from app.models import Report, TestCase
from app.processing import ProcessOutcome
from app.worker import GitHubWorker
from sqlalchemy import select, text

from tests.conftest import make_junit
from tests.factories import make_project, make_report
from tests.test_github import _flaky_test
from tests.test_worker import _worker


@pytest.fixture()
def gh_settings(monkeypatch):
    settings = Settings(
        github_token="tok",
        flake_threshold=0.3,
        github_issue_label="flakeradar",
        github_issue_min_score=0.3,
        github_issue_min_proven_flakes=0,
        github_issue_min_failures=0,
        score_window=50,
    )
    monkeypatch.setattr("app.github_integration.get_settings", lambda: settings)
    return settings


async def _wait_for(predicate, timeout: float = 10.0) -> None:
    """Poll `predicate` (sync or async) until it is true."""
    deadline = asyncio.get_running_loop().time() + timeout
    while True:
        result = predicate()
        if asyncio.iscoroutine(result):
            result = await result
        if result:
            return
        assert asyncio.get_running_loop().time() < deadline, "timed out"
        await asyncio.sleep(0.05)


def _outcome(tc) -> ProcessOutcome:
    return ProcessOutcome(
        report_id=1,
        status="processed",
        run_id=1,
        counts={},
        touched_test_ids=[tc.id],
        touched_job_ids=[],
        error=None,
    )


async def _filed_as(session_factory, test_case_id: int, number: int) -> bool:
    """True once the worker has stored `number` on the Test (in its own session)."""
    async with session_factory() as session:
        stmt = select(TestCase.github_issue_number).where(TestCase.id == test_case_id)
        return (await session.execute(stmt)).scalar() == number


async def test_enqueue_files_issues_in_the_background(session_factory, db, gh_settings):
    tc = await _flaky_test(db)
    calls: list[httpx.Request] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        await asyncio.sleep(0.3)  # a slow GitHub API must not delay enqueue
        return httpx.Response(201, json={"number": 77})

    worker = GitHubWorker(session_factory, sync_interval_seconds=3600.0, transport=httpx.MockTransport(handler))
    await worker.start()
    try:
        await worker.enqueue(_outcome(tc))
        assert calls == []  # enqueue returned before the POST was even issued
        await _wait_for(lambda: len(calls) == 1)
        assert calls[0].method == "POST"
        assert calls[0].url.path == "/repos/acme/app/issues"
        await _wait_for(lambda: _filed_as(session_factory, tc.id, 77))
    finally:
        await worker.stop()


async def test_sync_runs_on_start_and_on_interval(session_factory, db, gh_settings):
    await _flaky_test(db, github_issue_number=11)
    calls: list[httpx.Request] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        return httpx.Response(200, json={"state": "open"})

    worker = GitHubWorker(session_factory, sync_interval_seconds=0.1, transport=httpx.MockTransport(handler))
    await worker.start()
    try:
        await _wait_for(lambda: len(calls) >= 1, timeout=5.0)  # once as soon as the task starts
        # A second batch within ~1 s proves the interval re-fires, not just the start.
        await _wait_for(lambda: len(calls) >= 2, timeout=1.0)
    finally:
        await worker.stop()
    assert all(req.method == "GET" for req in calls)


async def test_stop_cancels_the_task(session_factory, gh_settings):
    worker = GitHubWorker(session_factory, sync_interval_seconds=3600.0)
    assert not worker.is_running
    await worker.start()
    assert worker.is_running
    await worker.stop()
    assert not worker.is_running


async def test_outcome_files_through_the_worker_transport(session_factory, gh_settings, monkeypatch):
    """The worker's transport reaches on_report_processed, which forwards it to filing."""
    seen: list[tuple[str, object]] = []

    async def fake_file(db, ids, *, transport=None):
        seen.append(("test", transport))

    async def fake_job_file(db, ids, *, transport=None):
        seen.append(("job", transport))

    monkeypatch.setattr(github_integration, "file_issues_for", fake_file)
    monkeypatch.setattr(github_integration, "file_job_issues_for", fake_job_file)
    transport = httpx.MockTransport(lambda request: httpx.Response(201, json={"number": 5}))
    worker = GitHubWorker(session_factory, sync_interval_seconds=3600.0, transport=transport)
    await worker.start()
    try:
        await worker.enqueue(
            ProcessOutcome(
                report_id=1,
                status="processed",
                run_id=1,
                counts={},
                touched_test_ids=[1],
                touched_job_ids=[2],
                error=None,
            )
        )
        await _wait_for(lambda: len(seen) == 2)
    finally:
        await worker.stop()
    assert [kind for kind, _ in seen] == ["test", "job"]
    assert all(got is transport for _, got in seen)


async def _is_processed(session_factory, report_id: int) -> bool:
    async with session_factory() as session:
        report = await session.get(Report, report_id)
        return report.status == "processed"


async def test_github_work_does_not_block_processing(engine, session_factory, db, gh_settings):
    """A 2 s issue POST must not delay the next pending Report."""
    started = asyncio.Event()
    returned = asyncio.Event()

    async def handler(request: httpx.Request) -> httpx.Response:
        started.set()
        await asyncio.sleep(2)
        returned.set()
        return httpx.Response(201, json={"number": 77})

    proj = await make_project(db)
    await make_report(db, proj, make_junit([("t", "failed"), ("t", "passed")]), commit_sha="a")
    await db.commit()

    github = GitHubWorker(session_factory, sync_interval_seconds=3600.0, transport=httpx.MockTransport(handler))
    worker = _worker(engine, session_factory, on_processed=github.enqueue, github=github)
    await worker.start()
    try:
        await _wait_for(started.is_set)  # the first issue POST is in flight

        report2 = await make_report(db, proj, make_junit([("t", "failed"), ("t", "passed")]), commit_sha="b")
        await db.commit()
        await _wait_for(lambda: _is_processed(session_factory, report2.id), timeout=1.5)
        assert not returned.is_set()  # processing got there before the call came back
    finally:
        await worker.stop()


async def test_github_task_stops_with_leadership(engine, session_factory):
    """ReportWorker starts the GitHub task when it leads and stops it when it does not."""
    github = GitHubWorker(session_factory, sync_interval_seconds=3600.0)
    worker = _worker(engine, session_factory, github=github)
    await worker.start()
    try:
        await _wait_for(lambda: worker.is_leader)
        assert github.is_running
    finally:
        await worker.stop()
    assert not github.is_running


async def test_losing_the_connection_stops_the_github_task(engine, session_factory):
    """The error path releases through _release too, even with the connection gone."""
    github = GitHubWorker(session_factory, sync_interval_seconds=3600.0)
    worker = _worker(engine, session_factory, github=github)
    await github.start()  # what _try_lead did while this process held the lock
    worker._lock_conn = None  # the lock connection is already broken
    await worker._release()
    assert not github.is_running


class _BlockingGitHubWorker(GitHubWorker):
    """A GitHub task whose stop waits, like an issue POST still in flight."""

    def __init__(self, *args, **kwargs) -> None:
        super().__init__(*args, **kwargs)
        self.stopping = asyncio.Event()
        self.unblock = asyncio.Event()

    async def stop(self) -> None:
        self.stopping.set()
        await self.unblock.wait()
        await super().stop()


async def _lock_is_free(engine) -> bool:
    """True when another session can take the worker lock right now."""
    async with engine.connect() as conn:
        got = (await conn.execute(text("SELECT pg_try_advisory_lock(:k)"), {"k": WORKER_LOCK_KEY})).scalar()
        if got:
            await conn.execute(text("SELECT pg_advisory_unlock(:k)"), {"k": WORKER_LOCK_KEY})
    return bool(got)


async def test_the_github_task_ends_before_the_lock_is_released(engine, session_factory):
    """Unlocking while a call is in flight would let a replacement leader file it too."""
    github = _BlockingGitHubWorker(session_factory, sync_interval_seconds=3600.0)
    worker = _worker(engine, session_factory, github=github)
    await worker.start()
    await _wait_for(lambda: worker.is_leader)
    assert github.is_running

    releasing = asyncio.create_task(worker.stop())
    try:
        await _wait_for(github.stopping.is_set)  # the call is still in flight ...
        assert not await _lock_is_free(engine)  # ... so the lock must still be held
        await asyncio.sleep(0.2)
        assert not await _lock_is_free(engine)
    finally:
        github.unblock.set()  # the call comes back
        await releasing

    assert not github.is_running
    assert await _lock_is_free(engine)  # only now does leadership actually end


async def test_the_replacement_leader_waits_for_the_former_github_task(engine, session_factory):
    """A standby cannot lead, so its GitHub task cannot start, until the old one is done."""
    former = _BlockingGitHubWorker(session_factory, sync_interval_seconds=3600.0)
    successor = GitHubWorker(session_factory, sync_interval_seconds=3600.0)
    leader = _worker(engine, session_factory, github=former)
    standby = _worker(engine, session_factory, github=successor)
    await leader.start()
    await _wait_for(lambda: leader.is_leader)
    await standby.start()
    assert not successor.is_running

    releasing = asyncio.create_task(leader.stop())
    try:
        await _wait_for(former.stopping.is_set)
        await asyncio.sleep(0.3)  # several standby polls, and one leader poll
        assert not standby.is_leader
        assert not successor.is_running
    finally:
        former.unblock.set()
        await releasing

    await _wait_for(lambda: standby.is_leader)
    assert successor.is_running
    await standby.stop()
