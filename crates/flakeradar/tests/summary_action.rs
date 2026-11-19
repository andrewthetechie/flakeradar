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
use flakeradar_test_support::{TestDb, app, get, make_project, make_report, post};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

/// A second job of the same run and attempt, whose failures this job's table and
/// gate must ignore.
const OTHER_JOB_ID: &str = "8813";

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

/// Uploads one JUnit report the way `actions/report/report.sh` does, for [`REPO`]
/// and [`RUN_ID`], attributed to the job id the caller passes: the query the script
/// builds, in its order.
async fn upload_as(app: &Router, junit: &str, job_id: &str) {
    let query = format!(
        "repo={REPO}&commit_sha=aaa111&branch=main&project=backend&ci_run_id={RUN_ID}&\
         ci_run_attempt={ATTEMPT}&ci_job_id={job_id}&pipeline=.github%2Fworkflows%2Fci.yml&\
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

/// Uploads one JUnit report attributed to [`JOB_ID`], via [`upload_as`].
async fn upload(app: &Router, junit: &str) {
    upload_as(app, junit, JOB_ID).await;
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

/// An event JSON file named for `case` under the temporary directory, holding
/// `json`: what the runner puts at `GITHUB_EVENT_PATH`.
fn event_file(case: &str, json: &str) -> String {
    let path = std::env::temp_dir().join(format!("flakeradar-summary-action-{case}.json"));
    std::fs::write(&path, json).expect("writing the event file succeeds");
    path.to_string_lossy().into_owned()
}

/// Runs `actions/summary/summary.sh` as the CI step does, against `base_url`.
///
/// The child gets only the variables the action's `env:` block passes, plus `PATH`
/// so it finds `bash`, `curl` and `jq`: clearing the rest keeps `GITHUB_STEP_SUMMARY`
/// off the run, which is what makes `summary_file` the only summary target.
/// `extra_env` carries the runner-provided variables a case needs, such as
/// `GITHUB_EVENT_PATH` and `GITHUB_API_URL` — after the clear they are the only
/// way those reach the script. `timeout` is the script's `FR_TIMEOUT`, and the
/// unreachable case needs a `0` deadline. Answers the exit code and stdout plus
/// stderr.
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
    extra_env: &[(&str, &str)],
) -> (i32, String) {
    let script = summary_script();
    let base_url = base_url.to_owned();
    let gate = gate.to_owned();
    let outcome = test_outcome.to_owned();
    let timeout = timeout.to_owned();
    let file = summary_file.to_owned();
    let extra: Vec<(String, String)> = extra_env
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let output = tokio::task::spawn_blocking(move || {
        let mut command = std::process::Command::new("bash");
        command
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
            .env("FR_TEST_OUTCOME", outcome);
        for (key, value) in &extra {
            command.env(key, value);
        }
        command.output()
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

/// One report whose only Test passed: the smallest job with no failures to talk
/// about, for the comment cases that must update or stay silent.
fn all_passing_junit() -> &'static str {
    r#"<?xml version="1.0" encoding="utf-8"?>
<testsuites>
  <testsuite name="pytest" errors="0" failures="0" skipped="0" tests="1" time="1.412">
    <testcase classname="tests.test_app" name="test_ok" time="0.902"/>
  </testsuite>
</testsuites>
"#
}

/// One report with a single failed Test in job [`JOB_ID`], failing for the first
/// time: the smallest job that still has a failure to talk about.
fn one_failure_junit() -> &'static str {
    r#"<?xml version="1.0" encoding="utf-8"?>
<testsuites>
  <testsuite name="pytest" errors="0" failures="1" skipped="0" tests="1" time="1.412">
    <testcase classname="tests.test_app" name="test_beta" time="0.902">
      <failure message="AssertionError: boom">AssertionError</failure>
    </testcase>
  </testsuite>
</testsuites>
"#
}

/// One report with a single failed Test, named so it appears nowhere else in this
/// file: the shape [`upload_as`] attributes to [`OTHER_JOB_ID`].
fn other_job_failure_junit() -> &'static str {
    r#"<?xml version="1.0" encoding="utf-8"?>
<testsuites>
  <testsuite name="pytest" errors="0" failures="1" skipped="0" tests="1" time="1.412">
    <testcase classname="tests.test_app" name="test_gamma" time="0.902">
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

    let (code, output) = run_summary(&base_url, "true", "failure", "5", &file, &[]).await;

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

    let (code, output) = run_summary(&base_url, "true", "failure", "5", &file, &[]).await;

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
        let (code, output) = run_summary(&base_url, "true", outcome, "5", &file, &[]).await;
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
    let (code, output) = run_summary(&base_url, "true", "failure", "5", "", &[]).await;
    assert_eq!(
        code, 1,
        "the gate polls and decides with nothing to write to: {output}"
    );
    assert!(
        output.contains("::error::") && output.contains("test_beta"),
        "one error line names the Test: {output}"
    );

    // With the gate off, that early exit is the unchanged behaviour.
    let (code, output) = run_summary(&base_url, "false", "failure", "5", "", &[]).await;
    assert_eq!(code, 0, "the gate-off early exit stays: {output}");
    assert!(
        output.contains("no summary target"),
        "the notice still says why: {output}"
    );
    serving.abort();
}

#[tokio::test]
async fn summary_action_gate_true_and_unreachable_follows_the_test_outcome() {
    // FlakeRadar cannot answer for itself, so the test step decides: its failure
    // keeps the gate red, and its success keeps it green. The unreachable line is
    // still the summary the reader gets.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let address = listener.local_addr().expect("the bound address");
    drop(listener);
    let base_url = format!("http://{address}");

    for (outcome, expected) in [("failure", 1), ("success", 0)] {
        let file = summary_file(&format!("unreachable_{outcome}"));
        let (code, output) = run_summary(&base_url, "true", outcome, "0", &file, &[]).await;
        assert_eq!(
            code, expected,
            "unreachable with outcome {outcome} decides on the test step: {output}"
        );
        assert!(
            output.contains("::notice::FlakeRadar gate: unreachable"),
            "one notice names the rule that fell back: {output}"
        );
        let written = std::fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("reading {file}: {error}\n{output}"));
        assert!(
            written.contains("**FlakeRadar:** could not reach FlakeRadar"),
            "the unreachable line is still written: {written}"
        );
    }

    // With the gate off this is the old read-only step again: exit 0.
    let file = summary_file("unreachable_gate_off");
    let (code, output) = run_summary(&base_url, "false", "failure", "0", &file, &[]).await;
    assert_eq!(code, 0, "the gate-off unreachable path stays: {output}");
    assert!(
        !output.contains("::notice::FlakeRadar gate"),
        "nothing falls back when nothing was gated: {output}"
    );

    // And with no summary target at all, the early exit must not skip the decision:
    // the fallback still happens, because it is the only thing gate mode has.
    let (code, output) = run_summary(&base_url, "true", "failure", "0", "", &[]).await;
    assert_eq!(
        code, 1,
        "the fallback decides with nothing to write to: {output}"
    );
    assert!(
        output.contains("::notice::FlakeRadar gate: unreachable"),
        "one notice names the rule that fell back: {output}"
    );
}

#[tokio::test]
async fn summary_action_gate_true_and_timeout_follows_the_test_outcome() {
    // The Report is uploaded and never processed, so the answer still says
    // `pending` is above 0 at the deadline and the timeout rule decides.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    upload(&app, two_failures_junit()).await;

    for (outcome, expected) in [("failure", 1), ("success", 0)] {
        let file = summary_file(&format!("timeout_{outcome}"));
        let (code, output) = run_summary(&base_url, "true", outcome, "0", &file, &[]).await;
        assert_eq!(
            code, expected,
            "timeout with outcome {outcome} decides on the test step: {output}"
        );
        assert!(
            output.contains("::notice::FlakeRadar gate: timeout"),
            "one notice names the rule that fell back: {output}"
        );
        let written = std::fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("reading {file}: {error}\n{output}"));
        assert!(
            written.contains("**FlakeRadar:** timed out after 0s"),
            "the timeout line is still written: {written}"
        );
    }
    serving.abort();
}

