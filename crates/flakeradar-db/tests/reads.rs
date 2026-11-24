//! The read services against a real database: the behaviour the HTTP tests
//! cannot reach, and the query functions the MCP server shares.
//!
//! The CI run failure cap and the recorded nextest flaky-fail case, and the
//! summaries that never touch the Execution tables.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a fixture or a read does not behave"
)]

use std::sync::Arc;

use chrono::{DateTime, Duration, FixedOffset};
use flakeradar_core::{
    dto::PipelineReportIn, dto::TestPage, dto::pipeline_body, model::ExecutionStatus, model::Tier,
    types::CiRunId,
};
use flakeradar_db::{
    entities::projects, entities::test_cases, processing::ProcessingSettings,
    processing::process_next, reads::ListOptions, reads::ReadSettings, reads::Scope,
    reads::SortKey, reads::TestListFilters, reads::ci_run_failures, reads::find_job_ids,
    reads::find_test_ids, reads::get_test, reads::job_summary, reads::latest_failure,
    reads::list_jobs, reads::list_tests, reads::search_jobs, reads::search_tests, reads::summary,
};
use flakeradar_test_support::{
    FIXTURE_INSTANT, FixedClock, TestDb, make_execution, make_job, make_job_execution,
    make_job_score, make_pipeline, make_project, make_report, make_run, make_test_case,
};
use sea_orm::{ConnectionTrait as _, DatabaseConnection};

/// A fixed instant, so "today" in the history reads is deterministic.
fn at() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-10-20T09:00:00+00:00").expect("a fixed instant")
}

/// The read settings a test uses: defaults, on a stopped clock.
fn settings() -> ReadSettings {
    ReadSettings {
        clock: Arc::new(FixedClock::new(at())),
        ..ReadSettings::default()
    }
}

/// The processor settings that produce the same days as [`settings`].
fn processing() -> ProcessingSettings {
    ProcessingSettings {
        clock: Arc::new(FixedClock::new(at())),
        ..ProcessingSettings::default()
    }
}

/// The fixture's JUnit body: one nextest flaky-fail retry.
const NEXTEST_FLAKY_FAIL: &str =
    include_str!("../../flakeradar-core/tests/fixtures/nextest_flaky_fail.xml");

/// A page request with every default and the given options.
fn options(change: impl FnOnce(&mut ListOptions)) -> ListOptions {
    let mut options = ListOptions {
        page: 1,
        page_size: 50,
        ..ListOptions::default()
    };
    change(&mut options);
    options
}

/// A search fixture: two Repos, a Project each, Tests with a matching name.
async fn seed_search(db: &TestDb) -> (i32, i32) {
    let app = make_project()
        .repo("acme/app")
        .project("backend")
        .create(db)
        .await;
    let other = make_project()
        .repo("acme/other")
        .project("backend")
        .create(db)
        .await;
    let wanted = make_test_case(&app)
        .name("test_login")
        .classname("tests.auth")
        .flakiness_score(0.6)
        .create(db)
        .await;
    make_test_case(&app)
        .name("test_logout")
        .classname("tests.auth")
        .create(db)
        .await;
    make_test_case(&other).name("test_login").create(db).await;
    (wanted.id, app.id)
}

/// A CI run id a test states as a literal.
fn run_id(text: &str) -> CiRunId {
    CiRunId::new(text).expect("the test states a CI run id")
}

#[tokio::test]
async fn search_tests_is_a_scoped_literal_substring_worst_first() {
    let db = TestDb::new().await.expect("a test database");
    let (wanted, _) = seed_search(&db).await;
    let found = search_tests(db.connection(), &Scope::default(), "LOGIN", &settings(), 20)
        .await
        .expect("the search");
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].flakiness_score.as_f64(), 0.6);
    let scoped = search_tests(
        db.connection(),
        &Scope::new(Some("acme/app".to_owned()), None).expect("a scope"),
        "login",
        &settings(),
        20,
    )
    .await
    .expect("the scoped search");
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].id, i64::from(wanted));
    assert_eq!(scoped[0].trend, None);
    let literal = search_tests(db.connection(), &Scope::default(), "%25", &settings(), 20)
        .await
        .expect("the literal search");
    assert_eq!(literal, Vec::new());
    let limited = search_tests(db.connection(), &Scope::default(), "test_", &settings(), 1)
        .await
        .expect("the limited search");
    assert_eq!(limited.len(), 1);
}

#[tokio::test]
async fn a_search_and_the_listing_answer_the_same_order() {
    // Two Tests with one score, so only the ordering past the score can tell them
    // apart. The newer one is inserted second and therefore holds the higher id:
    // a search that ordered `id` would answer it last, while the board and the
    // listing answer it first because it was seen most recently.
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().create(&db).await;
    let older = make_test_case(&project)
        .name("test_cron_a")
        .flakiness_score(0.5)
        .last_seen_at(at() - Duration::days(3))
        .create(&db)
        .await;
    let newer = make_test_case(&project)
        .name("test_cron_b")
        .flakiness_score(0.5)
        .last_seen_at(at())
        .create(&db)
        .await;
    let found = search_tests(db.connection(), &Scope::default(), "cron", &settings(), 20)
        .await
        .expect("the search");
    let searched: Vec<i64> = found.iter().map(|item| item.id).collect();
    assert_eq!(searched, vec![i64::from(newer.id), i64::from(older.id)]);
    let page = list_tests(
        db.connection(),
        &Scope::default(),
        &TestListFilters::default(),
        &options(|options| options.q = Some("cron".to_owned())),
        &settings(),
    )
    .await
    .expect("the page");
    let listed: Vec<i64> = page.items.iter().map(|item| item.id).collect();
    assert_eq!(listed, searched);
}

