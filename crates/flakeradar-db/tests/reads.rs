//! The read services against a real database.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a fixture or a read does not behave"
)]

use flakeradar_db::{ListOptions, ReadSettings, Scope, get_job, get_test, list_tests};
use flakeradar_test_support::{
    TestDb, make_execution, make_job, make_job_execution, make_pipeline, make_project, make_run,
    make_test_case,
};

#[tokio::test]
async fn scratch_test_detail() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("frontend")
        .root("frontend")
        .build(db.connection())
        .await
        .expect("the project");
    let test = make_test_case(&project)
        .name("renders")
        .classname("src/app.test.ts")
        .suite("vitest")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the test");
    let run = make_run(&project)
        .commit_sha("aaa111")
        .ci_run_id("1")
        .build(db.connection())
        .await
        .expect("the run");
    make_execution(&test, &run)
        .status("failed")
        .build(db.connection())
        .await
        .expect("the execution");
    let settings = ReadSettings::default();
    let history = get_test(db.connection(), i64::from(test.id), &settings, 60)
        .await
        .expect("the history");
    assert!(history.is_some());

    let page = list_tests(
        db.connection(),
        &Scope::default(),
        &flakeradar_db::TestListFilters::default(),
        &ListOptions { page_size: 50, page: 1, ..ListOptions::default() },
        &settings,
    )
    .await
    .expect("the page");
    assert_eq!(page.total, 1);
}

#[tokio::test]
async fn scratch_job_detail() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline().repo("acme/app").build(db.connection()).await.expect("the pipeline");
    let job = make_job(&pipeline).name("test").build(db.connection()).await.expect("the job");
    make_job_execution(&job)
        .status("failed")
        .ci_job_id("J-F")
        .build(db.connection())
        .await
        .expect("the execution");
    let settings = ReadSettings::default();
    let history = get_job(db.connection(), i64::from(job.id), &settings, 60)
        .await
        .expect("the history");
    assert!(history.is_some());
}