#[tokio::test]
async fn summary_action_gate_true_and_incomplete_answer_follows_the_test_outcome() {
    // A Report that failed to process is counted in `reports.failed` and lists no
    // failures, so the answer cannot settle the question even though nothing is
    // pending: the test step decides, and the empty table does not clear the job.
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo(REPO).create(&db).await;
    make_report(&project, project.repo_id)
        .body(b"<not xml")
        .ci_run_id(RUN_ID)
        .ci_run_attempt(1)
        .status("failed")
        .error("Not a valid JUnit XML report")
        .create(&db)
        .await;
    let (base_url, serving) = serve(&db).await;

    for (outcome, expected) in [("failure", 1), ("success", 0)] {
        let file = summary_file(&format!("incomplete_{outcome}"));
        let (code, output) = run_summary(&base_url, "true", outcome, "5", &file, &[]).await;
        assert_eq!(
            code, expected,
            "an incomplete answer with outcome {outcome} decides on the test step: {output}"
        );
        assert!(
            output.contains("::notice::FlakeRadar gate: the failures list is incomplete"),
            "one notice names the rule that fell back: {output}"
        );
        let written = std::fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("reading {file}: {error}\n{output}"));
        assert!(
            written.contains("**FlakeRadar:** no failed tests reported for this CI job."),
            "the table is still written before the fallback: {written}"
        );
    }
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

    let (code, output) = run_summary(&base_url, "false", "failure", "5", &file, &[]).await;

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

