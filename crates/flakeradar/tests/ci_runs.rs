//! The CI run failure listing: which attempt, Report counts, failures, failed Jobs.
//!
//! The CI run failure listing: the cap, the counts and the failed Jobs it names.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a fixture or an answer does not behave"
)]

use flakeradar_test_support::{
    TestDb, app, get, make_execution, make_job, make_job_execution, make_junit, make_pipeline,
    make_project, make_report, make_run, make_test_case, post, request,
};

use serde_json::{Value, json};

/// The token the ingest route needs.
const AUTH: &str = "test-token";

/// The failure URL for `ci_run_id`.
fn url(ci_run_id: &str, repo: &str, more: &str) -> String {
    format!("/api/ci-runs/{ci_run_id}/failures?repo={repo}{more}")
}

#[tokio::test]
async fn an_unknown_repo_is_a_404() {
    let db = TestDb::new().await.expect("a test database");
    let reply = get(&app(&db, &[]), &url("100", "acme/app", "")).await;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.json(), json!({"detail": "Repo not found"}));
}

#[tokio::test]
async fn an_invalid_repo_is_a_422() {
    let db = TestDb::new().await.expect("a test database");
    assert_eq!(
        get(&app(&db, &[]), &url("100", "nope", "")).await.status,
        422
    );
}

#[tokio::test]
async fn an_unknown_ci_run_is_a_404() {
    let db = TestDb::new().await.expect("a test database");
    make_project().repo("acme/app").create(&db).await;
    let reply = get(&app(&db, &[]), &url("100", "acme/app", "")).await;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.json(), json!({"detail": "CI run not found"}));
}

#[tokio::test]
async fn an_unknown_attempt_is_a_404() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    let reply = get(&app(&db, &[]), &url("100", "acme/app", "&ci_run_attempt=3")).await;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.json(), json!({"detail": "CI run attempt not found"}));
}

#[tokio::test]
async fn the_attempt_defaults_to_the_latest_including_a_pending_one() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    make_report(&project, project.repo_id)
        .body(b"<x/>")
        .ci_run_id("100")
        .status("processed")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    make_report(&project, project.repo_id)
        .body(b"<y/>")
        .ci_run_id("100")
        .status("pending")
        .ci_run_attempt(2)
        .create(&db)
        .await;
    let app = app(&db, &[]);
    let body = get(&app, &url("100", "acme/app", "")).await.json();
    assert_eq!(body["ci_run_attempt"], 2);
    assert_eq!(
        body["reports"],
        json!({"pending": 1, "processed": 0, "failed": 0})
    );
    let first = get(&app, &url("100", "acme/app", "&ci_run_attempt=1"))
        .await
        .json();
    assert_eq!(
        first["reports"],
        json!({"pending": 0, "processed": 1, "failed": 0})
    );
}

#[tokio::test]
async fn a_null_attempt_counts_as_one() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    make_report(&project, project.repo_id)
        .body(b"<x/>")
        .ci_run_id("100")
        .status("processed")
        .create(&db)
        .await;
    let body = get(&app(&db, &[]), &url("100", "acme/app", ""))
        .await
        .json();
    assert_eq!(body["ci_run_attempt"], 1);
    assert_eq!(body["reports"]["processed"], 1);
}

#[tokio::test]
async fn pipeline_reports_are_counted() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    make_report(&project, project.repo_id)
        .body(b"<x/>")
        .ci_run_id("1")
        .status("processed")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    make_report(&project, project.repo_id)
        .kind("pipeline")
        .body(b"{}")
        .ci_run_id("1")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    let body = get(&app(&db, &[]), &url("1", "acme/app", "")).await.json();
    assert_eq!(
        body["reports"],
        json!({"pending": 1, "processed": 1, "failed": 0})
    );
}

#[tokio::test]
async fn a_failed_report_is_counted_and_lists_no_failures() {
    // The summary action and Fabro read `reports.failed`; a Report kept after a
    // failed processing must be counted without inventing a failure entry.
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    make_report(&project, project.repo_id)
        .body(b"<not xml")
        .ci_run_id("300")
        .ci_run_attempt(1)
        .status("failed")
        .error("Not a valid JUnit XML report")
        .create(&db)
        .await;
    let body = get(&app(&db, &[]), &url("300", "acme/app", ""))
        .await
        .json();
    assert_eq!(body["ci_run_attempt"], 1);
    assert_eq!(
        body["reports"],
        json!({"pending": 0, "processed": 0, "failed": 1})
    );
    assert_eq!(body["failures"], json!([]));
    assert_eq!(body["failed_jobs"], json!([]));
    assert_eq!(body["truncated"], false);
}

