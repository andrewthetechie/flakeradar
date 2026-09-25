"""Leaderboard, Repos and summary: scoping, tiers, sorting, pagination."""
from datetime import timedelta

from app.models import utcnow
from tests.factories import (
    make_execution, make_project, make_run, make_test_case,
)


async def _seed(db):
    """writers-app/backend: flaky 0.8 (2 proofs), suspect 0.1, stable 0.
    writers-app/frontend: suspect 0.2 in src/app.test.ts.
    fantasy/backend: flaky 0.5 with the SAME test name as writers-app's flaky one."""
    now = utcnow()
    wb = await make_project(db, "andrewthetechie/writers-app", "backend", root="server")
    wf = await make_project(db, "andrewthetechie/writers-app", "frontend")
    fb = await make_project(db, "andrewthetechie/fantasy", "backend")
    await make_test_case(db, wb, name="test_flaky", flakiness_score=0.8,
                         confirmed_flake_count=2, last_seen_at=now - timedelta(hours=3))
    await make_test_case(db, wb, name="test_suspect", flakiness_score=0.1,
                         last_seen_at=now - timedelta(hours=2))
    stable = await make_test_case(db, wb, name="test_stable", flakiness_score=0.0,
                                  last_seen_at=now)
    await make_test_case(db, wf, name="renders", classname="src/app.test.ts",
                         file="src/app.test.ts", line=4, flakiness_score=0.2,
                         last_seen_at=now - timedelta(hours=1))
    await make_test_case(db, fb, name="test_flaky", flakiness_score=0.5,
                         confirmed_flake_count=0, last_seen_at=now - timedelta(hours=5))
    run = await make_run(db, wb)
    await make_execution(db, stable, run)
    await db.commit()


def _names(page) -> list[tuple[str, str, str]]:
    return [(t["repo"], t["project"], t["name"]) for t in page["items"]]


async def test_repos_endpoint(client, db):
    await _seed(db)
    repos = (await client.get("/api/repos")).json()
    assert repos == [
        {"name": "andrewthetechie/fantasy", "projects": [{"name": "backend", "root": ""}]},
        {"name": "andrewthetechie/writers-app", "projects": [
            {"name": "backend", "root": "server"}, {"name": "frontend", "root": ""}]},
    ]


async def test_default_leaderboard_hides_stable_and_sorts_by_score(client, db):
    await _seed(db)
    page = (await client.get("/api/tests")).json()
    assert page["total"] == 4 and page["page"] == 1 and page["page_size"] == 50
    assert _names(page) == [
        ("andrewthetechie/writers-app", "backend", "test_flaky"),
        ("andrewthetechie/fantasy", "backend", "test_flaky"),
        ("andrewthetechie/writers-app", "frontend", "renders"),
        ("andrewthetechie/writers-app", "backend", "test_suspect"),
    ]
    assert [t["tier"] for t in page["items"]] == ["flaky", "flaky", "suspect", "suspect"]
    assert page["items"][2]["file"] == "src/app.test.ts" and page["items"][2]["line"] == 4


async def test_scope_repo_and_project(client, db):
    await _seed(db)
    repo_page = (await client.get("/api/tests?repo=AndrewTheTechie/Writers-App")).json()
    assert {t["repo"] for t in repo_page["items"]} == {"andrewthetechie/writers-app"}
    assert repo_page["total"] == 3
    proj = (await client.get(
        "/api/tests?repo=andrewthetechie/writers-app&project=backend&include_stable=true")).json()
    assert [t["name"] for t in proj["items"]] == ["test_flaky", "test_suspect", "test_stable"]
    assert (await client.get("/api/tests?project=backend")).status_code == 422
    assert (await client.get("/api/tests?repo=nope")).status_code == 422
    unknown = (await client.get("/api/tests?repo=someone/else")).json()
    assert unknown == {"items": [], "total": 0, "page": 1, "page_size": 50}


async def test_sorting_and_pagination(client, db):
    await _seed(db)
    proven = (await client.get("/api/tests?sort=proven")).json()
    assert proven["items"][0]["confirmed_flake_count"] == 2
    seen = (await client.get("/api/tests?sort=last_seen")).json()
    assert [t["name"] for t in seen["items"]] == ["renders", "test_suspect", "test_flaky", "test_flaky"]
    p2 = (await client.get("/api/tests?page=2&page_size=3")).json()
    assert p2["total"] == 4 and len(p2["items"]) == 1
    assert (await client.get("/api/tests?page_size=101")).status_code == 422
    assert (await client.get("/api/tests?sort=name")).status_code == 422


async def test_file_filter_is_literal_substring(client, db):
    await _seed(db)
    page = (await client.get("/api/tests?file=app.test")).json()
    assert [t["name"] for t in page["items"]] == ["renders"]
    assert (await client.get("/api/tests?file=%25")).json()["total"] == 0  # "%" is literal


async def test_summary_scoped(client, db):
    await _seed(db)
    everything = (await client.get("/api/summary")).json()
    assert everything == {
        "total_tests": 5, "flaky_tests": 2, "suspect_tests": 2,
        "confirmed_flaky_tests": 1, "total_runs": 1, "total_executions": 1,
        "flake_threshold": 0.3,
    }
    one = (await client.get("/api/summary?repo=andrewthetechie/fantasy")).json()
    assert (one["total_tests"], one["flaky_tests"], one["total_runs"]) == (1, 1, 0)
