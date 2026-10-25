"""Leaderboard, Repos and summary: scoping, tiers, sorting, pagination."""

from datetime import timedelta

from app.models import utcnow

from tests.factories import (
    make_execution,
    make_project,
    make_run,
    make_test_case,
    make_test_score,
)


async def _seed(db):
    """writers-app/backend: flaky 0.8 (2 proofs), suspect 0.1, stable 0.
    writers-app/frontend: suspect 0.2 in src/app.test.ts.
    fantasy/backend: flaky 0.5 with the SAME test name as writers-app's flaky one."""
    now = utcnow()
    wb = await make_project(db, "andrewthetechie/writers-app", "backend", root="server")
    wf = await make_project(db, "andrewthetechie/writers-app", "frontend")
    fb = await make_project(db, "andrewthetechie/fantasy", "backend")
    await make_test_case(
        db, wb, name="test_flaky", flakiness_score=0.8, confirmed_flake_count=2, last_seen_at=now - timedelta(hours=3)
    )
    await make_test_case(db, wb, name="test_suspect", flakiness_score=0.1, last_seen_at=now - timedelta(hours=2))
    stable = await make_test_case(db, wb, name="test_stable", flakiness_score=0.0, last_seen_at=now)
    await make_test_case(
        db,
        wf,
        name="renders",
        classname="src/app.test.ts",
        file="src/app.test.ts",
        line=4,
        flakiness_score=0.2,
        last_seen_at=now - timedelta(hours=1),
    )
    await make_test_case(
        db, fb, name="test_flaky", flakiness_score=0.5, confirmed_flake_count=0, last_seen_at=now - timedelta(hours=5)
    )
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
        {
            "name": "andrewthetechie/writers-app",
            "projects": [{"name": "backend", "root": "server"}, {"name": "frontend", "root": ""}],
        },
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
    proj = (await client.get("/api/tests?repo=andrewthetechie/writers-app&project=backend&include_stable=true")).json()
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
        "total_tests": 5,
        "flaky_tests": 2,
        "suspect_tests": 2,
        "confirmed_flaky_tests": 1,
        "total_runs": 1,
        "total_executions": 1,
        "flake_threshold": 0.3,
        "category_counts": {"network": 0, "environment": 0, "timing": 0, "assertion": 0, "other": 0},
    }
    one = (await client.get("/api/summary?repo=andrewthetechie/fantasy")).json()
    assert (one["total_tests"], one["flaky_tests"], one["total_runs"]) == (1, 1, 0)


async def test_category_filter_and_counts(client, db):
    proj = await make_project(db, "acme/app", "backend")
    await make_test_case(db, proj, name="A", flakiness_score=0.8, failure_category="timing")
    await make_test_case(db, proj, name="B", flakiness_score=0.2, failure_category="network")
    await make_test_case(db, proj, name="C", flakiness_score=0.5, failure_category="timing")
    await make_test_case(db, proj, name="D", flakiness_score=0.0, failure_category="timing")
    await db.commit()

    timing = (await client.get("/api/tests?repo=acme/app&category=timing")).json()
    assert [t["name"] for t in timing["items"]] == ["A", "C"]
    assert timing["total"] == 2
    with_stable = (await client.get("/api/tests?repo=acme/app&category=timing&include_stable=true")).json()
    assert [t["name"] for t in with_stable["items"]] == ["A", "C", "D"]
    assert (await client.get("/api/tests?category=bogus")).status_code == 422
    summary = (await client.get("/api/summary?repo=acme/app")).json()
    assert summary["category_counts"] == {"network": 1, "environment": 0, "timing": 2, "assertion": 0, "other": 0}


