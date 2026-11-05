"""GitHub issues are filed in each Test's own Repo; failures never raise."""

import json
import logging

import httpx
import pytest
from app import github_integration, issue_links
from app.config import Settings
from app.models import Job, Project, Repo, TestCase
from app.processing import ProcessOutcome
from sqlalchemy import delete, select, update

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
    assert tc.github_issue_filed is True  # ours, so an admin delete may close it


def _race_handler(session_factory, victims: dict[type, int], numbers: list[int]):
    """MockTransport handler for the admin-delete race.

    Each create POST answers with the next number from `numbers`; while answers
    remain in `victims`, it first deletes that tracked row through a separate
    session, simulating an admin delete committing between the POST and the
    filer's commit. Comment POSTs answer 201, close PATCHes 200.
    """
    calls: list[httpx.Request] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.path.endswith("/issues") and numbers:
            if victims:
                model, row_id = victims.popitem()
                async with session_factory() as other:
                    await other.execute(delete(model).where(model.id == row_id))
                    await other.commit()
            return httpx.Response(201, json={"number": numbers.pop(0)})
        return httpx.Response(201 if request.method == "POST" else 200, json={})

    return calls, httpx.MockTransport(handler)


async def test_admin_delete_race_closes_the_just_filed_issue(db, gh_settings, session_factory, caplog):
    """An admin delete that commits between the create POST and the filer's commit
    must not strand the just-filed issue: the filer closes it best-effort."""
    tc = await _flaky_test(db)
    test_id = tc.id
    calls, transport = _race_handler(session_factory, {TestCase: test_id}, [77])
    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.file_issues_for(db, [test_id], transport=transport)
    assert [(r.method, r.url.path) for r in calls] == [
        ("POST", "/repos/acme/app/issues"),
        ("POST", "/repos/acme/app/issues/77/comments"),
        ("PATCH", "/repos/acme/app/issues/77"),
    ]
    assert any("closed issue" in r.getMessage() and "77" in r.getMessage() for r in caplog.records)


async def test_admin_delete_race_keeps_the_rest_of_the_batch(db, gh_settings, session_factory):
    """The raced filing costs only itself: the second candidate is still filed
    and its number committed."""
    a = await _flaky_test(db, project="p1", name="a")
    b = await _flaky_test(db, project="p2", name="b")
    calls, transport = _race_handler(session_factory, {TestCase: a.id}, [77, 88])
    await github_integration.file_issues_for(db, [a.id, b.id], transport=transport)
    assert [(r.method, r.url.path) for r in calls] == [
        ("POST", "/repos/acme/app/issues"),
        ("POST", "/repos/acme/app/issues/77/comments"),
        ("PATCH", "/repos/acme/app/issues/77"),
        ("POST", "/repos/acme/app/issues"),
    ]
    await db.refresh(b)
    assert b.github_issue_number == 88  # the SAVEPOINT recovery kept the batch alive
    assert b.github_issue_filed is True


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


def _outcome_resp(status, *, headers=None, body=None, raw=None):
    """A plain answered response — all the seam classifier needs."""
    if raw is not None:
        return httpx.Response(status, headers=headers or {}, text=raw)
    return httpx.Response(status, headers=headers or {}, json=body if body is not None else {})


def test_github_outcome_classifies_the_table():
    """Every kind, straight from a plain response: no client, no transport, no raise."""
    outcome = github_integration._github_outcome

    created = outcome(_outcome_resp(201, body={"number": 77}, headers={"x-ratelimit-remaining": "99"}), expect="number")
    assert (created.kind, created.status, created.remaining, created.number) == ("created", 201, "99", 77)

    # A 201 whose body carries no number filed nothing.
    for bodyless in (_outcome_resp(201), _outcome_resp(201, raw="<html>proxy</html>")):
        unusable = outcome(bodyless, expect="number")
        assert unusable.kind == "unusable_body" and unusable.number is None
    # A 200 from a create is not a create, whatever it says.
    assert outcome(_outcome_resp(200, body={"number": 77}), expect="number").kind == "ok"

    # A fetch parses the state, and an answer without one says nothing.
    fetched = outcome(_outcome_resp(200, body={"state": "closed", "state_reason": "not_planned"}), expect="state")
    assert (fetched.kind, fetched.state, fetched.state_reason) == ("ok", "closed", "not_planned")
    assert outcome(_outcome_resp(200, body={"number": 5}), expect="state").kind == "unusable_body"
    assert outcome(_outcome_resp(201, body={"state": "open"}), expect="state").kind == "other"

    # A comment or a close carries no body the caller reads, so it groups on status.
    assert outcome(_outcome_resp(201, body={"html_url": "u"})).kind == "created"
    assert outcome(_outcome_resp(200, body={"state": "closed"})).kind == "ok"
    assert outcome(_outcome_resp(204)).kind == "ok"

    # Throttling wins over every other reading, for each of the three body shapes;
    # the remaining statuses of the loops' grouping come apart.
    for expect in (None, "number", "state"):
        assert outcome(_outcome_resp(429, headers={"retry-after": "60"}), expect=expect).kind == "rate_limited"
        limited = outcome(_outcome_resp(403, headers={"x-ratelimit-remaining": "0"}), expect=expect)
        assert (limited.kind, limited.status, limited.remaining) == ("rate_limited", 403, "0")
        for status, kind in (
            (401, "unauthorized"),
            (403, "repo_denied"),  # a 403 that is not throttling
            (404, "not_found"),
            (410, "gone"),
            (301, "other"),
            (500, "other"),
        ):
            assert outcome(_outcome_resp(status, body={"message": "no"}), expect=expect).kind == kind

    # A body is excerpted to the log budget the warnings already use.
    long_body = json.dumps({"message": "x" * 500})
    denied = outcome(_outcome_resp(403, raw=long_body))
    assert denied.kind == "repo_denied"
    assert denied.excerpt == long_body[: github_integration._GITHUB_EXCERPT_MAX]
    assert len(denied.excerpt) == 300


