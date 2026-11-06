//! What the real consumers send and read, on the payloads they send.
//!
//! `frontend/` is covered by its own suite and the MCP tools by the `mcp`
//! scenario. What is left is the two CI integrations nobody's tests own: the
//! writers-app pipeline (a raw `curl` upload from `actions/report/report.sh`
//! and the JSON body of `samples/flakeradar-jobs.yml`) and Fabro, which reads
//! the leaderboard, a Test's history and one CI run's failures to decide
//! whether a flaky Test blocks a merge. Both payloads are copied from the
//! committed files rather than invented, so a renamed query parameter or
//! response field fails here.

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a consumer test that cannot send its payload or read the answer has failed"
)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request};
use chrono::NaiveDateTime;
use flakeradar_db::processing::ProcessingSettings;
use flakeradar_test_support::{TestDb, app, get};

use serde::Deserialize;
use serde_json::{Value, json};

/// The token every request presents, as a configured instance requires.
const TOKEN: &str = "consumer-token";

/// The settings a consumer test runs with.
fn settings_pairs() -> Vec<(&'static str, &'static str)> {
    vec![
        ("FLAKERADAR_API_TOKEN", TOKEN),
        ("FLAKERADAR_WEBHOOK_URLS", ""),
        ("FLAKERADAR_GITHUB_TOKEN", ""),
    ]
}

/// A pytest JUnit report, shaped as writers-app's `backend` suite writes it.
///
/// `junit_family=xunit1` puts the file and the line on each `<testcase>`, and
/// the paths are relative to the Project root, which the upload names as
/// `backend`. Each case is its name and whether it failed.
fn writers_app_junit(cases: &[(&str, bool)]) -> String {
    use std::fmt::Write as _;
    let mut testcases = String::new();
    for (name, failing) in cases {
        if *failing {
            let _ = writeln!(
                testcases,
                r#"    <testcase classname="tests.test_writers" name="{name}" file="tests/test_writers.py" line="40" time="1.381">
      <failure message="AssertionError: expected 3, got 4">AssertionError</failure>
    </testcase>"#
            );
        } else {
            let _ = writeln!(
                testcases,
                r#"    <testcase classname="tests.test_writers" name="{name}" file="tests/test_writers.py" line="12" time="0.031"/>"#
            );
        }
    }
    let failures = cases.iter().filter(|(_, failing)| *failing).count();
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<testsuites>
  <testsuite name="pytest" errors="0" failures="{failures}" skipped="0" tests="{}" time="1.412">
{testcases}  </testsuite>
</testsuites>
"#,
        cases.len()
    )
}

/// Sends one request and returns its status and JSON body.
async fn send(app: &Router, request: Request<Body>) -> (u16, Value) {
    let reply = flakeradar_test_support::send(app, request).await;
    (reply.status.as_u16(), reply.json())
}

/// Uploads one JUnit report the way `actions/report/report.sh` does.
///
/// The script builds the query string with `jq` and posts the file's bytes with
/// `Content-Type: application/xml`; the query parameters here are its list, in
/// its order, with the values a writers-app run computes.
async fn upload_as_the_report_action(
    app: &Router,
    repo: &str,
    commit_sha: &str,
    ci_run_id: &str,
    junit: &str,
) -> u64 {
    let query = format!(
        "repo={repo}&commit_sha={commit_sha}&branch=main&project=backend&ci_run_id={ci_run_id}&\
         ci_run_attempt=1&ci_job_id=8812&pipeline=.github%2Fworkflows%2Fci.yml&\
         default_branch=main&root=backend&line_base=0"
    );
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/ingest?{query}"))
        .header("x-api-key", TOKEN)
        .header("content-type", "application/xml")
        .body(Body::from(junit.to_owned()))
        .expect("the upload request builds");
    let (status, body) = send(app, request).await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(body.get("status"), Some(&json!("pending")), "{body}");
    assert_eq!(body.get("duplicate"), Some(&json!(false)), "{body}");
    body.get("report_id")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("the answer carries a report_id: {body}"))
}

/// Drains the queue the way the worker does.
async fn process_all(db: &TestDb) {
    let settings = ProcessingSettings::default();
    while flakeradar_db::processing::process_next(db.connection(), &settings)
        .await
        .expect("processing a queued Report succeeds")
        .is_some()
    {}
}