async def test_trend_from_history(client, db):
    proj = await make_project(db, "acme/app", "backend")
    a = await make_test_case(db, proj, name="A", flakiness_score=0.5)
    b = await make_test_case(db, proj, name="B", flakiness_score=0.5)
    await make_test_score(db, a, utcnow().date() - timedelta(days=20), 0.1)
    await make_test_score(db, b, utcnow().date() - timedelta(days=3), 0.5)
    await db.commit()

    page = (await client.get("/api/tests?repo=acme/app")).json()
    by_name = {t["name"]: t for t in page["items"]}
    assert by_name["A"]["trend"] == "worsening"
    assert by_name["B"]["trend"] is None
    assert "clean_streak" in by_name["A"]


async def test_new_within_days_filter(client, db):
    """new_within_days=N: flaky now, and no old-enough history row at or above the threshold."""
    proj = await make_project(db, "acme/app", "backend")
    crossed = await make_test_case(db, proj, name="crossed", flakiness_score=0.6)
    old_flaky = await make_test_case(db, proj, name="old_flaky", flakiness_score=0.6)
    brand_new = await make_test_case(db, proj, name="brand_new", flakiness_score=0.6)
    await make_test_case(db, proj, name="no_history", flakiness_score=0.6)
    await make_test_case(db, proj, name="suspect", flakiness_score=0.1)
    today = utcnow().date()
    await make_test_score(db, crossed, today - timedelta(days=20), 0.1)
    await make_test_score(db, crossed, today - timedelta(days=3), 0.6)
    await make_test_score(db, old_flaky, today - timedelta(days=30), 0.6)
    await make_test_score(db, brand_new, today - timedelta(days=1), 0.6)
    await db.commit()

    week = (await client.get("/api/tests?repo=acme/app&new_within_days=7")).json()
    assert sorted(t["name"] for t in week["items"]) == ["brand_new", "crossed", "no_history"]
    assert week["total"] == 3  # old_flaky and the below-threshold suspect are excluded before pagination
    # The filter runs in SQL, so it paginates over the filtered set only.
    first_page = (await client.get("/api/tests?repo=acme/app&new_within_days=7&page_size=2&page=1")).json()
    assert len(first_page["items"]) == 2
    assert first_page["total"] == 3
    second_page = (await client.get("/api/tests?repo=acme/app&new_within_days=7&page_size=2&page=2")).json()
    assert sorted(t["name"] for t in first_page["items"] + second_page["items"]) == [
        "brand_new",
        "crossed",
        "no_history",
    ]

    days2 = (await client.get("/api/tests?repo=acme/app&new_within_days=2")).json()
    assert sorted(t["name"] for t in days2["items"]) == ["brand_new", "no_history"]  # crossed's -3 row is at 0.6
    assert days2["total"] == 2

    assert (await client.get("/api/tests?repo=acme/app&new_within_days=0")).status_code == 422
    assert (await client.get("/api/tests?repo=acme/app&new_within_days=91")).status_code == 422


async def test_q_filter_matches_name_across_repos(client, db):
    await _seed(db)
    page = (await client.get("/api/tests?q=flaky")).json()
    assert [t["name"] for t in page["items"]] == ["test_flaky", "test_flaky"]
    assert page["total"] == 2


async def test_q_filter_is_case_insensitive_substring(client, db):
    await _seed(db)
    page = (await client.get("/api/tests?q=APP.TEST")).json()  # matches file and classname
    assert [t["name"] for t in page["items"]] == ["renders"]


async def test_q_filter_includes_stable_tests(client, db):
    await _seed(db)
    page = (await client.get("/api/tests?q=stable")).json()  # no include_stable=true
    assert [t["name"] for t in page["items"]] == ["test_stable"]
    assert page["total"] == 1


async def test_q_filter_composes_with_project(client, db):
    await _seed(db)
    page = (await client.get("/api/tests?repo=andrewthetechie/writers-app&project=backend&q=stable")).json()
    assert [t["name"] for t in page["items"]] == ["test_stable"]