#[tokio::test]
async fn summary_action_ignores_another_jobs_failure_in_the_table_and_the_gate() {
    // The table, the comment rule and the gate each apply the same job filter; the
    // comment at `summary.sh:278-279` says they agree, and this case is what makes
    // that structural: a second job's failure in the same run and attempt must
    // appear in neither this job's table nor this job's gate decision.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    upload(&app, one_failure_junit()).await;
    upload_as(&app, other_job_failure_junit(), OTHER_JOB_ID).await;
    process_all(&db).await;
    let file = summary_file("second_job");

    let (code, output) = run_summary(&base_url, "true", "failure", "5", &file, &[]).await;

    assert_eq!(
        code, 1,
        "this job's own failure still fails the gate: {output}"
    );
    assert!(
        output.contains("::error::") && output.contains("test_beta"),
        "the gate names this job's Test: {output}"
    );
    assert!(
        !output.contains("test_gamma"),
        "the other job's Test is not named: {output}"
    );
    let written = std::fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("reading {file}: {error}\n{output}"));
    // One row, not two: the other job's failure never reaches the table.
    assert!(
        written.contains("0 of 1 failures are known flaky tests"),
        "the headline counts only this job's failure: {written}"
    );
    assert!(
        written.contains("test_beta"),
        "this job's Test is in the table: {written}"
    );
    assert!(
        !written.contains("test_gamma"),
        "the other job's Test is not in the table: {written}"
    );

    // The other discriminating branch: with no job filter, both jobs' failures are
    // this job's business, so the empty `$job == ""` side of the predicate shows
    // them all.
    let file_all = summary_file("second_job_empty_filter");
    let (code_all, output_all) = run_summary(
        &base_url,
        "false",
        "failure",
        "5",
        &file_all,
        &[("FR_JOB_ID", "")],
    )
    .await;
    assert_eq!(code_all, 0, "the gate is off here: {output_all}");
    let written_all = std::fs::read_to_string(&file_all)
        .unwrap_or_else(|error| panic!("reading {file_all}: {error}\n{output_all}"));
    for name in ["test_beta", "test_gamma"] {
        assert!(
            written_all.contains(name),
            "{name} is in the unfiltered table: {written_all}"
        );
    }
    serving.abort();
}