async def _four_interactions(transport):
    """One of each seam interaction against `transport`, keyed for readable assertions."""
    async with github_integration._GitHubClient("tok", transport=transport) as client:
        return {
            "create": await client.create_issue("acme/app", "title", "body", labels=["flakeradar"]),
            "comment": await client.comment("acme/app", 77, "note"),
            "close": await client.close_issue("acme/app", 77, state_reason="completed"),
            "fetch": await client.fetch_issue("acme/app", 77),
        }


async def test_seam_rate_limit_is_a_rate_limit_for_every_interaction():
    """The representative case: this 403 stops a batch whichever interaction returned it."""

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            403,
            headers={"x-ratelimit-remaining": "0"},
            json={"message": "You have exceeded a rate limit"},
        )

    outcomes = await _four_interactions(httpx.MockTransport(handler))

    assert set(outcomes) == {"create", "comment", "close", "fetch"}
    for name, outcome in outcomes.items():
        assert (outcome.kind, outcome.status, outcome.remaining) == ("rate_limited", 403, "0"), name


async def test_seam_network_failure_is_a_kind_and_never_raises():
    def handler(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("offline", request=request)

    outcomes = await _four_interactions(httpx.MockTransport(handler))

    for name, outcome in outcomes.items():
        assert outcome.kind == "network", name
        assert isinstance(outcome.error, httpx.ConnectError), name
        assert (outcome.status, outcome.excerpt) == (None, ""), name


async def test_seam_client_sends_the_loops_wire_shape():
    """The seam owns base_url, headers, timeout and the transport, so the loops can move verbatim."""
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.method == "GET":
            return httpx.Response(200, json={"state": "open"})
        return httpx.Response(201, json={"number": 77})

    transport = httpx.MockTransport(handler)
    async with github_integration._GitHubClient("tok", transport=transport) as client:
        assert client._client.base_url == github_integration.API_BASE
        assert client._client.timeout == httpx.Timeout(15)
        create = await client.create_issue("acme/app", "title", "body", labels=["flakeradar"])
        comment = await client.comment("acme/app", 77, "note")
        plain_close = await client.close_issue("acme/app", 77)
        reasoned_close = await client.close_issue("acme/app", 77, state_reason="completed")
        fetch = await client.fetch_issue("acme/app", 77)

    assert (create.kind, create.number) == ("created", 77)
    assert comment.kind == "created"
    assert (plain_close.kind, reasoned_close.kind) == ("created", "created")
    assert (fetch.kind, fetch.state) == ("ok", "open")

    assert [(r.method, r.url.path) for r in calls] == [
        ("POST", "/repos/acme/app/issues"),
        ("POST", "/repos/acme/app/issues/77/comments"),
        ("PATCH", "/repos/acme/app/issues/77"),
        ("PATCH", "/repos/acme/app/issues/77"),
        ("GET", "/repos/acme/app/issues/77"),
    ]
    bodies = [json.loads(r.content) for r in calls[:4]]
    assert bodies[0] == {"title": "title", "body": "body", "labels": ["flakeradar"]}
    assert bodies[1] == {"body": "note"}
    assert bodies[2] == {"state": "closed"}  # the post-delete close sends no reason
    assert bodies[3] == {"state": "closed", "state_reason": "completed"}
    for r in calls:
        assert r.headers["Authorization"] == "Bearer tok"
        assert r.headers["Accept"] == "application/vnd.github+json"
        assert r.headers["X-GitHub-Api-Version"] == "2022-11-28"


async def test_does_not_refile_or_file_below_threshold(db, gh_settings):
    filed = await _flaky_test(db, name="filed", github_issue_number=5)
    proj = await make_project(db, "acme/other")
    calm = await make_test_case(db, proj, name="calm", flakiness_score=0.1)
    await db.commit()
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [filed.id, calm.id], transport=transport)
    assert calls == []


async def test_closed_as_not_planned_link_files_no_new_issue(db, gh_settings):
    """Issue #32: a kept not-planned link (see `test_sync_keeps_a_link_closed_as_not_planned`) is still
    a link, so the filing gate skips the row: no POST, and the link and flag survive the filer run."""
    wont_fix = await _flaky_test(
        db, name="wont_fix", github_issue_number=11, github_issue_filed=True, github_issue_closed=True
    )
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [wont_fix.id], transport=transport)
    assert calls == []  # the gate's `github_issue_number.is_(None)` is the dedupe
    await db.refresh(wont_fix)
    assert wont_fix.github_issue_number == 11
    assert wont_fix.github_issue_filed is True
    assert wont_fix.github_issue_closed is True


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
        await github_integration.file_job_issues_for(db, [j1.id, j2.id], transport=httpx.MockTransport(job_handler))
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


async def test_bad_credentials_401_skips_repo_for_rest_of_batch(db, gh_settings, caplog):
    """A 401 (the token itself is refused) is Repo-scoped for filing, like a 403.

    The seam classifies it apart from a permission 403; the loop's answer is the
    same skip, so a second candidate in that Repo is never posted to.
    """
    a1 = await _flaky_test(db, repo="acme/a", project="p1", name="a1")
    a2 = await _flaky_test(db, repo="acme/a", project="p2", name="a2")
    b = await _flaky_test(db, repo="acme/b", name="b")
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(401, json={"message": "Bad credentials"})
        return httpx.Response(201, json={"number": 99})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.file_issues_for(db, [a1.id, a2.id, b.id], transport=httpx.MockTransport(handler))
    assert [r.url.path for r in calls] == ["/repos/acme/a/issues", "/repos/acme/b/issues"]
    skip_warnings = [r.getMessage() for r in caplog.records if "skipped for repo acme/a" in r.getMessage()]
    assert len(skip_warnings) == 1
    assert "401" in skip_warnings[0] and "Bad credentials" in skip_warnings[0]
    await db.refresh(a1)
    await db.refresh(a2)
    await db.refresh(b)
    assert a1.github_issue_number is None
    assert a2.github_issue_number is None
    assert b.github_issue_number == 99


