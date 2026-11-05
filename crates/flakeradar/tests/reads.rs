//! The dashboard read APIs: Repos, the Tests leaderboard, summaries, Test detail
//! and quarantine.
//!
//! Ported from `backend/tests/test_tests_api.py` and
//! `backend/tests/test_test_detail_api.py`. Every assertion reads the response
//! the way the frontend and the actions do.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a fixture or an answer does not behave"
)]

mod common;

use chrono::{DateTime, Duration, FixedOffset, NaiveDateTime, Utc};
use common::{app, get, request_with_headers};
use flakeradar_test_support::{
    TestDb, make_execution, make_project, make_run, make_test_case, make_test_score,
};
use sea_orm::ConnectionTrait as _;
use sea_orm::entity::prelude::DateTimeWithTimeZone;
use serde_json::{Value, json};

/// A timestamp `hours` before now, as the database stores it.
fn hours_ago(hours: i64) -> DateTimeWithTimeZone {
    (Utc::now() - Duration::hours(hours)).fixed_offset()
}

/// The UTC day `days` before today.
fn day(days: i64) -> chrono::NaiveDate {
    (Utc::now() - Duration::days(days)).date_naive()
}

/// The `(repo, project, name)` triples of a leaderboard page.
fn names(page: &Value) -> Vec<(String, String, String)> {
    page["items"]
        .as_array()
        .expect("items is a list")
        .iter()
        .map(|item| {
            (
                item["repo"].as_str().expect("a repo").to_owned(),
                item["project"].as_str().expect("a project").to_owned(),
                item["name"].as_str().expect("a name").to_owned(),
            )
        })
        .collect()
}

/// The names of a page's items, in the order it lists them.
fn item_names(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .expect("items is a list")
        .iter()
        .map(|item| item["name"].as_str().expect("a name").to_owned())
        .collect()
}

/// The writers-app / fantasy fixture the Python tests seed.
async fn seed(db: &TestDb) {
    let connection = db.connection();
    let wb = make_project()
        .repo("andrewthetechie/writers-app")
        .project("backend")
        .root("server")
        .build(connection)
        .await
        .expect("the backend project");
    let wf = make_project()
        .repo("andrewthetechie/writers-app")
        .project("frontend")
        .build(connection)
        .await
        .expect("the frontend project");
    let fb = make_project()
        .repo("andrewthetechie/fantasy")
        .project("backend")
        .build(connection)
        .await
        .expect("the fantasy project");
    make_test_case(&wb)
        .name("test_flaky")
        .flakiness_score(0.8)
        .confirmed_flake_count(2)
        .last_seen_at(hours_ago(3))
        .build(connection)
        .await
        .expect("the flaky test");
    make_test_case(&wb)
        .name("test_suspect")
        .flakiness_score(0.1)
        .last_seen_at(hours_ago(2))
        .build(connection)
        .await
        .expect("the suspect test");
    let stable = make_test_case(&wb)
        .name("test_stable")
        .flakiness_score(0.0)
        .build(connection)
        .await
        .expect("the stable test");
    make_test_case(&wf)
        .name("renders")
        .classname("src/app.test.ts")
        .file("src/app.test.ts")
        .line(4)
        .flakiness_score(0.2)
        .last_seen_at(hours_ago(1))
        .build(connection)
        .await
        .expect("the frontend test");
    make_test_case(&fb)
        .name("test_flaky")
        .flakiness_score(0.5)
        .last_seen_at(hours_ago(5))
        .build(connection)
        .await
        .expect("the fantasy flaky test");
    let run = make_run(&wb).build(connection).await.expect("a run");
    make_execution(&stable, &run)
        .build(connection)
        .await
        .expect("an execution");
}

#[tokio::test]
async fn repos_endpoint_lists_each_repo_with_its_projects() {
    let db = TestDb::new().await.expect("a test database");
    seed(&db).await;
    let reply = get(&app(&db, &[]), "/api/repos").await;
    assert_eq!(reply.status, 200);
    assert_eq!(
        reply.json(),
        json!([
            {"name": "andrewthetechie/fantasy", "projects": [{"name": "backend", "root": ""}]},
            {
                "name": "andrewthetechie/writers-app",
                "projects": [
                    {"name": "backend", "root": "server"},
                    {"name": "frontend", "root": ""}
                ]
            }
        ])
    );
}

