"""GitHub issues are filed in each Test's own Repo; failures never raise."""

import json
import logging

import httpx
import pytest
from app import github_integration, issue_links
from app.config import Settings
from app.models import Project, Repo, TestCase
from app.processing import ProcessOutcome
from sqlalchemy import select

from tests.factories import (
    make_execution,
    make_job,
    make_job_execution,
    make_pipeline,
    make_project,
    make_run,
    make_test_case,
)


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


async def test_issue_body_omits_the_line_anchor_when_no_line_is_reported(db, gh_settings):
    tc = await _flaky_test(db, file="tests/test_x.py", line=None)
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [tc.id], transport=transport)

    (req,) = calls
    assert req.url.path == "/repos/acme/app/issues"
    body = json.loads(req.content)["body"]
    # The runner reported a file but no line: the permalink is still useful,
    # it just carries no anchor.
    assert "https://github.com/acme/app/blob/deadbeef/tests/test_x.py" in body
    assert "#L" not in body
    assert "`tests/test_x.py`" in body


async def test_issue_body_without_a_failing_execution(db, gh_settings):
    # A Test can still be a candidate with only passed Executions in the window
    # (the gates here are 0): the body then carries no failing-commit evidence.
    proj = await make_project(db, "acme/app", "backend")
    tc = await make_test_case(
        db,
        proj,
        name="t_passed",
        flakiness_score=0.6,
        confirmed_flake_count=1,
        file="tests/test_x.py",
        line=7,
    )
    run = await make_run(db, proj, commit_sha="deadbeef")
    await make_execution(db, tc, run, status="passed")
    await db.commit()

    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [tc.id], transport=transport)

    (req,) = calls
    assert req.url.path == "/repos/acme/app/issues"
    body = json.loads(req.content)["body"]
    assert "Last failing commit" not in body
    assert "Permalink" not in body
    assert "### Sample failure" not in body
    # The rest of the body is still there, so the absence above is about the
    # missing failure and not about an empty render.
    assert "### Last 10 executions" in body
    assert "| passed | `deadbeef` | main |" in body
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


def test_rate_limited_classifies_only_throttling():
    """429s and real rate-limit 403s only; a permission 403 must not look like one."""
    rl = github_integration._rate_limited

    def resp(status: int, *, headers=None, body=None, raw: str | None = None) -> httpx.Response:
        if raw is not None:
            return httpx.Response(status, headers=headers or {}, text=raw)
        return httpx.Response(status, headers=headers or {}, json=body if body is not None else {})

    assert rl(resp(429))
    assert rl(resp(403, headers={"x-ratelimit-remaining": "0"}))
    assert rl(resp(403, headers={"retry-after": "60"}))
    assert rl(resp(403, body={"message": "You have exceeded a secondary rate limit"}))
    assert rl(resp(403, body={"message": "You have exceeded a Secondary Rate Limit"}))
    assert not rl(resp(403, body={"message": "Resource not accessible by personal access token"}))
    assert not rl(resp(403, body={}))
    assert not rl(resp(403, raw="<html>Gateway</html>"))  # non-JSON body must not raise
    assert not rl(resp(403, raw="[1, 2]"))  # JSON, but not an object
    assert not rl(resp(403, raw="null"))
    assert not rl(resp(404))
    assert not rl(resp(410))
    assert not rl(resp(201, body={"number": 5}))


async def test_does_not_refile_or_file_below_threshold(db, gh_settings):
    filed = await _flaky_test(db, name="filed", github_issue_number=5)
    proj = await make_project(db, "acme/other")
    calm = await make_test_case(db, proj, name="calm", flakiness_score=0.1)
    await db.commit()
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [filed.id, calm.id], transport=transport)
    assert calls == []


async def test_rate_limit_stops_batch_for_tests_and_jobs(db, gh_settings):
    """A rate limit is a batch stop: one request per entry point, Tests and Jobs."""
    a = await _flaky_test(db, repo="acme/a")
    b = await _flaky_test(db, repo="acme/b")
    calls, transport = _recorder(status=403, headers={"x-ratelimit-remaining": "0"})
    await github_integration.file_issues_for(db, [a.id, b.id], transport=transport)
    assert len(calls) == 1

    ja = await _flaky_job(db, repo="acme/c", name="a", pipeline=".github/workflows/a.yml")
    jb = await _flaky_job(db, repo="acme/d", name="b", pipeline=".github/workflows/b.yml")
    calls, transport = _recorder(status=403, headers={"x-ratelimit-remaining": "0"})
    await github_integration.file_job_issues_for(db, [ja.id, jb.id], transport=transport)
    assert len(calls) == 1

    # A 404 is not a rate limit: the batch goes on to the other repo.
    calls, transport = _recorder(status=404)
    await github_integration.file_issues_for(db, [a.id, b.id], transport=transport)
    assert len(calls) == 2
    await db.refresh(a)
    assert a.github_issue_number is None