#[tokio::test]
async fn a_run_is_found_after_its_reports_are_pruned() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    make_run(&project)
        .ci_run_id("200")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    let reply = get(&app(&db, &[]), &url("200", "acme/app", "")).await;
    assert_eq!(reply.status, 200);
    let body = reply.json();
    assert_eq!(
        body["reports"],
        json!({"pending": 0, "processed": 0, "failed": 0})
    );
    assert_eq!(body["failures"], json!([]));
    assert_eq!(body["failed_jobs"], json!([]));
    assert_eq!(body["total_failures"], 0);
    assert_eq!(body["truncated"], false);
}

#[tokio::test]
async fn a_run_is_found_by_a_job_execution() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline().repo("acme/app").create(&db).await;
    let job = make_job(&pipeline).name("backend").create(&db).await;
    make_job_execution(&job)
        .ci_run_id("300")
        .ci_run_attempt(2)
        .create(&db)
        .await;
    let body = get(&app(&db, &[]), &url("300", "acme/app", ""))
        .await
        .json();
    assert_eq!(body["ci_run_attempt"], 2);
}

#[tokio::test]
async fn a_ci_run_id_is_scoped_to_its_repo() {
    let db = TestDb::new().await.expect("a test database");
    let other = make_project().repo("other/repo").create(&db).await;
    make_report(&other, other.repo_id)
        .body(b"<x/>")
        .ci_run_id("100")
        .status("processed")
        .create(&db)
        .await;
    make_project().repo("acme/app").create(&db).await;
    let reply = get(&app(&db, &[]), &url("100", "acme/app", "")).await;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.json(), json!({"detail": "CI run not found"}));
}

#[tokio::test]
async fn an_ingest_then_an_ask_sees_the_pending_report() {
    let db = TestDb::new().await.expect("a test database");
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", AUTH)]);
    let body = make_junit(&[("t1", "failed")]);
    let reply = post(
        &app,
        "/api/ingest?repo=acme/app&project=backend&commit_sha=abc&ci_run_id=555&ci_run_attempt=1",
        &[("X-API-Key", AUTH), ("Content-Type", "application/xml")],
        body,
    )
    .await;
    assert_eq!(reply.status, 202);
    let body = get(&app, &url("555", "acme/app", "")).await.json();
    assert_eq!(
        body["reports"],
        json!({"pending": 1, "processed": 0, "failed": 0})
    );
    assert_eq!(body["failures"], json!([]));
}

#[tokio::test]
async fn the_top_level_fields_echo_the_request() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    let body = get(&app(&db, &[]), &url("100", "ACME/App", ""))
        .await
        .json();
    assert_eq!(body["repo"], "acme/app");
    assert_eq!(body["ci_run_id"], "100");
    assert_eq!(body["flake_threshold"], 0.3);
}

#[tokio::test]
async fn each_attempt_lists_its_own_failures() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .create(&db)
        .await;
    let t1 = make_test_case(&project).name("t1").create(&db).await;
    let t2 = make_test_case(&project).name("t2").create(&db).await;
    let first = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    make_execution(&t1, &first)
        .status("failed")
        .create(&db)
        .await;
    make_execution(&t2, &first).create(&db).await;
    let second = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(2)
        .create(&db)
        .await;
    make_execution(&t1, &second).create(&db).await;
    make_execution(&t2, &second)
        .status("failed")
        .create(&db)
        .await;

    let app = app(&db, &[]);
    let latest = get(&app, &url("100", "acme/app", "")).await.json();
    assert_eq!(latest["ci_run_attempt"], 2);
    assert_eq!(latest["failures"][0]["name"], "t2");
    let first_attempt = get(&app, &url("100", "acme/app", "&ci_run_attempt=1"))
        .await
        .json();
    assert_eq!(first_attempt["failures"][0]["name"], "t1");
}