#[tokio::test]
async fn default_leaderboard_hides_stable_and_sorts_by_score() {
    let db = TestDb::new().await.expect("a test database");
    seed(&db).await;
    let page = get(&app(&db, &[]), "/api/tests").await.json();
    assert_eq!(page["total"], 4);
    assert_eq!(page["page"], 1);
    assert_eq!(page["page_size"], 50);
    assert_eq!(
        names(&page),
        vec![
            (
                "andrewthetechie/writers-app".to_owned(),
                "backend".to_owned(),
                "test_flaky".to_owned()
            ),
            (
                "andrewthetechie/fantasy".to_owned(),
                "backend".to_owned(),
                "test_flaky".to_owned()
            ),
            (
                "andrewthetechie/writers-app".to_owned(),
                "frontend".to_owned(),
                "renders".to_owned()
            ),
            (
                "andrewthetechie/writers-app".to_owned(),
                "backend".to_owned(),
                "test_suspect".to_owned()
            ),
        ]
    );
    let tiers: Vec<&str> = page["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["tier"].as_str().expect("a tier"))
        .collect();
    assert_eq!(tiers, ["flaky", "flaky", "suspect", "suspect"]);
    assert_eq!(page["items"][2]["file"], json!("src/app.test.ts"));
    assert_eq!(page["items"][2]["line"], json!(4));
}

#[tokio::test]
async fn scope_takes_a_repo_and_a_project_that_needs_it() {
    let db = TestDb::new().await.expect("a test database");
    seed(&db).await;
    let app = app(&db, &[]);
    let repo = get(&app, "/api/tests?repo=AndrewTheTechie/Writers-App")
        .await
        .json();
    assert_eq!(
        repo["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|item| item["repo"].as_str().expect("a repo"))
            .collect::<Vec<_>>(),
        ["andrewthetechie/writers-app"; 3]
    );
    assert_eq!(repo["total"], 3);

    let project = get(
        &app,
        "/api/tests?repo=andrewthetechie/writers-app&project=backend&include_stable=true",
    )
    .await
    .json();
    assert_eq!(
        item_names(&project),
        ["test_flaky", "test_suspect", "test_stable"]
    );

    assert_eq!(get(&app, "/api/tests?project=backend").await.status, 422);
    assert_eq!(get(&app, "/api/tests?repo=nope").await.status, 422);
    let unknown = get(&app, "/api/tests?repo=someone/else").await.json();
    assert_eq!(
        unknown,
        json!({"items": [], "total": 0, "page": 1, "page_size": 50})
    );
}

#[tokio::test]
async fn sorting_and_pagination_follow_the_order_keys() {
    let db = TestDb::new().await.expect("a test database");
    seed(&db).await;
    let app = app(&db, &[]);
    let proven = get(&app, "/api/tests?sort=proven").await.json();
    assert_eq!(proven["items"][0]["confirmed_flake_count"], 2);
    let seen = get(&app, "/api/tests?sort=last_seen").await.json();
    assert_eq!(
        item_names(&seen),
        ["renders", "test_suspect", "test_flaky", "test_flaky"]
    );
    let second = get(&app, "/api/tests?page=2&page_size=3").await.json();
    assert_eq!(second["total"], 4);
    assert_eq!(second["items"].as_array().expect("items").len(), 1);
    assert_eq!(get(&app, "/api/tests?page_size=101").await.status, 422);
    assert_eq!(get(&app, "/api/tests?sort=name").await.status, 422);
}

#[tokio::test]
async fn a_page_past_the_end_is_empty_but_still_counts() {
    let db = TestDb::new().await.expect("a test database");
    seed(&db).await;
    let page = get(&app(&db, &[]), "/api/tests?page=5&page_size=2")
        .await
        .json();
    assert_eq!(page["items"], json!([]));
    assert_eq!(page["total"], 4);
    assert_eq!(page["page"], 5);
}

#[tokio::test]
async fn the_file_filter_is_a_literal_substring() {
    let db = TestDb::new().await.expect("a test database");
    seed(&db).await;
    let app = app(&db, &[]);
    let page = get(&app, "/api/tests?file=app.test").await.json();
    assert_eq!(item_names(&page), ["renders"]);
    // A wildcard in the filter matches itself, not everything.
    assert_eq!(get(&app, "/api/tests?file=%25").await.json()["total"], 0);
    // An underscore is literal too, so "app.test" does not match "appXtest".
    assert_eq!(
        get(&app, "/api/tests?file=app_test").await.json()["total"],
        0
    );
    // A backslash is literal.
    assert_eq!(get(&app, "/api/tests?file=%5C").await.json()["total"], 0);
}

#[tokio::test]
async fn the_broken_filter_lists_broken_stable_tests() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("andrewthetechie/writers-app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the project");
    make_test_case(&project)
        .name("test_broken")
        .fail_streak(5)
        .build(db.connection())
        .await
        .expect("the broken test");
    make_test_case(&project)
        .name("test_passing")
        .build(db.connection())
        .await
        .expect("the passing test");

    let app = app(&db, &[]);
    let default = get(&app, "/api/tests?repo=andrewthetechie/writers-app")
        .await
        .json();
    assert_eq!(default["total"], 0);

    let broken = get(
        &app,
        "/api/tests?repo=andrewthetechie/writers-app&broken=true",
    )
    .await
    .json();
    assert_eq!(item_names(&broken), ["test_broken"]);
    assert_eq!(broken["items"][0]["broken"], true);
    assert_eq!(broken["items"][0]["fail_streak"], 5);
}

#[tokio::test]
async fn the_broken_filter_lists_nothing_when_detection_is_disabled() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("andrewthetechie/writers-app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the project");
    make_test_case(&project)
        .name("test_broken")
        .fail_streak(5)
        .build(db.connection())
        .await
        .expect("the broken test");
    let broken = get(
        &app(&db, &[("FLAKERADAR_BROKEN_STREAK", "0")]),
        "/api/tests?repo=andrewthetechie/writers-app&broken=true",
    )
    .await
    .json();
    assert_eq!(
        broken,
        json!({"items": [], "total": 0, "page": 1, "page_size": 50})
    );
}

#[tokio::test]
async fn the_summary_counts_one_scope() {
    let db = TestDb::new().await.expect("a test database");
    seed(&db).await;
    let app = app(&db, &[]);
    let everything = get(&app, "/api/summary").await.json();
    assert_eq!(
        everything,
        json!({
            "total_tests": 5,
            "flaky_tests": 2,
            "suspect_tests": 2,
            "confirmed_flaky_tests": 1,
            "new_flaky_7d": 2,
            "total_runs": 1,
            "executions_30d": 0,
            "failures_30d": 0,
            "flake_threshold": 0.3,
            "category_counts": {
                "network": 0, "environment": 0, "timing": 0, "assertion": 0, "other": 0
            }
        })
    );
    let one = get(&app, "/api/summary?repo=andrewthetechie/fantasy")
        .await
        .json();
    assert_eq!(one["total_tests"], 1);
    assert_eq!(one["flaky_tests"], 1);
    assert_eq!(one["total_runs"], 0);
}

#[tokio::test]
async fn the_summary_sums_the_last_thirty_days_in_scope() {
    let db = TestDb::new().await.expect("a test database");
    let backend = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the backend project");
    let elsewhere = make_project()
        .repo("acme/other")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the other project");
    let test = make_test_case(&backend)
        .name("test_flaky")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the test");
    let other = make_test_case(&elsewhere)
        .name("test_elsewhere")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the other test");
    for (entity, days, executions, failures) in
        [(&test, 0, 10, 2), (&test, 3, 5, 1), (&test, 31, 100, 50)]
    {
        make_test_score(entity, day(days), 0.6)
            .executions(executions)
            .failures(failures)
            .build(db.connection())
            .await
            .expect("a score row");
    }
    make_test_score(&other, day(0), 0.6)
        .executions(7)
        .failures(7)
        .build(db.connection())
        .await
        .expect("the other score row");

    let app = app(&db, &[]);
    let everything = get(&app, "/api/summary").await.json();
    assert_eq!(everything["executions_30d"], 22);
    assert_eq!(everything["failures_30d"], 10);
    let scoped = get(&app, "/api/summary?repo=acme/app").await.json();
    assert_eq!(scoped["executions_30d"], 15);
    assert_eq!(scoped["failures_30d"], 3);
    let other_repo = get(&app, "/api/summary?repo=acme/other").await.json();
    assert_eq!(other_repo["executions_30d"], 7);
    assert_eq!(other_repo["failures_30d"], 7);
}