/// The stub every skip case points `GITHUB_API_URL` at: nothing is mounted, so
/// any request the script makes is one it must not have made, and
/// `received_requests` sees it all the same.
///
/// The runner variables the comment decision reads ride in on [`run_summary`]'s
/// `extra_env`, because the action's own `env:` block passes only the inputs.
#[tokio::test]
async fn summary_action_pr_comment_sends_nothing_for_a_fork_pr() {
    // A fork's head repository is not the base one the token is scoped to, so the
    // comment does not apply to this run: the step still polls, still writes the
    // same job summary, and still exits 0 — it just sends nothing.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    let server = MockServer::start().await;
    upload(&app, one_failure_junit()).await;
    process_all(&db).await;
    let file = summary_file("pr_comment_fork");
    let event = event_file(
        "fork",
        r#"{"pull_request":{"number":12,"head":{"repo":{"full_name":"someone/app"}},"base":{"repo":{"full_name":"acme/app"}}}}"#,
    );

    let (code, output) = run_summary(
        &base_url,
        "false",
        "failure",
        "5",
        &file,
        &[
            ("GITHUB_API_URL", &server.uri()),
            ("FR_PR_COMMENT", "true"),
            ("FR_GITHUB_TOKEN", "gh-token"),
            ("FR_COMMENT_KEY", "ci/test"),
            ("GITHUB_EVENT_PATH", &event),
        ],
    )
    .await;

    assert_eq!(code, 0, "a fork PR never fails the step: {output}");
    let written = std::fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("reading {file}: {error}\n{output}"));
    assert!(
        written.contains("0 of 1 failures are known flaky tests"),
        "the summary is still written with the comment on: {written}"
    );
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert!(
        requests.is_empty(),
        "a fork PR must send nothing: {} requests",
        requests.len()
    );
    serving.abort();
}

#[tokio::test]
async fn summary_action_pr_comment_sends_nothing_without_a_pull_request_event() {
    // A push (or any non pull_request event) has no pull request to comment on:
    // nothing is sent, and the step stays read-only.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    let server = MockServer::start().await;
    upload(&app, one_failure_junit()).await;
    process_all(&db).await;
    let file = summary_file("pr_comment_not_a_pr");
    let event = event_file("not_a_pr", r#"{"ref":"refs/heads/main"}"#);

    let (code, output) = run_summary(
        &base_url,
        "false",
        "failure",
        "5",
        &file,
        &[
            ("GITHUB_API_URL", &server.uri()),
            ("FR_PR_COMMENT", "true"),
            ("FR_GITHUB_TOKEN", "gh-token"),
            ("FR_COMMENT_KEY", "ci/test"),
            ("GITHUB_EVENT_PATH", &event),
        ],
    )
    .await;

    assert_eq!(
        code, 0,
        "an event without a pull_request never fails: {output}"
    );
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert!(
        requests.is_empty(),
        "an event without a pull_request must send nothing: {} requests",
        requests.len()
    );
    serving.abort();
}

#[tokio::test]
async fn summary_action_pr_comment_sends_nothing_when_it_is_off() {
    // The default: `pr-comment` false sends nothing to GitHub, whatever the event
    // says — a workflow that never asked for comments gets none.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    let server = MockServer::start().await;
    upload(&app, one_failure_junit()).await;
    process_all(&db).await;
    let file = summary_file("pr_comment_off");
    let event = event_file(
        "off",
        r#"{"pull_request":{"number":12,"head":{"repo":{"full_name":"acme/app"}},"base":{"repo":{"full_name":"acme/app"}}}}"#,
    );

    let (code, output) = run_summary(
        &base_url,
        "false",
        "failure",
        "5",
        &file,
        &[
            ("GITHUB_API_URL", &server.uri()),
            ("FR_PR_COMMENT", "false"),
            ("FR_GITHUB_TOKEN", "gh-token"),
            ("FR_COMMENT_KEY", "ci/test"),
            ("GITHUB_EVENT_PATH", &event),
        ],
    )
    .await;

    assert_eq!(code, 0, "the default never fails the step: {output}");
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert!(
        requests.is_empty(),
        "pr-comment off must send nothing: {} requests",
        requests.len()
    );
    serving.abort();
}

/// The `pull_request` event of a non-fork run: number 12, head and base repo
/// [`REPO`] — the event file every send case points `GITHUB_EVENT_PATH` at.
const PR_EVENT: &str = r#"{"pull_request":{"number":12,"head":{"repo":{"full_name":"acme/app"}},"base":{"repo":{"full_name":"acme/app"}}}}"#;

/// The one request in `requests` that hit `verb` on `route`, if any.
fn sent<'a>(
    requests: &'a [wiremock::Request],
    verb: &str,
    route: &str,
) -> Option<&'a wiremock::Request> {
    requests
        .iter()
        .find(|request| request.method.as_str() == verb && request.url.path() == route)
}

