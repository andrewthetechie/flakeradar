"""MCP server: read-only tools over the same queries as REST; token-gated over HTTP."""

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
        "get_job",
        "get_test",
        "list_projects",
        "list_repos",
        "search_jobs",
        "search_tests",
        "top_flaky_jobs",
        "top_flaky_tests",
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