#[tokio::test]
async fn new_flaky_seven_days_counts_a_test_with_no_old_history() {
    let db = TestDb::new().await.expect("a test database");
    let backend = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the backend project");
    let frontend = make_project()
        .repo("acme/app")
        .project("frontend")
        .build(db.connection())
        .await
        .expect("the frontend project");
    let other = make_project()
        .repo("acme/other")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the other project");
    let mut made = Vec::new();
    for name in [
        "crossed",
        "at_threshold",
        "old_flaky",
        "boundary",
        "recent",
        "newest_old_low",
        "newest_old_high",
    ] {
        made.push(
            make_test_case(&backend)
                .name(name)
                .flakiness_score(0.6)
                .build(db.connection())
                .await
                .expect("a flaky test"),
        );
    }
    make_test_case(&backend)
        .name("suspect")
        .flakiness_score(0.1)
        .build(db.connection())
        .await
        .expect("the suspect test");
    make_test_case(&frontend)
        .name("fresh")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the fresh test");
    make_test_case(&other)
        .name("elsewhere")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the other test");
    let crossed = &made[0];
    let at_threshold = &made[1];
    let old_flaky = &made[2];
    let boundary = &made[3];
    let recent = &made[4];
    let newest_old_low = &made[5];
    let newest_old_high = &made[6];
    for (entity, days, score) in [
        (crossed, 20, 0.1),
        (crossed, 3, 0.6),
        (at_threshold, 10, 0.3),
        (old_flaky, 30, 0.6),
        (boundary, 7, 0.6),
        (recent, 6, 0.6),
        (newest_old_low, 25, 0.6),
        (newest_old_low, 10, 0.1),
        (newest_old_high, 25, 0.1),
        (newest_old_high, 10, 0.6),
    ] {
        make_test_score(entity, day(days), score)
            .build(db.connection())
            .await
            .expect("a score row");
    }

    let app = app(&db, &[]);
    let scoped = get(&app, "/api/summary?repo=acme/app&project=backend")
        .await
        .json();
    assert_eq!(scoped["flaky_tests"], 7);
    assert_eq!(scoped["new_flaky_7d"], 3);
    let repo = get(&app, "/api/summary?repo=acme/app").await.json();
    assert_eq!(repo["flaky_tests"], 8);
    assert_eq!(repo["new_flaky_7d"], 4);
    assert_eq!(
        get(&app, "/api/summary?repo=acme/other").await.json()["new_flaky_7d"],
        1
    );
    assert_eq!(get(&app, "/api/summary").await.json()["new_flaky_7d"], 5);
}

#[tokio::test]
async fn the_category_filter_and_the_counts_agree() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the project");
    for (name, score, category) in [
        ("A", 0.8, "timing"),
        ("B", 0.2, "network"),
        ("C", 0.5, "timing"),
        ("D", 0.0, "timing"),
    ] {
        make_test_case(&project)
            .name(name)
            .flakiness_score(score)
            .failure_category(category)
            .build(db.connection())
            .await
            .expect("a test");
    }

    let app = app(&db, &[]);
    let timing = get(&app, "/api/tests?repo=acme/app&category=timing")
        .await
        .json();
    assert_eq!(item_names(&timing), ["A", "C"]);
    assert_eq!(timing["total"], 2);
    let with_stable = get(
        &app,
        "/api/tests?repo=acme/app&category=timing&include_stable=true",
    )
    .await
    .json();
    assert_eq!(item_names(&with_stable), ["A", "C", "D"]);
    assert_eq!(get(&app, "/api/tests?category=bogus").await.status, 422);
    let summary = get(&app, "/api/summary?repo=acme/app").await.json();
    assert_eq!(
        summary["category_counts"],
        json!({"network": 1, "environment": 0, "timing": 2, "assertion": 0, "other": 0})
    );
}

#[tokio::test]
async fn the_trend_comes_from_the_score_history() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the project");
    let a = make_test_case(&project)
        .name("A")
        .flakiness_score(0.5)
        .build(db.connection())
        .await
        .expect("the first test");
    let b = make_test_case(&project)
        .name("B")
        .flakiness_score(0.5)
        .build(db.connection())
        .await
        .expect("the second test");
    make_test_score(&a, day(20), 0.1)
        .build(db.connection())
        .await
        .expect("the old row");
    make_test_score(&b, day(3), 0.5)
        .build(db.connection())
        .await
        .expect("the recent row");

    let page = get(&app(&db, &[]), "/api/tests?repo=acme/app").await.json();
    let by_name: std::collections::BTreeMap<String, Value> = page["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            (
                item["name"].as_str().expect("a name").to_owned(),
                item.clone(),
            )
        })
        .collect();
    assert_eq!(by_name["A"]["trend"], "worsening");
    assert_eq!(by_name["B"]["trend"], Value::Null);
    assert!(by_name["A"].get("clean_streak").is_some());
}

