"""GitHub issues are filed in each Test's own Repo; failures never raise."""

import json

import httpx
import pytest
from app import github_integration
from app.config import Settings
from app.models import Project, TestCase
from app.processing import ProcessOutcome
from sqlalchemy import select

from tests.factories import make_execution, make_project, make_run, make_test_case


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


async def _flaky_test(
    db,
    repo="acme/app",
    project="backend",
    name="t_flaky",
    flakiness_score=0.6,
    confirmed_flake_count=1,
    failure_details="Traceback: boom",
    **fields,
):
    proj = await make_project(db, repo, project, root=fields.pop("root", ""))
    tc = await make_test_case(
        db,
        proj,
        name=name,
        flakiness_score=flakiness_score,
        confirmed_flake_count=confirmed_flake_count,
        **fields,
    )
    run = await make_run(db, proj, commit_sha="deadbeef")
    await make_execution(db, tc, run, status="failed", message="assert 1 == 2", details=failure_details)
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
    tc = await _flaky_test(db, repo="andrewthetechie/writers-app", file="src/a.test.ts", line=12, root="frontend")
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [tc.id], transport=transport)

    (req,) = calls
    assert req.url.path == "/repos/andrewthetechie/writers-app/issues"
    assert req.headers["Authorization"] == "Bearer tok"
    body = json.loads(req.content)
    assert body["title"] == "[FlakeRadar] Flaky test: tests.test_mod::t_flaky"
    assert body["labels"] == ["flakeradar"]
    assert "`andrewthetechie/writers-app` / `backend`" in body["body"]
    assert "`frontend/src/a.test.ts` line 12" in body["body"]
    # Agent context: permalink to the failing code, proof, and traceback.
    assert "https://github.com/andrewthetechie/writers-app/blob/deadbeef/frontend/src/a.test.ts#L12" in body["body"]
    assert "Proven flakes" in body["body"]
    assert "Traceback: boom" in body["body"]
    # Run-hint must match the test's own framework, not always assume pytest.
    assert 'npx vitest run frontend/src/a.test.ts -t "t_flaky"' in body["body"]
    assert "pytest" not in body["body"]
    await db.refresh(tc)
    assert tc.github_issue_number == 77


def test_run_hint_matches_the_test_framework_by_file_extension():
    assert (
        github_integration._run_hint("tests/test_mod.py", "tests.test_mod", "t_flaky")
        == "pytest tests/test_mod.py::t_flaky"
    )
    assert (
        github_integration._run_hint("src/a.test.ts", "src.a", "t_flaky") == 'npx vitest run src/a.test.ts -t "t_flaky"'
    )
    assert github_integration._run_hint("pkg/foo_test.go", "pkg", "TestFoo") == "go test ./... -run TestFoo"
    assert (
        github_integration._run_hint("src/Foo.java", "com.acme.Foo", "testBar")
        == "mvn test -Dtest=com.acme.Foo#testBar"
    )
    # No file path reported, or an extension FlakeRadar doesn't recognize: fall
    # back to a generic hint rather than guessing a framework wrong.
    assert github_integration._run_hint(None, "tests.test_mod", "t_flaky") == (
        "# Re-run `tests.test_mod::t_flaky` via your project's test runner (no file path reported)."
    )
    assert github_integration._run_hint("script.weird", "c", "n") == (
        "# Re-run `c::n` via your project's test runner (script.weird)."
    )


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


async def test_custom_label_is_used(db, gh_settings):
    gh_settings.github_issue_label = "flaky"
    tc = await _flaky_test(db)
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [tc.id], transport=transport)
    body = json.loads(calls[0].content)
    assert body["labels"] == ["flaky"]


async def test_min_proven_flakes_gate(db, gh_settings):
    gh_settings.github_issue_min_proven_flakes = 2
    # Only one proven flake -> not filed.
    tc = await _flaky_test(db, name="under", project="p1", confirmed_flake_count=1)
    await db.commit()
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [tc.id], transport=transport)
    assert calls == []
    # Two proven flakes -> filed.
    tc2 = await _flaky_test(db, name="over", project="p2", confirmed_flake_count=2)
    await db.commit()
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [tc2.id], transport=transport)
    assert len(calls) == 1


async def _add_failure(db, tc, sha="cafe1234"):
    proj = (await db.execute(select(Project).join(TestCase, TestCase.project_id == Project.id))).scalar_one()
    run = await make_run(db, proj, commit_sha=sha)
    await make_execution(db, tc, run, status="failed", details="boom2")
    await db.commit()


async def test_min_failures_gate(db, gh_settings):
    gh_settings.github_issue_min_failures = 2
    tc = await _flaky_test(db, name="t", failure_details="boom")  # one failure in window
    await db.commit()
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [tc.id], transport=transport)
    assert calls == []

    # Add a second failing execution so the window has two failures.
    await _add_failure(db, tc)
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [tc.id], transport=transport)
    assert len(calls) == 1


async def test_score_gate_from_issue_min_score(db, gh_settings):
    gh_settings.github_issue_min_score = 0.9
    tc = await _flaky_test(db, name="t", flakiness_score=0.6)
    await db.commit()
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [tc.id], transport=transport)
    assert calls == []


async def test_sync_rearms_closed_issue_only(db, gh_settings, session_factory):
    closed = await _flaky_test(db, name="closed", github_issue_number=11)
    still_open = await _flaky_test(db, name="open", repo="acme/keep", github_issue_number=22)
    state = {"/acme/app/issues/11": "closed", "/acme/keep/issues/22": "open"}
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        for key, value in state.items():
            if key in request.url.path:
                return httpx.Response(200, json={"state": value})
        return httpx.Response(404)

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    await db.refresh(closed)
    await db.refresh(still_open)
    assert closed.github_issue_number is None
    assert still_open.github_issue_number == 22


async def test_sync_never_raises_and_rate_limit_stops(db, gh_settings, session_factory):
    await _flaky_test(db, name="a", repo="acme/a", github_issue_number=1)
    await _flaky_test(db, name="b", repo="acme/b", github_issue_number=2)

    def boom(request):
        raise httpx.ConnectError("offline")

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(boom))  # no raise

    calls: list[httpx.Request] = []

    def limited(request):
        calls.append(request)
        return httpx.Response(403, headers={"x-ratelimit-remaining": "0"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(limited))
    assert len(calls) == 1


async def test_hook_ignores_failed_reports(session_factory, gh_settings, monkeypatch):
    called = []

    async def fake_file(db, ids, **kw):
        called.append(ids)

    monkeypatch.setattr(github_integration, "file_issues_for", fake_file)
    failed = ProcessOutcome(report_id=1, status="failed", run_id=None, counts=None, touched_test_ids=[], error="x")
    done = ProcessOutcome(report_id=2, status="processed", run_id=1, counts={}, touched_test_ids=[3, 4], error=None)
    await github_integration.on_report_processed(session_factory, failed)
    await github_integration.on_report_processed(session_factory, done)
    assert called == [[3, 4]]


async def test_many_touched_ids_stay_under_the_bind_limit(db, gh_settings):
    # A huge Report touches more Tests than asyncpg can bind (32,767) in one IN list.
    tc = await _flaky_test(db)
    calls, transport = _recorder()
    ids = sorted([tc.id, *range(10_000_000, 10_040_000)])
    await github_integration.file_issues_for(db, ids, transport=transport)
    assert len(calls) == 1
