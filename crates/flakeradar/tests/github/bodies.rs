//! The issue bodies and the fixed-fix comment, byte for byte.

// Rust guideline compliant 2026-07-21

use flakeradar::github;
use flakeradar_core::{
    model::EntityKind, model::ExecutionStatus, parsing::fingerprint, types::CiRunId,
    types::Fingerprint, types::Score,
};
use flakeradar_db::filing::{JobRow, RecentExecution, TestRow};
use flakeradar_test_support::{
    FIXTURE_INSTANT, TestDb, make_execution, make_job_execution, make_project, make_run,
    make_test_case, system_clock,
};
use sea_orm::ConnectionTrait as _;
use sea_orm::EntityTrait as _;
use sea_orm::{DbBackend, Statement, Value};
use serde_json::{Value as Json, json};
use wiremock::{MockServer, ResponseTemplate};

use crate::support::{
    RECORDED_BODY, api, explain_job_execution, flaky_job_in, flaky_test_in, gh_settings, link_of,
    mount_create, unstopped,
};

#[tokio::test]
async fn a_field_the_body_prices_differently_is_still_the_written_sentence() {
    // The run hint falls back when no file was reported, and a Test with no line
    // still gets a permalink without an anchor.
    let test = TestRow {
        id: 1,
        classname: "tests.test_cart".to_owned(),
        name: "test_total".to_owned(),
        suite: String::new(),
        file: None,
        line: None,
        fingerprint: digest("", "tests.test_cart", "test_total"),
        flakiness_score: Score::new(0.5).expect("a score"),
        confirmed_flake_count: 0,
    };
    let recent = [RecentExecution {
        status: ExecutionStatus::Passed,
        message: String::new(),
        details: None,
        commit_sha: "abcdef1234567890".to_owned(),
        branch: "main".to_owned(),
        created_at: FIXTURE_INSTANT,
    }];
    let settings = gh_settings(&[]);
    let body = github::test_issue_body(&test, &recent, "acme/app", "backend", "", &settings);
    assert!(
        body.contains(
            "# Re-run `tests.test_cart::test_total` via your project's test runner (no file path reported)."
        ),
        "{body}"
    );
    assert!(
        body.contains("- **Location:** (not reported by the test runner)"),
        "{body}"
    );
    assert!(
        body.contains("- **Suite / classname:** `(none)` / `tests.test_cart`"),
        "{body}"
    );
    assert!(
        !body.contains("### Sample failure"),
        "a passing Test shows no failure"
    );
}

/// A Flakiness score a test states as a literal.
fn score(value: f64) -> Score {
    Score::new(value).expect("the test states a score")
}

/// The fingerprint of one Test identity.
fn digest(suite: &str, classname: &str, name: &str) -> Fingerprint {
    fingerprint(suite, classname, name)
}

#[test]
fn the_last_failing_commit_has_no_branch_clause_when_no_branch_was_reported() {
    let test = TestRow {
        id: 1,
        classname: "tests.test_cart".to_owned(),
        name: "test_total".to_owned(),
        suite: "backend".to_owned(),
        file: Some("tests/test_cart.py".to_owned()),
        line: Some(12),
        fingerprint: digest("backend", "tests.test_cart", "test_total"),
        flakiness_score: score(0.5),
        confirmed_flake_count: 1,
    };
    let recent = [RecentExecution {
        status: ExecutionStatus::Failed,
        message: "AssertionError".to_owned(),
        details: None,
        commit_sha: "abcdef1234567890".to_owned(),
        branch: String::new(),
        created_at: FIXTURE_INSTANT,
    }];
    let body =
        github::test_issue_body(&test, &recent, "acme/app", "backend", "", &gh_settings(&[]));
    assert!(
        body.contains("- **Last failing commit:** `abcdef1234567890`\n"),
        "a run that reported no branch names only the commit: {body}"
    );
    assert!(
        body.contains("https://github.com/acme/app/blob/abcdef1234567890/tests/test_cart.py#L12"),
        "{body}"
    );
}

#[test]
fn the_filed_body_matches_the_recorded_bytes() {
    let test = TestRow {
        id: 1,
        classname: "tests.test_cart".to_owned(),
        name: "test_total".to_owned(),
        suite: "unit".to_owned(),
        file: Some("tests/test_cart.py".to_owned()),
        line: Some(12),
        fingerprint: digest("unit", "tests.test_cart", "test_total"),
        flakiness_score: Score::new(0.5).expect("a score"),
        confirmed_flake_count: 2,
    };
    let at = |minute| {
        chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 3, 4, 5, minute, 0).unwrap()
    };
    let recent = [
        RecentExecution {
            status: ExecutionStatus::Passed,
            message: String::new(),
            details: Some(String::new()),
            commit_sha: "s2".to_owned(),
            branch: "main".to_owned(),
            created_at: at(23),
        },
        RecentExecution {
            status: ExecutionStatus::Failed,
            message: String::new(),
            details: Some(String::new()),
            commit_sha: "s2".to_owned(),
            branch: "main".to_owned(),
            created_at: at(22),
        },
        RecentExecution {
            status: ExecutionStatus::Failed,
            message: "boom older".to_owned(),
            details: Some("older details".to_owned()),
            commit_sha: "s1".to_owned(),
            branch: "main".to_owned(),
            created_at: at(11),
        },
    ];
    let body = github::test_issue_body(
        &test,
        &recent,
        "acme/app",
        "backend",
        "backend",
        &gh_settings(&[]),
    );
    assert_eq!(body, RECORDED_BODY);
}