async def test_permission_403_skips_repo_and_files_other_repos_for_tests_and_jobs(db, gh_settings, caplog):
    """A 403 that is not a rate limit (bad token for one repo) costs only that repo."""
    # A body longer than 300 characters: only its first 300 belong in the log.
    detail = "Resource not accessible by personal access token" + "x" * 400
    body = json.dumps({"message": detail})

    a1 = await _flaky_test(db, repo="acme/a", project="p1", name="a1")
    a2 = await _flaky_test(db, repo="acme/a", project="p2", name="a2")
    b = await _flaky_test(db, repo="acme/b", name="b")
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(403, text=body)
        return httpx.Response(201, json={"number": 88})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.file_issues_for(db, [a1.id, a2.id, b.id], transport=httpx.MockTransport(handler))
    # a2 is never retried: the whole repo is skipped for the rest of the batch.
    assert [r.url.path for r in calls] == ["/repos/acme/a/issues", "/repos/acme/b/issues"]
    warnings = [r.getMessage() for r in caplog.records if "skipped for repo" in r.getMessage()]
    assert len(warnings) == 1
    assert "acme/a" in warnings[0] and "403" in warnings[0]
    assert body[:300] in warnings[0] and body[300:] not in warnings[0]
    await db.refresh(a1)
    await db.refresh(a2)
    await db.refresh(b)
    assert a1.github_issue_number is None
    assert a2.github_issue_number is None
    assert b.github_issue_number == 88

    caplog.clear()

    ja1 = await _flaky_job(db, repo="acme/a", name="a1", pipeline=".github/workflows/a1.yml")
    ja2 = await _flaky_job(db, repo="acme/a", name="a2", pipeline=".github/workflows/a2.yml")
    jb = await _flaky_job(db, repo="acme/b", name="b", pipeline=".github/workflows/b.yml")
    job_calls: list[httpx.Request] = []

    def job_handler(request: httpx.Request) -> httpx.Response:
        job_calls.append(request)
        if request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(403, text=body)
        return httpx.Response(201, json={"number": 88})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.file_job_issues_for(
            db, [ja1.id, ja2.id, jb.id], transport=httpx.MockTransport(job_handler)
        )
    # ja2 is never retried: the whole repo is skipped for the rest of the batch.
    assert [r.url.path for r in job_calls] == ["/repos/acme/a/issues", "/repos/acme/b/issues"]
    warnings = [r.getMessage() for r in caplog.records if "skipped for repo" in r.getMessage()]
    assert len(warnings) == 1
    assert "acme/a" in warnings[0] and "403" in warnings[0]
    assert body[:300] in warnings[0] and body[300:] not in warnings[0]
    await db.refresh(ja1)
    await db.refresh(ja2)
    await db.refresh(jb)
    assert ja1.github_issue_number is None
    assert ja2.github_issue_number is None
    assert jb.github_issue_number == 88


@pytest.mark.parametrize("status", [404, 410])
async def test_404_and_410_skip_repo_for_rest_of_batch(db, gh_settings, caplog, status):
    """Issues disabled (404) or a moved repo (410): one warning per repo, then go on."""
    a1 = await _flaky_test(db, repo="acme/a", project="p1", name="a1")
    a2 = await _flaky_test(db, repo="acme/a", project="p2", name="a2")
    b = await _flaky_test(db, repo="acme/b", name="b")
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(status, json={"message": "Not Found"})
        return httpx.Response(201, json={"number": 99})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.file_issues_for(db, [a1.id, a2.id, b.id], transport=httpx.MockTransport(handler))
    # a2 is never retried: the repo is skipped for the rest of the batch.
    assert [r.url.path for r in calls] == ["/repos/acme/a/issues", "/repos/acme/b/issues"]
    skip_warnings = [r.getMessage() for r in caplog.records if "skipped for repo acme/a" in r.getMessage()]
    assert len(skip_warnings) == 1
    assert str(status) in skip_warnings[0]
    await db.refresh(b)
    assert b.github_issue_number == 99

    caplog.clear()

    ja1 = await _flaky_job(db, repo="acme/a", name="a1", pipeline=".github/workflows/a1.yml")
    ja2 = await _flaky_job(db, repo="acme/a", name="a2", pipeline=".github/workflows/a2.yml")
    jb = await _flaky_job(db, repo="acme/b", name="b", pipeline=".github/workflows/b.yml")
    job_calls: list[httpx.Request] = []

    def job_handler(request: httpx.Request) -> httpx.Response:
        job_calls.append(request)
        if request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(status, json={"message": "Not Found"})
        return httpx.Response(201, json={"number": 99})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.file_job_issues_for(
            db, [ja1.id, ja2.id, jb.id], transport=httpx.MockTransport(job_handler)
        )
    # ja2 is never retried: the repo is skipped for the rest of the batch.
    assert [r.url.path for r in job_calls] == ["/repos/acme/a/issues", "/repos/acme/b/issues"]
    skip_warnings = [r.getMessage() for r in caplog.records if "skipped for repo acme/a" in r.getMessage()]
    assert len(skip_warnings) == 1
    assert str(status) in skip_warnings[0]
    await db.refresh(ja1)
    await db.refresh(ja2)
    await db.refresh(jb)
    assert ja1.github_issue_number is None
    assert ja2.github_issue_number is None
    assert jb.github_issue_number == 99