#[tokio::test]
async fn a_failure_entry_carries_every_field() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .create(&db)
        .await;
    let t1 = make_test_case(&project)
        .name("t1")
        .flakiness_score(0.5)
        .confirmed_flake_count(2)
        .clean_streak(3)
        .quarantined()
        .github_issue_number(9)
        .create(&db)
        .await;
    let run = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .ci_job_id("J1")
        .create(&db)
        .await;
    make_execution(&t1, &run)
        .status("error")
        .message("boom")
        .failure_category("network")
        .create(&db)
        .await;

    let body = get(&app(&db, &[]), &url("100", "acme/app", ""))
        .await
        .json();
    assert_eq!(
        body["failures"],
        json!([{
            "test_id": t1.id,
            "project": "backend",
            "suite": "unit",
            "classname": "tests.test_mod",
            "name": "t1",
            "run_id": run.id,
            "ci_job_id": "J1",
            "status": "error",
            "failed_attempts": 1,
            "failure_category": "network",
            "message": "boom",
            "tier": "flaky",
            "flakiness_score": 0.5,
            "confirmed_flake_count": 2,
            "clean_streak": 3,
            "fail_streak": 0,
            "broken": false,
            "quarantined": true,
            "github_issue_number": 9,
            "github_issue_url": "https://github.com/acme/app/issues/9",
            "github_issue_closed": false
        }])
    );
    assert_eq!(body["total_failures"], 1);
    assert_eq!(body["truncated"], false);
}

#[tokio::test]
async fn a_failure_entry_marks_a_broken_test() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .create(&db)
        .await;
    let t1 = make_test_case(&project)
        .name("t1")
        .fail_streak(5)
        .create(&db)
        .await;
    let run = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    make_execution(&t1, &run).status("failed").create(&db).await;
    let body = get(&app(&db, &[]), &url("100", "acme/app", ""))
        .await
        .json();
    assert_eq!(body["failures"][0]["fail_streak"], 5);
    assert_eq!(body["failures"][0]["broken"], true);
}

#[tokio::test]
async fn retries_inside_one_run_are_one_entry() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .create(&db)
        .await;
    let t1 = make_test_case(&project).name("t1").create(&db).await;
    let t2 = make_test_case(&project).name("t2").create(&db).await;
    let t3 = make_test_case(&project).name("t3").create(&db).await;
    let run = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    make_execution(&t1, &run)
        .status("failed")
        .message("first")
        .failure_category("timing")
        .create(&db)
        .await;
    make_execution(&t1, &run).attempt(1).create(&db).await;
    make_execution(&t2, &run)
        .status("failed")
        .message("a")
        .create(&db)
        .await;
    make_execution(&t2, &run)
        .status("failed")
        .message("b")
        .attempt(1)
        .failure_category("assertion")
        .create(&db)
        .await;
    make_execution(&t3, &run).create(&db).await;

    let body = get(&app(&db, &[]), &url("100", "acme/app", ""))
        .await
        .json();
    assert_eq!(body["failures"].as_array().expect("failures").len(), 2);
    let by_name: std::collections::BTreeMap<String, Value> = body["failures"]
        .as_array()
        .expect("failures")
        .iter()
        .map(|failure| {
            (
                failure["name"].as_str().expect("a name").to_owned(),
                failure.clone(),
            )
        })
        .collect();
    assert_eq!(by_name["t1"]["status"], "passed");
    assert_eq!(by_name["t1"]["failed_attempts"], 1);
    assert_eq!(by_name["t1"]["message"], "first");
    assert_eq!(by_name["t1"]["failure_category"], "timing");
    assert_eq!(by_name["t2"]["status"], "failed");
    assert_eq!(by_name["t2"]["failed_attempts"], 2);
    assert_eq!(by_name["t2"]["message"], "b");
    assert_eq!(by_name["t2"]["failure_category"], "assertion");
}

#[tokio::test]
async fn the_same_test_in_two_runs_is_two_entries() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .create(&db)
        .await;
    let t1 = make_test_case(&project).name("t1").create(&db).await;
    let run_a = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .ci_job_id("A")
        .create(&db)
        .await;
    let run_b = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .ci_job_id("B")
        .create(&db)
        .await;
    make_execution(&t1, &run_a)
        .status("failed")
        .create(&db)
        .await;
    make_execution(&t1, &run_b)
        .status("failed")
        .create(&db)
        .await;
    let body = get(&app(&db, &[]), &url("100", "acme/app", ""))
        .await
        .json();
    let jobs: Vec<&str> = body["failures"]
        .as_array()
        .expect("failures")
        .iter()
        .map(|failure| failure["ci_job_id"].as_str().expect("a job"))
        .collect();
    assert_eq!(jobs, ["A", "B"]);
    let runs: Vec<i64> = body["failures"]
        .as_array()
        .expect("failures")
        .iter()
        .map(|failure| failure["run_id"].as_i64().expect("a run"))
        .collect();
    assert_eq!(runs, [i64::from(run_a.id), i64::from(run_b.id)]);
}