async def test_network_error_mid_batch_costs_the_rest_of_the_batch(db, gh_settings, caplog):
    """A transport error stops the batch where it stands, keeping what it linked.

    Pinned before the seam migration so the kind-based loop cannot turn one dead
    connection into a per-issue error: the first Test keeps its committed number,
    the third is never posted to, and the warning is the batch-stop one.
    """
    a = await _flaky_test(db, repo="acme/a", project="p1", name="a")
    b = await _flaky_test(db, repo="acme/b", project="p2", name="b")
    c = await _flaky_test(db, repo="acme/c", project="p3", name="c")
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if len(calls) == 1:
            return httpx.Response(201, json={"number": 77})
        raise httpx.ConnectError("offline")

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.file_issues_for(db, [a.id, b.id, c.id], transport=httpx.MockTransport(handler))
    # c is never tried: the error ended the batch, not just this one filing.
    assert [r.url.path for r in calls] == ["/repos/acme/a/issues", "/repos/acme/b/issues"]
    await db.refresh(a)
    await db.refresh(b)
    await db.refresh(c)
    assert a.github_issue_number == 77  # committed per issue, so the filing stands
    assert b.github_issue_number is None
    assert c.github_issue_number is None
    warnings = [r.getMessage() for r in caplog.records if "unreachable, skipping test issue filing" in r.getMessage()]
    assert len(warnings) == 1
    assert "offline" in warnings[0]


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


async def test_window_failures_counts_only_the_newest_window(db):
    """More than `window` Executions: failures older than the window do not count."""
    proj = await make_project(db, "acme/win", "p")
    tc_old = await make_test_case(db, proj, name="old_failures")
    tc_new = await make_test_case(db, proj, name="new_failures")
    run = await make_run(db, proj)
    # Inserted oldest-first, so ids ascend in this order per Test.
    old = ["failed", "failed", "passed", "passed", "passed"]  # window (3): no failures
    new = ["failed", "passed", "failed", "passed", "failed"]  # window: 2, one older outside
    for status_old, status_new in zip(old, new):
        await make_execution(db, tc_old, run, status=status_old)
        await make_execution(db, tc_new, run, status=status_new)
    await db.commit()

    counts = await github_integration._window_failures(db, [tc_old.id, tc_new.id], window=3)
    assert counts == {tc_new.id: 2}
    assert counts.get(tc_old.id, 0) == 0


async def test_score_gate_from_issue_min_score(db, gh_settings):
    gh_settings.github_issue_min_score = 0.9
    tc = await _flaky_test(db, name="t", flakiness_score=0.6)
    await db.commit()
    calls, transport = _recorder()
    await github_integration.file_issues_for(db, [tc.id], transport=transport)
    assert calls == []


async def test_sync_rearms_closed_issue_only(db, gh_settings, session_factory):
    closed = await _flaky_test(db, name="closed", github_issue_number=11, github_issue_filed=True)
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
    assert closed.github_issue_filed is None  # the flag resets with the number
    assert still_open.github_issue_number == 22


async def test_sync_keeps_a_link_closed_as_not_planned(db, gh_settings, session_factory):
    """Closed as not planned: keep the link and flag it, so the filer stays quiet (issue #32)."""
    wont_fix = await _flaky_test(db, name="wont_fix", github_issue_number=11, github_issue_filed=True)

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"state": "closed", "state_reason": "not_planned"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    await db.refresh(wont_fix)
    assert wont_fix.github_issue_number == 11  # the link stays, so no new issue is ever filed
    assert wont_fix.github_issue_filed is True
    assert wont_fix.github_issue_closed is True
    assert wont_fix.github_issue_checked_at is not None  # and the link stays in the sync rotation


async def test_sync_clears_the_flag_when_a_linked_issue_reopens(db, gh_settings, session_factory):
    """A flagged issue that reopens loses the flag and keeps its link."""
    reopened = await _flaky_test(
        db,
        name="reopened",
        github_issue_number=11,
        github_issue_filed=True,
        github_issue_closed=True,
    )

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"state": "open", "state_reason": "reopened"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    await db.refresh(reopened)
    assert reopened.github_issue_closed is False
    assert reopened.github_issue_number == 11  # the link itself is untouched
    assert reopened.github_issue_filed is True


async def test_sync_leaves_alone_an_issue_state_it_does_not_know(db, gh_settings, session_factory):
    """A state that is neither `open` nor `closed` says nothing: link, flag and stamp hold.

    GitHub reports `open` and `closed` today, and the sync only ever acts on those two
    names, so a state it cannot recognize is treated exactly like the unusable body it
    already refuses to guess about: the flagged link survives untouched, and the row is
    still stamped so it cannot take a batch slot on every later run.
    """
    odd = await _flaky_test(
        db,
        name="odd",
        github_issue_number=11,
        github_issue_filed=True,
        github_issue_closed=True,
    )

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"state": "archived", "state_reason": "not_planned"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    await db.refresh(odd)
    assert odd.github_issue_number == 11
    assert odd.github_issue_filed is True
    assert odd.github_issue_closed is True  # only a reported reopen clears it
    assert odd.github_issue_checked_at is not None


async def test_sync_clears_a_link_closed_as_completed(db, gh_settings, session_factory):
    """Closed as completed re-arms exactly like a close with no reported reason."""
    done = await _flaky_test(
        db,
        name="done",
        github_issue_number=11,
        github_issue_filed=True,
        github_issue_closed=True,
    )

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"state": "closed", "state_reason": "completed"})

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    await db.refresh(done)
    assert done.github_issue_number is None
    assert done.github_issue_filed is None
    assert done.github_issue_closed is False


async def test_sync_clears_the_flag_of_a_gone_issue(db, gh_settings, session_factory):
    """A 404 clears the flag with the link: a row with no number never keeps a stale flag."""
    gone = await _flaky_test(
        db,
        name="gone",
        github_issue_number=11,
        github_issue_filed=True,
        github_issue_closed=True,
    )

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(404)

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    await db.refresh(gone)
    assert gone.github_issue_number is None
    assert gone.github_issue_filed is None
    assert gone.github_issue_closed is False