#[tokio::test]
async fn new_within_days_filters_before_paging() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the project");
    let crossed = make_test_case(&project)
        .name("crossed")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("crossed");
    let old_flaky = make_test_case(&project)
        .name("old_flaky")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("old_flaky");
    let brand_new = make_test_case(&project)
        .name("brand_new")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("brand_new");
    make_test_case(&project)
        .name("no_history")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("no_history");
    make_test_case(&project)
        .name("suspect")
        .flakiness_score(0.1)
        .build(db.connection())
        .await
        .expect("suspect");
    for (entity, days, score) in [
        (&crossed, 20, 0.1),
        (&crossed, 3, 0.6),
        (&old_flaky, 30, 0.6),
        (&brand_new, 1, 0.6),
    ] {
        make_test_score(entity, day(days), score)
            .build(db.connection())
            .await
            .expect("a score row");
    }

    let app = app(&db, &[]);
    let week = get(&app, "/api/tests?repo=acme/app&new_within_days=7")
        .await
        .json();
    let mut found = item_names(&week);
    found.sort();
    assert_eq!(found, ["brand_new", "crossed", "no_history"]);
    assert_eq!(week["total"], 3);
    let first = get(
        &app,
        "/api/tests?repo=acme/app&new_within_days=7&page_size=2&page=1",
    )
    .await
    .json();
    assert_eq!(first["items"].as_array().expect("items").len(), 2);
    assert_eq!(first["total"], 3);
    let second = get(
        &app,
        "/api/tests?repo=acme/app&new_within_days=7&page_size=2&page=2",
    )
    .await
    .json();
    let mut both = item_names(&first);
    both.extend(item_names(&second));
    both.sort();
    assert_eq!(both, ["brand_new", "crossed", "no_history"]);

    let two = get(&app, "/api/tests?repo=acme/app&new_within_days=2")
        .await
        .json();
    let mut recent = item_names(&two);
    recent.sort();
    assert_eq!(recent, ["brand_new", "no_history"]);
    assert_eq!(two["total"], 2);
    assert_eq!(
        get(&app, "/api/tests?repo=acme/app&new_within_days=0")
            .await
            .status,
        422
    );
    assert_eq!(
        get(&app, "/api/tests?repo=acme/app&new_within_days=91")
            .await
            .status,
        422
    );
}

#[tokio::test]
async fn q_matches_a_name_across_repos() {
    let db = TestDb::new().await.expect("a test database");
    seed(&db).await;
    let page = get(&app(&db, &[]), "/api/tests?q=flaky").await.json();
    assert_eq!(item_names(&page), ["test_flaky", "test_flaky"]);
    assert_eq!(page["total"], 2);
}

#[tokio::test]
async fn q_is_a_case_insensitive_literal_substring() {
    let db = TestDb::new().await.expect("a test database");
    seed(&db).await;
    let app = app(&db, &[]);
    // Matches the file and the classname.
    let page = get(&app, "/api/tests?q=APP.TEST").await.json();
    assert_eq!(item_names(&page), ["renders"]);
    // "%" and "_" and "\" match themselves.
    assert_eq!(get(&app, "/api/tests?q=%25").await.json()["total"], 0);
    // "_test" would match ".test_mod" if "_" were a wildcard; it is literal.
    assert_eq!(get(&app, "/api/tests?q=_test").await.json()["total"], 0);
    assert_eq!(get(&app, "/api/tests?q=test_").await.json()["total"], 4);
    assert_eq!(get(&app, "/api/tests?q=%5C").await.json()["total"], 0);
}

#[tokio::test]
async fn q_includes_stable_tests_and_composes_with_a_project() {
    let db = TestDb::new().await.expect("a test database");
    seed(&db).await;
    let app = app(&db, &[]);
    // A search term keeps stable rows visible without include_stable.
    let page = get(&app, "/api/tests?q=stable").await.json();
    assert_eq!(item_names(&page), ["test_stable"]);
    assert_eq!(page["total"], 1);
    let scoped = get(
        &app,
        "/api/tests?repo=andrewthetechie/writers-app&project=backend&q=stable",
    )
    .await
    .json();
    assert_eq!(item_names(&scoped), ["test_stable"]);
    // An empty q behaves as absent.
    assert_eq!(get(&app, "/api/tests?q=").await.json()["total"], 4);
}

#[tokio::test]
async fn q_rejects_a_query_over_two_hundred_characters() {
    let db = TestDb::new().await.expect("a test database");
    seed(&db).await;
    let overlong = "a".repeat(201);
    let reply = get(&app(&db, &[]), &format!("/api/tests?q={overlong}")).await;
    assert_eq!(reply.status, 422);
    assert_eq!(reply.json()["detail"][0]["type"], "string_too_long");
    assert_eq!(reply.json()["detail"][0]["loc"], json!(["query", "q"]));
}

/// The quarantine fixture the Python tests seed.
async fn seed_quarantine(db: &TestDb) {
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the project");
    make_test_case(&project)
        .name("quarantined_stable")
        .quarantined()
        .github_issue_number(3)
        .build(db.connection())
        .await
        .expect("the quarantined stable test");
    make_test_case(&project)
        .name("stable")
        .build(db.connection())
        .await
        .expect("the stable test");
    make_test_case(&project)
        .name("quarantined_flaky")
        .flakiness_score(0.8)
        .quarantined()
        .build(db.connection())
        .await
        .expect("the quarantined flaky test");
    make_test_case(&project)
        .name("flaky")
        .flakiness_score(0.6)
        .github_issue_number(4)
        .build(db.connection())
        .await
        .expect("the flaky test");
}

#[tokio::test]
async fn the_quarantined_filter_includes_stable_tests() {
    let db = TestDb::new().await.expect("a test database");
    seed_quarantine(&db).await;
    let app = app(&db, &[]);
    let page = get(&app, "/api/tests?repo=acme/app&quarantined=true")
        .await
        .json();
    assert_eq!(
        item_names(&page),
        ["quarantined_flaky", "quarantined_stable"]
    );
    assert_eq!(page["total"], 2);
    let default = get(&app, "/api/tests?repo=acme/app").await.json();
    assert_eq!(item_names(&default), ["quarantined_flaky", "flaky"]);
}

#[tokio::test]
async fn quarantined_false_excludes_quarantined_tests() {
    let db = TestDb::new().await.expect("a test database");
    seed_quarantine(&db).await;
    let app = app(&db, &[]);
    let page = get(&app, "/api/tests?repo=acme/app&quarantined=false")
        .await
        .json();
    assert_eq!(item_names(&page), ["flaky"]);
    let with_stable = get(
        &app,
        "/api/tests?repo=acme/app&quarantined=false&include_stable=true",
    )
    .await
    .json();
    assert_eq!(item_names(&with_stable), ["flaky", "stable"]);
}

