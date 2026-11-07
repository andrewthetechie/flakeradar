//! The Jobs read API: leaderboard, detail, runner statistics and summary.
//!
//! The Job leaderboard, a Job's detail and its history.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a fixture or an answer does not behave"
)]

use chrono::{Duration, Utc};
use flakeradar_test_support::{
    TestDb, app, get, make_execution, make_job, make_job_execution, make_job_score, make_pipeline,
    make_project, make_run, make_test_case,
};

use serde_json::{Value, json};

/// The UTC day `days` before today.
fn day(days: i64) -> chrono::NaiveDate {
    (Utc::now() - Duration::days(days)).date_naive()
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

#[tokio::test]
async fn list_jobs_filters_stable_and_sorts_worst_first() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the pipeline");
    for (name, score) in [("a", 0.6), ("b", 0.1), ("c", 0.0)] {
        make_job(&pipeline)
            .name(name)
            .flakiness_score(score)
            .build(db.connection())
            .await
            .expect("a job");
    }
    let app = app(&db, &[]);
    let body = get(&app, "/api/jobs?repo=acme/app").await.json();
    assert_eq!(item_names(&body), ["a", "b"]);
    assert_eq!(body["total"], 2);
    let all = get(&app, "/api/jobs?repo=acme/app&include_stable=true")
        .await
        .json();
    assert_eq!(item_names(&all), ["a", "b", "c"]);
    assert_eq!(all["total"], 3);
}

#[tokio::test]
async fn list_jobs_without_a_repo_spans_every_repo() {
    let db = TestDb::new().await.expect("a test database");
    let a = make_pipeline()
        .repo("acme/a")
        .build(db.connection())
        .await
        .expect("the pipeline");
    let b = make_pipeline()
        .repo("acme/b")
        .build(db.connection())
        .await
        .expect("the other pipeline");
    make_job(&a)
        .name("ja")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("a job");
    make_job(&b)
        .name("jb")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the other job");
    let body = get(&app(&db, &[]), "/api/jobs").await.json();
    assert_eq!(body["total"], 2);
    let mut names = item_names(&body);
    names.sort();
    assert_eq!(names, ["ja", "jb"]);
}

#[tokio::test]
async fn job_history_outcomes_and_explained_by() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the project");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the pipeline");
    let job = make_job(&pipeline)
        .name("test")
        .build(db.connection())
        .await
        .expect("the job");
    make_job_execution(&job)
        .status("failed")
        .ci_job_id("J-F")
        .build(db.connection())
        .await
        .expect("the failing execution");
    make_job_execution(&job)
        .status("passed")
        .ci_job_id("J-P")
        .build(db.connection())
        .await
        .expect("the passing execution");
    let test = make_test_case(&project)
        .name("t1")
        .build(db.connection())
        .await
        .expect("the test");
    let run = make_run(&project)
        .commit_sha("s")
        .ci_job_id("J-F")
        .build(db.connection())
        .await
        .expect("the run");
    make_execution(&test, &run)
        .status("failed")
        .build(db.connection())
        .await
        .expect("the failing test execution");
    let lint = make_job(&pipeline)
        .name("lint")
        .build(db.connection())
        .await
        .expect("the lint job");
    make_job_execution(&lint)
        .status("failed")
        .ci_job_id("UNLINK")
        .build(db.connection())
        .await
        .expect("the unlinked failure");

    let app = app(&db, &[]);
    let body = get(&app, &format!("/api/jobs/{}/history", job.id))
        .await
        .json();
    let by_ci: std::collections::BTreeMap<String, Value> = body["executions"]
        .as_array()
        .expect("executions")
        .iter()
        .map(|execution| {
            (
                execution["ci_job_id"]
                    .as_str()
                    .expect("a ci job id")
                    .to_owned(),
                execution.clone(),
            )
        })
        .collect();
    assert_eq!(by_ci["J-P"]["outcome"], "passed");
    assert_eq!(by_ci["J-F"]["outcome"], "explained");
    assert_eq!(by_ci["J-F"]["status"], "failed");
    assert_eq!(by_ci["J-F"]["explained_by"][0]["name"], "t1");
    assert_eq!(by_ci["J-F"]["explained_by"][0]["status"], "failed");
    assert_eq!(by_ci["J-F"]["explained_by_jobs"], json!([]));
    assert_eq!(body["explained_failures"], 1);
    assert_eq!(body["unexplained_failures"], 0);

    let other = get(&app, &format!("/api/jobs/{}/history", lint.id))
        .await
        .json();
    assert_eq!(other["executions"][0]["outcome"], "failed");
    assert_eq!(other["unexplained_failures"], 1);
    assert_eq!(other["explained_failures"], 0);
}

