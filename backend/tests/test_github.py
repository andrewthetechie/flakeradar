"""GitHub issues are filed in each Test's own Repo; failures never raise."""
import json

import httpx
import pytest

from app import github_integration
from app.config import Settings
from app.models import TestCase
from app.processing import ProcessOutcome
from tests.factories import make_execution, make_project, make_run, make_test_case


@pytest.fixture()
def gh_settings(monkeypatch):
    settings = Settings(github_token="tok", flake_threshold=0.3)
    monkeypatch.setattr("app.github_integration.get_settings", lambda: settings)
    return settings


async def _flaky_test(db, repo="acme/app", project="backend", name="t_flaky", **fields):
    proj = await make_project(db, repo, project, root=fields.pop("root", ""))
    tc = await make_test_case(db, proj, name=name, flakiness_score=0.6,
                              confirmed_flake_count=1, **fields)
    run = await make_run(db, proj, commit_sha="deadbeef")
    await make_execution(db, tc, run, status="failed", message="assert 1 == 2",
                         details="Traceback: boom")
    await make_execution(db, tc, run, status="passed")
    await db.commit()
    return tc


def _recorder(status=201, number=77, headers=None):
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        return httpx.Response(status, json={"number": number}, headers=headers or {})

    return calls, httpx.MockTransport(handler)


async def test_unconfigured_is_noop(db):
    tc = await _flaky_test(db)
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [tc.id], transport=transport)
    assert calls == []


async def test_files_issue_in_the_tests_own_repo(db, gh_settings):
    tc = await _flaky_test(db, repo="andrewthetechie/writers-app", file="src/a.test.ts",
                           line=12, root="frontend")
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [tc.id], transport=transport)

    (req,) = calls
    assert req.url.path == "/repos/andrewthetechie/writers-app/issues"
    assert req.headers["Authorization"] == "Bearer tok"
    body = json.loads(req.content)
    assert body["title"] == "[FlakeRadar] Flaky test: tests.test_mod::t_flaky"
    assert body["labels"] == ["flakeradar"]
    assert "andrewthetechie/writers-app / backend" in body["body"]
    assert "`frontend/src/a.test.ts` line 12" in body["body"]
    assert "Traceback: boom" in body["body"]
    await db.refresh(tc)
    assert tc.github_issue_number == 77


async def test_does_not_refile_or_file_below_threshold(db, gh_settings):
    filed = await _flaky_test(db, name="filed", github_issue_number=5)
    proj = await make_project(db, "acme/other")
    calm = await make_test_case(db, proj, name="calm", flakiness_score=0.1)
    await db.commit()
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [filed.id, calm.id], transport=transport)
    assert calls == []


async def test_rate_limit_stops_batch_and_404_continues(db, gh_settings):
    a = await _flaky_test(db, repo="acme/a")
    b = await _flaky_test(db, repo="acme/b")
    calls, transport = _recorder(status=403, headers={"x-ratelimit-remaining": "0"})
    await github_integration.file_issues_for(db, [a.id, b.id], transport=transport)
    assert len(calls) == 1

    calls, transport = _recorder(status=404)
    await github_integration.file_issues_for(db, [a.id, b.id], transport=transport)
    assert len(calls) == 2
    await db.refresh(a)
    assert a.github_issue_number is None


async def test_network_error_never_raises(db, gh_settings):
    tc = await _flaky_test(db)

    def boom(request):
        raise httpx.ConnectError("offline")

    await github_integration.file_issues_for(db, [tc.id], transport=httpx.MockTransport(boom))
    await db.refresh(tc)
    assert tc.github_issue_number is None


async def test_hook_ignores_failed_reports(session_factory, gh_settings, monkeypatch):
    called = []

    async def fake_file(db, ids, **kw):
        called.append(ids)

    monkeypatch.setattr(github_integration, "file_issues_for", fake_file)
    failed = ProcessOutcome(report_id=1, status="failed", run_id=None, counts=None,
                            touched_test_ids=[], error="x")
    done = ProcessOutcome(report_id=2, status="processed", run_id=1, counts={},
                          touched_test_ids=[3, 4], error=None)
    await github_integration.on_report_processed(session_factory, failed)
    await github_integration.on_report_processed(session_factory, done)
    assert called == [[3, 4]]
