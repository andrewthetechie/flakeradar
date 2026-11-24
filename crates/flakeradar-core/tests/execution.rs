//! The Execution plan: attempt numbering, retries, latest state and counts.
//!
//! Each test pins one rule of [`build_executions`] with no database: the plan it
//! answers is what the database layer writes, so the values named here are the
//! values the rows carry.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the rules under test are wrong"
)]

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset};
use flakeradar_core::{
    execution::ExecutionError, execution::ExecutionPlan, execution::PlanCounts,
    execution::build_executions, model::ExecutionStatus, parsing::ClassifiedReport,
    parsing::classify_report, scoring::clean_streak, types::Fingerprint,
};

/// The Run every plan here is built for.
const RUN: i32 = 7;

/// The instant every plan here is built at, the one the caller stamps its rows
/// with.
fn now() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-07-21T12:00:00+00:00").expect("an instant")
}

/// A Report whose body is `xml`, parsed and classified as the processor does.
fn report(xml: &str) -> ClassifiedReport {
    classify_report(xml.as_bytes(), 1).expect("the report parses")
}

/// The id map `upsert_test_cases` would answer: each Test numbered from 1, in
/// order of first appearance.
fn ids_of(report: &ClassifiedReport) -> BTreeMap<Fingerprint, i32> {
    let mut ids = BTreeMap::new();
    for reported in &report.cases {
        let next = i32::try_from(ids.len()).unwrap_or(i32::MAX) + 1;
        ids.entry(reported.fingerprint.clone()).or_insert(next);
    }
    ids
}

/// The plan a body answers, with the ids its own cases imply.
fn plan(xml: &str) -> ExecutionPlan {
    let report = report(xml);
    build_executions(&report, &ids_of(&report), RUN, now()).expect("the rules answer a plan")
}

/// What a body answers instead of a plan.
fn refusal(xml: &str) -> ExecutionError {
    let report = report(xml);
    build_executions(&report, &ids_of(&report), RUN, now()).expect_err("the rules refuse")
}

#[test]
fn three_attempts_of_one_test_are_numbered_from_zero() {
    let plan = plan(
        r#"<testsuite name="unit"><testcase classname="c" name="t" time="0.5" file="src/test_a.rs" line="3"><failure message="boom">trace</failure></testcase><testcase classname="c" name="t" time="0.25" file="src/test_a.rs" line="3"><failure message="boom">trace</failure></testcase><testcase classname="c" name="t" time="0.1" file="src/test_a.rs" line="3"/></testsuite>"#,
    );
    let attempts: Vec<i32> = plan.executions.iter().map(|row| row.attempt).collect();
    assert_eq!(attempts, [0, 1, 2], "a Report numbers one Test from 0");
    assert!(
        plan.executions
            .iter()
            .all(|row| row.test_case_id == plan.executions[0].test_case_id),
        "all three cases are the same Test"
    );
    // Every column the plan answers is the value the Report reported.
    assert_eq!(plan.executions[0].status, ExecutionStatus::Failed);
    assert_eq!(plan.executions[0].message, "boom");
    assert_eq!(plan.executions[0].details, "trace");
    assert_eq!(plan.executions[0].duration, 0.5);
    assert_eq!(
        plan.executions[0].location,
        Some(("src/test_a.rs".to_owned(), 3))
    );
    assert_eq!(plan.run_id, RUN, "the plan answers its own Run");
    assert_eq!(
        plan.now,
        now(),
        "the plan answers back the instant the caller stamps its rows with"
    );
    let id = plan.executions[0].test_case_id;
    assert_eq!(plan.day_counts.get(&id), Some(&(3, 2)));
}

#[test]
fn a_passed_retry_answers_a_passed_execution_and_latest_status() {
    let plan = plan(
        r#"<testsuite name="unit"><testcase classname="c" name="t"><failure message="boom">trace</failure></testcase><testcase classname="c" name="t"><failure message="boom">trace</failure></testcase><testcase classname="c" name="t"/></testsuite>"#,
    );
    let last = plan.executions.last().expect("the retry is an Execution");
    assert_eq!(last.status, ExecutionStatus::Passed);
    assert_eq!(plan.latest.len(), 1, "one Test, three attempts");
    assert_eq!(
        plan.latest[0].last_status,
        ExecutionStatus::Passed,
        "the last occurrence wins the latest status"
    );
    assert_eq!(
        plan.latest[0].last_status, last.status,
        "and it is the status the retry answered"
    );
    assert_eq!(
        plan.counts,
        PlanCounts {
            passed: 1,
            failed: 2,
            error: 0,
            skipped: 0,
            retried: 1,
            duplicate: 0,
        }
    );
}