#[tokio::test]
async fn find_test_ids_matches_the_name_and_the_classname() {
    let db = TestDb::new().await.expect("a test database");
    let (wanted, _) = seed_search(&db).await;
    assert_eq!(
        find_test_ids(db.connection(), "acme/app", "backend", "test_login", None)
            .await
            .expect("the lookup"),
        vec![i64::from(wanted)]
    );
    assert_eq!(
        find_test_ids(
            db.connection(),
            "acme/app",
            "backend",
            "test_login",
            Some("tests.auth")
        )
        .await
        .expect("the lookup"),
        vec![i64::from(wanted)]
    );
    assert_eq!(
        find_test_ids(
            db.connection(),
            "acme/app",
            "backend",
            "test_login",
            Some("tests.other")
        )
        .await
        .expect("the lookup"),
        Vec::<i64>::new()
    );
    // The Project scopes the name: the same name exists in another Repo.
    assert_eq!(
        find_test_ids(
            db.connection(),
            "acme/other",
            "backend",
            "test_missing",
            None
        )
        .await
        .expect("the lookup"),
        Vec::<i64>::new()
    );
}

#[tokio::test]
async fn latest_failure_is_the_newest_failing_execution() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().create(&db).await;
    let test = make_test_case(&project).create(&db).await;
    assert!(
        latest_failure(db.connection(), i64::from(test.id))
            .await
            .expect("the lookup")
            .is_none()
    );
    let run = make_run(&project).create(&db).await;
    make_execution(&test, &run)
        .status("failed")
        .message("first")
        .create(&db)
        .await;
    make_execution(&test, &run)
        .status("failed")
        .message("second")
        .create(&db)
        .await;
    make_execution(&test, &run).create(&db).await;
    let failure = latest_failure(db.connection(), i64::from(test.id))
        .await
        .expect("the lookup")
        .expect("a failure");
    assert_eq!(failure.message, "second");
}

#[tokio::test]
async fn search_jobs_and_find_job_ids_match_a_job_by_name() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .name("ci.yml")
        .create(&db)
        .await;
    let wanted = make_job(&pipeline)
        .name("build-thing")
        .flakiness_score(0.6)
        .create(&db)
        .await;
    make_job(&pipeline).name("lint").create(&db).await;
    let found = search_jobs(db.connection(), "acme/app", "BUILD", &settings(), 20)
        .await
        .expect("the search");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, i64::from(wanted.id));
    let by_pipeline = search_jobs(db.connection(), "acme/app", "ci.yml", &settings(), 20)
        .await
        .expect("the pipeline search");
    assert_eq!(by_pipeline.len(), 2);
    assert_eq!(
        find_job_ids(db.connection(), "acme/app", "build-thing", None)
            .await
            .expect("the lookup"),
        vec![i64::from(wanted.id)]
    );
    assert_eq!(
        find_job_ids(db.connection(), "acme/app", "build-thing", Some("ci.yml"))
            .await
            .expect("the lookup"),
        vec![i64::from(wanted.id)]
    );
    assert_eq!(
        find_job_ids(
            db.connection(),
            "acme/app",
            "build-thing",
            Some("other.yml")
        )
        .await
        .expect("the lookup"),
        Vec::<i64>::new()
    );
}

#[tokio::test]
async fn the_flaky_only_listing_keeps_only_flaky_rows() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().create(&db).await;
    make_test_case(&project)
        .name("flaky")
        .flakiness_score(0.6)
        .create(&db)
        .await;
    make_test_case(&project).name("stable").create(&db).await;
    let page = list_tests(
        db.connection(),
        &Scope::default(),
        &TestListFilters::default(),
        &options(|options| options.flaky_only = true),
        &settings(),
    )
    .await
    .expect("the page");
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].name, "flaky");
}

#[tokio::test]
async fn a_page_whose_offset_overflows_is_an_error() {
    // Validation bounds `page` so the offset fits, and this is what a caller
    // that builds `ListOptions` itself gets instead of a wrapped offset: an
    // error, in a development build and in a release build alike.
    let db = TestDb::new().await.expect("a test database");
    let error = list_tests(
        db.connection(),
        &Scope::default(),
        &TestListFilters::default(),
        &options(|options| {
            options.page = i64::MAX;
            options.page_size = 100;
        }),
        &settings(),
    )
    .await
    .expect_err("a page whose offset does not fit is refused");
    assert!(
        error.to_string().contains("offset"),
        "the error names the offset: {error}"
    );
}

