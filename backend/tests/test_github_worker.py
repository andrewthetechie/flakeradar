"""GitHubWorker: filing and issue-state sync run off the processor's hot path."""

import asyncio

import httpx
import pytest
from app import github_integration
from app.config import Settings
from app.models import TestCase
from app.processing import ProcessOutcome
from app.worker import GitHubWorker
from sqlalchemy import select

from tests.test_github import _flaky_test


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
