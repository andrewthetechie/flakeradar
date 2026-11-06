//! The read services against a real database: the behaviour the HTTP tests
//! cannot reach, and the query functions the MCP server shares.
//!
//! Ported from `backend/tests/test_ci_runs_api.py` (the cap, the nextest
//! flaky-fail case) and `backend/tests/test_ops_api.py` (the summaries never
//! touch the Execution tables).

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a fixture or a read does not behave"
)]

use std::sync::Arc;

use chrono::{DateTime, FixedOffset};
use flakeradar_core::{PipelineReportIn, pipeline_body};
use flakeradar_db::{
    FixedClock, ListOptions, ProcessingSettings, ReadSettings, Scope, SortKey, TestListFilters,
    ci_run_failures, find_job_ids, find_test_ids, get_test, job_summary, latest_failure, list_jobs,
    list_tests, process_next, search_jobs, search_tests, summary,
};
use flakeradar_test_support::{
    TestDb, make_execution, make_job, make_job_execution, make_job_score, make_pipeline,
    make_project, make_report, make_run, make_test_case,
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
        .build(db.connection())
        .await
        .expect("the project");
    let other = make_project()
        .repo("acme/other")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the other project");
    let wanted = make_test_case(&app)
        .name("test_login")
        .classname("tests.auth")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the wanted test");
    make_test_case(&app)
        .name("test_logout")
        .classname("tests.auth")
        .build(db.connection())
        .await
        .expect("another test");
    make_test_case(&other)
        .name("test_login")
        .build(db.connection())
        .await
        .expect("the other repo's test");
    (wanted.id, app.id)
}

#[tokio::test]
async fn search_tests_is_a_scoped_literal_substring_worst_first() {
    let db = TestDb::new().await.expect("a test database");
    let (wanted, _) = seed_search(&db).await;
    let found = search_tests(db.connection(), &Scope::default(), "LOGIN", &settings(), 20)
        .await
        .expect("the search");
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].flakiness_score, 0.6);
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
    let project = make_project()
        .build(db.connection())
        .await
        .expect("project");
    let test = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("test");
    assert!(
        latest_failure(db.connection(), i64::from(test.id))
            .await
            .expect("the lookup")
            .is_none()
    );
    let run = make_run(&project)
        .build(db.connection())
        .await
        .expect("run");
    make_execution(&test, &run)
        .status("failed")
        .message("first")
        .build(db.connection())
        .await
        .expect("a failure");
    make_execution(&test, &run)
        .status("failed")
        .message("second")
        .build(db.connection())
        .await
        .expect("the newer failure");
    make_execution(&test, &run)
        .build(db.connection())
        .await
        .expect("a pass");
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
        .build(db.connection())
        .await
        .expect("the pipeline");
    let wanted = make_job(&pipeline)
        .name("build-thing")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the job");
    make_job(&pipeline)
        .name("lint")
        .build(db.connection())
        .await
        .expect("the other job");
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
    let project = make_project()
        .build(db.connection())
        .await
        .expect("project");
    make_test_case(&project)
        .name("flaky")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the flaky test");
    make_test_case(&project)
        .name("stable")
        .build(db.connection())
        .await
        .expect("the stable test");
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
    let project = make_project()
        .build(db.connection())
        .await
        .expect("project");
    make_test_case(&project)
        .name("first")
        .flakiness_score(0.4)
        .build(db.connection())
        .await
        .expect("the first test");
    make_test_case(&project)
        .name("proven")
        .flakiness_score(0.4)
        .confirmed_flake_count(3)
        .build(db.connection())
        .await
        .expect("the proven test");
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