#[tokio::test]
async fn a_stored_job_status_outside_the_three_is_served_and_counts_skipped() {
    // The reason `JobOut::last_status` and `JobExecutionOut::status` stay `String`
    // while `JobOutcome` is typed. Both columns are `varchar(16)` with no `CHECK`,
    // so a hand-written row can hold a word this version never writes; the read
    // serves that word and folds its outcome into `skipped`. Typing the fields
    // would either rewrite the stored word or refuse the row, and either is a
    // behaviour change, so this test pins the fold instead.
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the pipeline");
    let job = make_job(&pipeline)
        .name("build")
        .build(db.connection())
        .await
        .expect("the job");
    make_job_execution(&job)
        .status("passed")
        .ci_job_id("J-C")
        .build(db.connection())
        .await
        .expect("the execution");
    sea_orm::ConnectionTrait::execute_unprepared(
        db.connection(),
        "UPDATE job_executions SET status = 'cancelled' WHERE ci_job_id = 'J-C'; \
         UPDATE jobs SET last_status = 'cancelled'",
    )
    .await
    .expect("the status outside the three is written");
    let app = app(&db, &[]);
    let body = get(&app, &format!("/api/jobs/{}/history", job.id))
        .await
        .json();
    assert_eq!(body["executions"][0]["status"], "cancelled");
    assert_eq!(
        body["job"]["last_status"], "cancelled",
        "the Job row serves the stored word too"
    );
    assert_eq!(body["executions"][0]["outcome"], "skipped");
    assert_eq!(
        body["unexplained_failures"], 0,
        "an unknown word is no one's failure"
    );
}

#[tokio::test]
async fn job_history_lists_at_most_twenty_explaining_tests() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the project");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("the job");
    make_job_execution(&job)
        .status("failed")
        .ci_job_id("J-F")
        .build(db.connection())
        .await
        .expect("the failing execution");
    let run = make_run(&project)
        .commit_sha("s")
        .ci_job_id("J-F")
        .build(db.connection())
        .await
        .expect("the run");
    for index in 0..21 {
        let test = make_test_case(&project)
            .name(&format!("t{index:02}"))
            .build(db.connection())
            .await
            .expect("a test");
        make_execution(&test, &run)
            .status("failed")
            .build(db.connection())
            .await
            .expect("a failing execution");
    }
    let body = get(&app(&db, &[]), &format!("/api/jobs/{}/history", job.id))
        .await
        .json();
    let failed = body["executions"]
        .as_array()
        .expect("executions")
        .iter()
        .find(|execution| execution["ci_job_id"] == "J-F")
        .expect("the failing execution");
    assert_eq!(failed["outcome"], "explained");
    let names: Vec<&str> = failed["explained_by"]
        .as_array()
        .expect("explained by")
        .iter()
        .map(|test| test["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names.len(), 20);
    // Newest Test execution first, so the first created Test is the one left out.
    assert!(names.contains(&"t20"));
    assert!(!names.contains(&"t00"));
    assert_eq!(body["explained_failures"], 1);
    assert_eq!(body["unexplained_failures"], 0);
}

#[tokio::test]
async fn runner_statistics_group_by_label_set_and_repeated_name() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("the job");
    for index in 0..2 {
        make_job_execution(&job)
            .status("failed")
            .ci_job_id(&format!("GPU{index}"))
            .runner_name(&format!("gpu-node-{index}"))
            .runner_labels(&["self-hosted", "gpu"])
            .build(db.connection())
            .await
            .expect("a gpu execution");
    }
    make_job_execution(&job)
        .status("passed")
        .ci_job_id("GPU-OK")
        .runner_name("gpu-node-2")
        .runner_labels(&["self-hosted", "gpu"])
        .build(db.connection())
        .await
        .expect("the passing gpu execution");
    for index in 0..3 {
        make_job_execution(&job)
            .status("passed")
            .ci_job_id(&format!("UBU{index}"))
            .runner_name(&format!("ubuntu-{index}"))
            .runner_labels(&["ubuntu-latest"])
            .build(db.connection())
            .await
            .expect("a ubuntu execution");
    }
    let body = get(&app(&db, &[]), &format!("/api/jobs/{}/history", job.id))
        .await
        .json();
    assert_eq!(
        body["runners"][0],
        json!({
            "labels": ["gpu", "self-hosted"],
            "runner_name": null,
            "executions": 3,
            "unexplained_failures": 2
        })
    );
    assert_eq!(
        body["runners"][1],
        json!({
            "labels": ["ubuntu-latest"],
            "runner_name": null,
            "executions": 3,
            "unexplained_failures": 0
        })
    );
    assert!(
        body["runners"]
            .as_array()
            .expect("runners")
            .iter()
            .all(|runner| runner["runner_name"] == Value::Null)
    );

    for index in 0..2 {
        make_job_execution(&job)
            .status("failed")
            .ci_job_id(&format!("BOX{index}"))
            .runner_name("gpu-box-1")
            .runner_labels(&["self-hosted", "gpu"])
            .build(db.connection())
            .await
            .expect("a repeated runner execution");
    }
    let body = get(&app(&db, &[]), &format!("/api/jobs/{}/history", job.id))
        .await
        .json();
    let named: Vec<Value> = body["runners"]
        .as_array()
        .expect("runners")
        .iter()
        .filter(|runner| runner["runner_name"] == "gpu-box-1")
        .cloned()
        .collect();
    assert_eq!(
        named,
        vec![json!({
            "labels": ["gpu", "self-hosted"],
            "runner_name": "gpu-box-1",
            "executions": 2,
            "unexplained_failures": 2
        })]
    );
}