async def test_q_filter_is_literal_substring(client, db):
    await _seed(db)
    assert (await client.get("/api/tests?q=%25")).json()["total"] == 0  # "%" is literal
    assert (await client.get("/api/tests?q=")).json()["total"] == 4  # empty q behaves as absent


async def test_q_filter_rejects_overlong_query(client, db):
    await _seed(db)
    assert (await client.get(f"/api/tests?q={'a' * 201}")).status_code == 422


async def _seed_quarantine(db):
    """acme/app/backend: a stable quarantined Test (issue 3), a stable one, a quarantined
    flaky one and a flaky one with an issue link (4)."""
    proj = await make_project(db, "acme/app", "backend")
    await make_test_case(
        db, proj, name="quarantined_stable", flakiness_score=0.0, quarantined=True, github_issue_number=3
    )
    await make_test_case(db, proj, name="stable", flakiness_score=0.0)
    await make_test_case(db, proj, name="quarantined_flaky", flakiness_score=0.8, quarantined=True)
    await make_test_case(db, proj, name="flaky", flakiness_score=0.6, github_issue_number=4)
    await db.commit()


async def test_quarantined_filter_includes_stable_tests(client, db):
    await _seed_quarantine(db)
    page = (await client.get("/api/tests?repo=acme/app&quarantined=true")).json()
    assert [t["name"] for t in page["items"]] == ["quarantined_flaky", "quarantined_stable"]
    assert page["total"] == 2
    # Absent param is today's behavior: stable Tests hidden, quarantined ones not special.
    default = (await client.get("/api/tests?repo=acme/app")).json()
    assert [t["name"] for t in default["items"]] == ["quarantined_flaky", "flaky"]


async def test_quarantined_false_excludes_quarantined_tests(client, db):
    await _seed_quarantine(db)
    page = (await client.get("/api/tests?repo=acme/app&quarantined=false")).json()
    assert [t["name"] for t in page["items"]] == ["flaky"]  # stable still hidden by default
    with_stable = (await client.get("/api/tests?repo=acme/app&quarantined=false&include_stable=true")).json()
    assert [t["name"] for t in with_stable["items"]] == ["flaky", "stable"]


async def test_has_issue_filter(client, db):
    await _seed_quarantine(db)
    linked = (await client.get("/api/tests?repo=acme/app&has_issue=true&include_stable=true")).json()
    assert [t["name"] for t in linked["items"]] == ["flaky", "quarantined_stable"]
    unlinked = (await client.get("/api/tests?repo=acme/app&has_issue=false&include_stable=true")).json()
    assert [t["name"] for t in unlinked["items"]] == ["quarantined_flaky", "stable"]


async def test_quarantined_and_has_issue_compose(client, db):
    await _seed_quarantine(db)
    page = (await client.get("/api/tests?repo=acme/app&quarantined=true&has_issue=false")).json()
    assert [t["name"] for t in page["items"]] == ["quarantined_flaky"]
    assert page["total"] == 1
    linked = (await client.get("/api/tests?repo=acme/app&quarantined=true&has_issue=true")).json()
    assert [t["name"] for t in linked["items"]] == ["quarantined_stable"]


async def test_q_filter_composes_with_category(client, db):
    proj = await make_project(db, "acme/app", "backend")
    # Classnames carry no "d" so q=D can only hit the name.
    await make_test_case(db, proj, name="A", classname="grp", flakiness_score=0.8, failure_category="timing")
    await make_test_case(db, proj, name="B", classname="grp", flakiness_score=0.2, failure_category="network")
    await make_test_case(db, proj, name="C", classname="grp", flakiness_score=0.5, failure_category="timing")
    await make_test_case(db, proj, name="D", classname="grp", flakiness_score=0.0, failure_category="timing")
    await db.commit()

    page = (await client.get("/api/tests?repo=acme/app&category=timing&q=D")).json()
    assert [t["name"] for t in page["items"]] == ["D"]  # stable included, category still applied
    assert page["total"] == 1