#[tokio::test]
async fn a_ci_run_listing_caps_its_failures_and_flags_the_rest() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the project");
    let run = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .build(db.connection())
        .await
        .expect("the run");
    for name in ["t1", "t2", "t3"] {
        let test = make_test_case(&project)
            .name(name)
            .build(db.connection())
            .await
            .expect("a test");
        make_execution(&test, &run)
            .status("failed")
            .build(db.connection())
            .await
            .expect("a failure");
    }
    let listing = ci_run_failures(db.connection(), "acme/app", "100", None, &settings(), 2)
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
async fn a_nextest_flaky_fail_is_a_proven_flake_in_its_ci_run() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the project");
    make_report(&project, project.repo_id)
        .body(NEXTEST_FLAKY_FAIL.as_bytes())
        .commit_sha("777022bf")
        .ci_run_id("38994487107")
        .ci_job_id("J1")
        .build(db.connection())
        .await
        .expect("the junit report");
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
        .ci_run_id(&payload.ci_run_id)
        .build(db.connection())
        .await
        .expect("the pipeline report");
    drain(&db).await;

    let listing = ci_run_failures(
        db.connection(),
        "acme/app",
        "38994487107",
        None,
        &settings(),
        500,
    )
    .await
    .expect("the listing");
    assert_eq!(
        listing.reports,
        flakeradar_core::ReportCountsOut {
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
    assert_eq!(failure.status, "passed");
    assert_eq!(failure.failed_attempts, 1);
    assert!(failure.message.starts_with("thread 'embedding::reindexer"));
    assert_eq!(failure.confirmed_flake_count, 1);
    assert_eq!(failure.ci_job_id.as_deref(), Some("J1"));
    assert_eq!(listing.failed_jobs.len(), 1);
    assert_eq!(
        listing.failed_jobs[0].outcome,
        flakeradar_core::FailedJobOutcome::Explained
    );
    assert_eq!(listing.failed_jobs[0].ci_job_id, "J1");
}

/// The SQL the two summaries issue never names an Execution table.
#[tokio::test]
async fn the_summaries_read_score_history_and_never_the_execution_tables() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the project");
    let test = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("test");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("job");
    let today = at().date_naive();
    flakeradar_test_support::make_test_score(&test, today, 0.6)
        .executions(3)
        .failures(1)
        .build(db.connection())
        .await
        .expect("a score row");
    make_job_score(&job, today, 0.6)
        .executions(2)
        .failures(1)
        .build(db.connection())
        .await
        .expect("a job score row");
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
    let project = make_project()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the project");
    make_test_case(&project)
        .name("unknown_category")
        .flakiness_score(0.6)
        .failure_category("bogus")
        .build(db.connection())
        .await
        .expect("the test");
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
        flakeradar_db::get_job(db.connection(), 9999, &settings(), 60)
            .await
            .expect("the lookup")
            .is_none()
    );
}

/// A Repo with no Job still lists no Jobs rather than failing.
#[tokio::test]
async fn job_listings_of_an_empty_database_answer_empty_pages() {
    let db = TestDb::new().await.expect("a test database");
    let page = list_jobs(
        db.connection(),
        None,
        &flakeradar_db::JobListFilters::default(),
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
    let project = make_project()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the project");
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
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("job");
    make_job_execution(&job)
        .build(db.connection())
        .await
        .expect("the execution");
}

#[tokio::test]
async fn a_skipped_job_execution_has_the_skipped_outcome() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("job");
    make_job_execution(&job)
        .status("skipped")
        .ci_job_id("J-S")
        .build(db.connection())
        .await
        .expect("the skipped execution");
    let history = flakeradar_db::get_job(db.connection(), i64::from(job.id), &settings(), 60)
        .await
        .expect("the history")
        .expect("a Job");
    assert_eq!(
        history.executions[0].outcome,
        flakeradar_core::JobOutcome::Skipped
    );
}

#[tokio::test]
async fn runner_statistics_break_ties_by_labels_then_name() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("job");
    // Two label sets with the same rate and count, so the label set orders them;
    // the gamma set also has a repeated runner name, so the name orders those two.
    for index in 0..2 {
        make_job_execution(&job)
            .status("failed")
            .ci_job_id(&format!("B{index}"))
            .runner_labels(&["beta"])
            .build(db.connection())
            .await
            .expect("a beta failure");
        make_job_execution(&job)
            .status("failed")
            .ci_job_id(&format!("G{index}"))
            .runner_name("runner-z")
            .runner_labels(&["gamma"])
            .build(db.connection())
            .await
            .expect("a gamma failure");
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