#[tokio::test]
async fn runner_statistics_dedupe_a_repeated_label() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("the job");
    make_job_execution(&job)
        .status("failed")
        .ci_job_id("A")
        .runner_name("node-a")
        .runner_labels(&["linux", "self-hosted"])
        .build(db.connection())
        .await
        .expect("the first execution");
    make_job_execution(&job)
        .status("failed")
        .ci_job_id("B")
        .runner_name("node-b")
        .runner_labels(&["linux", "self-hosted", "linux"])
        .build(db.connection())
        .await
        .expect("the second execution");
    let body = get(&app(&db, &[]), &format!("/api/jobs/{}/history", job.id))
        .await
        .json();
    let label_sets: Vec<Value> = body["runners"]
        .as_array()
        .expect("runners")
        .iter()
        .filter(|runner| runner["runner_name"] == Value::Null)
        .cloned()
        .collect();
    assert_eq!(
        label_sets,
        vec![json!({
            "labels": ["linux", "self-hosted"],
            "runner_name": null,
            "executions": 2,
            "unexplained_failures": 2
        })]
    );
}

#[tokio::test]
async fn a_summary_job_lists_its_explaining_sibling_jobs() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the pipeline");
    let summary = make_job(&pipeline)
        .name("alls-green")
        .summary(true)
        .build(db.connection())
        .await
        .expect("the summary job");
    let sibling = make_job(&pipeline)
        .name("test")
        .build(db.connection())
        .await
        .expect("the sibling job");
    make_job_execution(&summary)
        .status("failed")
        .ci_job_id("J-S")
        .ci_run_id("run-1")
        .build(db.connection())
        .await
        .expect("the summary failure");
    make_job_execution(&sibling)
        .status("failed")
        .ci_job_id("J-T")
        .ci_run_id("run-1")
        .url("https://github.com/acme/app/actions/runs/1/jobs/2")
        .build(db.connection())
        .await
        .expect("the sibling failure");
    make_job_execution(&summary)
        .status("failed")
        .ci_job_id("J-S2")
        .ci_run_id("run-2")
        .build(db.connection())
        .await
        .expect("the second summary failure");

    let body = get(&app(&db, &[]), &format!("/api/jobs/{}/history", summary.id))
        .await
        .json();
    let by_run: std::collections::BTreeMap<String, Value> = body["executions"]
        .as_array()
        .expect("executions")
        .iter()
        .map(|execution| {
            (
                execution["ci_run_id"].as_str().expect("a run").to_owned(),
                execution.clone(),
            )
        })
        .collect();
    assert_eq!(by_run["run-1"]["outcome"], "explained");
    assert_eq!(by_run["run-1"]["explained_by"], json!([]));
    assert_eq!(
        by_run["run-1"]["explained_by_jobs"],
        json!([{
            "job_id": sibling.id,
            "pipeline": ".github/workflows/ci.yml",
            "name": "test",
            "url": "https://github.com/acme/app/actions/runs/1/jobs/2"
        }])
    );
    assert_eq!(body["explained_failures"], 1);
    assert_eq!(by_run["run-2"]["outcome"], "failed");
    assert_eq!(by_run["run-2"]["explained_by_jobs"], json!([]));
}

