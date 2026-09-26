"""Test detail (history, Location, permalink) and quarantine."""

from tests.conftest import AUTH
from tests.factories import make_execution, make_project, make_run, make_test_case


async def _seed(db, *, file="src/app.test.ts", line=12, root="frontend"):
    proj = await make_project(db, "andrewthetechie/writers-app", "frontend", root=root)
    tc = await make_test_case(
        db, proj, name="renders", file=file, line=line, flakiness_score=0.6, confirmed_flake_count=1
    )
    r1 = await make_run(db, proj, commit_sha="aaa111", branch="main", ci_run_id="1")
    await make_execution(
        db, tc, r1, status="failed", message="expected 3", details="Traceback\n  at src/app.test.ts:14"
    )
    r2 = await make_run(db, proj, commit_sha="bbb222", branch="feat", ci_run_id="2")
    await make_execution(db, tc, r2, status="passed", attempt=1)
    await db.commit()
    return tc


async def test_history_includes_location_permalink_and_details(client, db):
    tc = await _seed(db)
    body = (await client.get(f"/api/tests/{tc.id}/history")).json()
    assert body["test"]["repo"] == "andrewthetechie/writers-app"
    assert body["test"]["project"] == "frontend" and body["test"]["tier"] == "flaky"
    assert body["location"] == {
        "path": "frontend/src/app.test.ts",
        "line": 12,
        "url": "https://github.com/andrewthetechie/writers-app/blob/aaa111/frontend/src/app.test.ts#L12",
    }
    assert (body["last_failing_sha"], body["last_failing_branch"]) == ("aaa111", "main")
    assert [e["status"] for e in body["executions"]] == ["passed", "failed"]  # newest first
    assert body["executions"][0]["attempt"] == 1
    assert body["executions"][1]["details"] == "Traceback\n  at src/app.test.ts:14"
    assert body["executions"][1]["message"] == "expected 3"


async def test_history_limit_and_404(client, db):
    tc = await _seed(db)
    body = (await client.get(f"/api/tests/{tc.id}/history?limit=1")).json()
    assert len(body["executions"]) == 1
    assert (await client.get("/api/tests/9999/history")).status_code == 404
    assert (await client.get(f"/api/tests/{tc.id}/history?limit=501")).status_code == 422


async def test_location_edge_cases(client, db):
    no_root_line0 = await _seed(db, root="", line=0)
    body = (await client.get(f"/api/tests/{no_root_line0.id}/history")).json()
    assert body["location"]["url"] == "https://github.com/andrewthetechie/writers-app/blob/aaa111/src/app.test.ts"


async def test_no_file_means_no_location(client, db):
    tc = await _seed(db, file=None, line=None)
    body = (await client.get(f"/api/tests/{tc.id}/history")).json()
    assert body["location"] is None and body["last_failing_sha"] == "aaa111"


async def test_never_failed_has_path_but_no_url(client, db):
    proj = await make_project(db, "acme/app")
    tc = await make_test_case(db, proj, file="tests/test_a.py", line=3)
    run = await make_run(db, proj)
    await make_execution(db, tc, run, status="passed")
    await db.commit()
    body = (await client.get(f"/api/tests/{tc.id}/history")).json()
    assert body["location"] == {"path": "tests/test_a.py", "line": 3, "url": None}
    assert body["last_failing_sha"] is None


async def test_quarantine_toggle_and_scoped_list(client, db):
    tc = await _seed(db)
    other_proj = await make_project(db, "andrewthetechie/writers-app", "backend")
    await make_test_case(db, other_proj, name="renders", quarantined=True)
    await db.commit()

    on = await client.post(f"/api/tests/{tc.id}/quarantine", json={"quarantined": True})
    assert on.status_code == 200
    assert on.json()["quarantined"] is True and on.json()["quarantined_at"] is not None
    assert on.json()["repo"] == "andrewthetechie/writers-app"

    url = "/api/quarantine?repo=andrewthetechie/writers-app&project=frontend"
    assert (await client.get(url)).status_code == 401
    items = (await client.get(url, headers=AUTH)).json()
    assert [(i["name"], i["file"], i["line"]) for i in items] == [("renders", "src/app.test.ts", 12)]
    default = (await client.get("/api/quarantine?repo=andrewthetechie/writers-app", headers=AUTH)).json()
    assert default == []  # project defaults to "default", which has nothing
    assert (await client.get("/api/quarantine?project=frontend", headers=AUTH)).status_code == 422

    off = await client.post(f"/api/tests/{tc.id}/quarantine", json={"quarantined": False})
    assert off.json()["quarantined"] is False and off.json()["quarantined_at"] is None
    assert (await client.post("/api/tests/9999/quarantine", json={"quarantined": True})).status_code == 404