async def test_unusable_status_is_logged_and_the_batch_goes_on_for_tests_and_jobs(db, gh_settings, caplog):
    """A status that is not 201, a rate limit or 403/404/410 costs one issue only.

    A 500 says nothing about the Repo, so it must not skip it for the rest of
    the batch: the second candidate in the same Repo is still posted.
    """
    a1 = await _flaky_test(db, repo="acme/a", project="p1", name="a1")
    a2 = await _flaky_test(db, repo="acme/a", project="p2", name="a2")
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if len(calls) == 1:
            return httpx.Response(500, text="server exploded")
        return httpx.Response(201, json={"number": 77})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.file_issues_for(db, [a1.id, a2.id], transport=httpx.MockTransport(handler))
    # a2 is tried despite a1's 500: two POSTs to the very same Repo.
    assert [r.url.path for r in calls] == ["/repos/acme/a/issues", "/repos/acme/a/issues"]
    await db.refresh(a1)
    await db.refresh(a2)
    assert a1.github_issue_number is None
    assert a2.github_issue_number == 77
    messages = [r.getMessage() for r in caplog.records]
    failures = [m for m in messages if "GitHub issue creation failed for test" in m and "500" in m]
    assert len(failures) == 1
    assert "acme/a" in failures[0] and "server exploded" in failures[0]
    assert not [m for m in messages if "skipped for repo" in m]

    caplog.clear()

    j1 = await _flaky_job(db, repo="acme/a", name="j1", pipeline=".github/workflows/j1.yml")
    j2 = await _flaky_job(db, repo="acme/a", name="j2", pipeline=".github/workflows/j2.yml")
    job_calls: list[httpx.Request] = []

    def job_handler(request: httpx.Request) -> httpx.Response:
        job_calls.append(request)
        if len(job_calls) == 1:
            return httpx.Response(500, text="server exploded")
        return httpx.Response(201, json={"number": 77})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.file_job_issues_for(
            db, [j1.id, j2.id], transport=httpx.MockTransport(job_handler)
        )
    # j2 is tried despite j1's 500: two POSTs to the very same Repo.
    assert [r.url.path for r in job_calls] == ["/repos/acme/a/issues", "/repos/acme/a/issues"]
    await db.refresh(j1)
    await db.refresh(j2)
    assert j1.github_issue_number is None
    assert j2.github_issue_number == 77
    messages = [r.getMessage() for r in caplog.records]
    failures = [m for m in messages if "GitHub issue creation failed for job" in m and "500" in m]
    assert len(failures) == 1
    assert "acme/a" in failures[0] and "server exploded" in failures[0]
    assert not [m for m in messages if "skipped for repo" in m]


@pytest.mark.parametrize(
    "raw",
    ["<html>Bad Gateway</html>", "[1, 2]", "null", '{"number": "not-a-number"}', "{}"],
)
async def test_malformed_201_body_never_raises_and_keeps_the_batch_for_tests_and_jobs(db, gh_settings, caplog, raw):
    """A 201 whose body is not a usable issue is logged, not stored and not raised."""
    a = await _flaky_test(db, repo="acme/a", name="a")
    b = await _flaky_test(db, repo="acme/b", name="b")
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(201, text=raw)
        return httpx.Response(201, json={"number": 91})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.file_issues_for(db, [a.id, b.id], transport=httpx.MockTransport(handler))
    assert [r.url.path for r in calls] == ["/repos/acme/a/issues", "/repos/acme/b/issues"]
    await db.refresh(a)
    await db.refresh(b)
    assert a.github_issue_number is None  # nothing stored, so a later Report can re-file
    assert b.github_issue_number == 91
    warnings = [r.getMessage() for r in caplog.records if "no issue number" in r.getMessage()]
    assert len(warnings) == 1
    assert "acme/a" in warnings[0] and raw[:300] in warnings[0]

    caplog.clear()

    ja = await _flaky_job(db, repo="acme/a", name="a", pipeline=".github/workflows/a.yml")
    jb = await _flaky_job(db, repo="acme/b", name="b", pipeline=".github/workflows/b.yml")
    job_calls: list[httpx.Request] = []

    def job_handler(request: httpx.Request) -> httpx.Response:
        job_calls.append(request)
        if request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(201, text=raw)
        return httpx.Response(201, json={"number": 92})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.file_job_issues_for(db, [ja.id, jb.id], transport=httpx.MockTransport(job_handler))
    assert [r.url.path for r in job_calls] == ["/repos/acme/a/issues", "/repos/acme/b/issues"]
    await db.refresh(ja)
    await db.refresh(jb)
    assert ja.github_issue_number is None  # nothing stored, so a later Report can re-file
    assert jb.github_issue_number == 92
    warnings = [r.getMessage() for r in caplog.records if "no issue number" in r.getMessage()]
    assert len(warnings) == 1
    assert "acme/a" in warnings[0] and raw[:300] in warnings[0]


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