#[tokio::test]
async fn the_issue_filter_and_the_quarantine_filter_compose() {
    let db = TestDb::new().await.expect("a test database");
    seed_quarantine(&db).await;
    let app = app(&db, &[]);
    let linked = get(
        &app,
        "/api/tests?repo=acme/app&has_issue=true&include_stable=true",
    )
    .await
    .json();
    assert_eq!(item_names(&linked), ["flaky", "quarantined_stable"]);
    let unlinked = get(
        &app,
        "/api/tests?repo=acme/app&has_issue=false&include_stable=true",
    )
    .await
    .json();
    assert_eq!(item_names(&unlinked), ["quarantined_flaky", "stable"]);
    let page = get(
        &app,
        "/api/tests?repo=acme/app&quarantined=true&has_issue=false",
    )
    .await
    .json();
    assert_eq!(item_names(&page), ["quarantined_flaky"]);
    assert_eq!(page["total"], 1);
    let only_linked = get(
        &app,
        "/api/tests?repo=acme/app&quarantined=true&has_issue=true",
    )
    .await
    .json();
    assert_eq!(item_names(&only_linked), ["quarantined_stable"]);
}

#[tokio::test]
async fn q_composes_with_a_category_and_keeps_a_stable_match() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the project");
    // No classname carries a "d", so q=D can only hit a name.
    for (name, score, category) in [
        ("A", 0.8, "timing"),
        ("B", 0.2, "network"),
        ("C", 0.5, "timing"),
        ("D", 0.0, "timing"),
    ] {
        make_test_case(&project)
            .name(name)
            .classname("grp")
            .flakiness_score(score)
            .failure_category(category)
            .build(db.connection())
            .await
            .expect("a test");
    }
    let page = get(
        &app(&db, &[]),
        "/api/tests?repo=acme/app&category=timing&q=D",
    )
    .await
    .json();
    assert_eq!(item_names(&page), ["D"]);
    assert_eq!(page["total"], 1);
}

/// A test's history fixture: a failing then a passing execution.
async fn seed_detail(db: &TestDb, file: Option<&str>, line: Option<i32>, root: &str) -> i32 {
    let project = make_project()
        .repo("andrewthetechie/writers-app")
        .project("frontend")
        .root(root)
        .build(db.connection())
        .await
        .expect("the project");
    let mut builder = make_test_case(&project)
        .name("renders")
        .classname("src/app.test.ts")
        .flakiness_score(0.6)
        .confirmed_flake_count(1)
        .failure_category("assertion");
    if let Some(file) = file {
        builder = builder.file(file);
    }
    if let Some(line) = line {
        builder = builder.line(line);
    }
    let test = builder.build(db.connection()).await.expect("the test");
    let failing = make_run(&project)
        .commit_sha("aaa111")
        .ci_run_id("1")
        .build(db.connection())
        .await
        .expect("the failing run");
    make_execution(&test, &failing)
        .status("failed")
        .message("expected 3")
        .details("Traceback\n  at src/app.test.ts:14")
        .failure_category("assertion")
        .build(db.connection())
        .await
        .expect("the failing execution");
    let passing = make_run(&project)
        .commit_sha("bbb222")
        .branch("feat")
        .ci_run_id("2")
        .build(db.connection())
        .await
        .expect("the passing run");
    make_execution(&test, &passing)
        .status("passed")
        .attempt(1)
        .build(db.connection())
        .await
        .expect("the passing execution");
    test.id
}

#[tokio::test]
async fn history_includes_the_location_permalink_and_details() {
    let db = TestDb::new().await.expect("a test database");
    let test_id = seed_detail(&db, Some("src/app.test.ts"), Some(12), "frontend").await;
    let body = get(&app(&db, &[]), &format!("/api/tests/{test_id}/history"))
        .await
        .json();
    assert_eq!(body["test"]["repo"], "andrewthetechie/writers-app");
    assert_eq!(body["test"]["project"], "frontend");
    assert_eq!(body["test"]["tier"], "flaky");
    assert_eq!(
        body["location"],
        json!({
            "path": "frontend/src/app.test.ts",
            "line": 12,
            "url": "https://github.com/andrewthetechie/writers-app/blob/aaa111/frontend/src/app.test.ts#L12"
        })
    );
    assert_eq!(body["last_failing_sha"], "aaa111");
    assert_eq!(body["last_failing_branch"], "main");
    let statuses: Vec<&str> = body["executions"]
        .as_array()
        .expect("executions")
        .iter()
        .map(|execution| execution["status"].as_str().expect("a status"))
        .collect();
    assert_eq!(statuses, ["passed", "failed"]);
    assert_eq!(body["test"]["failure_category"], "assertion");
    assert_eq!(body["executions"][1]["failure_category"], "assertion");
    assert_eq!(body["executions"][0]["attempt"], 1);
    assert_eq!(
        body["executions"][1]["details"],
        "Traceback\n  at src/app.test.ts:14"
    );
    assert_eq!(body["executions"][1]["message"], "expected 3");
}

#[tokio::test]
async fn history_limit_and_unknown_test() {
    let db = TestDb::new().await.expect("a test database");
    let test_id = seed_detail(&db, Some("src/app.test.ts"), Some(12), "frontend").await;
    let app = app(&db, &[]);
    let body = get(&app, &format!("/api/tests/{test_id}/history?limit=1"))
        .await
        .json();
    assert_eq!(body["executions"].as_array().expect("executions").len(), 1);
    let missing = get(&app, "/api/tests/9999/history").await;
    assert_eq!(missing.status, 404);
    assert_eq!(missing.json(), json!({"detail": "Test not found"}));
    assert_eq!(
        get(&app, &format!("/api/tests/{test_id}/history?limit=501"))
            .await
            .status,
        422
    );
}

#[tokio::test]
async fn a_location_anchors_only_a_positive_line() {
    let db = TestDb::new().await.expect("a test database");
    let test_id = seed_detail(&db, Some("src/app.test.ts"), Some(0), "").await;
    let body = get(&app(&db, &[]), &format!("/api/tests/{test_id}/history"))
        .await
        .json();
    assert_eq!(
        body["location"]["url"],
        "https://github.com/andrewthetechie/writers-app/blob/aaa111/src/app.test.ts"
    );
}

#[tokio::test]
async fn no_file_means_no_location() {
    let db = TestDb::new().await.expect("a test database");
    let test_id = seed_detail(&db, None, None, "frontend").await;
    let body = get(&app(&db, &[]), &format!("/api/tests/{test_id}/history"))
        .await
        .json();
    assert_eq!(body["location"], Value::Null);
    assert_eq!(body["last_failing_sha"], "aaa111");
}

