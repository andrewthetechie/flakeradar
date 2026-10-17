"""MCP server: read tools and the Issue link write tools over the same services as REST; token-gated over HTTP."""

import pytest
from app.mcp_server import build_mcp
from fastmcp import Client

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
def mcp(session_factory):
    return build_mcp(session_factory, api_token=None)


async def _seed(db):
    wf = await make_project(db, "andrewthetechie/writers-app", "frontend", root="frontend")
    wb = await make_project(db, "andrewthetechie/writers-app", "backend")
    flaky = await make_test_case(
        db,
        wf,
        name="renders",
        classname="src/app.test.ts",
        file="src/app.test.ts",
        line=12,
        flakiness_score=0.7,
        confirmed_flake_count=1,
    )
    await make_test_case(db, wf, name="suspect_one", flakiness_score=0.1)
    await make_test_case(
        db, wb, name="renders", classname="tests.test_views", file="tests/test_views.py", flakiness_score=0.4
    )
    await make_test_case(db, wb, name="calm", flakiness_score=0.0)
    run = await make_run(db, wf, commit_sha="aaa111", branch="main")
    await make_execution(db, flaky, run, status="failed", message="expected 3", details="Traceback\n" + "x" * 5000)
    await make_execution(db, flaky, run, status="passed")
    await db.commit()
    return flaky


async def test_tools_are_listed(mcp):
    async with Client(mcp) as c:
        names = sorted(t.name for t in await c.list_tools())
    assert names == [
        "ci_run_failures",
        "get_job",
        "get_test",
        "link_issue",
        "list_projects",
        "list_repos",
        "search_jobs",
        "search_tests",
        "top_flaky_jobs",
        "top_flaky_tests",
        "unlink_issue",
    ]


async def test_list_repos_and_projects(mcp, db):
    await _seed(db)
    async with Client(mcp) as c:
        repos = (await c.call_tool("list_repos", {})).data
        projects = (await c.call_tool("list_projects", {"repo": "AndrewTheTechie/Writers-App"})).data
        missing = await c.call_tool("list_projects", {"repo": "nobody/here"}, raise_on_error=False)
    assert [r["name"] for r in repos] == ["andrewthetechie/writers-app"]
    assert projects == [{"name": "backend", "root": ""}, {"name": "frontend", "root": "frontend"}]
    assert missing.is_error and "Unknown repo" in missing.content[0].text


async def test_top_flaky_tests(mcp, db):
    await _seed(db)
    async with Client(mcp) as c:
        repo_wide = (await c.call_tool("top_flaky_tests", {"repo": "andrewthetechie/writers-app"})).data
        flaky_only = (
            await c.call_tool("top_flaky_tests", {"repo": "andrewthetechie/writers-app", "include_suspect": False})
        ).data
        in_file = (
            await c.call_tool("top_flaky_tests", {"repo": "andrewthetechie/writers-app", "file": "test_views"})
        ).data
        bad = await c.call_tool("top_flaky_tests", {"repo": "x", "limit": 5}, raise_on_error=False)
    assert [(t["project"], t["name"], t["tier"]) for t in repo_wide] == [
        ("frontend", "renders", "flaky"),
        ("backend", "renders", "flaky"),
        ("frontend", "suspect_one", "suspect"),
    ]
    assert len(flaky_only) == 2
    assert [t["file"] for t in in_file] == ["tests/test_views.py"]
    assert bad.is_error


async def test_top_flaky_tests_category(mcp, db):
    proj = await make_project(db, "acme/app", "backend")
    await make_test_case(db, proj, name="A", flakiness_score=0.8, failure_category="timing")
    await make_test_case(db, proj, name="B", flakiness_score=0.2, failure_category="network")
    await make_test_case(db, proj, name="C", flakiness_score=0.5, failure_category="timing")
    await db.commit()
    async with Client(mcp) as c:
        network = (await c.call_tool("top_flaky_tests", {"repo": "acme/app", "category": "network"})).data
        bad = await c.call_tool("top_flaky_tests", {"repo": "acme/app", "category": "nope"}, raise_on_error=False)
    assert [t["name"] for t in network] == ["B"]
    assert bad.is_error and "category must be one of" in bad.content[0].text


async def test_search_tests_matches_name_classname_and_file(mcp, db):
    await _seed(db)
    async with Client(mcp) as c:
        by_file = (await c.call_tool("search_tests", {"repo": "andrewthetechie/writers-app", "query": "APP.TEST"})).data
        by_name = (await c.call_tool("search_tests", {"repo": "andrewthetechie/writers-app", "query": "calm"})).data
    assert [t["name"] for t in by_file] == ["renders"]
    assert [t["name"] for t in by_name] == ["calm"]  # stable tests are searchable