#[tokio::test]
async fn list_tests_sorts_by_proven_flakes_and_by_last_seen() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().create(&db).await;
    make_test_case(&project)
        .name("first")
        .flakiness_score(0.4)
        .create(&db)
        .await;
    make_test_case(&project)
        .name("proven")
        .flakiness_score(0.4)
        .confirmed_flake_count(3)
        .create(&db)
        .await;
    let by_proven = list_tests(
        db.connection(),
        &Scope::default(),
        &TestListFilters::default(),
        &options(|options| options.sort = SortKey::Proven),
        &settings(),
    )
    .await
    .expect("the page");
    assert_eq!(by_proven.items[0].name, "proven");
    let by_seen = list_tests(
        db.connection(),
        &Scope::default(),
        &TestListFilters::default(),
        &options(|options| options.sort = SortKey::LastSeen),
        &settings(),
    )
    .await
    .expect("the page");
    assert_eq!(by_seen.items.len(), 2);
}

/// The impact order lists the Test that broke the most CI runs first, whatever its
/// score, and `impact` is the spelling the query parameter takes.
#[tokio::test]
async fn list_tests_sorts_by_impacted_runs() {
    assert_eq!(SortKey::parse("impact"), Some(SortKey::Impact));
    assert_eq!(SortKey::parse("score"), Some(SortKey::Score));
    assert_eq!(SortKey::default(), SortKey::Score);

    let db = TestDb::new().await.expect("a test database");
    let project = impact_project(&db, Some("main")).await;
    let ran = instant("2026-10-15T09:00:00+00:00");
    let scored = make_test_case(&project)
        .name("scored")
        .flakiness_score(0.4)
        .create(&db)
        .await;
    let first_run = make_run(&project)
        .ci_run_id("100")
        .branch("main")
        .created_at(ran)
        .create(&db)
        .await;
    make_execution(&scored, &first_run)
        .status("failed")
        .created_at(ran)
        .duration(1.0)
        .create(&db)
        .await;
    let costly = make_test_case(&project)
        .name("costly")
        .flakiness_score(0.3)
        .create(&db)
        .await;
    for ci_run_id in ["200", "201", "202"] {
        let run = make_run(&project)
            .ci_run_id(ci_run_id)
            .branch("feat/x")
            .created_at(ran)
            .create(&db)
            .await;
        make_execution(&costly, &run)
            .status("failed")
            .created_at(ran)
            .duration(1.0)
            .create(&db)
            .await;
    }

    let by_impact = list_tests(
        db.connection(),
        &Scope::default(),
        &TestListFilters::default(),
        &options(|options| options.sort = SortKey::Impact),
        &settings(),
    )
    .await
    .expect("the page");
    assert_eq!(by_impact.items[0].name, "costly");
    assert_eq!(by_impact.items[1].name, "scored");
    assert_eq!(by_impact.items[0].impacted_runs, 3);
    assert_eq!(by_impact.items[1].impacted_runs, 1);

    // The default order is unchanged: the higher score still leads.
    let by_score = listed_tests(&db).await;
    assert_eq!(by_score.items[0].name, "scored");
}

/// An instant from its RFC 3339 text, for a fixture that names its own date.
fn instant(text: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(text).expect("a fixed instant")
}

/// A Repo with a Project, its default branch set only when the test names one.
async fn impact_project(db: &TestDb, default_branch: Option<&str>) -> projects::Model {
    let project = make_project().repo("acme/app").create(db).await;
    if let Some(branch) = default_branch {
        db.connection()
            .execute_unprepared(&format!("UPDATE repos SET default_branch = '{branch}'"))
            .await
            .expect("the default branch is stored");
    }
    project
}

/// A Test that failed in CI runs `100` (the default branch) and `101` (a feature
/// branch), in Executions of 1.5, 2.0 and 0.5 seconds, five days before the clock.
async fn impact_test(db: &TestDb, project: &projects::Model) -> test_cases::Model {
    let test = make_test_case(project)
        .name("flaky")
        .flakiness_score(0.6)
        .create(db)
        .await;
    let ran = instant("2026-10-15T09:00:00+00:00");
    let main_run = make_run(project)
        .ci_run_id("100")
        .branch("main")
        .created_at(ran)
        .create(db)
        .await;
    let feature_run = make_run(project)
        .ci_run_id("101")
        .branch("feat/x")
        .created_at(ran)
        .create(db)
        .await;
    make_execution(&test, &main_run)
        .status("failed")
        .created_at(ran)
        .duration(1.5)
        .create(db)
        .await;
    make_execution(&test, &feature_run)
        .status("failed")
        .created_at(ran)
        .duration(2.0)
        .create(db)
        .await;
    make_execution(&test, &feature_run)
        .status("error")
        .created_at(ran)
        .duration(0.5)
        .create(db)
        .await;
    test
}

/// One page of the Tests leaderboard.
async fn listed_tests(db: &TestDb) -> TestPage {
    list_tests(
        db.connection(),
        &Scope::default(),
        &TestListFilters::default(),
        &options(|_| {}),
        &settings(),
    )
    .await
    .expect("the page")
}