#[tokio::test]
async fn a_test_that_never_failed_has_a_path_but_no_url() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("project");
    let test = make_test_case(&project)
        .file("tests/test_a.py")
        .line(3)
        .build(db.connection())
        .await
        .expect("the test");
    let run = make_run(&project)
        .build(db.connection())
        .await
        .expect("run");
    make_execution(&test, &run)
        .build(db.connection())
        .await
        .expect("the execution");
    let body = get(&app(&db, &[]), &format!("/api/tests/{}/history", test.id))
        .await
        .json();
    assert_eq!(
        body["location"],
        json!({"path": "tests/test_a.py", "line": 3, "url": null})
    );
    assert_eq!(body["last_failing_sha"], Value::Null);
}

/// Sends a POST with a JSON body, as the frontend does.
async fn post(
    app: &axum::Router,
    uri: &str,
    headers: &[(&str, &str)],
    body: impl Into<axum::body::Body>,
) -> common::Reply {
    let mut all: Vec<(&str, &str)> = headers.to_vec();
    if !all
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
    {
        all.push(("content-type", "application/json"));
    }
    common::post(app, uri, &all, body).await
}

/// The token the tests present to the quarantine route.
const AUTH: &str = "test-token";

/// The app with the shared token set, so the quarantine list answers.
fn token_app(db: &TestDb) -> axum::Router {
    app(db, &[("FLAKERADAR_API_TOKEN", AUTH)])
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one fixture proves the whole quarantine contract in the order a human drives it"
)]
async fn quarantine_toggles_and_lists_one_project() {
    let db = TestDb::new().await.expect("a test database");
    let test_id = seed_detail(&db, Some("src/app.test.ts"), Some(12), "frontend").await;
    let other = make_project()
        .repo("andrewthetechie/writers-app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the other project");
    make_test_case(&other)
        .name("renders")
        .quarantined()
        .build(db.connection())
        .await
        .expect("the other test");

    let app = token_app(&db);
    let on = post(
        &app,
        &format!("/api/tests/{test_id}/quarantine"),
        &[],
        json!({"quarantined": true}).to_string(),
    )
    .await;
    assert_eq!(on.status, 200);
    assert_eq!(on.json()["quarantined"], true);
    assert!(on.json()["quarantined_at"].is_string());
    assert_eq!(on.json()["repo"], "andrewthetechie/writers-app");

    let url = "/api/quarantine?repo=andrewthetechie/writers-app&project=frontend";
    assert_eq!(get(&app, url).await.status, 401);
    let items = request_with_headers(&app, axum::http::Method::GET, url, &[("X-API-Key", AUTH)])
        .await
        .json();
    assert_eq!(
        items,
        json!([{
            "suite": "unit",
            "classname": "src/app.test.ts",
            "name": "renders",
            "fingerprint": items[0]["fingerprint"],
            "file": "src/app.test.ts",
            "line": 12,
            "quarantined_at": items[0]["quarantined_at"],
            "reason": null,
            "expires_at": null
        }])
    );

    let reason_on = post(
        &app,
        &format!("/api/tests/{test_id}/quarantine"),
        &[],
        json!({"quarantined": true, "reason": "flaky on windows", "expires_in_days": 7})
            .to_string(),
    )
    .await;
    assert_eq!(reason_on.status, 200);
    assert_eq!(reason_on.json()["quarantine_reason"], "flaky on windows");
    let expires: DateTime<FixedOffset> = DateTime::parse_from_rfc3339(
        reason_on.json()["quarantine_expires_at"]
            .as_str()
            .expect("an expiry"),
    )
    .expect("the expiry is RFC 3339");
    let expected = Utc::now() + Duration::days(7);
    assert!((expires.with_timezone(&Utc) - expected).abs() < Duration::minutes(5));

    let items = request_with_headers(&app, axum::http::Method::GET, url, &[("X-API-Key", AUTH)])
        .await
        .json();
    assert_eq!(items[0]["reason"], "flaky on windows");
    assert_eq!(
        items[0]["expires_at"],
        reason_on.json()["quarantine_expires_at"]
    );

    for days in [0, 366] {
        let bad = post(
            &app,
            &format!("/api/tests/{test_id}/quarantine"),
            &[],
            json!({"quarantined": true, "expires_in_days": days}).to_string(),
        )
        .await;
        assert_eq!(bad.status, 422);
        assert_eq!(
            bad.json()["detail"][0]["loc"],
            json!(["body", "expires_in_days"])
        );
    }
    let long_reason = post(
        &app,
        &format!("/api/tests/{test_id}/quarantine"),
        &[],
        json!({"quarantined": true, "reason": "x".repeat(501)}).to_string(),
    )
    .await;
    assert_eq!(long_reason.status, 422);
    assert_eq!(
        long_reason.json()["detail"][0]["loc"],
        json!(["body", "reason"])
    );

    // A rejected body never touched the stored decision.
    let items = request_with_headers(&app, axum::http::Method::GET, url, &[("X-API-Key", AUTH)])
        .await
        .json();
    assert_eq!(items[0]["reason"], "flaky on windows");

    let re_on = post(
        &app,
        &format!("/api/tests/{test_id}/quarantine"),
        &[],
        json!({"quarantined": true}).to_string(),
    )
    .await;
    assert_eq!(re_on.json()["quarantine_reason"], Value::Null);
    assert_eq!(re_on.json()["quarantine_expires_at"], Value::Null);

    let default_project = request_with_headers(
        &app,
        axum::http::Method::GET,
        "/api/quarantine?repo=andrewthetechie/writers-app",
        &[("X-API-Key", AUTH)],
    )
    .await
    .json();
    assert_eq!(default_project, json!([]));
    assert_eq!(
        request_with_headers(
            &app,
            axum::http::Method::GET,
            "/api/quarantine?project=frontend",
            &[("X-API-Key", AUTH)],
        )
        .await
        .status,
        422
    );

    let off = post(
        &app,
        &format!("/api/tests/{test_id}/quarantine"),
        &[],
        json!({"quarantined": false}).to_string(),
    )
    .await;
    assert_eq!(off.json()["quarantined"], false);
    assert_eq!(off.json()["quarantined_at"], Value::Null);
    assert_eq!(off.json()["quarantine_reason"], Value::Null);
    assert_eq!(off.json()["quarantine_expires_at"], Value::Null);
    assert_eq!(
        post(
            &app,
            "/api/tests/9999/quarantine",
            &[],
            json!({"quarantined": true}).to_string(),
        )
        .await
        .status,
        404
    );
}