#[test]
fn both_bodies_name_the_executions_the_filing_reads() {
    // The heading was a literal in each body. Both now state `BODY_EXECUTIONS`,
    // which is also the count the filer asks for, so the promise and the read
    // cannot drift apart.
    let test = TestRow {
        id: 1,
        classname: "tests.test_cart".to_owned(),
        name: "test_total".to_owned(),
        suite: "unit".to_owned(),
        file: Some("tests/test_cart.py".to_owned()),
        line: Some(12),
        fingerprint: digest("unit", "tests.test_cart", "test_total"),
        flakiness_score: Score::new(0.5).expect("a score"),
        confirmed_flake_count: 0,
    };
    let job = JobRow {
        id: 2,
        name: "unit".to_owned(),
        flakiness_score: Score::new(0.5).expect("a score"),
        confirmed_flake_count: 0,
    };
    let heading = format!("### Last {} executions", github::BODY_EXECUTIONS);
    let test_body =
        github::test_issue_body(&test, &[], "acme/app", "backend", "", &gh_settings(&[]));
    let job_body = github::job_issue_body(&job, "ci", "acme/app", None, &gh_settings(&[]));
    for body in [test_body, job_body] {
        assert!(
            body.lines().any(|line| line == heading),
            "both bodies name `{heading}`: {body}"
        );
    }
}

#[tokio::test]
async fn the_run_hint_follows_the_file_extension() {
    for (path, expected) in [
        (
            Some("tests/test_cart.py"),
            "pytest tests/test_cart.py::test_total",
        ),
        (
            Some("src/cart.test.ts"),
            "npx vitest run src/cart.test.ts -t \"test_total\"",
        ),
        (Some("cart_test.go"), "go test ./... -run test_total"),
        (Some("CartTest.java"), "mvn test -Dtest=CartTest#test_total"),
        (
            Some("cart_spec.rb"),
            "bundle exec rspec cart_spec.rb -e \"test_total\"",
        ),
        (
            Some("cart_test.rs"),
            "# Re-run `CartTest::test_total` via your project's test runner (cart_test.rs).",
        ),
        (
            Some("Makefile"),
            "# Re-run `CartTest::test_total` via your project's test runner (Makefile).",
        ),
        (
            None,
            "# Re-run `CartTest::test_total` via your project's test runner (no file path reported).",
        ),
    ] {
        let hint = github::run_hint(path, "CartTest", "test_total");
        assert_eq!(hint, expected, "{path:?}");
    }
}

#[test]
fn the_fixed_comment_links_a_job_to_the_job_view() {
    let comment = github::fixed_comment(
        EntityKind::Job,
        7,
        3,
        score(0.5),
        None,
        "https://flakes.example.com/",
    );
    assert_eq!(
        comment,
        "FlakeRadar: 3 passing runs since the last failure (score now 0.50). \
         The fix appears to hold.\n\nhttps://flakes.example.com/?view=jobs&job=7"
    );
}

#[tokio::test]
async fn a_job_body_without_history_still_carries_the_table_heading() {
    let job = JobRow {
        id: 3,
        name: "unit".to_owned(),
        flakiness_score: score(0.5),
        confirmed_flake_count: 1,
    };
    let body = github::job_issue_body(&job, "ci", "acme/app", None, &gh_settings(&[]));
    assert!(body.contains("**Job:** `unit`"), "{body}");
    assert!(body.contains("**Flakiness score:**"), "{body}");
    assert!(!body.contains("Unexplained failures"), "{body}");
    assert!(!body.contains("Explained by tests"), "{body}");
    let table = body
        .split("### Last 10 executions")
        .nth(1)
        .expect("the executions heading is there");
    assert!(
        table.contains("| Outcome | Commit | Branch | Attempt | Runner | Job |"),
        "{body}"
    );
    assert!(table.contains("|---|---|---|---|---|---|"), "{body}");
    assert!(!table.contains("| failed |"), "{body}");
    assert!(!table.contains("| passed |"), "{body}");
}