#[tokio::test]
async fn summary_action_pr_comment_posts_a_new_comment_when_none_exists() {
    // A PR event with failures and no marked comment yet: the script posts the
    // marked Markdown once, and the marker line is what a re-run will find.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    let server = MockServer::start().await;
    upload(&app, one_failure_junit()).await;
    process_all(&db).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 777})))
        .expect(1)
        .mount(&server)
        .await;
    let file = summary_file("pr_comment_new");
    let event = event_file("new", PR_EVENT);

    let (code, output) = run_summary(
        &base_url,
        "false",
        "failure",
        "5",
        &file,
        &[
            ("GITHUB_API_URL", &server.uri()),
            ("FR_PR_COMMENT", "true"),
            ("FR_GITHUB_TOKEN", "gh-token"),
            ("FR_COMMENT_KEY", "ci/test"),
            ("GITHUB_EVENT_PATH", &event),
        ],
    )
    .await;

    assert_eq!(code, 0, "a created comment never fails the step: {output}");
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    let posted = sent(&requests, "POST", "/repos/acme/app/issues/12/comments")
        .expect("the comment was posted");
    let body: Value = serde_json::from_slice(&posted.body).expect("the body is JSON");
    let comment = body["body"].as_str().expect("the comment carries a body");
    assert!(
        comment.starts_with("<!-- flakeradar-summary:ci/test -->"),
        "the marker is the comment's first line: {comment}"
    );
    assert!(
        comment.contains("test_beta"),
        "the comment is this job's failure table: {comment}"
    );
    serving.abort();
}

#[tokio::test]
async fn summary_action_pr_comment_updates_the_existing_marked_comment() {
    // A re-run finds its earlier comment by the marker and edits it instead of
    // piling up a second one: one PATCH to that id, never a POST.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    let server = MockServer::start().await;
    upload(&app, one_failure_junit()).await;
    process_all(&db).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 555, "body": "<!-- flakeradar-summary:ci/test -->\nold"},
            {"id": 556, "body": "someone else's review comment"}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/comments/555"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 555})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 777})))
        .expect(0)
        .mount(&server)
        .await;
    let file = summary_file("pr_comment_update");
    let event = event_file("update", PR_EVENT);

    let (code, output) = run_summary(
        &base_url,
        "false",
        "failure",
        "5",
        &file,
        &[
            ("GITHUB_API_URL", &server.uri()),
            ("FR_PR_COMMENT", "true"),
            ("FR_GITHUB_TOKEN", "gh-token"),
            ("FR_COMMENT_KEY", "ci/test"),
            ("GITHUB_EVENT_PATH", &event),
        ],
    )
    .await;

    assert_eq!(code, 0, "an updated comment never fails the step: {output}");
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    let patched = sent(&requests, "PATCH", "/repos/acme/app/issues/comments/555")
        .expect("the marked comment was edited");
    let body: Value = serde_json::from_slice(&patched.body).expect("the body is JSON");
    let comment = body["body"].as_str().expect("the comment carries a body");
    assert!(
        comment.starts_with("<!-- flakeradar-summary:ci/test -->"),
        "the edit keeps the marker first: {comment}"
    );
    assert!(
        comment.contains("test_beta"),
        "the edit carries the fresh table: {comment}"
    );
    serving.abort();
}