#[tokio::test]
async fn a_summary_job_with_no_failed_sibling_explains_nothing() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the pipeline");
    let summary = make_job(&pipeline)
        .name("alls-green")
        .summary(true)
        .build(db.connection())
        .await
        .expect("the summary job");
    make_job_execution(&summary)
        .status("passed")
        .ci_job_id("J-OK")
        .ci_run_id("run-1")
        .build(db.connection())
        .await
        .expect("the passing execution");
    make_job_execution(&summary)
        .status("failed")
        .ci_job_id("J-BAD")
        .ci_run_id("run-1")
        .build(db.connection())
        .await
        .expect("the failing execution");
    let body = get(&app(&db, &[]), &format!("/api/jobs/{}/history", summary.id))
        .await
        .json();
    let by_ci: std::collections::BTreeMap<String, Value> = body["executions"]
        .as_array()
        .expect("executions")
        .iter()
        .map(|execution| {
            (
                execution["ci_job_id"]
                    .as_str()
                    .expect("a ci job id")
                    .to_owned(),
                execution.clone(),
            )
        })
        .collect();
    assert_eq!(by_ci["J-OK"]["outcome"], "passed");
    assert_eq!(by_ci["J-BAD"]["outcome"], "failed");
    assert_eq!(by_ci["J-BAD"]["explained_by"], json!([]));
    assert_eq!(by_ci["J-BAD"]["explained_by_jobs"], json!([]));
    assert_eq!(body["explained_failures"], 0);
    assert_eq!(body["unexplained_failures"], 1);
}

#[tokio::test]
async fn a_summary_job_lists_at_most_twenty_siblings() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the pipeline");
    let summary = make_job(&pipeline)
        .name("alls-green")
        .summary(true)
        .build(db.connection())
        .await
        .expect("the summary job");
    make_job_execution(&summary)
        .status("failed")
        .ci_job_id("J-S")
        .ci_run_id("run-1")
        .build(db.connection())
        .await
        .expect("the summary failure");
    for index in 0..21 {
        let leg = make_job(&pipeline)
            .name(&format!("leg-{index:02}"))
            .build(db.connection())
            .await
            .expect("a leg");
        make_job_execution(&leg)
            .status("failed")
            .ci_run_id("run-1")
            .build(db.connection())
            .await
            .expect("a leg failure");
    }
    let body = get(&app(&db, &[]), &format!("/api/jobs/{}/history", summary.id))
        .await
        .json();
    let failed = body["executions"]
        .as_array()
        .expect("executions")
        .iter()
        .find(|execution| execution["ci_job_id"] == "J-S")
        .expect("the summary failure");
    assert_eq!(failed["outcome"], "explained");
    assert_eq!(failed["explained_by"], json!([]));
    let names: Vec<&str> = failed["explained_by_jobs"]
        .as_array()
        .expect("siblings")
        .iter()
        .map(|job| job["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names.len(), 20);
    // Oldest Job execution first, so the last created leg is left out.
    assert!(names.contains(&"leg-00"));
    assert!(!names.contains(&"leg-20"));
    assert_eq!(body["explained_failures"], 1);
    assert_eq!(body["unexplained_failures"], 0);
}

#[tokio::test]
async fn a_non_github_pipeline_has_no_issue_url() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .name("pipeline")
        .provider("gitlab")
        .build(db.connection())
        .await
        .expect("the pipeline");
    make_job(&pipeline)
        .name("build")
        .github_issue_number(5)
        .build(db.connection())
        .await
        .expect("the job");
    let body = get(
        &app(&db, &[]),
        "/api/jobs?repo=acme/app&include_stable=true",
    )
    .await
    .json();
    let job = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|job| job["name"] == "build")
        .expect("the build job");
    assert_eq!(job["provider"], "gitlab");
    assert_eq!(job["github_issue_url"], Value::Null);
    assert_eq!(job["github_issue_number"], 5);
}