/// The listing answers how many CI runs and branches the failures broke and what
/// they cost, in one read for the whole page.
#[tokio::test]
async fn the_impact_window_counts_runs_branches_and_time_lost() {
    let db = TestDb::new().await.expect("a test database");
    let project = impact_project(&db, Some("main")).await;
    let test = impact_test(&db, &project).await;
    let page = listed_tests(&db).await;
    assert_eq!(page.items[0].impacted_runs, 2);
    assert_eq!(page.items[0].impacted_branches, 1);
    assert_eq!(page.items[0].time_lost_seconds, 4.0);

    // A failure 31 days back is outside the window, and one in a Run the infra
    // marking flagged is not the Test's fault: neither moves any of the three.
    let ran = instant("2026-10-15T09:00:00+00:00");
    let old_run = make_run(&project)
        .ci_run_id("102")
        .branch("feat/y")
        .created_at(instant("2026-09-19T09:00:00+00:00"))
        .create(&db)
        .await;
    make_execution(&test, &old_run)
        .status("failed")
        .created_at(instant("2026-09-19T09:00:00+00:00"))
        .duration(9.0)
        .create(&db)
        .await;
    let suspect_run = make_run(&project)
        .ci_run_id("103")
        .branch("feat/y")
        .infra_suspect(true)
        .created_at(ran)
        .create(&db)
        .await;
    make_execution(&test, &suspect_run)
        .status("failed")
        .created_at(ran)
        .duration(9.0)
        .create(&db)
        .await;
    let page = listed_tests(&db).await;
    assert_eq!(page.items[0].impacted_runs, 2);
    assert_eq!(page.items[0].impacted_branches, 1);
    assert_eq!(page.items[0].time_lost_seconds, 4.0);

    // A Run whose reporter sent no CI run ID breaks a branch but names no run, so
    // it moves the branch count and the time, never the run count.
    let unnamed_run = make_run(&project)
        .branch("release/9")
        .created_at(ran)
        .create(&db)
        .await;
    make_execution(&test, &unnamed_run)
        .status("failed")
        .created_at(ran)
        .duration(1.0)
        .create(&db)
        .await;
    let page = listed_tests(&db).await;
    assert_eq!(page.items[0].impacted_runs, 2);
    assert_eq!(page.items[0].impacted_branches, 2);
    assert_eq!(page.items[0].time_lost_seconds, 5.0);

    // The detail and the search answer the same three values: every select that
    // feeds the mapper carries the impact rule.
    let detail = get_test(db.connection(), i64::from(test.id), &settings(), 60)
        .await
        .expect("the detail")
        .expect("the Test has a detail");
    assert_eq!(detail.test.impacted_runs, 2);
    assert_eq!(detail.test.impacted_branches, 2);
    assert_eq!(detail.test.time_lost_seconds, 5.0);
    let found = search_tests(db.connection(), &Scope::default(), "flaky", &settings(), 20)
        .await
        .expect("the search");
    assert_eq!(found[0].impacted_runs, 2);
    assert_eq!(found[0].impacted_branches, 2);
    assert_eq!(found[0].time_lost_seconds, 5.0);
}

/// A Repo whose default branch is unknown counts no branch as impacted.
#[tokio::test]
async fn an_unknown_default_branch_impacts_no_branches() {
    let db = TestDb::new().await.expect("a test database");
    let project = impact_project(&db, None).await;
    impact_test(&db, &project).await;
    let page = listed_tests(&db).await;
    assert_eq!(page.items[0].impacted_runs, 2);
    assert_eq!(page.items[0].impacted_branches, 0);
    assert_eq!(page.items[0].time_lost_seconds, 4.0);
}

/// The Job listing sums the unexplained failures in the window: an execution an
/// attribution rule explains costs nothing, and one without both timestamps adds
/// to the runs but not to the time.
#[tokio::test]
async fn the_impact_window_counts_unexplained_job_failures() {
    let db = TestDb::new().await.expect("a test database");
    let project = impact_project(&db, Some("main")).await;
    let pipeline = make_pipeline().repo("acme/app").create(&db).await;
    let job = make_job(&pipeline)
        .name("unit")
        .flakiness_score(0.6)
        .create(&db)
        .await;
    let ran = instant("2026-10-15T09:00:00+00:00");
    make_job_execution(&job)
        .status("failed")
        .ci_run_id("100")
        .created_at(ran)
        .started_at(ran)
        .completed_at(ran + Duration::seconds(60))
        .create(&db)
        .await;
    make_job_execution(&job)
        .status("failed")
        .ci_run_id("101")
        .created_at(ran)
        .started_at(ran)
        .completed_at(ran + Duration::seconds(30))
        .create(&db)
        .await;
    let listed = list_jobs(
        db.connection(),
        None,
        &flakeradar_db::reads::JobListFilters::default(),
        &options(|_| {}),
        &settings(),
    )
    .await
    .expect("the page");
    assert_eq!(listed.items[0].impacted_runs, 2);
    assert_eq!(listed.items[0].impacted_branches, 0);
    assert_eq!(listed.items[0].time_lost_seconds, 90.0);

    // A failure the runner never finished has no duration to charge.
    make_job_execution(&job)
        .status("failed")
        .ci_run_id("102")
        .ci_job_id("j8")
        .created_at(ran)
        .started_at(ran)
        .create(&db)
        .await;
    // A failure on a branch that is not the default breaks one more branch.
    make_job_execution(&job)
        .status("failed")
        .ci_run_id("104")
        .ci_job_id("j10")
        .branch("feat/j")
        .created_at(ran)
        .started_at(ran)
        .completed_at(ran + Duration::seconds(10))
        .create(&db)
        .await;
    // A failure with the CI job ID of a Run whose Test failed is explained.
    make_job_execution(&job)
        .status("failed")
        .ci_run_id("103")
        .ci_job_id("j9")
        .created_at(ran)
        .started_at(ran)
        .completed_at(ran + Duration::seconds(60))
        .create(&db)
        .await;
    let run = make_run(&project).ci_job_id("j9").create(&db).await;
    let test = make_test_case(&project).name("explainer").create(&db).await;
    make_execution(&test, &run)
        .status("failed")
        .create(&db)
        .await;
    let listed = list_jobs(
        db.connection(),
        None,
        &flakeradar_db::reads::JobListFilters::default(),
        &options(|_| {}),
        &settings(),
    )
    .await
    .expect("the page");
    assert_eq!(listed.items[0].impacted_runs, 4);
    assert_eq!(listed.items[0].impacted_branches, 1);
    assert_eq!(listed.items[0].time_lost_seconds, 100.0);

    // The Job's detail answer carries the same three values.
    let detail = flakeradar_db::reads::get_job(db.connection(), i64::from(job.id), &settings(), 60)
        .await
        .expect("the detail")
        .expect("the Job has a detail");
    assert_eq!(detail.job.impacted_runs, 4);
    assert_eq!(detail.job.impacted_branches, 1);
    assert_eq!(detail.job.time_lost_seconds, 100.0);
}