#[tokio::test]
async fn quarantine_takes_pydantics_lax_boolean_and_404s_a_missing_test() {
    let db = TestDb::new().await.expect("a test database");
    let app = token_app(&db);
    for spelling in ["yes", "on", "1", "TRUE", "true"] {
        let reply = post(
            &app,
            "/api/tests/9999/quarantine",
            &[],
            json!({"quarantined": spelling}).to_string(),
        )
        .await;
        assert_eq!(reply.status, 404, "{spelling}");
        assert_eq!(reply.json(), json!({"detail": "Test not found"}));
    }
    for spelling in ["no", "off", "0", "FALSE"] {
        let reply = post(
            &app,
            "/api/tests/9999/quarantine",
            &[],
            json!({"quarantined": spelling}).to_string(),
        )
        .await;
        assert_eq!(reply.status, 404, "{spelling}");
    }
    for spelling in [json!(1), json!(0), json!(true), json!(false)] {
        let reply = post(
            &app,
            "/api/tests/9999/quarantine",
            &[],
            json!({"quarantined": spelling}).to_string(),
        )
        .await;
        assert_eq!(reply.status, 404, "{spelling}");
    }
    // A spelling Pydantic refused is still a 422, not a silent coercion.
    for spelling in [json!("maybe"), json!(2), json!(1.5)] {
        let reply = post(
            &app,
            "/api/tests/9999/quarantine",
            &[],
            json!({"quarantined": spelling}).to_string(),
        )
        .await;
        assert_eq!(reply.status, 422, "{spelling}");
        assert_eq!(reply.json()["detail"][0]["type"], "bool_parsing");
        assert_eq!(
            reply.json()["detail"][0]["loc"],
            json!(["body", "quarantined"])
        );
    }
    // A missing Test is not reached with no body at all.
    let empty = post(&app, "/api/tests/9999/quarantine", &[], "").await;
    assert_eq!(empty.status, 422);
    assert_eq!(empty.json()["detail"][0]["type"], "missing");
}

#[tokio::test]
async fn a_quarantine_body_that_fails_while_it_is_read_answers_the_500() {
    // A client that goes away mid-body is not a size problem, so the extractor's
    // body-read arm answers the same 500 every other route gives it.
    let db = TestDb::new().await.expect("a test database");
    let app = token_app(&db);
    let stream = futures_util::stream::iter(vec![
        Ok(axum::body::Bytes::from_static(b"{\"quarantined\":")),
        Err(std::io::Error::other("the client went away")),
    ]);
    let request = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri("/api/tests/9999/quarantine")
        .header("content-type", "application/json")
        .body(axum::body::Body::from_stream(stream))
        .expect("the fixture request builds");
    let reply = common::send(&app, request).await;
    assert_eq!(reply.status, 500);
    assert!(
        reply.text().contains("Reading the request body failed"),
        "{}",
        reply.text()
    );
}

#[tokio::test]
async fn the_quarantine_list_rejects_unusable_names() {
    let db = TestDb::new().await.expect("a test database");
    let app = token_app(&db);
    let bad_repo = request_with_headers(
        &app,
        axum::http::Method::GET,
        "/api/quarantine?repo=not-a-repo",
        &[("X-API-Key", AUTH)],
    )
    .await;
    assert_eq!(bad_repo.status, 422);
    assert!(
        bad_repo.json()["detail"]
            .as_str()
            .expect("a detail")
            .contains("repo must look like 'owner/name'")
    );
    let bad_project = request_with_headers(
        &app,
        axum::http::Method::GET,
        "/api/quarantine?repo=acme/app&project=Bad%20Name!",
        &[("X-API-Key", AUTH)],
    )
    .await;
    assert_eq!(bad_project.status, 422);
    assert!(
        bad_project.json()["detail"]
            .as_str()
            .expect("a detail")
            .starts_with("project must be 1-100 chars")
    );
}

#[tokio::test]
async fn score_history_is_embedded_oldest_first() {
    let db = TestDb::new().await.expect("a test database");
    let test_id = seed_detail(&db, Some("src/app.test.ts"), Some(12), "frontend").await;
    let test = seed_model(&db, test_id).await;
    for (days, score) in [(100, 0.9), (5, 0.5), (0, 0.2)] {
        make_test_score(&test, day(days), score)
            .build(db.connection())
            .await
            .expect("a score row");
    }
    let body = get(&app(&db, &[]), &format!("/api/tests/{test_id}/history"))
        .await
        .json();
    let days: Vec<&str> = body["score_history"]
        .as_array()
        .expect("score history")
        .iter()
        .map(|point| point["day"].as_str().expect("a day"))
        .collect();
    assert_eq!(
        days,
        [
            day(5).format("%Y-%m-%d").to_string(),
            day(0).format("%Y-%m-%d").to_string()
        ]
    );
    assert_eq!(body["score_history"][0]["flakiness_score"], 0.5);
}

/// The stored Test row `id`, read back for a fixture that needs its columns.
async fn seed_model(db: &TestDb, id: i32) -> flakeradar_db::entities::test_cases::Model {
    use sea_orm::{ColumnTrait as _, EntityTrait as _, QueryFilter as _};
    flakeradar_db::entities::test_cases::Entity::find()
        .filter(flakeradar_db::entities::test_cases::Column::Id.eq(id))
        .one(db.connection())
        .await
        .expect("the read succeeds")
        .expect("the Test exists")
}

/// Fabro's own view of a Test, copied from `flakeradar.rs` and `scout/flakeradar.rs`.
///
/// Every field Fabro reads is here, with timestamps as the text FastAPI sent, so
/// a missing or reshaped field fails to deserialize.
#[derive(Debug, serde::Deserialize)]
struct FabroTest {
    id: i64,
    project: String,
    fingerprint: String,
    suite: String,
    classname: String,
    name: String,
    file: Option<String>,
    line: Option<i64>,
    flakiness_score: f64,
    tier: String,
    confirmed_flake_count: i64,
    failure_category: Option<String>,
    last_seen_at: String,
    quarantined: bool,
}