#[tokio::test]
async fn summary_action_pr_comment_updates_the_first_marked_comment_across_pages() {
    // When an earlier duplicate left two marked comments on different list
    // pages, the first one is the comment the script must edit: paging stops at
    // the first match, and the later duplicate is left untouched.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    let server = MockServer::start().await;
    upload(&app, one_failure_junit()).await;
    process_all(&db).await;
    // A full first page (100 entries) whose marked comment is its last entry,
    // and a short second page holding a duplicate marked comment.
    let mut page_one: Vec<Value> = (0..99)
        .map(|i| json!({"id": i, "body": format!("filler comment {i}")}))
        .collect();
    page_one.push(json!({"id": 555, "body": "<!-- flakeradar-summary:ci/test -->\nold"}));
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page_one))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 999, "body": "<!-- flakeradar-summary:ci/test -->\nold"},
            {"id": 1000, "body": "filler comment"}
        ])))
        // The first match stops the paging, so the duplicate page is never read.
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/comments/555"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 555})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/comments/999"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 999})))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 777})))
        .expect(0)
        .mount(&server)
        .await;
    let file = summary_file("pr_comment_first_across_pages");
    let event = event_file("first_across_pages", PR_EVENT);

    let (code, output) = run_summary(
        &base_url,
        "false",
        "failure",
        "5",
        &file,
        &[
            ("GITHUB_API_URL", &server.uri()),
            ("FR_PR_COMMENT", "true"),
            ("FR_GITHUB_TOKEN", "gh-token"),
            ("FR_COMMENT_KEY", "ci/test"),
            ("GITHUB_EVENT_PATH", &event),
        ],
    )
    .await;

    assert_eq!(code, 0, "an updated comment never fails the step: {output}");
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    let patched = sent(&requests, "PATCH", "/repos/acme/app/issues/comments/555")
        .expect("the first marked comment was edited");
    let body: Value = serde_json::from_slice(&patched.body).expect("the body is JSON");
    let comment = body["body"].as_str().expect("the comment carries a body");
    assert!(
        comment.starts_with("<!-- flakeradar-summary:ci/test -->"),
        "the edit keeps the marker first: {comment}"
    );
    assert!(
        comment.contains("test_beta"),
        "the edit carries the fresh table: {comment}"
    );
    serving.abort();
}

#[tokio::test]
async fn summary_action_pr_comment_updates_the_comment_even_with_no_failures() {
    // A green re-run of a job whose marked comment exists must not leave the old
    // red table behind: the comment is edited to the no-failures line.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    let server = MockServer::start().await;
    upload(&app, all_passing_junit()).await;
    process_all(&db).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([
                {"id": 555, "body": "<!-- flakeradar-summary:ci/test -->\n**FlakeRadar:** 0 of 1 failures are known flaky tests."}
            ])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/comments/555"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 555})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 777})))
        .expect(0)
        .mount(&server)
        .await;
    let file = summary_file("pr_comment_green_update");
    let event = event_file("green_update", PR_EVENT);

    let (code, output) = run_summary(
        &base_url,
        "false",
        "success",
        "5",
        &file,
        &[
            ("GITHUB_API_URL", &server.uri()),
            ("FR_PR_COMMENT", "true"),
            ("FR_GITHUB_TOKEN", "gh-token"),
            ("FR_COMMENT_KEY", "ci/test"),
            ("GITHUB_EVENT_PATH", &event),
        ],
    )
    .await;

    assert_eq!(code, 0, "a green job never fails the step: {output}");
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    let patched = sent(&requests, "PATCH", "/repos/acme/app/issues/comments/555")
        .expect("the marked comment was edited");
    let body: Value = serde_json::from_slice(&patched.body).expect("the body is JSON");
    assert!(
        body["body"]
            .as_str()
            .expect("the comment carries a body")
            .contains("no failed tests reported for this CI job"),
        "a green re-run clears the table in place: {}",
        body["body"]
    );
    serving.abort();
}