async def test_get_test_by_id_and_by_name(mcp, db):
    flaky = await _seed(db)
    async with Client(mcp) as c:
        by_id = (await c.call_tool("get_test", {"test_id": flaky.id})).data
        by_name = (
            await c.call_tool(
                "get_test", {"repo": "andrewthetechie/writers-app", "project": "frontend", "name": "renders"}
            )
        ).data
        missing = await c.call_tool(
            "get_test",
            {"repo": "andrewthetechie/writers-app", "project": "frontend", "name": "nope"},
            raise_on_error=False,
        )
        nothing = await c.call_tool("get_test", {}, raise_on_error=False)
    assert by_id == by_name
    assert by_id["test"]["repo"] == "andrewthetechie/writers-app"
    assert by_id["location"]["url"] == (
        "https://github.com/andrewthetechie/writers-app/blob/aaa111/frontend/src/app.test.ts#L12"
    )
    assert by_id["last_failing_sha"] == "aaa111"
    failure = by_id["latest_failure"]
    assert failure["message"] == "expected 3"
    assert len(failure["details"]) == 4096 + len("\n…[truncated]")
    assert [e["status"] for e in by_id["executions"]] == ["passed", "failed"]
    assert all("attempt" in e for e in by_id["executions"])
    assert all("failure_category" in e for e in by_id["executions"])
    assert "score_history" in by_id
    assert "clean_streak" in by_id["test"]
    assert "details" not in by_id["executions"][0]
    assert missing.is_error and "No test named 'nope'" in missing.content[0].text
    assert nothing.is_error