#[tokio::test]
async fn the_job_summary_counts_each_tile() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .name("ci.yml")
        .build(db.connection())
        .await
        .expect("the pipeline");
    make_job(&pipeline)
        .name("flaky")
        .flakiness_score(0.6)
        .confirmed_flake_count(2)
        .build(db.connection())
        .await
        .expect("the flaky job");
    make_job(&pipeline)
        .name("suspect")
        .flakiness_score(0.1)
        .build(db.connection())
        .await
        .expect("the suspect job");
    make_job(&pipeline)
        .name("stable")
        .build(db.connection())
        .await
        .expect("the stable job");
    let x = make_job(&pipeline)
        .name("x")
        .build(db.connection())
        .await
        .expect("the x job");
    make_job_execution(&x)
        .build(db.connection())
        .await
        .expect("an execution");
    let body = get(&app(&db, &[]), "/api/jobs/summary?repo=acme/app")
        .await
        .json();
    assert_eq!(body["total_jobs"], 4);
    assert_eq!(body["flaky_jobs"], 1);
    assert_eq!(body["suspect_jobs"], 1);
    assert_eq!(body["confirmed_flaky_jobs"], 1);
    assert_eq!(body["new_flaky_7d"], 1);
    assert_eq!(body["job_executions_30d"], 0);
    assert_eq!(body["job_failures_30d"], 0);
}

#[tokio::test]
async fn the_job_summary_sums_the_last_thirty_days() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .name("ci.yml")
        .build(db.connection())
        .await
        .expect("the pipeline");
    let job = make_job(&pipeline)
        .name("flaky")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the job");
    for (days, executions, failures) in [(0, 7, 3), (5, 4, 2), (40, 100, 90)] {
        make_job_score(&job, day(days), 0.6)
            .executions(executions)
            .failures(failures)
            .build(db.connection())
            .await
            .expect("a score row");
    }
    let body = get(&app(&db, &[]), "/api/jobs/summary?repo=acme/app")
        .await
        .json();
    assert_eq!(body["job_executions_30d"], 11);
    assert_eq!(body["job_failures_30d"], 5);
}

#[tokio::test]
async fn the_job_summary_counts_new_flaky_over_seven_days() {
    let db = TestDb::new().await.expect("a test database");
    let app_ci = make_pipeline()
        .repo("acme/app")
        .name("ci.yml")
        .build(db.connection())
        .await
        .expect("the app pipeline");
    let other = make_pipeline()
        .repo("acme/other")
        .name("ci.yml")
        .build(db.connection())
        .await
        .expect("the other pipeline");
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
            make_job(&app_ci)
                .name(name)
                .flakiness_score(0.6)
                .build(db.connection())
                .await
                .expect("a flaky job"),
        );
    }
    make_job(&app_ci)
        .name("suspect")
        .flakiness_score(0.1)
        .build(db.connection())
        .await
        .expect("the suspect job");
    make_job(&other)
        .name("elsewhere")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the other job");
    for (entity, days, score) in [
        (&made[0], 20, 0.1),
        (&made[0], 3, 0.6),
        (&made[1], 10, 0.3),
        (&made[2], 30, 0.6),
        (&made[3], 7, 0.6),
        (&made[4], 6, 0.6),
        (&made[5], 25, 0.6),
        (&made[5], 10, 0.1),
        (&made[6], 25, 0.1),
        (&made[6], 10, 0.6),
    ] {
        make_job_score(entity, day(days), score)
            .build(db.connection())
            .await
            .expect("a score row");
    }

    let app = app(&db, &[]);
    let scoped = get(&app, "/api/jobs/summary?repo=acme/app").await.json();
    assert_eq!(scoped["flaky_jobs"], 7);
    assert_eq!(scoped["new_flaky_7d"], 3);
    assert_eq!(
        get(&app, "/api/jobs/summary?repo=acme/other").await.json()["new_flaky_7d"],
        1
    );
    assert_eq!(
        get(&app, "/api/jobs/summary").await.json()["new_flaky_7d"],
        4
    );
}