#[tokio::test]
async fn the_listing_reports_a_first_seen_timestamp_and_whether_the_test_is_new() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().create(&db).await;
    let seen = |at: &str| DateTime::parse_from_rfc3339(at).expect("a fixed instant");
    make_test_case(&project)
        .name("fresh")
        .flakiness_score(0.4)
        .first_seen_at(seen("2026-10-15T00:00:00+00:00"))
        .create(&db)
        .await;
    make_test_case(&project)
        .name("old")
        .flakiness_score(0.4)
        .first_seen_at(seen("2026-10-01T00:00:00+00:00"))
        .create(&db)
        .await;
    let page = list_tests(
        db.connection(),
        &Scope::default(),
        &TestListFilters::default(),
        &options(|options| options.sort = SortKey::LastSeen),
        &settings(),
    )
    .await
    .expect("the page");
    let fresh = page
        .items
        .iter()
        .find(|test| test.name == "fresh")
        .expect("the fresh test");
    let old_out = page
        .items
        .iter()
        .find(|test| test.name == "old")
        .expect("the old test");
    assert!(fresh.new);
    assert!(!old_out.new);
    assert_eq!(
        old_out.first_seen_at,
        seen("2026-10-01T00:00:00+00:00").with_timezone(&chrono::Utc)
    );
    assert_eq!(
        serde_json::to_value(old_out.first_seen_at).expect("the timestamp serializes"),
        "2026-10-01T00:00:00Z"
    );
}

#[tokio::test]
async fn the_new_filter_lists_only_tests_first_seen_in_the_window() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().create(&db).await;
    let seen = |at: &str| DateTime::parse_from_rfc3339(at).expect("a fixed instant");
    // Both rows keep the factory's default score of 0, so they are Stable: listing
    // them is the `new` filter's own stable keep, not a flaky row's.
    make_test_case(&project)
        .name("fresh")
        .first_seen_at(seen("2026-10-15T00:00:00+00:00"))
        .create(&db)
        .await;
    make_test_case(&project)
        .name("old")
        .first_seen_at(seen("2026-10-01T00:00:00+00:00"))
        .create(&db)
        .await;
    let listed = |page: &TestPage| {
        let mut names: Vec<String> = page.items.iter().map(|test| test.name.clone()).collect();
        names.sort_unstable();
        names
    };
    let page = list_tests(
        db.connection(),
        &Scope::default(),
        &TestListFilters {
            new: true,
            ..TestListFilters::default()
        },
        &options(|options| options.sort = SortKey::LastSeen),
        &settings(),
    )
    .await
    .expect("the page");
    assert_eq!(listed(&page), ["fresh"]);
    // Without the filter, the stable-hiding default still hides both rows, as it
    // does today.
    let hidden = list_tests(
        db.connection(),
        &Scope::default(),
        &TestListFilters::default(),
        &options(|options| options.sort = SortKey::LastSeen),
        &settings(),
    )
    .await
    .expect("the page");
    assert_eq!(hidden.total, 0);
    assert_eq!(hidden.items, Vec::new());
    // And asking for stable rows, it still answers both of them.
    let page = list_tests(
        db.connection(),
        &Scope::default(),
        &TestListFilters::default(),
        &options(|options| {
            options.sort = SortKey::LastSeen;
            options.include_stable = true;
        }),
        &settings(),
    )
    .await
    .expect("the page");
    assert_eq!(listed(&page), ["fresh", "old"]);
}