#[test]
fn a_job_body_names_every_outcome_runner_and_link_it_has() {
    use flakeradar_core::{
        dto::JobExecutionOut, dto::JobHistoryOut, dto::JobOut, dto::RunnerStatOut,
        model::JobOutcome,
    };

    /// One Job execution with the fields the body prints.
    fn execution(outcome: JobOutcome, url: &str, runner_name: &str) -> JobExecutionOut {
        JobExecutionOut {
            id: 1,
            status: "failed".to_owned(),
            outcome,
            commit_sha: "a1b2c3d4".to_owned(),
            branch: "main".to_owned(),
            ci_run_id: CiRunId::new("9").expect("a CI run id"),
            ci_run_attempt: 1,
            ci_job_id: "unit".to_owned(),
            url: url.to_owned(),
            runner_name: runner_name.to_owned(),
            runner_labels: Vec::new(),
            started_at: None,
            completed_at: None,
            created_at: FIXTURE_INSTANT,
            explained_by: Vec::new(),
            explained_by_jobs: Vec::new(),
        }
    }

    let job = JobRow {
        id: 3,
        name: "unit".to_owned(),
        flakiness_score: score(0.5),
        confirmed_flake_count: 1,
    };
    let history = JobHistoryOut {
        job: JobOut {
            id: 3,
            repo: "acme/app".to_owned(),
            provider: "github".to_owned(),
            pipeline: "ci".to_owned(),
            name: "unit".to_owned(),
            flakiness_score: Score::new(0.5).expect("a score"),
            tier: flakeradar_core::model::Tier::Suspect,
            confirmed_flake_count: 1,
            clean_streak: 0,
            trend: None,
            last_status: "failed".to_owned(),
            last_seen_at: FIXTURE_INSTANT,
            summary: false,
            github_issue_number: None,
            github_issue_url: None,
            github_issue_closed: false,
        },
        unexplained_failures: 1,
        explained_failures: 0,
        executions: vec![
            execution(
                JobOutcome::Skipped,
                "https://ci.example.com/job/9",
                "ubuntu-latest",
            ),
            execution(JobOutcome::Failed, "https://ci.example.com/job/10", ""),
        ],
        runners: vec![RunnerStatOut {
            labels: vec!["self-hosted".to_owned()],
            runner_name: Some(String::new()),
            executions: 2,
            unexplained_failures: 1,
        }],
        score_history: Vec::new(),
    };
    let body = github::job_issue_body(&job, "ci", "acme/app", Some(&history), &gh_settings(&[]));
    assert!(body.contains("| skipped |"), "{body}");
    assert!(
        body.contains("[job](https://ci.example.com/job/9)"),
        "{body}"
    );
    assert!(body.contains("| ubuntu-latest |"), "{body}");
    assert!(
        body.contains("  - `self-hosted`: 1 unexplained of 2 executions"),
        "a runner with no name is named by its labels: {body}"
    );
}

#[tokio::test]
async fn a_file_without_a_line_still_gets_a_permalink() {
    let db = TestDb::new().await.expect("a test database");
    let row_id = flaky_test_in(&db, "acme/app", "backend", "t_file").await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET file = 'tests/test_x.py', line = NULL WHERE id = $1",
            [Value::from(row_id)],
        ))
        .await
        .expect("the location writes");
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;

    github::file_for(
        db.connection(),
        EntityKind::Test,
        &[row_id],
        &gh_settings(&[]),
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    let requests = server.received_requests().await.expect("a record");
    assert_eq!(requests.len(), 1);
    let body: Json = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    let text = body["body"].as_str().expect("a body");
    assert!(
        text.contains("https://github.com/acme/app/blob/abcdef1234567890/tests/test_x.py"),
        "the permalink is still useful without an anchor: {text}"
    );
    assert!(!text.contains("#L"), "{text}");
    assert!(text.contains("`tests/test_x.py`"), "{text}");
}

#[tokio::test]
async fn a_test_with_only_passed_executions_gets_no_failure_evidence() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .repo("acme/app")
        .project("backend")
        .create(&db)
        .await;
    let quiet = make_test_case(&project)
        .name("t_passed")
        .classname("tests.test_x")
        .flakiness_score(0.6)
        .confirmed_flake_count(1)
        .file("tests/test_x.py")
        .line(7)
        .create(&db)
        .await;
    let run = make_run(&project)
        .commit_sha("deadbeef")
        .branch("main")
        .create(&db)
        .await;
    make_execution(&quiet, &run)
        .status("passed")
        .create(&db)
        .await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;

    github::file_for(
        db.connection(),
        EntityKind::Test,
        &[quiet.id],
        &gh_settings(&[]),
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    let requests = server.received_requests().await.expect("a record");
    assert_eq!(requests.len(), 1);
    let body: Json = serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    let text = body["body"].as_str().expect("a body");
    assert!(!text.contains("Last failing commit"), "{text}");
    assert!(!text.contains("Permalink"), "{text}");
    assert!(!text.contains("### Sample failure"), "{text}");
    assert!(
        text.contains("### Last 10 executions"),
        "the body is not empty, it only lacks a failure: {text}"
    );
    assert!(text.contains("| passed | `deadbeef` | main |"), "{text}");
    assert_eq!(
        link_of(&db, EntityKind::Test, quiet.id).await.number,
        Some(77)
    );
}

