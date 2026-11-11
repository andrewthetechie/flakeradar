//! `actions/summary/summary.sh` run as the CI step runs it.
//!
//! The other suites call the router; this one starts the real script as a child
//! process, pointed at the real app served on a loopback port, with a JUnit
//! report uploaded and processed the way `actions/report/report.sh` does. What a
//! case asserts is therefore what the step leaves behind: the exit status, the
//! lines the script printed, and the file it wrote.
//!
//! `serve`, `upload`, `process_all` and `run_summary` are the seams the gate's
//! cases share, so they hold the whole path: an upload the worker has drained, a
//! server the script can poll, and a child with only the `FR_*` variables the
//! action passes.
//!
//! Every case name starts with `summary_action`, because a nextest filter reads
//! test names and not file names: that prefix is what makes
//! `run_tests rust summary_action` select exactly this file's cases.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test that cannot serve, upload, process or run the script has failed"
)]

use std::path::{Path, PathBuf};

use axum::Router;
use flakeradar_db::processing::ProcessingSettings;
use flakeradar_test_support::{TestDb, app, get, post};
use serde_json::{Value, json};

/// The token the app requires and the script presents as `FR_TOKEN`.
const TOKEN: &str = "summary-token";

/// The Repo the upload names and the script asks about, as `FR_REPO`.
const REPO: &str = "acme/app";

/// The CI run every case uploads into and asks the script about, as `FR_RUN_ID`.
const RUN_ID: &str = "7001";

/// The attempt of that run, as `FR_ATTEMPT`.
const ATTEMPT: &str = "1";

/// The job the failures are attributed to and the script filters on, as `FR_JOB_ID`.
const JOB_ID: &str = "8812";

/// The sleep between polls: the report is processed before the script runs, so the
/// first answer is final and no case should ever sleep.
const POLL_INTERVAL: &str = "0";

/// The app under test, with this file's token.
///
/// A case calls `post` and `get` on its own handle, while [`serve`] puts a second
/// handle behind the socket the script reaches. Both read the same database, so the
/// report one uploads is the failure listing the other answers.
fn router(db: &TestDb) -> Router {
    app(db, &[("FLAKERADAR_API_TOKEN", TOKEN)])
}

/// Serves [`router`] on a loopback port the kernel picks.
///
/// Answers its base URL, such as `http://127.0.0.1:41023`, and the task, which the
/// case aborts when it ends.
async fn serve(db: &TestDb) -> (String, tokio::task::JoinHandle<()>) {
    let app = router(db);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let address = listener.local_addr().expect("the bound address");
    let serving = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}"), serving)
}

/// Uploads one JUnit report the way `actions/report/report.sh` does, for [`REPO`],
/// [`RUN_ID`] and [`JOB_ID`]: the query the script builds, in its order.
async fn upload(app: &Router, junit: &str) {
    let query = format!(
        "repo={REPO}&commit_sha=aaa111&branch=main&project=backend&ci_run_id={RUN_ID}&\
         ci_run_attempt={ATTEMPT}&ci_job_id={JOB_ID}&pipeline=.github%2Fworkflows%2Fci.yml&\
         default_branch=main&root=backend&line_base=0"
    );
    let reply = post(
        app,
        &format!("/api/ingest?{query}"),
        &[("x-api-key", TOKEN), ("content-type", "application/xml")],
        junit.to_owned(),
    )
    .await;
    assert_eq!(reply.status, 202, "{}", reply.json());
}

/// Drains the queue the way the worker does, because [`router`] starts none.
async fn process_all(db: &TestDb) {
    let settings = ProcessingSettings::default();
    while flakeradar_db::processing::process_next(db.connection(), &settings)
        .await
        .expect("processing a queued Report succeeds")
        .is_some()
    {}
}

/// The script every case runs, beside this crate in the CI action's own directory.
fn summary_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../actions/summary/summary.sh")
}

/// A summary file named for `case` under the temporary directory.
///
/// The script appends to `FR_SUMMARY_FILE`, so anything an earlier run left behind
/// would let a case read output it did not write.
fn summary_file(case: &str) -> String {
    let path = std::env::temp_dir().join(format!("flakeradar-summary-action-{case}.md"));
    // A file that is not there yet is the expected case, so its error is nobody's.
    let _ = std::fs::remove_file(&path);
    path.to_string_lossy().into_owned()
}