#[tokio::test]
async fn a_ci_run_listing_caps_its_failures_and_flags_the_rest() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .create(&db)
        .await;
    let run = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    for name in ["t1", "t2", "t3"] {
        let test = make_test_case(&project).name(name).create(&db).await;
        make_execution(&test, &run)
            .status("failed")
            .create(&db)
            .await;
    }
    let listing = ci_run_failures(
        db.connection(),
        "acme/app",
        &run_id("100"),
        None,
        &settings(),
        2,
    )
    .await
    .expect("the listing");
    assert_eq!(listing.total_failures, 3);
    assert!(listing.truncated);
    assert_eq!(listing.failures.len(), 2);
    let names: Vec<&str> = listing
        .failures
        .iter()
        .map(|failure| failure.name.as_str())
        .collect();
    assert_eq!(names, ["t1", "t2"]);
}

/// Runs the processor until the queue is empty.
async fn drain(db: &TestDb) {
    while process_next(db.connection(), &processing())
        .await
        .expect("the processor runs")
        .is_some()
    {}
}

#[tokio::test]
async fn a_report_status_this_server_does_not_write_counts_as_none_of_the_three() {
    // The counts answer one number per status this version writes, so a row that
    // carries any other — one a newer server stored — leaves all three alone rather
    // than arriving as a fourth number.
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    make_report(&project, project.repo_id)
        .ci_run_id("700")
        .commit_sha("pending")
        .create(&db)
        .await;
    make_report(&project, project.repo_id)
        .ci_run_id("700")
        .commit_sha("retired")
        .status("retired")
        .create(&db)
        .await;
    let listing = ci_run_failures(
        db.connection(),
        "acme/app",
        &run_id("700"),
        None,
        &settings(),
        10,
    )
    .await
    .expect("the listing");
    assert_eq!(
        (
            listing.reports.pending,
            listing.reports.processed,
            listing.reports.failed
        ),
        (1, 0, 0),
        "the retired Report counts nowhere"
    );
}

#[tokio::test]
async fn a_nextest_flaky_fail_is_a_proven_flake_in_its_ci_run() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    make_report(&project, project.repo_id)
        .body(NEXTEST_FLAKY_FAIL.as_bytes())
        .commit_sha("777022bf")
        .ci_run_id("38994487107")
        .ci_job_id("J1")
        .create(&db)
        .await;
    let payload: PipelineReportIn = serde_json::from_value(serde_json::json!({
        "repo": "acme/app",
        "provider": "github",
        "pipeline": ".github/workflows/ci.yml",
        "commit_sha": "777022bf",
        "branch": "main",
        "default_branch": "main",
        "ci_run_id": "38994487107",
        "ci_run_attempt": 1,
        "jobs": [{
            "ci_job_id": "J1",
            "name": "test",
            "status": "failed",
            "url": "",
            "runner_name": "",
            "runner_labels": []
        }]
    }))
    .expect("the pipeline payload");
    make_report(&project, project.repo_id)
        .kind("pipeline")
        .body(&pipeline_body(&payload))
        .commit_sha(&payload.commit_sha)
        .branch(&payload.branch)
        .ci_run_id(payload.ci_run_id.as_str())
        .create(&db)
        .await;
    drain(&db).await;

    let listing = ci_run_failures(
        db.connection(),
        "acme/app",
        &run_id("38994487107"),
        None,
        &settings(),
        500,
    )
    .await
    .expect("the listing");
    assert_eq!(
        listing.reports,
        flakeradar_core::dto::ReportCountsOut {
            pending: 0,
            processed: 2,
            failed: 0
        }
    );
    assert_eq!(listing.failures.len(), 1);
    let failure = &listing.failures[0];
    assert!(
        failure
            .name
            .ends_with("invalidate_priority_readiness_drops_from_each_driver")
    );
    assert_eq!(failure.status, ExecutionStatus::Passed);
    assert_eq!(failure.failed_attempts, 1);
    assert!(failure.message.starts_with("thread 'embedding::reindexer"));
    assert_eq!(failure.confirmed_flake_count, 1);
    assert_eq!(failure.ci_job_id.as_deref(), Some("J1"));
    assert_eq!(listing.failed_jobs.len(), 1);
    assert_eq!(
        listing.failed_jobs[0].outcome,
        flakeradar_core::model::FailedJobOutcome::Explained
    );
    assert_eq!(listing.failed_jobs[0].ci_job_id, "J1");
}

/// The SQL the two summaries issue never names an Execution table.
#[tokio::test]
async fn the_summaries_read_score_history_and_never_the_execution_tables() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    let test = make_test_case(&project).create(&db).await;
    let pipeline = make_pipeline().repo("acme/app").create(&db).await;
    let job = make_job(&pipeline).create(&db).await;
    let today = at().date_naive();
    flakeradar_test_support::make_test_score(&test, today, 0.6)
        .executions(3)
        .failures(1)
        .create(&db)
        .await;
    make_job_score(&job, today, 0.6)
        .executions(2)
        .failures(1)
        .create(&db)
        .await;
    let counted = summary(db.connection(), &Scope::default(), &settings())
        .await
        .expect("the summary");
    assert_eq!(counted.executions_30d, 3);
    let jobs = job_summary(db.connection(), None, &settings())
        .await
        .expect("the job summary");
    assert_eq!(jobs.job_executions_30d, 2);

    // Renaming both Execution tables proves neither summary read them: a
    // statement that named one would fail rather than answer.
    db.connection()
        .execute_unprepared(
            "ALTER TABLE test_executions RENAME TO test_executions_moved; \
             ALTER TABLE job_executions RENAME TO job_executions_moved",
        )
        .await
        .expect("the tables are renamed");
    let counted = summary(db.connection(), &Scope::default(), &settings())
        .await
        .expect("the summary still answers");
    assert_eq!(counted.total_tests, 1);
    let jobs = job_summary(db.connection(), None, &settings())
        .await
        .expect("the job summary still answers");
    assert_eq!(jobs.total_jobs, 1);
    // A read that does name one now fails, so the rename really bit.
    let _ = get_test(db.connection(), i64::from(test.id), &settings(), 60)
        .await
        .expect_err("a renamed table makes the detail read fail");
}