#[test]
fn a_skipped_case_answers_skipped_and_says_nothing_about_flakiness() {
    let plan = plan(
        r#"<testsuite name="unit"><testcase classname="c" name="pass"/><testcase classname="c" name="skip"><skipped message="not today"/></testcase></testsuite>"#,
    );
    let statuses: Vec<ExecutionStatus> = plan.executions.iter().map(|row| row.status).collect();
    assert_eq!(
        statuses,
        [ExecutionStatus::Passed, ExecutionStatus::Skipped]
    );
    assert_eq!(plan.counts.skipped, 1);
    assert_eq!(plan.latest[1].last_status, ExecutionStatus::Skipped);
    // The same rule the rescore applies to stored rows: a skip breaks no streak.
    assert_eq!(
        clean_streak(&statuses),
        1,
        "a Skipped Execution is neither clean nor failing"
    );
}

#[test]
fn a_case_without_a_location_leaves_the_previous_one() {
    let plan = plan(
        r#"<testsuite name="unit"><testcase classname="c" name="t" file="src/test_a.rs" line="12"><failure message="boom">trace</failure></testcase><testcase classname="c" name="t"/></testsuite>"#,
    );
    assert_eq!(
        plan.executions[1].location, None,
        "the retry reported no Location of its own"
    );
    assert_eq!(plan.latest[0].last_status, ExecutionStatus::Passed);
    assert_eq!(
        (plan.latest[0].file.as_deref(), plan.latest[0].line),
        (Some("src/test_a.rs"), Some(12)),
        "a Report without a Location never erases one"
    );
}

#[test]
fn a_line_too_large_for_the_column_is_refused() {
    let error = refusal(
        r#"<testsuite name="unit"><testcase classname="c" name="t" file="src/test_a.rs" line="9999999999"/></testsuite>"#,
    );
    assert_eq!(
        error.to_string(),
        "DataError: line 9999999999 is out of range"
    );
    assert_eq!(
        error,
        ExecutionError::LineOutOfRange {
            line: 9_999_999_999,
        }
    );
}

#[test]
fn a_line_without_a_file_is_never_planned_and_never_refused() {
    let plan = plan(
        r#"<testsuite name="unit"><testcase classname="c" name="t" line="9999999999"/></testsuite>"#,
    );
    assert_eq!(plan.executions[0].location, None);
    assert_eq!(
        (plan.latest[0].file.as_deref(), plan.latest[0].line),
        (None, None)
    );
}

#[test]
fn the_latest_rows_keep_first_appearance_order() {
    let body = r#"<testsuite name="unit"><testcase classname="c" name="second"/><testcase classname="c" name="first"/><testcase classname="c" name="second"/></testsuite>"#;
    let report = report(body);
    // The ids run the other way, so a plan that sorted them by id is caught.
    let mut ids = BTreeMap::new();
    ids.insert(report.cases[0].fingerprint.clone(), 9);
    ids.insert(report.cases[1].fingerprint.clone(), 4);
    let plan = build_executions(&report, &ids, RUN, now()).expect("the rules answer a plan");
    let latest_ids: Vec<i32> = plan.latest.iter().map(|row| row.test_case_id).collect();
    assert_eq!(
        latest_ids,
        [9, 4],
        "the latest rows come in order of first appearance"
    );
    let attempts: Vec<i32> = plan.executions.iter().map(|row| row.attempt).collect();
    assert_eq!(attempts, [0, 0, 1], "the Executions stay in Report order");
    assert_eq!(plan.counts.retried, 1);
}

#[test]
fn a_test_missing_from_the_id_map_vanishes() {
    let body = r#"<testsuite name="unit"><testcase classname="c" name="t"/></testsuite>"#;
    let report = report(body);
    let fingerprint = report.cases[0].fingerprint.clone();
    let error = build_executions(&report, &BTreeMap::new(), RUN, now())
        .expect_err("an empty id map has no id for the Test");
    assert_eq!(
        error.to_string(),
        format!("DataError: Test {fingerprint} vanished")
    );
    assert_eq!(error, ExecutionError::Vanished { fingerprint });
}