/// The body of the one issue a job filing posted to `path`.
async fn only_issue_body(server: &MockServer) -> String {
    let requests = server.received_requests().await.expect("a record");
    let request = requests.first().expect("exactly one issue is filed");
    assert_eq!(requests.len(), 1, "exactly one issue is filed");
    let body: Json = serde_json::from_slice(&request.body).expect("the body is JSON");
    body.get("body")
        .and_then(Json::as_str)
        .expect("a body")
        .to_owned()
}

#[tokio::test]
async fn a_job_body_lists_its_worst_runner_groups_first() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job_in(
        &db,
        "acme/app",
        ".github/workflows/ci.yml",
        "e2e (ubuntu-latest)",
    )
    .await;
    let pipeline = flakeradar_db::entities::jobs::Entity::find_by_id(job)
        .one(db.connection())
        .await
        .expect("the Job reads")
        .expect("the Job exists");
    for _ in 0..2 {
        make_job_execution(&pipeline)
            .status("failed")
            .runner_labels(&["self-hosted", "gpu"])
            .create(&db)
            .await;
    }
    make_job_execution(&pipeline)
        .status("failed")
        .runner_labels(&["ubuntu-latest"])
        .create(&db)
        .await;
    make_job_execution(&pipeline)
        .status("passed")
        .runner_labels(&["windows-latest"])
        .create(&db)
        .await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;

    github::file_for(
        db.connection(),
        EntityKind::Job,
        &[job],
        &gh_settings(&[]),
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    let body = only_issue_body(&server).await;
    assert!(body.contains("**Failures by runner:**"), "{body}");
    let gpu = "- `gpu, self-hosted`: 2 unexplained of 2 executions";
    let ubuntu = "- `ubuntu-latest`: 1 unexplained of 1 executions";
    assert!(body.contains(gpu), "{body}");
    assert!(body.contains(ubuntu), "{body}");
    assert!(
        body.find(gpu) < body.find(ubuntu),
        "the worst group comes first: {body}"
    );
    assert!(
        !body.contains("windows-latest"),
        "a group with no unexplained failure is not listed: {body}"
    );
}

#[tokio::test]
async fn a_job_body_caps_the_runner_groups_at_three() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job_in(
        &db,
        "acme/app",
        ".github/workflows/many.yml",
        "many-runners",
    )
    .await;
    let row = flakeradar_db::entities::jobs::Entity::find_by_id(job)
        .one(db.connection())
        .await
        .expect("the Job reads")
        .expect("the Job exists");
    for (label, failures) in [("a-lab", 3), ("b-lab", 2), ("c-lab", 1), ("d-lab", 1)] {
        for _ in 0..failures {
            make_job_execution(&row)
                .status("failed")
                .runner_labels(&[label])
                .create(&db)
                .await;
        }
    }
    make_job_execution(&row)
        .status("passed")
        .runner_labels(&["d-lab"])
        .create(&db)
        .await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;

    github::file_for(
        db.connection(),
        EntityKind::Job,
        &[job],
        &gh_settings(&[]),
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    let body = only_issue_body(&server).await;
    for expected in [
        "- `a-lab`: 3 unexplained of 3 executions",
        "- `b-lab`: 2 unexplained of 2 executions",
        "- `c-lab`: 1 unexplained of 1 executions",
    ] {
        assert!(body.contains(expected), "{expected}: {body}");
    }
    assert!(!body.contains("d-lab"), "only three groups: {body}");
}

#[tokio::test]
async fn a_job_body_omits_the_runner_section_without_an_unexplained_failure() {
    let db = TestDb::new().await.expect("a test database");
    let job = flaky_job_in(
        &db,
        "acme/app",
        ".github/workflows/explained.yml",
        "explained-only",
    )
    .await;
    // The one failed execution the fixture made is explained by a failing Test the
    // same job attempt uploaded, so the Job has no unexplained failure at all.
    explain_job_execution(&db, "explained-only").await;
    let server = MockServer::start().await;
    mount_create(
        &server,
        ResponseTemplate::new(201).set_body_json(json!({"number": 77})),
    )
    .await;

    github::file_for(
        db.connection(),
        EntityKind::Job,
        &[job],
        &gh_settings(&[]),
        &api(&server),
        &system_clock(),
        &unstopped(),
    )
    .await;
    let body = only_issue_body(&server).await;
    assert!(!body.contains("Failures by runner"), "{body}");
}