async def test_link_writes_reset_the_closed_flag(db):
    """`clear_link` and `store_filed_link` both reset the flag: unlink re-arms, a link starts clean."""
    tc = await _flaky_test(
        db, name="flagged", github_issue_number=11, github_issue_filed=True, github_issue_closed=True
    )
    issue_links.clear_link(tc)
    assert tc.github_issue_number is None
    assert tc.github_issue_filed is None
    assert tc.github_issue_closed is False

    relinked = await _flaky_test(db, project="p2", name="relinked", github_issue_number=11, github_issue_closed=True)
    issue_links.store_filed_link(relinked, 44)
    assert relinked.github_issue_number == 44
    assert relinked.github_issue_closed is False  # a fresh link points at an open issue


async def test_sync_rearms_an_issue_gone_as_404(db, gh_settings, session_factory):
    """A 404 (issue deleted, or the token lost it) re-arms like a closed one.

    Both the number and the filed flag reset, so a subsequent re-file of the
    test is treated as ours again, not as a stale human link.
    """
    gone = await _flaky_test(db, name="gone", github_issue_number=11, github_issue_filed=True)

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(404)

    await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))
    await db.refresh(gone)
    assert gone.github_issue_number is None
    assert gone.github_issue_filed is None


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
        return set()  # the real filer reports the ids it linked

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
    assert job.github_issue_filed is True  # ours, so an admin delete may close it


async def test_job_admin_delete_race_closes_the_just_filed_issue(db, gh_settings, session_factory, caplog):
    """Job side of the race: the just-filed issue is closed, never left stranded."""
    job = await _flaky_job(db)
    await _job_exec(db, job, "1")
    job_id = job.id
    calls, transport = _race_handler(session_factory, {Job: job_id}, [77])
    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.file_job_issues_for(db, [job_id], transport=transport)
    assert [(r.method, r.url.path) for r in calls] == [
        ("POST", "/repos/acme/app/issues"),
        ("POST", "/repos/acme/app/issues/77/comments"),
        ("PATCH", "/repos/acme/app/issues/77"),
    ]
    assert any("closed issue" in r.getMessage() and "77" in r.getMessage() for r in caplog.records)


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


def _close_recorder():
    """Records requests; a comment POST answers 201, a close PATCH 200."""
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        return httpx.Response(201 if request.method == "POST" else 200, json={"number": 77})

    return calls, httpx.MockTransport(handler)


async def test_admin_delete_close_comments_then_closes_each_issue(gh_settings):
    calls, transport = _close_recorder()
    await github_integration.close_issues_for_deleted_repo("acme/app", [123, 456], transport=transport)
    assert [(r.method, r.url.path) for r in calls] == [
        ("POST", "/repos/acme/app/issues/123/comments"),
        ("PATCH", "/repos/acme/app/issues/123"),
        ("POST", "/repos/acme/app/issues/456/comments"),
        ("PATCH", "/repos/acme/app/issues/456"),
    ]
    comment = json.loads(calls[0].content)["body"]
    assert "admin" in comment and "delet" in comment  # explains why the issue is closing
    assert json.loads(calls[1].content) == {"state": "closed"}


async def test_admin_delete_close_empty_number_list_is_noop(gh_settings):
    calls, transport = _close_recorder()
    await github_integration.close_issues_for_deleted_repo("acme/app", [], transport=transport)
    assert calls == []

    calls, transport = _close_recorder()
    await github_integration.close_issues_for_deleted_repo("acme/app", [123], transport=transport)
    assert len(calls) == 2  # the fixture configured the integration


async def test_admin_delete_close_without_a_token_is_noop(db):
    calls, transport = _close_recorder()
    await github_integration.close_issues_for_deleted_repo("acme/app", [123], transport=transport)
    assert calls == []