async def test_sync_rotates_past_the_cap(db, gh_settings, session_factory):
    """With a cap of 2 and 3 linked Tests, the third Test is checked on the second run."""
    gh_settings.github_sync_batch = 2
    first = await _flaky_test(db, project="p1", name="t1", github_issue_number=11)
    await _flaky_test(db, project="p2", name="t2", github_issue_number=22)
    await _flaky_test(db, project="p3", name="t3", github_issue_number=33)
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        return httpx.Response(200, json={"state": "open"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    # Every stamp is NULL, so the first run takes the two lowest ids and clears nothing.
    assert [r.url.path for r in calls] == ["/repos/acme/app/issues/11", "/repos/acme/app/issues/22"]
    await db.refresh(first)
    assert first.github_issue_number == 11
    assert first.github_issue_checked_at is not None  # an open issue is still a check

    calls.clear()
    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    # The never-checked Test is in the batch now, next to the oldest stamped row.
    assert [r.url.path for r in calls] == ["/repos/acme/app/issues/33", "/repos/acme/app/issues/11"]


async def test_sync_checks_never_checked_links_first(db, gh_settings, session_factory):
    """A link that was never checked goes before links checked in an earlier run."""
    gh_settings.github_sync_batch = 2
    await _flaky_test(db, project="p1", name="t1", github_issue_number=11)
    await _flaky_test(db, project="p2", name="t2", github_issue_number=22)
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        return httpx.Response(200, json={"state": "open"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    assert len(calls) == 2  # both links carry a stamp from this run

    fresh = await _flaky_test(db, project="p3", name="t3", github_issue_number=33)
    gh_settings.github_sync_batch = 1
    calls.clear()
    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    assert [r.url.path for r in calls] == ["/repos/acme/app/issues/33"]
    await db.refresh(fresh)
    assert fresh.github_issue_checked_at is not None


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


async def test_sync_commits_stamps_before_an_early_stop(db, gh_settings, session_factory):
    """A rate-limit stop still commits the stamps of the rows checked before it."""
    a = await _flaky_test(db, name="a", repo="acme/a", github_issue_number=1)
    b = await _flaky_test(db, name="b", repo="acme/b", github_issue_number=2)

    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.path == "/repos/acme/a/issues/1":
            return httpx.Response(200, json={"state": "open"})
        return httpx.Response(403, headers={"x-ratelimit-remaining": "0"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    # The stop happens on the second request, after a was checked.
    assert [r.url.path for r in calls] == ["/repos/acme/a/issues/1", "/repos/acme/b/issues/2"]
    await db.refresh(a)
    await db.refresh(b)
    assert a.github_issue_checked_at is not None  # committed despite the early stop
    assert b.github_issue_checked_at is None  # the rate-limited row was never stamped

    calls.clear()

    def open_handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        return httpx.Response(200, json={"state": "open"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(open_handler))
    # Rotation continues from the committed stamp: the unstamped row goes first.
    assert [r.url.path for r in calls] == ["/repos/acme/b/issues/2", "/repos/acme/a/issues/1"]


async def test_sync_permission_403_keeps_link_and_syncs_other_repos(db, gh_settings, session_factory):
    denied = await _flaky_test(db, name="denied", repo="acme/a", github_issue_number=11)
    closed = await _flaky_test(db, name="closed", repo="acme/b", github_issue_number=22)

    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(403, json={"message": "Resource not accessible by personal access token"})
        return httpx.Response(200, json={"state": "closed"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    await db.refresh(denied)
    await db.refresh(closed)
    # A permission 403 is not a rate limit: the link stays as it is and repo B is still synced.
    assert denied.github_issue_number == 11
    assert denied.github_issue_checked_at is not None  # the denied row rotates like a checked one
    assert closed.github_issue_number is None


async def test_sync_permission_403_skips_later_rows_in_that_repo(db, gh_settings, session_factory):
    a1 = await _flaky_test(db, repo="acme/a", project="p1", name="a1", github_issue_number=11)
    a2 = await _flaky_test(db, repo="acme/a", project="p2", name="a2", github_issue_number=12)
    b = await _flaky_test(db, repo="acme/b", name="b", github_issue_number=22)
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(403, json={"message": "Resource not accessible by personal access token"})
        return httpx.Response(200, json={"state": "closed"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    # acme/a is skipped for the rest of the sync: its later row issues no GET.
    assert sorted(r.url.path for r in calls) == [
        "/repos/acme/a/issues/11",
        "/repos/acme/b/issues/22",
    ]
    await db.refresh(a1)
    await db.refresh(a2)
    await db.refresh(b)
    assert a1.github_issue_number == 11
    assert a2.github_issue_number == 12
    assert b.github_issue_number is None
    # The 403 row and the row skipped for its Repo both rotate instead of pinning the queue.
    assert a1.github_issue_checked_at is not None
    assert a2.github_issue_checked_at is not None


async def test_sync_rotates_past_a_denied_repo(db, gh_settings, session_factory):
    """A permanently denied Repo cannot pin the queue: the next run reaches the other Repo."""
    gh_settings.github_sync_batch = 2
    a1 = await _flaky_test(db, repo="acme/a", project="p1", name="a1", github_issue_number=11)
    a2 = await _flaky_test(db, repo="acme/a", project="p2", name="a2", github_issue_number=12)
    b = await _flaky_test(db, repo="acme/b", name="b", github_issue_number=22)
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(403, json={"message": "Resource not accessible by personal access token"})
        return httpx.Response(200, json={"state": "closed"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    # The cap holds a1 and a2: a1 takes the 403, a2 is skipped for its Repo and issues no GET.
    assert [r.url.path for r in calls] == ["/repos/acme/a/issues/11"]

    calls.clear()
    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    # The denied rows were stamped, so the never-checked Test of the other Repo sorts first.
    assert [r.url.path for r in calls] == ["/repos/acme/b/issues/22", "/repos/acme/a/issues/11"]
    await db.refresh(a1)
    await db.refresh(a2)
    await db.refresh(b)
    assert a1.github_issue_number == 11  # a permission 403 clears nothing
    assert a2.github_issue_number == 12
    assert b.github_issue_number is None  # rotation reached repo B and its closed issue re-arms
    assert b.github_issue_checked_at is not None


@pytest.mark.parametrize("status", [401, 410])
async def test_sync_definitive_client_error_keeps_link_and_skips_repo(db, gh_settings, session_factory, status):
    """A bad-credentials 401 or a gone-Repo 410 behaves like a permission 403.

    Anything else would leave those rows forever unstamped, at the front of
    the NULLS FIRST queue, starving the other Repos' links of batch slots.
    """
    a1 = await _flaky_test(db, repo="acme/a", project="p1", name="a1", github_issue_number=11)
    a2 = await _flaky_test(db, repo="acme/a", project="p2", name="a2", github_issue_number=12)
    closed = await _flaky_test(db, repo="acme/b", name="closed", github_issue_number=22)
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(status, json={"message": "no"})
        return httpx.Response(200, json={"state": "closed"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    # a1 takes the error, a2 is skipped for its Repo without a GET, repo B still syncs.
    assert [r.url.path for r in calls] == ["/repos/acme/a/issues/11", "/repos/acme/b/issues/22"]
    await db.refresh(a1)
    await db.refresh(a2)
    await db.refresh(closed)
    assert a1.github_issue_number == 11  # a definitive client error clears nothing
    assert a1.github_issue_checked_at is not None  # but the row rotates like a checked one
    assert a2.github_issue_checked_at is not None
    assert closed.github_issue_number is None  # the other Repos' links still re-arm


@pytest.mark.parametrize("raw", ["", "<html>Gateway</html>", "[1, 2]", "null", '{"state": ["closed"]}'])
async def test_sync_malformed_200_body_never_raises_and_keeps_the_link(db, gh_settings, session_factory, caplog, raw):
    """An unreadable 200 says nothing about the issue: keep the link, log it, go on."""
    unreadable = await _flaky_test(db, repo="acme/a", name="a", github_issue_number=11)
    closed = await _flaky_test(db, repo="acme/b", name="b", github_issue_number=22)
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(200, text=raw)
        return httpx.Response(200, json={"state": "closed"})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    assert [r.url.path for r in calls] == ["/repos/acme/a/issues/11", "/repos/acme/b/issues/22"]
    await db.refresh(unreadable)
    await db.refresh(closed)
    assert unreadable.github_issue_number == 11
    assert closed.github_issue_number is None
    warnings = [r.getMessage() for r in caplog.records if "unusable" in r.getMessage()]
    assert len(warnings) == 1
    assert "acme/a#11" in warnings[0]


async def test_sync_rotates_past_an_unreadable_link(db, gh_settings, session_factory):
    """A permanently unreadable 200 body rotates instead of starving the other links."""
    gh_settings.github_sync_batch = 2
    bad = await _flaky_test(db, repo="acme/a", project="p1", name="a", github_issue_number=11)
    await _flaky_test(db, repo="acme/b", project="p2", name="b", github_issue_number=22)
    third = await _flaky_test(db, repo="acme/c", project="p3", name="c", github_issue_number=33)
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(200, text="<html>Gateway</html>")
        return httpx.Response(200, json={"state": "open"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    # Every stamp is NULL, so the first run takes the two lowest ids.
    assert [r.url.path for r in calls] == ["/repos/acme/a/issues/11", "/repos/acme/b/issues/22"]
    await db.refresh(bad)
    assert bad.github_issue_number == 11  # an unusable body clears nothing
    assert bad.github_issue_checked_at is not None  # but the row rotates like a checked one

    calls.clear()
    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    # The never-checked Test reaches the batch instead of queueing behind the bad link forever.
    assert [r.url.path for r in calls] == ["/repos/acme/c/issues/33", "/repos/acme/a/issues/11"]
    await db.refresh(third)
    assert third.github_issue_checked_at is not None


@pytest.mark.parametrize("status", [404, 500])
@pytest.mark.parametrize("entity", ["test", "job"])
async def test_sync_gone_or_unexpected_status(db, gh_settings, session_factory, caplog, status, entity):
    """A gone issue re-arms its link; any other answer keeps it but stamps the row.

    Only a 404 says the issue is unreachable for dedup purposes. There is no
    branch for the rest: an unusable answer leaves the link alone, yet the row
    is still stamped, so it cannot take a batch slot on every later run.
    """
    expected = 11 if entity == "test" else 5
    if entity == "test":
        obj = await _flaky_test(db, name="linked", github_issue_number=expected)
    else:
        obj = await _flaky_job(db, name="linked", github_issue_number=expected)
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        return httpx.Response(status, json={"message": "Gone" if status == 404 else "Server error"})

    # INFO because the re-arm of a gone issue is logged at that level.
    with caplog.at_level(logging.INFO, logger="flakeradar.github"):
        await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))

    # Exactly one GET, for the linked issue itself: a wrong path could answer 404
    # for some other reason and still leave the row looking correctly re-armed.
    assert [r.url.path for r in calls] == [f"/repos/acme/app/issues/{expected}"]
    await db.refresh(obj)
    if status == 404:
        assert obj.github_issue_number is None  # re-armed, so the next Report can re-file
    else:
        assert obj.github_issue_number == expected  # an unusable answer clears nothing
    assert obj.github_issue_checked_at is not None  # either way the row rotates like a checked one

    gone = [r for r in caplog.records if r.name == "flakeradar.github" and "Issue gone" in r.getMessage()]
    if status == 404:
        assert len(gone) == 1
        assert gone[0].levelno == logging.INFO
        assert f"acme/app#{expected}" in gone[0].getMessage()
        assert entity in gone[0].getMessage()  # the line says which kind of link re-armed
    else:
        assert gone == []
        # An unexpected status is nobody's fault: no warning about this Repo, so
        # nothing skipped it and nothing was reported as unusable.
        warned = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
        assert [m for m in warned if "acme/app" in m] == []


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


# --- Job issue filing (task 10) ------------------------------------------


async def _flaky_job(
    db,
    provider="github",
    name="e2e (ubuntu-latest)",
    *,
    repo="acme/app",
    pipeline=".github/workflows/ci.yml",
    flakiness_score=0.6,
    confirmed_flake_count=1,
    **fields,
):
    pipe = await make_pipeline(db, repo, pipeline, provider=provider)
    job = await make_job(
        db, pipe, name=name, flakiness_score=flakiness_score, confirmed_flake_count=confirmed_flake_count, **fields
    )
    await db.commit()
    return job


async def _job_exec(db, job, ci_job_id, explained=False, status="failed"):
    """Add a Job execution, optionally linked to a failing Test (explained)."""
    await make_job_execution(db, job, status=status, ci_job_id=ci_job_id, branch="main")
    if explained:
        proj = (
            await db.execute(
                select(Project)
                .join(Repo, Project.repo_id == Repo.id)
                .where(Repo.name == "acme/app", Project.name == "backend")
            )
        ).scalar_one_or_none()
        if proj is None:
            proj = await make_project(db, "acme/app", "backend")
        tc = await make_test_case(db, proj, name=f"t-{ci_job_id}")
        run = await make_run(db, proj, commit_sha="s", ci_job_id=ci_job_id)
        await make_execution(db, tc, run, status="failed")
    await db.commit()


async def test_job_files_issue_with_expected_title_and_label(db, gh_settings):
    job = await _flaky_job(db)
    await _job_exec(db, job, "1", explained=True)
    calls, transport = _recorder()
    await github_integration.file_job_issues_for(db, [job.id], transport=transport)
    (req,) = calls
    assert req.url.path == "/repos/acme/app/issues"
    body = json.loads(req.content)
    assert body["title"] == "[FlakeRadar] Flaky CI job: .github/workflows/ci.yml / e2e (ubuntu-latest)"
    assert body["labels"] == ["flakeradar"]
    assert "**Job:** `e2e (ubuntu-latest)`" in body["body"]
    assert "Unexplained failures" in body["body"] and "Explained by tests" in body["body"]
    await db.refresh(job)
    assert job.github_issue_number == 77


async def test_job_issue_body_lists_top_runner_groups(db, gh_settings):
    job = await _flaky_job(db)
    for _ in range(2):
        await make_job_execution(db, job, status="failed", runner_labels=["self-hosted", "gpu"])
    await make_job_execution(db, job, status="failed", runner_labels=["ubuntu-latest"])
    await make_job_execution(db, job, status="passed", runner_labels=["windows-latest"])  # no unexplained
    await db.commit()
    calls, transport = _recorder()
    await github_integration.file_job_issues_for(db, [job.id], transport=transport)
    (req,) = calls
    body = json.loads(req.content)["body"]
    assert "**Failures by runner:**" in body
    gpu_line = "- `gpu, self-hosted`: 2 unexplained of 2 executions"  # labels are the sorted set
    ubuntu_line = "- `ubuntu-latest`: 1 unexplained of 1 executions"
    assert gpu_line in body
    assert ubuntu_line in body
    assert body.index(gpu_line) < body.index(ubuntu_line)  # worst group first
    assert "windows-latest" not in body  # zero-unexplained groups are not listed


async def test_job_issue_body_caps_runner_groups_at_three(db, gh_settings):
    job = await _flaky_job(db, name="many-runners", pipeline=".github/workflows/many.yml")
    for label, failures in (("a-lab", 3), ("b-lab", 2), ("c-lab", 1), ("d-lab", 1)):
        for _ in range(failures):
            await make_job_execution(db, job, status="failed", runner_labels=[label])
    # d-lab's rate (0.5) is the worst-ranked of the four: one extra passing execution pushes it out of the top 3.
    await make_job_execution(db, job, status="passed", runner_labels=["d-lab"])
    await db.commit()
    calls, transport = _recorder()
    await github_integration.file_job_issues_for(db, [job.id], transport=transport)
    (req,) = calls
    body = json.loads(req.content)["body"]
    assert "- `a-lab`: 3 unexplained of 3 executions" in body
    assert "- `b-lab`: 2 unexplained of 2 executions" in body
    assert "- `c-lab`: 1 unexplained of 1 executions" in body
    assert "d-lab" not in body


async def test_job_issue_body_omits_runner_section_without_unexplained(db, gh_settings):
    job = await _flaky_job(db, name="explained-only", pipeline=".github/workflows/explained.yml")
    await _job_exec(db, job, "ex-1", explained=True)
    calls, transport = _recorder()
    await github_integration.file_job_issues_for(db, [job.id], transport=transport)
    (req,) = calls
    body = json.loads(req.content)["body"]
    assert "Failures by runner" not in body


async def test_job_issue_body_without_job_history(db, gh_settings, monkeypatch):
    """A Job that vanishes between candidacy and rendering still gets its issue.

    `queries.get_job` returns None when the Job row is gone; the body drops the
    history-derived sections and keeps the empty executions table.
    """

    async def fake_get_job(*args, **kwargs):
        return None

    monkeypatch.setattr("app.github_integration.queries.get_job", fake_get_job)
    job = await _flaky_job(db, name="vanished", pipeline=".github/workflows/vanished.yml")
    calls, transport = _recorder()
    await github_integration.file_job_issues_for(db, [job.id], transport=transport)

    (req,) = calls
    body = json.loads(req.content)["body"]
    assert "**Job:** `vanished`" in body
    assert "**Flakiness score:**" in body
    assert "Unexplained failures" not in body and "Explained by tests" not in body
    assert "### Last 10 executions" in body
    table = body.split("### Last 10 executions", 1)[1]
    assert "| Outcome | Commit | Branch | Attempt | Runner | Job |" in table
    assert "|---|---|---|---|---|---|" in table
    assert "| failed |" not in table and "| passed |" not in table
    await db.refresh(job)
    assert job.github_issue_number == 77


async def test_job_403_skips_the_other_job_in_the_same_repo(db, gh_settings, caplog):
    """A permission 403 skips the whole Repo, including its second Job."""
    j1 = await _flaky_job(db, repo="acme/a", name="j1", pipeline=".github/workflows/j1.yml")
    j2 = await _flaky_job(db, repo="acme/a", name="j2", pipeline=".github/workflows/j2.yml")
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        # No rate-limit headers and no "rate limit" in the message: a permission
        # 403, which skips the repo instead of stopping the batch.
        return httpx.Response(403, json={"message": "Resource not accessible by personal access token"})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.file_job_issues_for(db, [j1.id, j2.id], transport=httpx.MockTransport(handler))
    # j2 is never tried: its Repo was skipped by the first Job's 403.
    assert [r.url.path for r in calls] == ["/repos/acme/a/issues"]
    await db.refresh(j1)
    await db.refresh(j2)
    assert j1.github_issue_number is None
    assert j2.github_issue_number is None
    warnings = [r.getMessage() for r in caplog.records if "skipped for repo acme/a" in r.getMessage()]
    assert len(warnings) == 1


async def test_job_does_not_refile(db, gh_settings):
    job = await _flaky_job(db)
    await _job_exec(db, job, "1")
    _, transport = _recorder()
    await github_integration.file_job_issues_for(db, [job.id], transport=transport)
    calls, transport = _recorder()
    await github_integration.file_job_issues_for(db, [job.id], transport=transport)
    assert calls == []


async def test_gitlab_job_is_not_filed(db, gh_settings):
    job = await _flaky_job(db, provider="gitlab")
    await _job_exec(db, job, "1")
    calls, transport = _recorder()
    await github_integration.file_job_issues_for(db, [job.id], transport=transport)
    assert calls == []


async def test_job_unexplained_failure_gate(db, gh_settings):
    gh_settings.github_issue_min_failures = 2
    # Three failures, all explained -> not filed.
    all_explained = await _flaky_job(db, name="all-explained", pipeline=".github/workflows/a.yml")
    for i in range(3):
        await _job_exec(db, all_explained, f"ex-{i}", explained=True)
    calls, transport = _recorder()
    await github_integration.file_job_issues_for(db, [all_explained.id], transport=transport)
    assert calls == []
    # Two unexplained (plus one explained) -> filed.
    mixed = await _flaky_job(db, name="mixed", pipeline=".github/workflows/b.yml")
    await _job_exec(db, mixed, "mix-1", explained=True)
    await _job_exec(db, mixed, "mix-2")
    await _job_exec(db, mixed, "mix-3")
    calls, transport = _recorder()
    await github_integration.file_job_issues_for(db, [mixed.id], transport=transport)
    assert len(calls) == 1


async def test_job_sync_rearms_closed_issue_and_refiles(db, gh_settings, session_factory):
    job = await _flaky_job(db)
    await _job_exec(db, job, "1")
    _, transport = _recorder()
    await github_integration.file_job_issues_for(db, [job.id], transport=transport)
    await db.refresh(job)
    assert job.github_issue_number == 77

    def closed(request):
        return httpx.Response(200, json={"state": "closed"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(closed))
    await db.refresh(job)
    assert job.github_issue_number is None

    calls, transport = _recorder()
    await github_integration.file_job_issues_for(db, [job.id], transport=transport)
    assert len(calls) == 1
    await db.refresh(job)
    assert job.github_issue_number == 77
    assert job.github_issue_checked_at is None  # a new link is checked first


async def test_429_stops_the_batch(db, gh_settings):
    """A 429 is a rate limit like a primary 403: one request, then stop."""
    ta = await _flaky_test(db, repo="acme/a", name="a")
    tb = await _flaky_test(db, repo="acme/b", name="b")
    calls, transport = _recorder(status=429)
    await github_integration.file_issues_for(db, [ta.id, tb.id], transport=transport)
    assert len(calls) == 1

    ja = await _flaky_job(db, repo="acme/c", name="a", pipeline=".github/workflows/a.yml")
    jb = await _flaky_job(db, repo="acme/d", name="b", pipeline=".github/workflows/b.yml")
    calls, transport = _recorder(status=429)
    await github_integration.file_job_issues_for(db, [ja.id, jb.id], transport=transport)
    assert len(calls) == 1


async def test_job_unconfigured_is_noop(db):
    job = await _flaky_job(db)
    await _job_exec(db, job, "1")
    calls, transport = _recorder()
    await github_integration.file_job_issues_for(db, [job.id], transport=transport)
    assert calls == []


# --- Issue links from other filers (issue #6) ---


async def test_linked_test_is_not_filed(db, gh_settings):
    linked = await _flaky_test(db, name="linked")
    other = await _flaky_test(db, repo="acme/other", name="other")
    await issue_links.link_test_issue(db, linked.id, 300, threshold=0.3)
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [linked.id, other.id], transport=transport)
    assert len(calls) == 1
    assert calls[0].url.path == "/repos/acme/other/issues"
    await db.refresh(linked)
    assert linked.github_issue_number == 300


async def test_linked_job_is_not_filed(db, gh_settings):
    job = await _flaky_job(db)
    await _job_exec(db, job, "1")
    await issue_links.link_job_issue(db, job.id, 301, threshold=0.3)
    calls, transport = _recorder()
    await github_integration.file_job_issues_for(db, [job.id], transport=transport)
    assert calls == []
    await db.refresh(job)
    assert job.github_issue_number == 301


async def test_sync_rearms_a_linked_test_and_the_filer_files_again(db, gh_settings, session_factory):
    tc = await _flaky_test(db)
    await issue_links.link_test_issue(db, tc.id, 300, threshold=0.3)
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        return httpx.Response(200, json={"state": "closed"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    assert len(calls) == 1
    assert calls[0].url.path == "/repos/acme/app/issues/300"
    await db.refresh(tc)
    assert tc.github_issue_number is None

    filed, transport = _recorder()
    await github_integration.file_issues_for(db, [tc.id], transport=transport)
    assert len(filed) == 1
    await db.refresh(tc)
    assert tc.github_issue_number == 77
    assert tc.github_issue_checked_at is None  # a new link is checked first


async def test_sync_keeps_a_linked_issue_while_it_is_open(db, gh_settings, session_factory):
    tc = await _flaky_test(db)
    await issue_links.link_test_issue(db, tc.id, 300, threshold=0.3)

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"state": "open"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    await db.refresh(tc)
    assert tc.github_issue_number == 300


async def test_sync_rearms_a_linked_job(db, gh_settings, session_factory):
    job = await _flaky_job(db)
    await issue_links.link_job_issue(db, job.id, 301, threshold=0.3)

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"state": "closed"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    await db.refresh(job)
    assert job.github_issue_number is None


async def test_relinked_issue_is_checked_first(db, gh_settings, session_factory):
    """A link whose number changed goes before links checked in an earlier run."""
    gh_settings.github_sync_batch = 2
    a = await _flaky_test(db, project="p1", name="t1", github_issue_number=11)
    await _flaky_test(db, project="p2", name="t2", github_issue_number=22)
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        return httpx.Response(200, json={"state": "open"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    assert len(calls) == 2  # both links carry a stamp from this run
    await db.refresh(a)
    assert a.github_issue_checked_at is not None

    await issue_links.link_test_issue(db, a.id, 44, replace=True, threshold=0.3)
    await db.refresh(a)
    assert a.github_issue_number == 44
    assert a.github_issue_checked_at is None  # the new link moves to the front

    gh_settings.github_sync_batch = 1
    calls.clear()
    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    assert [r.url.path for r in calls] == ["/repos/acme/app/issues/44"]


async def test_unlink_clears_the_check_stamp(db, gh_settings, session_factory):
    gh_settings.github_sync_batch = 2
    tc = await _flaky_test(db, project="p1", name="t1", github_issue_number=300)

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"state": "open"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    await db.refresh(tc)
    assert tc.github_issue_checked_at is not None

    await issue_links.unlink_test_issue(db, tc.id, threshold=0.3)
    await db.refresh(tc)
    assert tc.github_issue_number is None
    assert tc.github_issue_checked_at is None


async def test_same_number_link_keeps_the_check_stamp(db, gh_settings, session_factory):
    """Linking the number it already has is a no-op: the stamp stays."""
    gh_settings.github_sync_batch = 2
    tc = await _flaky_test(db, project="p1", name="t1", github_issue_number=300)

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"state": "open"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    await db.refresh(tc)
    before = tc.github_issue_checked_at
    assert before is not None

    await issue_links.link_test_issue(db, tc.id, 300, threshold=0.3)
    await db.refresh(tc)
    assert tc.github_issue_number == 300
    assert tc.github_issue_checked_at == before


async def test_job_link_change_and_unlink_reset_the_check_stamp(db, gh_settings, session_factory):
    gh_settings.github_sync_batch = 2
    job = await _flaky_job(db)
    await _job_exec(db, job, "1")

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"state": "open"})

    await issue_links.link_job_issue(db, job.id, 301, threshold=0.3)
    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    await db.refresh(job)
    assert job.github_issue_checked_at is not None

    await issue_links.link_job_issue(db, job.id, 44, replace=True, threshold=0.3)
    await db.refresh(job)
    assert job.github_issue_number == 44
    assert job.github_issue_checked_at is None

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    await db.refresh(job)
    assert job.github_issue_checked_at is not None

    await issue_links.unlink_job_issue(db, job.id, threshold=0.3)
    await db.refresh(job)
    assert job.github_issue_number is None
    assert job.github_issue_checked_at is None