#[tokio::test]
async fn summary_action_pr_comment_sends_nothing_for_a_green_job_without_a_comment() {
    // No failures and no comment of ours to update: creating one would only say
    // "nothing failed", so the job stays silent — the list call is all it makes.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    let server = MockServer::start().await;
    upload(&app, all_passing_junit()).await;
    process_all(&db).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 777})))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/comments/555"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 555})))
        .expect(0)
        .mount(&server)
        .await;
    let file = summary_file("pr_comment_green_new");
    let event = event_file("green_new", PR_EVENT);

    let (code, output) = run_summary(
        &base_url,
        "false",
        "success",
        "5",
        &file,
        &[
            ("GITHUB_API_URL", &server.uri()),
            ("FR_PR_COMMENT", "true"),
            ("FR_GITHUB_TOKEN", "gh-token"),
            ("FR_COMMENT_KEY", "ci/test"),
            ("GITHUB_EVENT_PATH", &event),
        ],
    )
    .await;

    assert_eq!(code, 0, "a silent green job never fails the step: {output}");
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert_eq!(requests.len(), 1, "only the listing went out: {requests:?}");
    serving.abort();
}

#[tokio::test]
async fn summary_action_pr_comment_uses_the_workflow_and_job_when_the_key_is_empty() {
    // The default key stands in for comment-key, because action metadata cannot
    // read the workflow and job contexts: the marker then names CI/test.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    let server = MockServer::start().await;
    upload(&app, one_failure_junit()).await;
    process_all(&db).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 777})))
        .expect(1)
        .mount(&server)
        .await;
    let file = summary_file("pr_comment_default_key");
    let event = event_file("default_key", PR_EVENT);

    let (code, output) = run_summary(
        &base_url,
        "false",
        "failure",
        "5",
        &file,
        &[
            ("GITHUB_API_URL", &server.uri()),
            ("FR_PR_COMMENT", "true"),
            ("FR_GITHUB_TOKEN", "gh-token"),
            ("FR_COMMENT_KEY", ""),
            ("GITHUB_WORKFLOW", "CI"),
            ("GITHUB_JOB", "test"),
            ("GITHUB_EVENT_PATH", &event),
        ],
    )
    .await;

    assert_eq!(code, 0, "the default key never fails the step: {output}");
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    let posted = sent(&requests, "POST", "/repos/acme/app/issues/12/comments")
        .expect("the comment was posted");
    let body: Value = serde_json::from_slice(&posted.body).expect("the body is JSON");
    assert!(
        body["body"]
            .as_str()
            .expect("the comment carries a body")
            .starts_with("<!-- flakeradar-summary:CI/test -->"),
        "the marker uses workflow/job when comment-key is empty: {}",
        body["body"]
    );
    serving.abort();
}