#[tokio::test]
async fn the_writers_app_curl_upload_reaches_a_processed_run() {
    let db = TestDb::new().await.expect("a test database");
    let app = app(&db, &settings_pairs());
    let report_id = upload_as_the_report_action(
        &app,
        "andrewthetechie/writers-app",
        "4f2c1d9",
        "42001",
        &writers_app_junit(&[
            ("test_a_writer_keeps_its_name", false),
            ("test_a_writer_rejects_a_duplicate_slug", true),
        ]),
    )
    .await;
    process_all(&db).await;

    let report = get(&app, &format!("/api/reports/{report_id}")).await;
    assert_eq!(report.status, 200);
    let report = report.json();
    assert_eq!(report["status"], "processed");
    assert_eq!(report["repo"], "andrewthetechie/writers-app");
    assert_eq!(report["project"], "backend");
    // The script sends pytest's 0-based lines, and the upload says so.
    assert_eq!(report["commit_sha"], "4f2c1d9");
    assert_eq!(report["branch"], "main");
    assert_eq!(report["ci_run_id"], "42001");
    assert!(report["run_id"].as_i64().is_some());
    assert_eq!(report["counts"]["passed"], 1);
    assert_eq!(report["counts"]["failed"], 1);

    // The dashboard, the summary action and Fabro read the Tests this Run
    // produced, so the upload's Project root, Pipeline and metadata must be
    // visible there.
    let page = get(
        &app,
        "/api/tests?repo=andrewthetechie%2Fwriters-app&include_stable=true",
    )
    .await;
    assert_eq!(page.status, 200);
    let page = page.json();
    let items = page["items"]
        .as_array()
        .unwrap_or_else(|| panic!("the leaderboard has items: {page}"));
    assert_eq!(items.len(), 2, "{page}");
    let failing = items
        .iter()
        .find(|item| item["name"] == "test_a_writer_rejects_a_duplicate_slug")
        .unwrap_or_else(|| panic!("the failing Test is listed: {page}"));
    assert_eq!(failing["project"], "backend");
    assert_eq!(failing["suite"], "pytest");
    assert_eq!(failing["classname"], "tests.test_writers");
    // The location is relative to the Project root the upload named.
    assert_eq!(failing["file"], "tests/test_writers.py");
    // pytest's xunit1 lines are 0-based, and the upload says `line_base=0`, so
    // the runner's line 40 is the file's line 41.
    assert_eq!(failing["line"], 41);
    assert_eq!(failing["tier"], "stable");
    assert_eq!(failing["failure_category"], "assertion");
}

/// The body `samples/flakeradar-jobs.yml` posts, for one attempt of one CI run.
fn jobs_payload(commit_sha: &str, ci_run_id: &str) -> Value {
    json!({
        "repo": "andrewthetechie/writers-app",
        "provider": "github",
        "pipeline": ".github/workflows/ci.yml",
        "commit_sha": commit_sha,
        "branch": "main",
        "default_branch": "main",
        "ci_run_id": ci_run_id,
        "ci_run_attempt": 1,
        "jobs": [
            {
                "ci_job_id": "8812",
                "name": "test (ubuntu, 3.12)",
                "status": "failed",
                "url": "https://github.com/andrewthetechie/writers-app/actions/runs/42002/job/8812",
                "runner_name": "GitHub Actions 2",
                "runner_labels": ["ubuntu-latest"],
                "started_at": "2026-01-02T03:04:05Z",
                "completed_at": "2026-01-02T03:09:05Z",
                "summary": false
            },
            {
                "ci_job_id": "8813",
                "name": "alls-green",
                "status": "failed",
                "url": "",
                "runner_name": "",
                "runner_labels": [],
                "started_at": null,
                "completed_at": null,
                "summary": true
            }
        ]
    })
}