async def test_http_mount_requires_bearer_token(client):
    resp = await client.post(
        "/mcp/",
        json={"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
        headers={"Accept": "application/json, text/event-stream"},
    )
    assert resp.status_code == 401


async def _seed_jobs(db):
    """A Repo with an infra-flaky job, an explained job, and a linked Test."""
    proj = await make_project(db, "acme/app", "backend")
    p = await make_pipeline(db, "acme/app", "ci.yml")
    e2e = await make_job(db, p, name="e2e (ubuntu-latest)", flakiness_score=0.4, last_status="failed")
    await make_job_execution(db, e2e, status="failed", ci_job_id="e2e-1")  # no linked Test: unexplained
    test = await make_job(db, p, name="test", flakiness_score=0.1, last_status="failed")
    # A failing Job execution explained by a failing linked Test.
    tc = await make_test_case(db, proj, name="t1")
    run = await make_run(db, proj, commit_sha="s", ci_job_id="ex-1")
    await make_execution(db, tc, run, status="failed")
    await make_job_execution(db, test, status="failed", ci_job_id="ex-1")
    await make_job_execution(db, test, status="passed", ci_job_id="ex-2")
    await db.commit()
    return e2e, test, tc


async def test_top_flaky_jobs(mcp, db):
    await _seed_jobs(db)
    async with Client(mcp) as c:
        wide = (await c.call_tool("top_flaky_jobs", {"repo": "acme/app"})).data
        flaky_only = (await c.call_tool("top_flaky_jobs", {"repo": "acme/app", "include_suspect": False})).data
        bad = await c.call_tool("top_flaky_jobs", {"repo": "acme/app", "limit": 0}, raise_on_error=False)
    assert [j["name"] for j in wide] == ["e2e (ubuntu-latest)", "test"]  # worst first
    assert [j["name"] for j in flaky_only] == ["e2e (ubuntu-latest)"]  # test is suspect (0.1 < 0.3)
    assert bad.is_error


async def test_search_jobs(mcp, db):
    await _seed_jobs(db)
    async with Client(mcp) as c:
        by_name = (await c.call_tool("search_jobs", {"repo": "acme/app", "query": "e2e"})).data
        by_pipe = (await c.call_tool("search_jobs", {"repo": "acme/app", "query": "ci.yml"})).data
    assert [j["name"] for j in by_name] == ["e2e (ubuntu-latest)"]
    assert {j["name"] for j in by_pipe} == {"e2e (ubuntu-latest)", "test"}


async def test_get_job_by_id_and_by_name_with_explained(mcp, db):
    _, test, _ = await _seed_jobs(db)
    async with Client(mcp) as c:
        by_id = (await c.call_tool("get_job", {"job_id": test.id})).data
        by_name = (await c.call_tool("get_job", {"repo": "acme/app", "name": "test"})).data
        missing = await c.call_tool("get_job", {"repo": "acme/app", "name": "nope"}, raise_on_error=False)
    assert by_id == by_name
    assert "score_history" in by_id
    assert "clean_streak" in by_id["job"]
    assert len(by_id["executions"]) == 2
    outcomes = {e["ci_job_id"]: e["outcome"] for e in by_id["executions"]}
    assert outcomes["ex-1"] == "explained" and outcomes["ex-2"] == "passed"
    explained = next(e for e in by_id["executions"] if e["ci_job_id"] == "ex-1")
    assert [t["name"] for t in explained["explained_by"]] == ["t1"]
    assert set(explained) == {
        "outcome",
        "status",
        "commit_sha",
        "branch",
        "ci_run_attempt",
        "ci_job_id",
        "url",
        "runner_name",
        "runner_labels",
        "created_at",
        "explained_by",
    }
    assert explained["created_at"].endswith("+00:00")  # isoformat(), as before the schema dump
    assert missing.is_error and "No job named 'nope'" in missing.content[0].text


async def test_get_job_ambiguous_name_lists_ids(mcp, db):
    a = await make_pipeline(db, "acme/app", "a.yml")
    b = await make_pipeline(db, "acme/app", "b.yml")
    ja = await make_job(db, a, name="build")
    jb = await make_job(db, b, name="build")
    await db.commit()
    async with Client(mcp) as c:
        got = await c.call_tool("get_job", {"repo": "acme/app", "name": "build"}, raise_on_error=False)
        by_pipe = (await c.call_tool("get_job", {"repo": "acme/app", "name": "build", "pipeline": "b.yml"})).data
    assert got.is_error
    assert str(ja.id) in got.content[0].text and str(jb.id) in got.content[0].text
    assert "pass pipeline" in got.content[0].text
    assert by_pipe["job"]["id"] == jb.id


async def test_get_test_includes_jobs(mcp, db):
    _, _, tc = await _seed_jobs(db)
    async with Client(mcp) as c:
        got = (await c.call_tool("get_test", {"test_id": tc.id})).data
    assert len(got["jobs"]) == 1
    assert got["jobs"][0]["pipeline"] == "ci.yml" and got["jobs"][0]["name"] == "test"


async def test_ci_run_failures(mcp, db):
    proj = await make_project(db, "acme/app", "backend")
    t1 = await make_test_case(db, proj, name="t1", flakiness_score=0.5)
    run = await make_run(db, proj, ci_run_id="100", ci_run_attempt=1)
    await make_execution(db, t1, run, status="failed", message="boom")
    await db.commit()

    async with Client(mcp) as c:
        data = (await c.call_tool("ci_run_failures", {"repo": "ACME/App", "ci_run_id": "100"})).data
    assert data["repo"] == "acme/app"
    assert data["ci_run_attempt"] == 1
    assert [(f["name"], f["tier"], f["message"]) for f in data["failures"]] == [("t1", "flaky", "boom")]
    assert data["failed_jobs"] == []
    assert set(data["reports"]) == {"pending", "processed", "failed"}


async def test_ci_run_failures_errors(mcp, db):
    proj = await make_project(db, "acme/app", "backend")
    t1 = await make_test_case(db, proj, name="t1")
    run = await make_run(db, proj, ci_run_id="100", ci_run_attempt=1)
    await make_execution(db, t1, run, status="failed")
    await db.commit()

    async with Client(mcp) as c:
        unknown_run = await c.call_tool(
            "ci_run_failures", {"repo": "acme/app", "ci_run_id": "nope"}, raise_on_error=False
        )
        bad_attempt = await c.call_tool(
            "ci_run_failures", {"repo": "acme/app", "ci_run_id": "100", "ci_run_attempt": 0}, raise_on_error=False
        )
        unknown_attempt = await c.call_tool(
            "ci_run_failures", {"repo": "acme/app", "ci_run_id": "100", "ci_run_attempt": 5}, raise_on_error=False
        )
        unknown_repo = await c.call_tool(
            "ci_run_failures", {"repo": "nobody/here", "ci_run_id": "100"}, raise_on_error=False
        )
        bad_repo = await c.call_tool("ci_run_failures", {"repo": "nope", "ci_run_id": "100"}, raise_on_error=False)
    assert unknown_run.is_error and "CI run not found" in unknown_run.content[0].text
    assert bad_attempt.is_error and "ci_run_attempt must be 1 or more" in bad_attempt.content[0].text
    assert unknown_attempt.is_error and "CI run attempt not found" in unknown_attempt.content[0].text
    assert unknown_repo.is_error and "Repo not found" in unknown_repo.content[0].text
    assert bad_repo.is_error


async def test_tool_annotations(mcp):
    async with Client(mcp) as c:
        tools = {t.name: t.annotations for t in await c.list_tools()}
    for name, annotations in tools.items():
        if name in ("link_issue", "unlink_issue"):
            continue
        assert annotations.read_only_hint is True, name
    link = tools["link_issue"]
    assert link.read_only_hint is False
    assert link.destructive_hint is False
    assert link.idempotent_hint is True
    assert link.open_world_hint is False
    unlink = tools["unlink_issue"]
    assert unlink.read_only_hint is False
    assert unlink.destructive_hint is True
    assert unlink.idempotent_hint is True
    assert unlink.open_world_hint is False


async def test_link_and_unlink_a_test_issue(mcp, db):
    flaky = await _seed(db)
    async with Client(mcp) as c:
        linked = (await c.call_tool("link_issue", {"number": 31, "test_id": flaky.id})).data
        got = (await c.call_tool("get_test", {"test_id": flaky.id})).data
        unlinked = (await c.call_tool("unlink_issue", {"test_id": flaky.id})).data
    assert linked["id"] == flaky.id
    assert linked["github_issue_number"] == 31
    assert linked["github_issue_url"] == "https://github.com/andrewthetechie/writers-app/issues/31"
    assert got["test"]["github_issue_number"] == 31
    assert unlinked["github_issue_number"] is None


async def test_link_and_unlink_a_job_issue(mcp, db):
    pipe = await make_pipeline(db, "acme/app")
    job = await make_job(db, pipe, name="e2e")
    await db.commit()
    async with Client(mcp) as c:
        linked = (await c.call_tool("link_issue", {"number": 9, "job_id": job.id})).data
        unlinked = (await c.call_tool("unlink_issue", {"job_id": job.id})).data
    assert linked["id"] == job.id
    assert linked["github_issue_number"] == 9
    assert linked["github_issue_url"] == "https://github.com/acme/app/issues/9"
    assert unlinked["github_issue_number"] is None


async def test_link_issue_errors(mcp, db):
    flaky = await _seed(db)
    pipe = await make_pipeline(db, "acme/app")
    job = await make_job(db, pipe, name="e2e")
    gl_pipe = await make_pipeline(db, "acme/app", ".gitlab-ci.yml", provider="gitlab")
    gl = await make_job(db, gl_pipe, name="gl")
    await db.commit()
    async with Client(mcp) as c:
        neither = await c.call_tool("link_issue", {"number": 1}, raise_on_error=False)
        both = await c.call_tool(
            "link_issue", {"number": 1, "test_id": flaky.id, "job_id": job.id}, raise_on_error=False
        )
        no_test = await c.call_tool("link_issue", {"number": 1, "test_id": 99999}, raise_on_error=False)
        no_job = await c.call_tool("link_issue", {"number": 1, "job_id": 99999}, raise_on_error=False)
        bad_number = await c.call_tool("link_issue", {"number": 0, "test_id": flaky.id}, raise_on_error=False)
        gitlab = await c.call_tool("link_issue", {"number": 1, "job_id": gl.id}, raise_on_error=False)
        no_target = await c.call_tool("unlink_issue", {}, raise_on_error=False)
        unknown = await c.call_tool("unlink_issue", {"test_id": 99999}, raise_on_error=False)
    assert neither.is_error and "Pass exactly one of test_id or job_id." in neither.content[0].text
    assert both.is_error and "Pass exactly one of test_id or job_id." in both.content[0].text
    assert no_test.is_error and "Test not found" in no_test.content[0].text
    assert no_job.is_error and "Job not found" in no_job.content[0].text
    assert bad_number.is_error and "number must be between 1 and 2147483647" in bad_number.content[0].text
    assert gitlab.is_error and "Only Jobs of GitHub Pipelines can link a GitHub issue" in gitlab.content[0].text
    assert no_target.is_error and "Pass exactly one of test_id or job_id." in no_target.content[0].text
    assert unknown.is_error and "Test not found" in unknown.content[0].text


async def test_link_issue_conflict_and_replace(mcp, db):
    flaky = await _seed(db)
    async with Client(mcp) as c:
        first = (await c.call_tool("link_issue", {"number": 31, "test_id": flaky.id})).data
        conflict = await c.call_tool("link_issue", {"number": 32, "test_id": flaky.id}, raise_on_error=False)
        replaced = (await c.call_tool("link_issue", {"number": 32, "test_id": flaky.id, "replace": True})).data
        again = (await c.call_tool("link_issue", {"number": 32, "test_id": flaky.id})).data
    assert first["github_issue_number"] == 31
    assert conflict.is_error
    assert "Test already links issue #31; pass replace=true to replace it" in conflict.content[0].text
    assert replaced["github_issue_number"] == 32
    assert again["github_issue_number"] == 32