#[tokio::test]
async fn skipped_and_passing_tests_are_not_listed() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .create(&db)
        .await;
    let t1 = make_test_case(&project).name("t1").create(&db).await;
    let t2 = make_test_case(&project).name("t2").create(&db).await;
    let run = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    make_execution(&t1, &run).create(&db).await;
    make_execution(&t2, &run)
        .status("skipped")
        .create(&db)
        .await;
    let body = get(&app(&db, &[]), &url("100", "acme/app", ""))
        .await
        .json();
    assert_eq!(body["failures"], json!([]));
    assert_eq!(body["total_failures"], 0);
}

#[tokio::test]
async fn a_message_is_capped_at_one_thousand_and_twenty_four_characters() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .create(&db)
        .await;
    let t1 = make_test_case(&project).name("t1").create(&db).await;
    let run = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    make_execution(&t1, &run)
        .status("failed")
        .message(&"x".repeat(5000))
        .create(&db)
        .await;
    let body = get(&app(&db, &[]), &url("100", "acme/app", ""))
        .await
        .json();
    assert_eq!(
        body["failures"][0]["message"]
            .as_str()
            .expect("a message")
            .len(),
        1024
    );
}

#[tokio::test]
async fn failures_are_ordered_by_project_then_identity() {
    let db = TestDb::new().await.expect("a test database");
    let api = make_project()
        .repo("acme/app")
        .project("api")
        .create(&db)
        .await;
    let backend = make_project()
        .repo("acme/app")
        .project("backend")
        .create(&db)
        .await;
    let z = make_test_case(&backend)
        .name("z")
        .classname("tests.b")
        .create(&db)
        .await;
    let y = make_test_case(&backend)
        .name("y")
        .classname("tests.a")
        .create(&db)
        .await;
    let x = make_test_case(&api)
        .name("x")
        .classname("tests.a")
        .create(&db)
        .await;
    let run = make_run(&api)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    for test in [&z, &y, &x] {
        make_execution(test, &run)
            .status("failed")
            .create(&db)
            .await;
    }
    let body = get(&app(&db, &[]), &url("100", "acme/app", ""))
        .await
        .json();
    let order: Vec<(&str, &str)> = body["failures"]
        .as_array()
        .expect("failures")
        .iter()
        .map(|failure| {
            (
                failure["project"].as_str().expect("a project"),
                failure["name"].as_str().expect("a name"),
            )
        })
        .collect();
    assert_eq!(order, [("api", "x"), ("backend", "y"), ("backend", "z")]);
}

#[tokio::test]
async fn other_ci_runs_are_ignored() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .create(&db)
        .await;
    let t1 = make_test_case(&project).name("t1").create(&db).await;
    let other = make_run(&project)
        .ci_run_id("999")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    make_execution(&t1, &other)
        .status("failed")
        .create(&db)
        .await;
    let this = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    make_execution(&t1, &this).create(&db).await;
    let body = get(&app(&db, &[]), &url("100", "acme/app", ""))
        .await
        .json();
    assert_eq!(body["failures"], json!([]));
}

#[tokio::test]
async fn failed_jobs_are_explained_or_unexplained() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .create(&db)
        .await;
    let pipeline = make_pipeline().repo("acme/app").create(&db).await;
    let backend = make_job(&pipeline).name("backend").create(&db).await;
    let e2e = make_job(&pipeline).name("e2e").create(&db).await;
    let lint = make_job(&pipeline).name("lint").create(&db).await;
    for (job, status, ci_job_id) in [
        (&backend, "failed", "J-B"),
        (&e2e, "failed", "J-E"),
        (&lint, "passed", "J-L"),
    ] {
        make_job_execution(job)
            .status(status)
            .ci_job_id(ci_job_id)
            .ci_run_id("100")
            .create(&db)
            .await;
    }
    let t1 = make_test_case(&project).name("t1").create(&db).await;
    let run = make_run(&project)
        .ci_run_id("100")
        .ci_run_attempt(1)
        .ci_job_id("J-B")
        .create(&db)
        .await;
    make_execution(&t1, &run).status("failed").create(&db).await;
    let body = get(&app(&db, &[]), &url("100", "acme/app", ""))
        .await
        .json();
    let jobs: Vec<(&str, &str)> = body["failed_jobs"]
        .as_array()
        .expect("failed jobs")
        .iter()
        .map(|job| {
            (
                job["name"].as_str().expect("a name"),
                job["outcome"].as_str().expect("an outcome"),
            )
        })
        .collect();
    assert_eq!(jobs, [("backend", "explained"), ("e2e", "unexplained")]);
    assert_eq!(body["failures"][0]["name"], "t1");
}