#[tokio::test]
async fn the_writers_app_jobs_upload_lands_as_job_executions() {
    let db = TestDb::new().await.expect("a test database");
    let app = app(&db, &settings_pairs());
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/ingest/pipeline")
        .header("x-api-key", TOKEN)
        .header("content-type", "application/json")
        .body(Body::from(jobs_payload("4f2c1d9", "42002").to_string()))
        .expect("the jobs upload builds");
    let (status, body) = send(&app, request).await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(body.get("status"), Some(&json!("pending")), "{body}");
    process_all(&db).await;

    let page = get(
        &app,
        "/api/jobs?repo=andrewthetechie%2Fwriters-app&include_stable=true",
    )
    .await;
    assert_eq!(page.status, 200);
    let page = page.json();
    let items = page["items"]
        .as_array()
        .unwrap_or_else(|| panic!("the Job leaderboard has items: {page}"));
    assert_eq!(items.len(), 2, "{page}");
    let job = items
        .iter()
        .find(|item| item["name"] == "test (ubuntu, 3.12)")
        .unwrap_or_else(|| panic!("the failing Job is listed: {page}"));
    assert_eq!(job["pipeline"], ".github/workflows/ci.yml");
    assert_eq!(job["last_status"], "failed");
    // No Report named this Job's Tests, so the failure is unexplained and is
    // the one that counts against its Flakiness score.
    let history = get(&app, &format!("/api/jobs/{}/history", job["id"])).await;
    assert_eq!(history.status, 200);
    let history = history.json();
    assert_eq!(history["unexplained_failures"], 1, "{history}");
    assert_eq!(history["explained_failures"], 0, "{history}");
    assert_eq!(history["executions"][0]["outcome"], "failed", "{history}");
    let summary = items
        .iter()
        .find(|item| item["name"] == "alls-green")
        .unwrap_or_else(|| panic!("the Summary Job is listed: {page}"));
    assert_eq!(summary["summary"], true);
    // A failed Summary Job is explained by the failed non-summary Job in the
    // same CI run attempt, so it does not count against its own score.
    let history = get(&app, &format!("/api/jobs/{}/history", summary["id"])).await;
    let history = history.json();
    assert_eq!(history["unexplained_failures"], 0, "{history}");
    assert_eq!(history["explained_failures"], 1, "{history}");
    assert_eq!(
        history["executions"][0]["outcome"], "explained",
        "{history}"
    );
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

/// Fabro's own view of a Test, copied from `flakeradar.rs` and `scout/flakeradar.rs`.
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
struct FabroExecution {
    status: String,
    message: String,
    details: String,
    created_at: String,
    branch: String,
    ci_run_id: String,
}

/// Fabro's view of a Test's history.
#[derive(Debug, Deserialize)]
struct FabroHistory {
    executions: Vec<FabroExecution>,
    last_failing_sha: Option<String>,
    last_failing_branch: Option<String>,
}

/// Fabro's view of one CI run's failures, as `scout/flakeradar.rs` reads it.
#[derive(Debug, Deserialize)]
struct FabroFailures {
    reports: FabroReportCounts,
    truncated: bool,
    failures: Vec<FabroFailure>,
    failed_jobs: Vec<FabroFailedJob>,
}

/// The Report counts Fabro polls for.
#[derive(Debug, Deserialize)]
struct FabroReportCounts {
    pending: i64,
    failed: i64,
}

/// One failed Test in a CI run.
#[derive(Debug, Deserialize)]
struct FabroFailure {
    name: String,
    project: String,
    ci_job_id: String,
    status: String,
    tier: String,
    flakiness_score: f64,
    confirmed_flake_count: i64,
    failure_category: Option<String>,
    message: String,
    github_issue_number: Option<i64>,
}

/// One failed Job in a CI run.
#[derive(Debug, Deserialize)]
struct FabroFailedJob {
    ci_job_id: String,
    outcome: String,
    job_id: i64,
}

#[tokio::test]
async fn fabro_reads_every_field_of_the_test_reads_through_the_writers_app_payloads() {
    let db = TestDb::new().await.expect("a test database");
    let app = app(&db, &settings_pairs());
    // The same commit twice: the Test passes in the first Report and fails in
    // the second, which is a Proven flake and puts it in the flaky tier.
    upload_as_the_report_action(
        &app,
        "andrewthetechie/writers-app",
        "4f2c1d9",
        "42001",
        &writers_app_junit(&[
            ("test_a_writer_keeps_its_name", false),
            ("test_a_writer_rejects_a_duplicate_slug", false),
        ]),
    )
    .await;
    let failing_id = upload_as_the_report_action(
        &app,
        "andrewthetechie/writers-app",
        "4f2c1d9",
        "42002",
        &writers_app_junit(&[
            ("test_a_writer_keeps_its_name", true),
            ("test_a_writer_rejects_a_duplicate_slug", false),
        ]),
    )
    .await;
    process_all(&db).await;
    assert_eq!(
        get(&app, &format!("/api/reports/{failing_id}"))
            .await
            .json()["status"],
        "processed"
    );

    let page = get(&app, "/api/tests?include_stable=true").await.json();
    let items: Vec<FabroTest> =
        serde_json::from_value(page["items"].clone()).expect("Fabro's struct reads the page");
    assert_eq!(items.len(), 2, "{page}");
    for item in &items {
        assert!(item.id > 0);
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
    let flaked = items
        .iter()
        .find(|item| item.name == "test_a_writer_keeps_its_name")
        .unwrap_or_else(|| panic!("the Test that flipped is listed"));
    assert_eq!(flaked.confirmed_flake_count, 1);
    assert_eq!(flaked.tier, "flaky");
    assert!(flaked.flakiness_score > 0.0);

    let history: FabroHistory = serde_json::from_value(
        get(&app, &format!("/api/tests/{}/history", flaked.id))
            .await
            .json(),
    )
    .expect("Fabro's struct reads the history");
    assert_eq!(history.executions.len(), 2, "{history:?}");
    for execution in &history.executions {
        assert!(
            fabro_accepts(&execution.created_at),
            "{}",
            execution.created_at
        );
        assert_ne!(execution.status, "");
        assert_ne!(execution.branch, "");
        let _ = (&execution.message, &execution.details, &execution.ci_run_id);
    }
    assert_eq!(history.last_failing_sha.as_deref(), Some("4f2c1d9"));
    assert_eq!(history.last_failing_branch.as_deref(), Some("main"));
}

#[tokio::test]
async fn fabro_reads_a_ci_runs_failures_through_the_writers_app_payloads() {
    let db = TestDb::new().await.expect("a test database");
    let app = app(&db, &settings_pairs());
    // The same commit passes first and then fails, so the CI run's failure is a
    // Test FlakeRadar already knows as flaky: the case that decides a merge.
    upload_as_the_report_action(
        &app,
        "andrewthetechie/writers-app",
        "4f2c1d9",
        "42001",
        &writers_app_junit(&[("test_a_writer_keeps_its_name", false)]),
    )
    .await;
    let failing_id = upload_as_the_report_action(
        &app,
        "andrewthetechie/writers-app",
        "4f2c1d9",
        "42002",
        &writers_app_junit(&[("test_a_writer_keeps_its_name", true)]),
    )
    .await;
    process_all(&db).await;
    assert_eq!(
        get(&app, &format!("/api/reports/{failing_id}"))
            .await
            .json()["status"],
        "processed"
    );

    let failures: FabroFailures = serde_json::from_value(
        get(
            &app,
            "/api/ci-runs/42002/failures?repo=andrewthetechie%2Fwriters-app",
        )
        .await
        .json(),
    )
    .expect("Fabro's struct reads the failures");
    assert_eq!(failures.reports.pending, 0);
    assert_eq!(failures.reports.failed, 0);
    assert!(!failures.truncated);
    assert_eq!(failures.failures.len(), 1, "{failures:?}");
    let failure = &failures.failures[0];
    assert_eq!(failure.name, "test_a_writer_keeps_its_name");
    assert_eq!(failure.project, "backend");
    assert_eq!(failure.ci_job_id, "8812");
    assert_eq!(failure.status, "failed");
    assert_eq!(failure.tier, "flaky");
    assert_eq!(failure.confirmed_flake_count, 1);
    assert_eq!(failure.failure_category.as_deref(), Some("assertion"));
    assert!(failure.message.starts_with("AssertionError"));
    assert_eq!(failure.github_issue_number, None);
    assert!(failure.flakiness_score >= 0.0);
    for job in &failures.failed_jobs {
        assert_ne!(job.ci_job_id, "");
        assert_ne!(job.outcome, "");
        assert!(job.job_id > 0);
    }
    assert!(failures.failed_jobs.is_empty(), "{failures:?}");
}

#[test]
fn fabro_rejects_a_timestamp_without_a_zone() {
    assert!(fabro_accepts("2026-01-02T03:04:05Z"));
    assert!(fabro_accepts("2026-01-02T03:04:05.000000Z"));
    assert!(fabro_accepts("2026-01-02T03:04:05+00:00"));
    assert!(!fabro_accepts("2026-01-02T03:04:05"));
    assert!(!fabro_accepts("2026-01-02T03:04:05."));
}