#[tokio::test]
async fn a_test_history_lists_the_jobs_it_ran_in() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .build(db.connection())
        .await
        .expect("the project");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .name("ci.yml")
        .build(db.connection())
        .await
        .expect("the pipeline");
    let job = make_job(&pipeline)
        .name("test")
        .build(db.connection())
        .await
        .expect("the job");
    make_job_execution(&job)
        .status("passed")
        .ci_job_id("X")
        .build(db.connection())
        .await
        .expect("the job execution");
    let test = make_test_case(&project)
        .name("t1")
        .build(db.connection())
        .await
        .expect("the test");
    let run = make_run(&project)
        .commit_sha("s")
        .ci_job_id("X")
        .build(db.connection())
        .await
        .expect("the run");
    make_execution(&test, &run)
        .build(db.connection())
        .await
        .expect("the execution");
    let body = get(&app(&db, &[]), &format!("/api/tests/{}/history", test.id))
        .await
        .json();
    assert_eq!(
        body["jobs"],
        json!([{"job_id": job.id, "pipeline": "ci.yml", "name": "test"}])
    );
}

#[tokio::test]
async fn a_job_out_carries_its_summary_flag() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .name("ci.yml")
        .build(db.connection())
        .await
        .expect("the pipeline");
    make_job(&pipeline)
        .name("alls-green")
        .summary(true)
        .build(db.connection())
        .await
        .expect("the summary job");
    make_job(&pipeline)
        .name("test")
        .build(db.connection())
        .await
        .expect("the test job");
    let body = get(
        &app(&db, &[]),
        "/api/jobs?repo=acme/app&include_stable=true",
    )
    .await
    .json();
    let flags: std::collections::BTreeMap<String, bool> = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|job| {
            (
                job["name"].as_str().expect("a name").to_owned(),
                job["summary"].as_bool().expect("a flag"),
            )
        })
        .collect();
    assert!(flags["alls-green"]);
    assert!(!flags["test"]);
}

#[tokio::test]
async fn an_unknown_job_and_a_bad_repo_are_refused() {
    let db = TestDb::new().await.expect("a test database");
    let app = app(&db, &[]);
    let missing = get(&app, "/api/jobs/99999/history").await;
    assert_eq!(missing.status, 404);
    assert_eq!(missing.json(), json!({"detail": "Job not found"}));
    assert_eq!(get(&app, "/api/jobs?repo=bad").await.status, 422);
    assert_eq!(get(&app, "/api/tests/99999/history").await.status, 404);
    assert_eq!(get(&app, "/api/jobs/abc/history").await.status, 422);
}

#[tokio::test]
async fn a_job_trend_and_score_history_come_from_the_history_table() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .name(".github/workflows/ci.yml")
        .build(db.connection())
        .await
        .expect("the pipeline");
    let job = make_job(&pipeline)
        .name("e2e")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the job");
    make_job_score(&job, day(20), 0.1)
        .build(db.connection())
        .await
        .expect("the old row");
    make_job_score(&job, day(0), 0.6)
        .build(db.connection())
        .await
        .expect("the new row");
    let app = app(&db, &[]);
    let body = get(&app, "/api/jobs?repo=acme/app").await.json();
    assert_eq!(body["items"][0]["trend"], "worsening");
    assert_eq!(body["items"][0]["clean_streak"], 0);
    let history = get(&app, &format!("/api/jobs/{}/history", job.id))
        .await
        .json();
    let days: Vec<&str> = history["score_history"]
        .as_array()
        .expect("score history")
        .iter()
        .map(|point| point["day"].as_str().expect("a day"))
        .collect();
    assert_eq!(
        days,
        [
            day(20).format("%Y-%m-%d").to_string(),
            day(0).format("%Y-%m-%d").to_string()
        ]
    );
}