#[tokio::test]
async fn a_summary_ignores_a_category_the_enum_does_not_know() {
    // `failure_category` is free text in the baseline schema, so a row can hold a
    // word the five Failure categories do not name; it counts under none of them.
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    make_test_case(&project)
        .name("unknown_category")
        .flakiness_score(0.6)
        .failure_category("bogus")
        .create(&db)
        .await;
    let counted = summary(db.connection(), &Scope::default(), &settings())
        .await
        .expect("the summary");
    assert_eq!(counted.total_tests, 1);
    assert_eq!(counted.category_counts.len(), 5);
    assert_eq!(counted.category_counts.values().copied().sum::<i64>(), 0);
}

/// One Test's detail page needs its Project, and an unknown id has none.
#[tokio::test]
async fn an_unknown_test_has_no_detail() {
    let db = TestDb::new().await.expect("a test database");
    assert!(
        get_test(db.connection(), 9999, &settings(), 60)
            .await
            .expect("the lookup")
            .is_none()
    );
    assert!(
        flakeradar_db::reads::get_job(db.connection(), 9999, &settings(), 60)
            .await
            .expect("the lookup")
            .is_none()
    );
}

#[tokio::test]
async fn an_execution_with_no_ci_job_id_names_no_job() {
    // A Runner that sends an empty `ci_job_id` stores the empty string on its Run,
    // which is no Job's id: the detail lists the Execution and links no Job.
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    let run = make_run(&project).ci_job_id("").create(&db).await;
    let test = make_test_case(&project).name("no_job").create(&db).await;
    make_execution(&test, &run).create(&db).await;
    let detail = get_test(db.connection(), i64::from(test.id), &settings(), 60)
        .await
        .expect("the detail")
        .expect("the Test has a detail");
    assert_eq!(detail.executions.len(), 1, "the Execution is listed");
    assert!(
        detail.jobs.is_empty(),
        "the empty id names no Job: {:?}",
        detail.jobs
    );
}

/// A Repo with no Job still lists no Jobs rather than failing.
#[tokio::test]
async fn job_listings_of_an_empty_database_answer_empty_pages() {
    let db = TestDb::new().await.expect("a test database");
    let page = list_jobs(
        db.connection(),
        None,
        &flakeradar_db::reads::JobListFilters::default(),
        &options(|_| {}),
        &settings(),
    )
    .await
    .expect("the page");
    assert_eq!(page.total, 0);
    assert_eq!(page.items, Vec::new());
    let jobs = job_summary(db.connection(), Some("acme/none"), &settings())
        .await
        .expect("the summary");
    assert_eq!(jobs.total_jobs, 0);
    assert_eq!(jobs.pipelines, Vec::<String>::new());
}

/// A Job execution without a Job still explains nothing, and the helpers are
/// safe on a Repo with no attribution data.
#[tokio::test]
async fn attribution_queries_tolerate_an_empty_repo() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    let repo_id = project.repo_id;
    assert!(
        flakeradar_db::explained_ci_job_ids(db.connection(), repo_id, &[])
            .await
            .expect("the attribution")
            .is_empty()
    );
}
/// Silences the unused warning for the pool type the helpers take.
fn _connection_is_a_pool(connection: &DatabaseConnection) -> bool {
    let _ = connection;
    true
}

/// The unused fixture helper stays exercised through [`make_job_execution`].
#[tokio::test]
async fn a_job_execution_fixture_needs_a_job() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline().create(&db).await;
    let job = make_job(&pipeline).create(&db).await;
    make_job_execution(&job).create(&db).await;
}

#[tokio::test]
async fn a_skipped_job_execution_has_the_skipped_outcome() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline().create(&db).await;
    let job = make_job(&pipeline).create(&db).await;
    make_job_execution(&job)
        .status("skipped")
        .ci_job_id("J-S")
        .create(&db)
        .await;
    let history =
        flakeradar_db::reads::get_job(db.connection(), i64::from(job.id), &settings(), 60)
            .await
            .expect("the history")
            .expect("a Job");
    assert_eq!(
        history.executions[0].outcome,
        flakeradar_core::model::JobOutcome::Skipped
    );
}