#[tokio::test]
async fn summary_action_pr_comment_warns_and_stays_green_when_github_fails() {
    // GitHub answers 500: one warning line, and the exit is untouched — with the
    // gate off the step still succeeds, because a comment is not worth a job.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    let server = MockServer::start().await;
    upload(&app, one_failure_junit()).await;
    process_all(&db).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"message": "no"})))
        .expect(1)
        .mount(&server)
        .await;
    let file = summary_file("pr_comment_github_error");
    let event = event_file("github_error", PR_EVENT);

    let (code, output) = run_summary(
        &base_url,
        "false",
        "failure",
        "5",
        &file,
        &[
            ("GITHUB_API_URL", &server.uri()),
            ("FR_PR_COMMENT", "true"),
            ("FR_GITHUB_TOKEN", "gh-token"),
            ("FR_COMMENT_KEY", "ci/test"),
            ("GITHUB_EVENT_PATH", &event),
        ],
    )
    .await;

    assert_eq!(code, 0, "a failed call never fails the step: {output}");
    assert_eq!(
        output.matches("::warning::").count(),
        1,
        "exactly one warning names the failed call: {output}"
    );
    serving.abort();
}

#[tokio::test]
async fn summary_action_pr_comment_posts_nothing_when_the_comment_list_fails() {
    // The list call failing leaves it unknown whether our comment exists: a POST
    // would duplicate it on the next re-run, so a failed listing only warns and
    // stays silent — with the exit untouched, as for any other failed call.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    let server = MockServer::start().await;
    upload(&app, one_failure_junit()).await;
    process_all(&db).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"message": "no"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 777})))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/comments/555"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 555})))
        .expect(0)
        .mount(&server)
        .await;
    let file = summary_file("pr_comment_list_error");
    let event = event_file("list_error", PR_EVENT);

    let (code, output) = run_summary(
        &base_url,
        "false",
        "failure",
        "5",
        &file,
        &[
            ("GITHUB_API_URL", &server.uri()),
            ("FR_PR_COMMENT", "true"),
            ("FR_GITHUB_TOKEN", "gh-token"),
            ("FR_COMMENT_KEY", "ci/test"),
            ("GITHUB_EVENT_PATH", &event),
        ],
    )
    .await;

    assert_eq!(code, 0, "a failed list never fails the step: {output}");
    assert_eq!(
        output.matches("::warning::").count(),
        1,
        "exactly one warning names the failed listing: {output}"
    );
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert!(
        sent(&requests, "POST", "/repos/acme/app/issues/12/comments").is_none(),
        "an unknown comment state must not post a duplicate: {requests:?}"
    );
    assert_eq!(
        requests.len(),
        1,
        "only the failed listing went out: {requests:?}"
    );
    serving.abort();
}

#[tokio::test]
async fn summary_action_pr_comment_sends_the_comment_before_the_gate_decides() {
    // The comment is sent before the gate runs, so a gated failure still leaves
    // its comment on the pull request even though the step then exits 1.
    let db = TestDb::new().await.expect("a test database");
    let app = router(&db);
    let (base_url, serving) = serve(&db).await;
    let server = MockServer::start().await;
    upload(&app, one_failure_junit()).await;
    process_all(&db).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 555, "body": "<!-- flakeradar-summary:ci/test -->\nold"}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/app/issues/comments/555"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 555})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues/12/comments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 777})))
        .expect(0)
        .mount(&server)
        .await;
    let file = summary_file("pr_comment_gate_order");
    let event = event_file("gate_order", PR_EVENT);

    let (code, output) = run_summary(
        &base_url,
        "true",
        "failure",
        "5",
        &file,
        &[
            ("GITHUB_API_URL", &server.uri()),
            ("FR_PR_COMMENT", "true"),
            ("FR_GITHUB_TOKEN", "gh-token"),
            ("FR_COMMENT_KEY", "ci/test"),
            ("GITHUB_EVENT_PATH", &event),
        ],
    )
    .await;

    assert_eq!(
        code, 1,
        "the unquarantined failure still fails the gate: {output}"
    );
    let requests = server
        .received_requests()
        .await
        .expect("the stub records requests");
    assert!(
        sent(&requests, "PATCH", "/repos/acme/app/issues/comments/555").is_some(),
        "the comment went out before the gate decided: {requests:?}"
    );
    serving.abort();
}