/// Runs `actions/summary/summary.sh` as the CI step does, against `base_url`.
///
/// The child gets only the variables the action's `env:` block passes, plus `PATH`
/// so it finds `bash`, `curl` and `jq`: clearing the rest keeps `GITHUB_STEP_SUMMARY`
/// off the run, which is what makes `summary_file` the only summary target.
/// `timeout` is the script's `FR_TIMEOUT`, and the unreachable case needs a `0`
/// deadline. Answers the exit code and stdout plus stderr.
///
/// `.output()` blocks until the script exits and the script polls the app this test
/// serves, so it runs on a blocking thread rather than the runtime one that answers
/// it.
async fn run_summary(
    base_url: &str,
    gate: &str,
    test_outcome: &str,
    timeout: &str,
    summary_file: &str,
) -> (i32, String) {
    let script = summary_script();
    let base_url = base_url.to_owned();
    let gate = gate.to_owned();
    let outcome = test_outcome.to_owned();
    let timeout = timeout.to_owned();
    let file = summary_file.to_owned();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new("bash")
            .arg(script)
            .env_clear()
            .env("PATH", path)
            .env("FR_URL", base_url)
            .env("FR_TOKEN", TOKEN)
            .env("FR_REPO", REPO)
            .env("FR_RUN_ID", RUN_ID)
            .env("FR_ATTEMPT", ATTEMPT)
            .env("FR_JOB_ID", JOB_ID)
            .env("FR_TIMEOUT", timeout)
            .env("FR_POLL_INTERVAL", POLL_INTERVAL)
            .env("FR_SUMMARY_FILE", file)
            .env("FR_GATE", gate)
            .env("FR_TEST_OUTCOME", outcome)
            .output()
    })
    .await
    .expect("the summary script ran on a blocking thread")
    .unwrap_or_else(|error| panic!("the summary script could not be started: {error}"));
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let code = output
        .status
        .code()
        .unwrap_or_else(|| panic!("the summary script exited with a code: {printed}"));
    (code, printed)
}

/// One report with two failed Tests in job [`JOB_ID`], each failing for the first
/// time: nothing here is a Test FlakeRadar already knows as flaky.
fn two_failures_junit() -> &'static str {
    r#"<?xml version="1.0" encoding="utf-8"?>
<testsuites>
  <testsuite name="pytest" errors="0" failures="2" skipped="0" tests="2" time="1.412">
    <testcase classname="tests.test_app" name="test_alpha" time="0.512">
      <failure message="AssertionError: boom">AssertionError</failure>
    </testcase>
    <testcase classname="tests.test_app" name="test_beta" time="0.902">
      <failure message="AssertionError: boom">AssertionError</failure>
    </testcase>
  </testsuite>
</testsuites>
"#
}

/// One report whose only Test failed once and then passed on retry, so the Run
/// holds both a failed and a passed Execution of it and its newest status is
/// `passed`. A Test that passed on retry is not a failure of the job.
fn passed_on_retry_junit() -> &'static str {
    r#"<?xml version="1.0" encoding="utf-8"?>
<testsuites>
  <testsuite name="pytest" errors="0" failures="0" skipped="0" tests="1" time="1.412">
    <testcase classname="tests.test_app" name="test_retry" time="0.512">
      <flakyFailure message="AssertionError: boom">AssertionError</flakyFailure>
    </testcase>
  </testsuite>
</testsuites>
"#
}

/// The failures endpoint's entry for the Test called `name` in this run: the same
/// listing the script reads, so a case inspects and quarantines exactly the
/// failures the gate will look at.
async fn listed_failure(app: &Router, name: &str) -> Value {
    let listing = get(
        app,
        &format!("/api/ci-runs/{RUN_ID}/failures?repo={REPO}&ci_run_attempt={ATTEMPT}"),
    )
    .await
    .json();
    listing
        .get("failures")
        .and_then(Value::as_array)
        .and_then(|failures| {
            failures
                .iter()
                .find(|failure| failure.get("name").and_then(Value::as_str) == Some(name))
                .cloned()
        })
        .unwrap_or_else(|| panic!("{name} is not a listed failure: {listing}"))
}

/// Quarantines the Test called `name`, the way the dashboard does.
async fn quarantine(app: &Router, name: &str) {
    let test_id = listed_failure(app, name)
        .await
        .get("test_id")
        .and_then(Value::as_i64)
        .expect("the listing carries a test_id");
    let reply = post(
        app,
        &format!("/api/tests/{test_id}/quarantine"),
        &[("content-type", "application/json")],
        json!({"quarantined": true}).to_string(),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", reply.json());
}

#[tokio::test]
async fn summary_action_gate_on_exits_zero_when_every_failure_is_quarantined() {
    // The whole point of gate mode: a team that quarantines its flaky Tests keeps
    // the build green, and still gets the table.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    upload(&app, two_failures_junit()).await;
    process_all(&db).await;
    for name in ["test_alpha", "test_beta"] {
        quarantine(&app, name).await;
    }
    let file = summary_file("gate_on_all_quarantined");

    let (code, output) = run_summary(&base_url, "true", "failure", "5", &file).await;

    assert_eq!(code, 0, "a quarantined failure is not the job's: {output}");
    let written = std::fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("reading {file}: {error}\n{output}"));
    // The table is what the step exists to write, and the gate runs only after it:
    // neither exit may skip it. Quarantine itself is not what makes a failure a
    // known flaky one, so the headline still counts zero.
    assert!(
        written.contains("0 of 2 failures are known flaky tests"),
        "the table is written before the gate decides: {written}"
    );
    for name in ["test_alpha", "test_beta"] {
        let row = written
            .lines()
            .find(|line| line.contains(name))
            .unwrap_or_else(|| panic!("{name} is not in the table: {written}"));
        assert!(row.contains("| yes |"), "{name} is not quarantined: {row}");
    }
    serving.abort();
}