#[tokio::test]
async fn new_within_days_filters_jobs_before_paging() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .name(".github/workflows/ci.yml")
        .build(db.connection())
        .await
        .expect("the pipeline");
    let crossed = make_job(&pipeline)
        .name("crossed")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("crossed");
    let old_flaky = make_job(&pipeline)
        .name("old_flaky")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("old_flaky");
    let no_old_history = make_job(&pipeline)
        .name("no_old_history")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("no_old_history");
    make_job(&pipeline)
        .name("suspect")
        .flakiness_score(0.1)
        .build(db.connection())
        .await
        .expect("suspect");
    for (entity, days, score) in [
        (&crossed, 20, 0.1),
        (&crossed, 3, 0.6),
        (&old_flaky, 30, 0.6),
        (&no_old_history, 1, 0.6),
    ] {
        make_job_score(entity, day(days), score)
            .build(db.connection())
            .await
            .expect("a score row");
    }
    let app = app(&db, &[]);
    let week = get(&app, "/api/jobs?repo=acme/app&new_within_days=7")
        .await
        .json();
    let mut names = item_names(&week);
    names.sort();
    assert_eq!(names, ["crossed", "no_old_history"]);
    assert_eq!(week["total"], 2);
    let first = get(
        &app,
        "/api/jobs?repo=acme/app&new_within_days=7&page_size=1&page=1",
    )
    .await
    .json();
    assert_eq!(first["items"].as_array().expect("items").len(), 1);
    assert_eq!(first["total"], 2);
    let second = get(
        &app,
        "/api/jobs?repo=acme/app&new_within_days=7&page_size=1&page=2",
    )
    .await
    .json();
    let mut both = item_names(&first);
    both.extend(item_names(&second));
    both.sort();
    assert_eq!(both, ["crossed", "no_old_history"]);
    let two = get(&app, "/api/jobs?repo=acme/app&new_within_days=2")
        .await
        .json();
    assert_eq!(item_names(&two), ["no_old_history"]);
    assert_eq!(two["total"], 1);
    assert_eq!(
        get(&app, "/api/jobs?repo=acme/app&new_within_days=0")
            .await
            .status,
        422
    );
    assert_eq!(
        get(&app, "/api/jobs?repo=acme/app&new_within_days=91")
            .await
            .status,
        422
    );
}

/// The search fixture: ci.yml holds build-thing and lint-fast, nightly.yml holds e2e.
async fn seed_search(db: &TestDb) {
    let ci = make_pipeline()
        .repo("acme/app")
        .name(".github/workflows/ci.yml")
        .build(db.connection())
        .await
        .expect("the ci pipeline");
    make_job(&ci)
        .name("build-thing")
        .flakiness_score(0.6)
        .build(db.connection())
        .await
        .expect("the build job");
    make_job(&ci)
        .name("lint-fast")
        .build(db.connection())
        .await
        .expect("the lint job");
    let nightly = make_pipeline()
        .repo("acme/app")
        .name(".github/workflows/nightly.yml")
        .build(db.connection())
        .await
        .expect("the nightly pipeline");
    make_job(&nightly)
        .name("e2e")
        .flakiness_score(0.3)
        .build(db.connection())
        .await
        .expect("the e2e job");
}

#[tokio::test]
async fn q_matches_a_job_name_including_a_stable_one() {
    let db = TestDb::new().await.expect("a test database");
    seed_search(&db).await;
    let page = get(&app(&db, &[]), "/api/jobs?repo=acme/app&q=FAST")
        .await
        .json();
    assert_eq!(item_names(&page), ["lint-fast"]);
    assert_eq!(page["total"], 1);
}

#[tokio::test]
async fn q_matches_a_pipeline_name() {
    let db = TestDb::new().await.expect("a test database");
    seed_search(&db).await;
    let page = get(&app(&db, &[]), "/api/jobs?repo=acme/app&q=nightly")
        .await
        .json();
    assert_eq!(item_names(&page), ["e2e"]);
}

#[tokio::test]
async fn q_is_literal_and_length_bounded() {
    let db = TestDb::new().await.expect("a test database");
    seed_search(&db).await;
    let app = app(&db, &[]);
    assert_eq!(
        get(&app, "/api/jobs?repo=acme/app&q=%25").await.json()["total"],
        0
    );
    // An empty q: the stable job stays hidden.
    assert_eq!(
        get(&app, "/api/jobs?repo=acme/app&q=").await.json()["total"],
        2
    );
    let overlong = "a".repeat(201);
    let reply = get(&app, &format!("/api/jobs?repo=acme/app&q={overlong}")).await;
    assert_eq!(reply.status, 422);
    assert_eq!(reply.json()["detail"][0]["type"], "string_too_long");
    assert_eq!(reply.json()["detail"][0]["loc"], json!(["query", "q"]));
}

#[tokio::test]
async fn q_composes_with_a_sort() {
    let db = TestDb::new().await.expect("a test database");
    seed_search(&db).await;
    let page = get(
        &app(&db, &[]),
        "/api/jobs?repo=acme/app&q=fast&sort=last_seen",
    )
    .await
    .json();
    assert_eq!(item_names(&page), ["lint-fast"]);
    assert_eq!(page["page"], 1);
}

