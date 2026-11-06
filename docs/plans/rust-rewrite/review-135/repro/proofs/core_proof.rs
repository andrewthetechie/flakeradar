//! Proofs for the parser and validation findings, rewritten after the fix so each
//! one asserts the answer this head gives.
//!
//! At `f9330fd` each of these asserted the defect: `NaN` parsed, a wide `Nd` digit
//! read as nine, a long suite name was copied once per case, and a bad runner
//! label pointed at `jobs[0]`.

use flakeradar_core::dto::{BodyRefusal, PipelineReportIn};
use flakeradar_core::parsing::{ParseError, parse_junit_xml};
use serde_json::{Value, json};

fn one(time: &str) -> Result<f64, ParseError> {
    let xml = format!(
        r#"<testsuite name="s"><testcase classname="c" name="t" time="{time}"/></testsuite>"#
    );
    let cases = parse_junit_xml(xml.as_bytes(), 1)?;
    Ok(cases[0].duration)
}

#[test]
fn time_nan_and_overflow_are_refused() {
    // H19b: a duration the database and the API cannot carry is refused with the
    // typed `InvalidTime` the route answers 400 for.
    for value in ["NaN", "1e999", "inf"] {
        let reading = one(value);
        println!("time={value} -> {reading:?}");
        assert!(
            matches!(reading, Err(ParseError::InvalidTime { .. })),
            "time={value} must be refused"
        );
    }
    // A finite value keeps the reference's reading.
    assert_eq!(one("-5").expect("a negative time still parses"), -5.0);
    assert_eq!(one("1.5").expect("a fractional time still parses"), 1.5);
}

#[test]
fn double_struck_digit_reads_as_one() {
    // J18: U+1D7D9 MATHEMATICAL DOUBLE-STRUCK DIGIT ONE. Python: float("\U0001D7D9") == 1.0.
    let duration = one("\u{1D7D9}").expect("the digit is a number");
    println!("time=U+1D7D9 -> {duration}");
    assert_eq!(duration, 1.0);
}

#[test]
fn a_long_suite_name_is_refused_rather_than_amplified() {
    // H3: the copied Test identities are budgeted, so a name the parser would
    // repeat once per case is refused instead of multiplied.
    let name = "A".repeat(50_000);
    let mut xml = format!(r#"<testsuite name="{name}">"#);
    for index in 0..5_000 {
        xml.push_str(&format!(r#"<testcase name="t{index}"/>"#));
    }
    xml.push_str("</testsuite>");
    let parsed = parse_junit_xml(xml.as_bytes(), 1);
    println!(
        "body {} bytes -> {:?}",
        xml.len(),
        parsed.as_ref().err()
    );
    assert!(
        matches!(parsed, Err(ParseError::InvalidXml { .. })),
        "an identity list past the budget must be refused"
    );
}

#[test]
fn retry_children_cannot_amplify_the_test_name() {
    // H3, the retry shape: one case named once per `<rerunFailure>`.
    let name = "N".repeat(50_000);
    let mut xml = format!(r#"<testsuite name="s"><testcase name="{name}">"#);
    for _ in 0..5_000 {
        xml.push_str("<rerunFailure/>");
    }
    xml.push_str("</testcase></testsuite>");
    let parsed = parse_junit_xml(xml.as_bytes(), 1);
    println!(
        "body {} bytes -> {:?}",
        xml.len(),
        parsed.as_ref().err()
    );
    assert!(matches!(parsed, Err(ParseError::InvalidXml { .. })));
}

#[test]
fn a_bad_runner_label_points_at_its_own_job() {
    // H18a: the issue's `loc` carries the Job index.
    let body = json!({
        "repo": "a/b",
        "provider": "github",
        "pipeline": "ci",
        "commit_sha": "abc",
        "branch": "main",
        "ci_run_id": "1",
        "ci_run_attempt": 1,
        "jobs": [
            {"ci_job_id": "j0", "name": "j0", "status": "passed"},
            {"ci_job_id": "j1", "name": "j1", "status": "passed"},
            {"ci_job_id": "j2", "name": "j2", "status": "passed",
             "runner_labels": ["x".repeat(256)]}
        ]
    });
    let object: serde_json::Map<String, Value> = body.as_object().cloned().expect("an object");
    let refusal = PipelineReportIn::from_value(&object).expect_err("the label is too long");
    let BodyRefusal::Issues(issues) = refusal else {
        panic!("a long label is a validation issue, not an attempt refusal");
    };
    let loc = serde_json::to_value(&issues[0].loc).expect("a loc serializes");
    println!("loc for jobs[2].runner_labels[0]: {loc}");
    assert_eq!(loc, json!(["body", "jobs", 2, "runner_labels", 0]));
}