async def test_admin_delete_close_network_error_skips_only_that_issue(gh_settings, caplog):
    calls: list[httpx.Request] = []

    def boom(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if "/123" in request.url.path:
            raise httpx.ConnectError("offline")
        return httpx.Response(201 if request.method == "POST" else 200, json={"number": 77})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.close_issues_for_deleted_repo(
            "acme/app", [123, 456], transport=httpx.MockTransport(boom)
        )
    assert [r.url.path for r in calls] == [
        "/repos/acme/app/issues/123/comments",
        "/repos/acme/app/issues/456/comments",
        "/repos/acme/app/issues/456",
    ]
    # The failure is logged, since the delete's response cannot report it.
    assert any("123" in r.getMessage() for r in caplog.records)


async def test_admin_delete_close_server_error_skips_only_that_issue(gh_settings):
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if "/123" in request.url.path:
            return httpx.Response(500, text="boom")
        return httpx.Response(201 if request.method == "POST" else 200, json={"number": 77})

    await github_integration.close_issues_for_deleted_repo(
        "acme/app", [123, 456], transport=httpx.MockTransport(handler)
    )
    assert [r.url.path for r in calls] == [
        "/repos/acme/app/issues/123/comments",
        "/repos/acme/app/issues/456/comments",
        "/repos/acme/app/issues/456",
    ]


async def test_admin_delete_close_gone_issue_skips_the_close(gh_settings):
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if "/123" in request.url.path:
            return httpx.Response(404, json={"message": "Not Found"})
        return httpx.Response(201 if request.method == "POST" else 200, json={"number": 77})

    await github_integration.close_issues_for_deleted_repo(
        "acme/app", [123, 456], transport=httpx.MockTransport(handler)
    )
    assert [r.url.path for r in calls] == [
        "/repos/acme/app/issues/123/comments",
        "/repos/acme/app/issues/456/comments",
        "/repos/acme/app/issues/456",
    ]


async def test_admin_delete_close_rate_limit_stops_the_batch(gh_settings):
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        return httpx.Response(429, json={"message": "Too Many"})

    await github_integration.close_issues_for_deleted_repo(
        "acme/app", [123, 456], transport=httpx.MockTransport(handler)
    )
    assert len(calls) == 1


async def test_admin_delete_close_patch_rate_limit_stops_the_batch(gh_settings, caplog):
    """A rate limit on the close PATCH (not the comment) also stops the batch.

    The first issue is commented on, then the close PATCH is throttled; the
    remaining numbers are never touched because the batch stops there.
    """
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.method == "POST":
            return httpx.Response(201, json={"number": 77})
        return httpx.Response(429, headers={"x-ratelimit-remaining": "0"})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.close_issues_for_deleted_repo(
            "acme/app", [123, 456], transport=httpx.MockTransport(handler)
        )
    assert [r.url.path for r in calls] == [
        "/repos/acme/app/issues/123/comments",
        "/repos/acme/app/issues/123",
    ]


async def test_admin_delete_close_patch_gone_issue_is_logged(gh_settings, caplog):
    """An issue deleted between the comment and the close PATCH is skipped, not fatal."""
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.method == "POST":
            return httpx.Response(201, json={"number": 77})
        return httpx.Response(404)

    with caplog.at_level(logging.INFO, logger="flakeradar.github"):
        await github_integration.close_issues_for_deleted_repo(
            "acme/app", [123, 456], transport=httpx.MockTransport(handler)
        )
    assert [r.url.path for r in calls] == [
        "/repos/acme/app/issues/123/comments",
        "/repos/acme/app/issues/123",
        "/repos/acme/app/issues/456/comments",
        "/repos/acme/app/issues/456",
    ]


async def test_admin_delete_close_close_patch_failure_is_a_warning(gh_settings, caplog):
    """A 5xx on the close PATCH costs that one issue; the rest of the batch goes on."""
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.method == "POST":
            return httpx.Response(201, json={"number": 77})
        return httpx.Response(500, text="boom")

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await github_integration.close_issues_for_deleted_repo(
            "acme/app", [123, 456], transport=httpx.MockTransport(handler)
        )
    assert [r.url.path for r in calls] == [
        "/repos/acme/app/issues/123/comments",
        "/repos/acme/app/issues/123",
        "/repos/acme/app/issues/456/comments",
        "/repos/acme/app/issues/456",
    ]


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


# --- "the fix appears to hold" note (issue #33) -------------------------


@pytest.fixture()
def fixed_settings(gh_settings):
    """The filing settings plus an enabled Clean-streak note threshold."""
    gh_settings.github_fixed_streak = 5
    return gh_settings


def _note_recorder(*, post_status=201, patch_status=200, headers=None):
    """Like `_recorder`, but a close PATCH answers 200 the way GitHub does."""
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.method == "PATCH":
            return httpx.Response(patch_status, json={}, headers=headers or {})
        return httpx.Response(post_status, json={"number": 99}, headers=headers or {})

    return calls, httpx.MockTransport(handler)


def _note_entry(entity: str):
    """The note entry point of one entity, so every scenario can run both sides."""
    return (
        github_integration.note_fixed_issues_for if entity == "test" else github_integration.note_fixed_job_issues_for
    )


async def _note_row(db, entity: str, name: str, *, repo="acme/app", **fields):
    """A linked Test/Job row at the note threshold, one Project/Pipeline each."""
    if entity == "test":
        return await _flaky_test(db, repo=repo, project=name, name=name, **fields)
    return await _flaky_job(db, repo=repo, pipeline=f".github/workflows/{name}.yml", name=name, **fields)


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_comments_once_on_the_linked_issue(db, fixed_settings, entity):
    """The streak is reached with an issue linked: exactly one comment, and the stamp lands."""
    note = _note_entry(entity)
    obj = await _note_row(db, entity, "linked", github_issue_number=77, github_issue_filed=True, clean_streak=5)
    calls, transport = _note_recorder()
    await note(db, [obj.id], transport=transport)
    (req,) = calls
    assert req.method == "POST"
    assert req.url.path == "/repos/acme/app/issues/77/comments"
    body = json.loads(req.content)["body"]
    assert "FlakeRadar: 5 passing runs" in body
    assert "(score now 0.60)" in body
    assert "The fix appears to hold." in body
    await db.refresh(obj)
    assert obj.github_issue_fixed_noted_at is not None
    assert obj.github_issue_number == 77  # noting leaves the link itself alone


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_names_the_default_branch(db, fixed_settings, entity):
    """The Clean streak is counted on the Default branch, so the comment names it."""
    obj = await _note_row(db, entity, "linked", github_issue_number=77, clean_streak=5)
    repo = (await db.execute(select(Repo).where(Repo.name == "acme/app"))).scalar_one()
    repo.default_branch = "main"
    await db.commit()
    calls, transport = _note_recorder()
    await _note_entry(entity)(db, [obj.id], transport=transport)
    body = json.loads(calls[0].content)["body"]
    assert "FlakeRadar: 5 passing runs on `main` since the last failure (score now 0.60)." in body


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_omits_the_branch_when_no_default_branch_is_known(db, fixed_settings, entity):
    """An unknown Default branch scores every branch, so the comment claims none."""
    obj = await _note_row(db, entity, "linked", github_issue_number=77, clean_streak=5)
    calls, transport = _note_recorder()
    await _note_entry(entity)(db, [obj.id], transport=transport)
    body = json.loads(calls[0].content)["body"]
    assert body.startswith("FlakeRadar: 5 passing runs since the last failure (score now 0.60).")
    assert "\n" not in body  # and no deep link without PUBLIC_URL


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_posts_once_and_a_cleared_stamp_posts_again(db, fixed_settings, entity):
    """One comment per fix: the next Report is silent, and a cleared stamp notes again.

    t3 wires the rescore reset that clears the stamp for real; the test clears
    it by hand, which is the same condition the select filters on.
    """
    note = _note_entry(entity)
    obj = await _note_row(db, entity, "linked", github_issue_number=77, clean_streak=5)
    calls, transport = _note_recorder()
    await note(db, [obj.id], transport=transport)
    await note(db, [obj.id], transport=transport)  # the next Report: the stamped row is filtered out
    assert len(calls) == 1

    obj.github_issue_fixed_noted_at = None  # a failure reset the Clean streak
    await db.commit()
    await note(db, [obj.id], transport=transport)
    assert len(calls) == 2
    await db.refresh(obj)
    assert obj.github_issue_fixed_noted_at is not None


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_ignores_rows_that_are_not_noteable(db, fixed_settings, entity):
    """Below the streak, unlinked, declined as not planned, or streak disabled: silence."""
    note = _note_entry(entity)
    short = await _note_row(db, entity, "short", github_issue_number=71, clean_streak=4)
    unlinked = await _note_row(db, entity, "unlinked", clean_streak=9)
    declined = await _note_row(db, entity, "declined", github_issue_number=72, github_issue_closed=True, clean_streak=9)
    await db.commit()
    calls, transport = _note_recorder()
    await note(db, [short.id, unlinked.id, declined.id], transport=transport)
    assert calls == []  # a not-planned link is already closed: nothing to note or close
    await db.refresh(declined)
    assert declined.github_issue_fixed_noted_at is None

    fixed_settings.github_fixed_streak = 0  # the feature ships disabled
    at_streak = await _note_row(db, entity, "streak-off", github_issue_number=73, clean_streak=9)
    calls, transport = _note_recorder()
    await note(db, [at_streak.id], transport=transport)
    assert calls == []


async def test_fixed_note_skips_a_job_of_another_provider(db, fixed_settings):
    """Only a github Pipeline has an issue to note (same rule as filing)."""
    job = await _flaky_job(db, provider="gitlab", github_issue_number=77, clean_streak=5)
    calls, transport = _note_recorder()
    await github_integration.note_fixed_job_issues_for(db, [job.id], transport=transport)
    assert calls == []


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_closes_only_with_close_fixed(db, gh_settings, entity):
    """CLOSE_FIXED off: a comment alone. On: that comment, then the close PATCH."""
    gh_settings.github_fixed_streak = 5
    note = _note_entry(entity)
    obj = await _note_row(db, entity, "linked", github_issue_number=77, clean_streak=5)
    calls, transport = _note_recorder()
    await note(db, [obj.id], transport=transport)
    assert [(r.method, r.url.path) for r in calls] == [("POST", "/repos/acme/app/issues/77/comments")]

    gh_settings.github_close_fixed = True
    obj.github_issue_fixed_noted_at = None
    await db.commit()
    calls, transport = _note_recorder()
    await note(db, [obj.id], transport=transport)
    assert [(r.method, r.url.path) for r in calls] == [
        ("POST", "/repos/acme/app/issues/77/comments"),
        ("PATCH", "/repos/acme/app/issues/77"),
    ]
    assert json.loads(calls[1].content) == {"state": "closed", "state_reason": "completed"}


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_keeps_the_stamp_when_the_close_fails(db, fixed_settings, entity, caplog):
    """A failed close must not buy a second comment: the stamp landed with the POST."""
    fixed_settings.github_close_fixed = True
    note = _note_entry(entity)
    obj = await _note_row(db, entity, "linked", github_issue_number=77, clean_streak=5)
    calls, transport = _note_recorder(patch_status=500)
    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await note(db, [obj.id], transport=transport)
    assert [(r.method, r.url.path) for r in calls] == [
        ("POST", "/repos/acme/app/issues/77/comments"),
        ("PATCH", "/repos/acme/app/issues/77"),
    ]
    await db.refresh(obj)
    assert obj.github_issue_fixed_noted_at is not None
    assert any("close of acme/app#77 failed" in r.getMessage() for r in caplog.records)

    calls.clear()
    await note(db, [obj.id], transport=transport)
    assert calls == []  # no re-post after a failed close: one comment per fix


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_rate_limit_stops_batch_for_tests_and_jobs(db, fixed_settings, entity):
    """A 429, and a rate-limit 403, stop the note batch after the first request."""
    note = _note_entry(entity)
    a = await _note_row(db, entity, "a", repo="acme/a", github_issue_number=71, clean_streak=5)
    b = await _note_row(db, entity, "b", repo="acme/b", github_issue_number=72, clean_streak=5)
    for status, headers in ((429, {}), (403, {"x-ratelimit-remaining": "0"})):
        calls, transport = _note_recorder(post_status=status, headers=headers)
        await note(db, [a.id, b.id], transport=transport)
        assert [r.url.path for r in calls] == ["/repos/acme/a/issues/71/comments"]
        await db.refresh(b)
        assert b.github_issue_fixed_noted_at is None  # the second Repo is never touched


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_permission_403_skips_only_that_repo(db, fixed_settings, entity):
    """A 403 that is not a rate limit costs one Repo; the other still gets its comment."""
    note = _note_entry(entity)
    a1 = await _note_row(db, entity, "a1", repo="acme/a", github_issue_number=71, clean_streak=5)
    a2 = await _note_row(db, entity, "a2", repo="acme/a", github_issue_number=73, clean_streak=5)
    other = await _note_row(db, entity, "b", repo="acme/b", github_issue_number=72, clean_streak=5)

    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(403, json={"message": "Resource not accessible by personal access token"})
        return httpx.Response(201, json={"number": 99})

    await note(db, [a1.id, other.id, a2.id], transport=httpx.MockTransport(handler))
    assert [r.url.path for r in calls] == [
        "/repos/acme/a/issues/71/comments",
        "/repos/acme/b/issues/72/comments",
    ]  # a2 is never retried: the whole Repo is skipped for the rest of the batch
    await db.refresh(other)
    assert other.github_issue_fixed_noted_at is not None


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_comment_links_the_dashboard_only_with_public_url(db, fixed_settings, entity):
    """PUBLIC_URL off: no link. Set: the comment ends with the entity's own deep link."""
    note = _note_entry(entity)
    obj = await _note_row(db, entity, "linked", github_issue_number=77, clean_streak=5)
    calls, transport = _note_recorder()
    await note(db, [obj.id], transport=transport)
    assert "?" not in json.loads(calls[0].content)["body"]

    fixed_settings.public_url = "https://flake.example.com/"  # a trailing slash is tolerated
    obj.github_issue_fixed_noted_at = None
    await db.commit()
    calls, transport = _note_recorder()
    await note(db, [obj.id], transport=transport)
    (req,) = calls
    body = json.loads(req.content)["body"]
    assert "The fix appears to hold." in body
    if entity == "job":
        assert body.endswith(f"https://flake.example.com/?view=jobs&job={obj.id}")
    else:
        assert body.endswith(f"https://flake.example.com/?test={obj.id}")


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_comment_body_is_exactly_the_expected_sentence(db, fixed_settings, entity):
    """The Done-when wording, verbatim, for both entities."""
    note = _note_entry(entity)
    obj = await _note_row(db, entity, "linked", github_issue_number=77, clean_streak=7)
    repo = (await db.execute(select(Repo).where(Repo.name == "acme/app"))).scalar_one()
    repo.default_branch = "main"
    await db.commit()
    calls, transport = _note_recorder()
    await note(db, [obj.id], transport=transport)
    (req,) = calls
    assert json.loads(req.content) == {
        "body": (
            "FlakeRadar: 7 passing runs on `main` since the last failure (score now 0.60). The fix appears to hold."
        )
    }


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_unusable_comment_status_goes_on(db, fixed_settings, entity, caplog):
    """A 500 on the comment is nobody's Repo-wide fault: log it, note no row, go on."""
    note = _note_entry(entity)
    bad = await _note_row(db, entity, "bad", repo="acme/a", github_issue_number=71, clean_streak=5)
    other = await _note_row(db, entity, "ok", repo="acme/b", github_issue_number=72, clean_streak=5)
    calls, transport = _note_recorder(post_status=500)
    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await note(db, [bad.id, other.id], transport=transport)
    assert [r.url.path for r in calls] == [
        "/repos/acme/a/issues/71/comments",
        "/repos/acme/b/issues/72/comments",
    ]
    for obj in (bad, other):
        await db.refresh(obj)
        assert obj.github_issue_fixed_noted_at is None  # nothing landed, so nothing is stamped
    assert any("fixed-note comment failed" in r.getMessage() for r in caplog.records)


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_close_rate_limit_stops_batch(db, fixed_settings, entity):
    """A rate limit on the close PATCH stops the batch like any other request would."""
    fixed_settings.github_close_fixed = True
    note = _note_entry(entity)
    a = await _note_row(db, entity, "a", repo="acme/a", github_issue_number=71, clean_streak=5)
    b = await _note_row(db, entity, "b", repo="acme/b", github_issue_number=72, clean_streak=5)
    calls, transport = _note_recorder(patch_status=429, headers={"x-ratelimit-remaining": "0"})
    await note(db, [a.id, b.id], transport=transport)
    assert [(r.method, r.url.path) for r in calls] == [
        ("POST", "/repos/acme/a/issues/71/comments"),
        ("PATCH", "/repos/acme/a/issues/71"),
    ]
    await db.refresh(a)
    await db.refresh(b)
    assert a.github_issue_fixed_noted_at is not None  # the comment landed before the throttle
    assert b.github_issue_fixed_noted_at is None


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_close_permission_403_skips_repo(db, fixed_settings, entity, caplog):
    """A permission 403 on the close costs that Repo's remaining rows, not the note."""
    fixed_settings.github_close_fixed = True
    note = _note_entry(entity)
    a = await _note_row(db, entity, "a", repo="acme/a", github_issue_number=71, clean_streak=5)
    b = await _note_row(db, entity, "b", repo="acme/b", github_issue_number=72, clean_streak=5)
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.method == "PATCH" and request.url.path.startswith("/repos/acme/a/"):
            return httpx.Response(403, json={"message": "Resource not accessible by personal access token"})
        return httpx.Response(201 if request.method == "POST" else 200, json={})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await note(db, [a.id, b.id], transport=httpx.MockTransport(handler))
    assert [(r.method, r.url.path) for r in calls] == [
        ("POST", "/repos/acme/a/issues/71/comments"),
        ("PATCH", "/repos/acme/a/issues/71"),
        ("POST", "/repos/acme/b/issues/72/comments"),
        ("PATCH", "/repos/acme/b/issues/72"),
    ]
    for obj in (a, b):
        await db.refresh(obj)
        assert obj.github_issue_fixed_noted_at is not None
    assert any("issue close skipped for repo acme/a" in r.getMessage() for r in caplog.records)


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_admin_delete_race_costs_only_that_note(db, fixed_settings, entity, session_factory, caplog):
    """The row vanishing between the comment POST and the stamp costs this note only."""
    doomed = await _note_row(db, entity, "doomed", github_issue_number=77, clean_streak=5)
    other = await _note_row(db, entity, "kept", repo="acme/b", github_issue_number=78, clean_streak=5)
    model = TestCase if entity == "test" else Job
    calls: list[httpx.Request] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.path == "/repos/acme/app/issues/77/comments":
            async with session_factory() as other_session:  # an admin delete wins the race
                await other_session.execute(delete(model).where(model.id == doomed.id))
                await other_session.commit()
        return httpx.Response(201, json={"number": 99})

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await _note_entry(entity)(db, [doomed.id, other.id], transport=httpx.MockTransport(handler))
    assert [r.url.path for r in calls] == [
        "/repos/acme/app/issues/77/comments",
        "/repos/acme/b/issues/78/comments",
    ]  # the StaleDataError costs one note, not the batch
    await db.refresh(other)
    assert other.github_issue_fixed_noted_at is not None
    assert any("deleted (admin delete) while noting" in r.getMessage() for r in caplog.records)


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_stamp_loses_to_a_clean_streak_reset_during_the_post(
    db, fixed_settings, entity, session_factory, caplog
):
    """A comment that lands after the streak reset must not stamp the row (issue #33).

    The note is selected at the threshold, the POST is slow, and the processor handles a
    failure whose rescore resets `clean_streak` to 0 and clears the stamp: stamping here
    would undo that reset, and every later clean run is filtered out as already noted.
    """
    note = _note_entry(entity)
    obj = await _note_row(db, entity, "linked", github_issue_number=77, clean_streak=5)
    model = TestCase if entity == "test" else Job
    calls: list[httpx.Request] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.method == "POST":
            async with session_factory() as other_session:  # `rescore` resets the streak
                await other_session.execute(
                    update(model).where(model.id == obj.id).values(clean_streak=0, github_issue_fixed_noted_at=None)
                )
                await other_session.commit()
        return httpx.Response(201 if request.method == "POST" else 200, json={})

    fixed_settings.github_close_fixed = True  # a fix that did not hold must not be closed
    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await note(db, [obj.id], transport=httpx.MockTransport(handler))
    assert [(r.method, r.url.path) for r in calls] == [("POST", "/repos/acme/app/issues/77/comments")]
    await db.refresh(obj)
    assert obj.clean_streak == 0
    assert obj.github_issue_fixed_noted_at is None  # the reset stands: the row is noteable again
    assert any("stopped being noteable" in r.getMessage() for r in caplog.records)

    # the re-armed note really does come: the streak is back at the threshold
    async with session_factory() as other_session:
        await other_session.execute(update(model).where(model.id == obj.id).values(clean_streak=5))
        await other_session.commit()
    calls, transport = _note_recorder()
    await note(db, [obj.id], transport=transport)
    assert [(r.method, r.url.path) for r in calls] == [
        ("POST", "/repos/acme/app/issues/77/comments"),
        ("PATCH", "/repos/acme/app/issues/77"),
    ]
    await db.refresh(obj)
    assert obj.github_issue_fixed_noted_at is not None


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_fixed_note_network_error_is_logged_and_never_raises(db, fixed_settings, entity, caplog):
    """An unreachable GitHub costs the note batch, with no exception out of it."""
    note = _note_entry(entity)
    obj = await _note_row(db, entity, "linked", github_issue_number=77, clean_streak=5)

    def handler(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("GitHub is down", request=request)

    with caplog.at_level(logging.WARNING, logger="flakeradar.github"):
        await note(db, [obj.id], transport=httpx.MockTransport(handler))
    await db.refresh(obj)
    assert obj.github_issue_fixed_noted_at is None
    assert any("unreachable, skipping" in r.getMessage() for r in caplog.records)


# --- filing and noting in one Report (issue #33) --------------------------


def _file_entry(entity: str):
    """The filing entry point of one entity, so every scenario can run both sides."""
    return github_integration.file_issues_for if entity == "test" else github_integration.file_job_issues_for


def _hook_outcome(entity: str, objs) -> ProcessOutcome:
    """A processed Report whose only touched ids are these Tests/Jobs."""
    ids = [o.id for o in objs]
    return ProcessOutcome(
        report_id=1,
        status="processed",
        run_id=1,
        counts={},
        touched_test_ids=ids if entity == "test" else [],
        touched_job_ids=ids if entity == "job" else [],
        error=None,
    )


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_filing_gate_skips_rows_at_the_fixed_note_streak(db, fixed_settings, entity):
    """A row whose fix holds answers with a note, not an Issue: the gate refuses it."""
    at_streak = await _note_row(db, entity, "at-streak", clean_streak=5)  # score 0.6 clears the filing gate
    below = await _note_row(db, entity, "below", clean_streak=4)
    calls, transport = _recorder(number=101)
    filed = await _file_entry(entity)(db, [at_streak.id, below.id], transport=transport)
    assert filed == {below.id}
    assert [(r.method, r.url.path) for r in calls] == [("POST", "/repos/acme/app/issues")]
    await db.refresh(at_streak)
    assert at_streak.github_issue_number is None  # no Issue to comment "the fix appears to hold" on
    await db.refresh(below)
    assert below.github_issue_number == 101


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_processed_report_never_notes_an_issue_it_filed(db, fixed_settings, entity, session_factory):
    """One Report files and notes, and never the same link: no note on the Issue it just filed.

    With `github_close_fixed` on, the collision was a loop: file, note, close, re-arm,
    file again, one turn per Report with no new failure anywhere. A Test that only the
    filing gate fits and a sibling already linked at the threshold show that both steps
    ran in this same pass, and the second Report shows the silence is the steady state,
    not one lucky pass.
    """
    fixed_settings.github_close_fixed = True
    live = await _note_row(db, entity, "live", clean_streak=0)  # files, and must not be noted
    fresh = await _note_row(db, entity, "fresh", clean_streak=5)  # meets the filing gate and the note one
    linked = await _note_row(db, entity, "linked", github_issue_number=88, clean_streak=5)  # the note step's work
    calls, transport = _note_recorder()
    outcome = _hook_outcome(entity, [live, fresh, linked])

    await github_integration.on_report_processed(session_factory, outcome, transport=transport)
    assert [(r.method, r.url.path) for r in calls] == [
        ("POST", "/repos/acme/app/issues"),
        ("POST", "/repos/acme/app/issues/88/comments"),
        ("PATCH", "/repos/acme/app/issues/88"),
    ]  # the Issue filed in this pass gets no comment and no close: only the old link is touched
    await db.refresh(fresh)
    assert fresh.github_issue_number is None
    await db.refresh(live)
    assert live.github_issue_number == 99
    assert live.github_issue_fixed_noted_at is None

    await github_integration.on_report_processed(session_factory, outcome, transport=transport)
    assert len(calls) == 3  # the next Report adds neither an Issue nor a comment


@pytest.mark.parametrize("entity", ["test", "job"])
async def test_note_step_skips_the_link_this_same_pass_filed(db, fixed_settings, entity, session_factory):
    """A fresh link stays off-limits to the note step even if it becomes noteable mid-pass.

    A concurrent Report can lift the streak between the two selects, so the filing gate
    alone cannot catch this: the row was below the threshold when it was filed, and
    reaches it only while the Issue is in flight. Commenting there, and closing it with
    `github_close_fixed`, is the self-contradictory note on a brand-new Issue.
    """
    obj = await _note_row(db, entity, "fresh", clean_streak=4)
    fixed_settings.github_close_fixed = True
    model = TestCase if entity == "test" else Job
    calls: list[httpx.Request] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.path == "/repos/acme/app/issues":
            async with session_factory() as other_session:  # another Report's rescore
                await other_session.execute(update(model).where(model.id == obj.id).values(clean_streak=5))
                await other_session.commit()
        return httpx.Response(201 if request.method == "POST" else 200, json={"number": 123})

    await github_integration.on_report_processed(
        session_factory, _hook_outcome(entity, [obj]), transport=httpx.MockTransport(handler)
    )
    assert [(r.method, r.url.path) for r in calls] == [("POST", "/repos/acme/app/issues")]
    await db.refresh(obj)
    assert obj.github_issue_number == 123  # the filing itself stands
    assert obj.github_issue_fixed_noted_at is None
    assert obj.clean_streak == 5  # and it is noteable from the next Report on
