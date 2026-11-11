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

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test that cannot serve, upload, process or run the script has failed"
)]

use std::path::{Path, PathBuf};

use axum::Router;
use flakeradar_db::processing::ProcessingSettings;
use flakeradar_test_support::{TestDb, app, post};

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

#[tokio::test]
async fn gate_off_writes_the_table_and_never_fails_the_job() {
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