/// Fabro's view of one Execution in a Test's history.
#[derive(Debug, serde::Deserialize)]
struct FabroExecution {
    status: String,
    message: String,
    details: String,
    created_at: String,
    branch: String,
    ci_run_id: String,
}

/// Fabro's view of a Test's history.
#[derive(Debug, serde::Deserialize)]
struct FabroHistory {
    executions: Vec<FabroExecution>,
    last_failing_sha: Option<String>,
    last_failing_branch: Option<String>,
}

/// Whether `text` is a timestamp Fabro's `normalise_ts` accepts: a date and a
/// time, an optional fraction, then `Z` or `+00:00`.
fn fabro_accepts(text: &str) -> bool {
    let rest = match text.strip_suffix('Z') {
        Some(rest) => rest,
        None => match text.strip_suffix("+00:00") {
            Some(rest) => rest,
            None => return false,
        },
    };
    let (whole, fraction) = match rest.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (rest, None),
    };
    NaiveDateTime::parse_from_str(whole, "%Y-%m-%dT%H:%M:%S").is_ok()
        && fraction.is_none_or(|digits| {
            !digits.is_empty() && digits.chars().all(|digit| digit.is_ascii_digit())
        })
}

#[tokio::test]
async fn fabro_reads_every_field_of_the_test_reads() {
    let db = TestDb::new().await.expect("a test database");
    let test_id = seed_detail(&db, Some("src/app.test.ts"), Some(12), "frontend").await;
    let app = app(&db, &[]);
    let page = get(&app, "/api/tests").await.json();
    let items: Vec<FabroTest> =
        serde_json::from_value(page["items"].clone()).expect("Fabro's struct reads the page");
    assert!(!items.is_empty());
    for item in &items {
        assert_ne!(item.id, 0);
        assert_ne!(item.project, "");
        assert_ne!(item.fingerprint, "");
        assert_ne!(item.suite, "");
        assert_ne!(item.classname, "");
        assert_ne!(item.name, "");
        assert!(item.confirmed_flake_count >= 0);
        assert!(fabro_accepts(&item.last_seen_at), "{}", item.last_seen_at);
        let _ = (
            &item.file,
            item.line,
            item.flakiness_score,
            &item.failure_category,
            item.quarantined,
            &item.tier,
        );
    }

    let history: FabroHistory = serde_json::from_value(
        get(&app, &format!("/api/tests/{test_id}/history"))
            .await
            .json(),
    )
    .expect("Fabro's struct reads the history");
    assert!(!history.executions.is_empty());
    for execution in &history.executions {
        assert!(fabro_accepts(&execution.created_at));
        assert_ne!(execution.status, "");
        assert_ne!(execution.branch, "");
        let _ = (&execution.message, &execution.details, &execution.ci_run_id);
    }
    assert_eq!(history.last_failing_sha.as_deref(), Some("aaa111"));
    assert_eq!(history.last_failing_branch.as_deref(), Some("main"));
}

#[test]
fn fabro_rejects_a_timestamp_without_a_zone() {
    assert!(fabro_accepts("2026-01-02T03:04:05Z"));
    assert!(fabro_accepts("2026-01-02T03:04:05.000000Z"));
    assert!(fabro_accepts("2026-01-02T03:04:05+00:00"));
    assert!(!fabro_accepts("2026-01-02T03:04:05"));
    assert!(!fabro_accepts("2026-01-02T03:04:05."));
    assert!(!fabro_accepts("2026-01-02 03:04:05Z"));
    assert!(!fabro_accepts("2026-01-02T03:04:05+01:00"));
}

/// A LibreOffice-free sanity check that `request_with_headers` reaches a route
/// behind the token; the assertion is the 200 the list answers.
#[tokio::test]
async fn the_quarantine_route_needs_the_token_and_nothing_else() {
    let db = TestDb::new().await.expect("a test database");
    let app = token_app(&db);
    assert_eq!(get(&app, "/api/quarantine?repo=acme/app").await.status, 401);
    let reply = request_with_headers(
        &app,
        axum::http::Method::GET,
        "/api/quarantine?repo=acme/app",
        &[("X-API-Key", AUTH)],
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.json(), json!([]));
}

#[tokio::test]
async fn a_scope_that_fails_its_length_rule_is_a_422() {
    let db = TestDb::new().await.expect("a test database");
    let app = token_app(&db);
    let long_repo = "a".repeat(256);
    for path in [
        format!("/api/summary?repo={long_repo}"),
        format!("/api/tests?repo={long_repo}"),
        format!("/api/quarantine?repo={long_repo}"),
    ] {
        let reply =
            request_with_headers(&app, axum::http::Method::GET, &path, &[("X-API-Key", AUTH)])
                .await;
        assert_eq!(reply.status, 422, "{path}");
        assert_eq!(reply.json()["detail"][0]["loc"], json!(["query", "repo"]));
    }
    let long_project = "b".repeat(101);
    let reply = get(
        &app,
        &format!("/api/summary?repo=acme/app&project={long_project}"),
    )
    .await;
    assert_eq!(reply.status, 422);
    assert_eq!(
        reply.json()["detail"][0]["loc"],
        json!(["query", "project"])
    );
    let long_file = "c".repeat(1025);
    let reply = get(&app, &format!("/api/tests?file={long_file}")).await;
    assert_eq!(reply.status, 422);
    assert_eq!(reply.json()["detail"][0]["loc"], json!(["query", "file"]));
}

#[tokio::test]
async fn a_failed_read_query_answers_the_plain_text_500() {
    let db = TestDb::new().await.expect("a test database");
    db.connection()
        .execute_unprepared("DROP TABLE test_cases CASCADE")
        .await
        .expect("the table is dropped");
    let reply = get(&app(&db, &[]), "/api/tests").await;
    assert_eq!(reply.status, 500);
    assert_eq!(reply.text(), "Internal Server Error");
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("text/plain; charset=utf-8")
    );
    let missing = get(&app(&db, &[]), "/api/tests/1/history").await;
    assert_eq!(missing.status, 500);
}