#[tokio::test]
async fn summary_action_gate_on_exits_one_and_names_an_unquarantined_failure() {
    // Quarantine covers one Test, not its jobmate: the gate fails and says which
    // Test it failed on.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    upload(&app, two_failures_junit()).await;
    process_all(&db).await;
    quarantine(&app, "test_alpha").await;
    let file = summary_file("gate_on_one_unquarantined");

    let (code, output) = run_summary(&base_url, "true", "failure", "5", &file).await;

    assert_eq!(code, 1, "an unquarantined failure fails the gate: {output}");
    assert!(
        output.contains("::error::") && output.contains("test_beta"),
        "one error line names the Test: {output}"
    );
    assert!(
        !output.contains("test_alpha"),
        "the quarantined Test is not named: {output}"
    );
    let written = std::fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("reading {file}: {error}\n{output}"));
    assert!(
        written.contains("0 of 2 failures are known flaky tests"),
        "the table is still written before the gate fails: {written}"
    );
    serving.abort();
}

#[tokio::test]
async fn summary_action_gate_on_ignores_a_failure_that_passed_on_retry() {
    // The one entry this job has passed on retry, so it is not a failure of the
    // job. `outcome` is `success` and then `failure`: only the second run proves
    // the exit came from the passed-on-retry rule, since the gate mode that cannot
    // decide would exit 1 for a failed test step.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    upload(&app, passed_on_retry_junit()).await;
    process_all(&db).await;
    // The listing does hold the Test, with `quarantined` false: the gate's answer
    // comes from its status, not from the failure being absent.
    let retry = listed_failure(&app, "test_retry").await;
    assert_eq!(retry["status"], "passed", "{retry}");
    assert_eq!(retry["quarantined"], false, "{retry}");

    for outcome in ["success", "failure"] {
        let file = summary_file(&format!("gate_on_passed_on_retry_{outcome}"));
        let (code, output) = run_summary(&base_url, "true", outcome, "5", &file).await;
        assert_eq!(
            code, 0,
            "a Test that passed on retry with outcome {outcome}: {output}"
        );
        let written = std::fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("reading {file}: {error}\n{output}"));
        assert!(
            written.contains("passed on retry"),
            "the table still lists the retry: {written}"
        );
    }
    serving.abort();
}

#[tokio::test]
async fn summary_action_gate_on_without_a_summary_target_still_decides() {
    // A workflow that runs the gate without a summary target (no step summary, no
    // `summary-file`) used to leave the script at its early exit before it ever
    // polled. The decision is the gate's whole point, so it must still happen.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    upload(&app, two_failures_junit()).await;
    process_all(&db).await;
    quarantine(&app, "test_alpha").await;

    // An empty `summary-file` and a cleared environment mean no target at all.
    let (code, output) = run_summary(&base_url, "true", "failure", "5", "").await;
    assert_eq!(
        code, 1,
        "the gate polls and decides with nothing to write to: {output}"
    );
    assert!(
        output.contains("::error::") && output.contains("test_beta"),
        "one error line names the Test: {output}"
    );

    // With the gate off, that early exit is the unchanged behaviour.
    let (code, output) = run_summary(&base_url, "false", "failure", "5", "").await;
    assert_eq!(code, 0, "the gate-off early exit stays: {output}");
    assert!(
        output.contains("no summary target"),
        "the notice still says why: {output}"
    );
    serving.abort();
}

#[tokio::test]
async fn summary_action_gate_off_writes_the_table_and_never_fails_the_job() {
    // With `gate` false the step is the one every workflow already runs: whatever
    // the failures are, it writes the summary and exits 0.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    upload(&app, two_failures_junit()).await;
    process_all(&db).await;
    let file = summary_file("gate_off_writes_the_table_and_never_fails_the_job");

    let (code, output) = run_summary(&base_url, "false", "failure", "5", &file).await;

    assert_eq!(
        code, 0,
        "an unquarantined failure never fails the step: {output}"
    );
    let written = std::fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("reading {file}: {error}\n{output}"));
    // Two first-ever failures are two new failures, and the table names both, so
    // the script really read this job's failures off the served app.
    assert!(
        written.contains("0 of 2 failures are known flaky tests"),
        "the headline counts only the known flakes: {written}"
    );
    for name in ["test_alpha", "test_beta"] {
        assert!(
            written.contains(name),
            "{name} is not in the table: {written}"
        );
    }
    serving.abort();
}