#[tokio::test]
async fn runner_statistics_break_ties_by_labels_then_name() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline().create(&db).await;
    let job = make_job(&pipeline).create(&db).await;
    // Two label sets with the same rate and count, so the label set orders them;
    // the gamma set also has a repeated runner name, so the name orders those two.
    for index in 0..2 {
        make_job_execution(&job)
            .status("failed")
            .ci_job_id(&format!("B{index}"))
            .runner_labels(&["beta"])
            .create(&db)
            .await;
        make_job_execution(&job)
            .status("failed")
            .ci_job_id(&format!("G{index}"))
            .runner_name("runner-z")
            .runner_labels(&["gamma"])
            .create(&db)
            .await;
    }
    let stats = flakeradar_db::reads::runner_stats(db.connection(), job.id, 50)
        .await
        .expect("the statistics");
    assert_eq!(stats.len(), 3);
    // Worst rate first, then the execution count, then the label set, then the name.
    assert_eq!(stats[0].labels, ["gamma"]);
    assert_eq!(stats[0].runner_name.as_deref(), Some("runner-z"));
    assert_eq!(stats[1].labels, ["gamma"]);
    assert_eq!(stats[1].runner_name, None);
    assert_eq!(stats[2].labels, ["beta"]);
    assert_eq!(stats[2].runner_name, None);
}

#[tokio::test]
async fn a_required_row_that_is_missing_names_the_query() {
    let db = TestDb::new().await.expect("a test database");
    let error = flakeradar_db::sql::required_one(
        db.connection(),
        "SELECT 1 AS one WHERE false".to_owned(),
        Vec::new(),
        "aggregate",
    )
    .await
    .expect_err("a statement with no row is an error");
    assert!(
        error
            .to_string()
            .contains("the aggregate query returned no row"),
        "{error}"
    );
}

#[tokio::test]
async fn a_column_the_row_does_not_hold_is_a_database_error() {
    let db = TestDb::new().await.expect("a test database");
    let rows = flakeradar_db::sql::rows(db.connection(), "SELECT 1 AS one".to_owned(), Vec::new())
        .await
        .expect("one row");
    let row = rows.first().expect("one row");
    let read: Result<i32, _> = flakeradar_db::sql::get(row, "missing");
    assert!(read.is_err(), "a column the statement did not select fails");
}

/// A Test's tier reads its stored flaky state, not its score alone.
#[tokio::test]
async fn a_tests_tier_reads_the_stored_flaky_state() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    // Between the two thresholds, but flaky because the state is set.
    let flaky = make_test_case(&project)
        .name("held_flaky")
        .flakiness_score(0.25)
        .flaky_since(Some(FIXTURE_INSTANT.fixed_offset()))
        .create(&db)
        .await;
    let detail = get_test(db.connection(), i64::from(flaky.id), &settings(), 60)
        .await
        .expect("the lookup")
        .expect("the Test");
    assert_eq!(detail.test.tier, Tier::Flaky);
    // Above the Flake threshold, but not flaky while nothing is stored.
    let suspect = make_test_case(&project)
        .name("not_flaky")
        .flakiness_score(0.35)
        .flaky_since(None)
        .create(&db)
        .await;
    let detail = get_test(db.connection(), i64::from(suspect.id), &settings(), 60)
        .await
        .expect("the lookup")
        .expect("the Test");
    assert_eq!(detail.test.tier, Tier::Suspect);
}

/// Every SQL tier comparison reads the stored flaky state, not the score.
#[tokio::test]
async fn every_sql_tier_comparison_reads_the_stored_flaky_state() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    // `a` is flaky below the Flake threshold, `b` is above the threshold with
    // nothing stored, and `c` is stable.
    make_test_case(&project)
        .name("a")
        .flakiness_score(0.25)
        .flaky_since(Some(FIXTURE_INSTANT.fixed_offset()))
        .create(&db)
        .await;
    make_test_case(&project)
        .name("b")
        .flakiness_score(0.35)
        .flaky_since(None)
        .create(&db)
        .await;
    make_test_case(&project).name("c").create(&db).await;

    let counted = summary(db.connection(), &Scope::default(), &settings())
        .await
        .expect("the summary");
    assert_eq!(
        (
            counted.flaky_tests,
            counted.suspect_tests,
            counted.new_flaky_7d
        ),
        (1, 1, 1),
        "`a` is the flaky row, `b` the suspect, and `a` is new because it has no Score history"
    );

    let page = list_tests(
        db.connection(),
        &Scope::default(),
        &TestListFilters::default(),
        &options(|options| options.flaky_only = true),
        &settings(),
    )
    .await
    .expect("the page");
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].name, "a");

    // The same three shapes as Jobs of one Pipeline.
    let pipeline = make_pipeline().repo("acme/app").create(&db).await;
    make_job(&pipeline)
        .name("a")
        .flakiness_score(0.25)
        .flaky_since(Some(FIXTURE_INSTANT.fixed_offset()))
        .create(&db)
        .await;
    make_job(&pipeline)
        .name("b")
        .flakiness_score(0.35)
        .flaky_since(None)
        .create(&db)
        .await;
    make_job(&pipeline).name("c").create(&db).await;
    let jobs = job_summary(db.connection(), None, &settings())
        .await
        .expect("the job summary");
    assert_eq!(
        (jobs.flaky_jobs, jobs.suspect_jobs),
        (1, 1),
        "a Job counts by its stored flaky state, exactly as a Test does"
    );
}