#[tokio::test]
async fn a_red_run_without_failing_tests_still_lists_its_jobs() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline().repo("acme/app").create(&db).await;
    let e2e = make_job(&pipeline).name("e2e").create(&db).await;
    make_job_execution(&e2e)
        .status("failed")
        .ci_job_id("J-E")
        .ci_run_id("100")
        .create(&db)
        .await;
    let body = get(&app(&db, &[]), &url("100", "acme/app", ""))
        .await
        .json();
    assert_eq!(body["failures"], json!([]));
    assert_eq!(
        body["reports"],
        json!({"pending": 0, "processed": 0, "failed": 0})
    );
    assert_eq!(body["failed_jobs"].as_array().expect("jobs").len(), 1);
    assert_eq!(body["failed_jobs"][0]["outcome"], "unexplained");
}

#[tokio::test]
async fn a_failed_job_carries_every_field() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline().repo("acme/app").create(&db).await;
    let job = make_job(&pipeline)
        .name("e2e")
        .flakiness_score(0.1)
        .confirmed_flake_count(1)
        .clean_streak(2)
        .github_issue_number(4)
        .create(&db)
        .await;
    make_job_execution(&job)
        .status("failed")
        .ci_job_id("J-E")
        .ci_run_id("100")
        .url("https://ci/1")
        .create(&db)
        .await;
    let body = get(&app(&db, &[]), &url("100", "acme/app", ""))
        .await
        .json();
    assert_eq!(
        body["failed_jobs"],
        json!([{
            "job_id": job.id,
            "pipeline": ".github/workflows/ci.yml",
            "name": "e2e",
            "ci_job_id": "J-E",
            "url": "https://ci/1",
            "outcome": "unexplained",
            "summary": false,
            "tier": "suspect",
            "flakiness_score": 0.1,
            "confirmed_flake_count": 1,
            "clean_streak": 2,
            "github_issue_number": 4,
            "github_issue_url": "https://github.com/acme/app/issues/4",
            "github_issue_closed": false
        }])
    );
}

#[tokio::test]
async fn a_summary_job_is_explained_by_a_failed_sibling() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .create(&db)
        .await;
    let pipeline = make_pipeline()
        .repo("acme/app")
        .name(".github/workflows/playwright.yml")
        .create(&db)
        .await;
    let tests = make_job(&pipeline)
        .name("test-playwright")
        .create(&db)
        .await;
    let summary = make_job(&pipeline)
        .name("alls-green-playwright")
        .summary(true)
        .create(&db)
        .await;
    make_job_execution(&tests)
        .status("failed")
        .ci_job_id("J-T")
        .ci_run_id("38973797537")
        .create(&db)
        .await;
    make_job_execution(&summary)
        .status("failed")
        .ci_job_id("J-G")
        .ci_run_id("38973797537")
        .create(&db)
        .await;
    let t1 = make_test_case(&project)
        .name("Squad screen controls renders stat tile values unbroken")
        .create(&db)
        .await;
    let run = make_run(&project)
        .ci_run_id("38973797537")
        .ci_run_attempt(1)
        .ci_job_id("J-T")
        .create(&db)
        .await;
    make_execution(&t1, &run).status("failed").create(&db).await;

    let body = get(&app(&db, &[]), &url("38973797537", "acme/app", ""))
        .await
        .json();
    let jobs: Vec<(&str, &str, bool)> = body["failed_jobs"]
        .as_array()
        .expect("failed jobs")
        .iter()
        .map(|job| {
            (
                job["name"].as_str().expect("a name"),
                job["outcome"].as_str().expect("an outcome"),
                job["summary"].as_bool().expect("a flag"),
            )
        })
        .collect();
    assert_eq!(
        jobs,
        [
            ("alls-green-playwright", "explained", true),
            ("test-playwright", "explained", false)
        ]
    );
    assert_eq!(
        body["failures"][0]["name"],
        "Squad screen controls renders stat tile values unbroken"
    );
}