#[tokio::test]
async fn the_pipeline_filter_is_an_exact_name_match() {
    let db = TestDb::new().await.expect("a test database");
    seed_search(&db).await;
    let app = app(&db, &[]);
    let nightly = get(
        &app,
        "/api/jobs?repo=acme/app&pipeline=.github/workflows/nightly.yml",
    )
    .await
    .json();
    assert_eq!(item_names(&nightly), ["e2e"]);
    let ci = get(
        &app,
        "/api/jobs?repo=acme/app&pipeline=.github/workflows/ci.yml",
    )
    .await
    .json();
    assert_eq!(item_names(&ci), ["build-thing"]);
    assert_eq!(
        get(&app, "/api/jobs?repo=acme/app&pipeline=nightly.yml")
            .await
            .json()["total"],
        0
    );
    assert_eq!(
        get(
            &app,
            "/api/jobs?repo=acme/app&pipeline=.github/workflows/NIGHTLY.yml"
        )
        .await
        .json()["total"],
        0
    );
}

#[tokio::test]
async fn the_pipeline_filter_composes_with_include_stable() {
    let db = TestDb::new().await.expect("a test database");
    seed_search(&db).await;
    let page = get(
        &app(&db, &[]),
        "/api/jobs?repo=acme/app&pipeline=.github/workflows/ci.yml&include_stable=true",
    )
    .await
    .json();
    assert_eq!(item_names(&page), ["build-thing", "lint-fast"]);
}

#[tokio::test]
async fn the_issue_filter_applies_to_jobs() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .build(db.connection())
        .await
        .expect("the pipeline");
    make_job(&pipeline)
        .name("tracked")
        .flakiness_score(0.6)
        .github_issue_number(7)
        .build(db.connection())
        .await
        .expect("the tracked job");
    make_job(&pipeline)
        .name("untracked")
        .flakiness_score(0.4)
        .build(db.connection())
        .await
        .expect("the untracked job");
    let app = app(&db, &[]);
    let linked = get(&app, "/api/jobs?repo=acme/app&has_issue=true")
        .await
        .json();
    assert_eq!(item_names(&linked), ["tracked"]);
    let unlinked = get(&app, "/api/jobs?repo=acme/app&has_issue=false")
        .await
        .json();
    assert_eq!(item_names(&unlinked), ["untracked"]);
    assert_eq!(
        get(&app, "/api/jobs?repo=acme/app").await.json()["total"],
        2
    );
}

#[tokio::test]
async fn the_job_summary_lists_pipelines_in_scope() {
    let db = TestDb::new().await.expect("a test database");
    seed_search(&db).await;
    let other = make_pipeline()
        .repo("acme/other")
        .name(".github/workflows/release.yml")
        .build(db.connection())
        .await
        .expect("the release pipeline");
    make_job(&other)
        .name("publish")
        .flakiness_score(0.2)
        .build(db.connection())
        .await
        .expect("the publish job");
    let app = app(&db, &[]);
    let scoped = get(&app, "/api/jobs/summary?repo=acme/app").await.json();
    assert_eq!(
        scoped["pipelines"],
        json!([".github/workflows/ci.yml", ".github/workflows/nightly.yml"])
    );
    let unscoped = get(&app, "/api/jobs/summary").await.json();
    assert_eq!(
        unscoped["pipelines"],
        json!([
            ".github/workflows/ci.yml",
            ".github/workflows/nightly.yml",
            ".github/workflows/release.yml"
        ])
    );
    let empty = get(&app, "/api/jobs/summary?repo=acme/nothing")
        .await
        .json();
    assert_eq!(empty["pipelines"], json!([]));
}

#[tokio::test]
async fn a_pipeline_name_over_its_length_rule_is_a_422() {
    let db = TestDb::new().await.expect("a test database");
    let long_pipeline = "p".repeat(513);
    let reply = get(
        &app(&db, &[]),
        &format!("/api/jobs?repo=acme/app&pipeline={long_pipeline}"),
    )
    .await;
    assert_eq!(reply.status, 422);
    assert_eq!(
        reply.json()["detail"][0]["loc"],
        json!(["query", "pipeline"])
    );
}