#[tokio::test]
async fn a_summary_job_alone_is_unexplained() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline().repo("acme/app").create(&db).await;
    let summary = make_job(&pipeline)
        .name("alls-green")
        .summary(true)
        .create(&db)
        .await;
    make_job_execution(&summary)
        .status("failed")
        .ci_job_id("J-G")
        .ci_run_id("100")
        .create(&db)
        .await;
    let lint = make_job(&pipeline).name("lint").create(&db).await;
    make_job_execution(&lint)
        .status("passed")
        .ci_job_id("J-L")
        .ci_run_id("100")
        .create(&db)
        .await;
    let body = get(&app(&db, &[]), &url("100", "acme/app", ""))
        .await
        .json();
    assert_eq!(
        body["failed_jobs"],
        json!([{
            "job_id": summary.id,
            "pipeline": ".github/workflows/ci.yml",
            "name": "alls-green",
            "ci_job_id": "J-G",
            "url": "",
            "outcome": "unexplained",
            "summary": true,
            "tier": "stable",
            "flakiness_score": 0.0,
            "confirmed_flake_count": 0,
            "clean_streak": 0,
            "github_issue_number": null,
            "github_issue_url": null,
            "github_issue_closed": false
        }])
    );
}

#[tokio::test]
async fn a_non_github_job_has_no_issue_url() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .repo("acme/app")
        .name("ci.yml")
        .provider("gitlab")
        .create(&db)
        .await;
    let job = make_job(&pipeline)
        .name("e2e")
        .github_issue_number(4)
        .create(&db)
        .await;
    make_job_execution(&job)
        .status("failed")
        .ci_job_id("J-E")
        .ci_run_id("100")
        .create(&db)
        .await;
    let body = get(&app(&db, &[]), &url("100", "acme/app", ""))
        .await
        .json();
    assert_eq!(body["failed_jobs"][0]["github_issue_number"], 4);
    assert_eq!(body["failed_jobs"][0]["github_issue_url"], Value::Null);
}

#[tokio::test]
async fn failed_jobs_of_another_attempt_are_ignored() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline().repo("acme/app").create(&db).await;
    let e2e = make_job(&pipeline).name("e2e").create(&db).await;
    make_job_execution(&e2e)
        .status("failed")
        .ci_job_id("J-1")
        .ci_run_id("100")
        .ci_run_attempt(1)
        .create(&db)
        .await;
    make_job_execution(&e2e)
        .status("passed")
        .ci_job_id("J-2")
        .ci_run_id("100")
        .ci_run_attempt(2)
        .create(&db)
        .await;
    let app = app(&db, &[]);
    let latest = get(&app, &url("100", "acme/app", "")).await.json();
    assert_eq!(latest["failed_jobs"], json!([]));
    let first = get(&app, &url("100", "acme/app", "&ci_run_attempt=1"))
        .await
        .json();
    assert_eq!(first["failed_jobs"][0]["name"], "e2e");
    assert_eq!(first["failed_jobs"][0]["outcome"], "unexplained");
}

#[tokio::test]
async fn an_empty_ci_run_id_answers_the_404_body() {
    let db = TestDb::new().await.expect("a test database");
    // `/api/ci-runs//failures` matches no route, so it answers the 404 body.
    let reply = request(
        &app(&db, &[]),
        axum::http::Method::GET,
        "/api/ci-runs//failures?repo=acme/app",
    )
    .await;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.json(), json!({"detail": "Not Found"}));
}

#[tokio::test]
async fn a_ci_run_id_over_two_hundred_and_fifty_five_characters_is_a_422() {
    let db = TestDb::new().await.expect("a test database");
    let long = "r".repeat(256);
    let reply = get(&app(&db, &[]), &url(&long, "acme/app", "")).await;
    assert_eq!(reply.status, 422);
    assert_eq!(reply.json()["detail"][0]["type"], "string_too_long");
    assert_eq!(
        reply.json()["detail"][0]["loc"],
        json!(["path", "ci_run_id"])
    );
}
