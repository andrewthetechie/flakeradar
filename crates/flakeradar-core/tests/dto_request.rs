//! The upload validation rules the request bodies apply.
//!
//! Each case builds a body, reads it the way the route reads one, and pins the
//! issues or the normalized result.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails loudly when a DTO or its validation is wrong"
)]

use flakeradar_core::dto::{
    AcknowledgeIn, BodyRefusal, IssueLinkIn, JobResultIn, LocSegment, PipelineReportIn,
    QuarantineIn, ValidationIssue,
};
use serde_json::{Value, json};

/// A minimal valid Pipeline report body.
fn valid_report() -> Value {
    json!({
        "repo": " Acme/App ",
        "provider": " GitHub ",
        "pipeline": "ci.yml",
        "commit_sha": "abc",
        "branch": "main",
        "jobs": [{"ci_job_id": "1", "name": " build ", "status": "passed"}]
    })
}

/// Reads the fixture body the way the route reads one.
#[expect(
    clippy::panic_in_result_fn,
    reason = "the helper returns the refusal but fails loudly on a fixture that is no object"
)]
fn validated(body: &Value) -> Result<PipelineReportIn, Vec<ValidationIssue>> {
    let object = body
        .as_object()
        .unwrap_or_else(|| panic!("the fixture is a JSON object: {body}"));
    PipelineReportIn::from_value(object).map_err(|refusal| match refusal {
        BodyRefusal::Issues(issues) => issues,
        BodyRefusal::AttemptPastColumn => panic!("an attempt inside the column is refused"),
    })
}

#[test]
fn a_valid_pipeline_report_is_normalized() {
    let report = validated(&valid_report()).expect("the report is valid");
    assert_eq!(report.repo, "acme/app");
    assert_eq!(report.provider, "github");
    assert_eq!(report.jobs[0].name, "build");
    assert_eq!(report.ci_run_attempt, 1);
}

#[test]
fn a_blank_default_branch_is_unknown() {
    let mut body = valid_report();
    body["default_branch"] = json!("  ");
    let report = validated(&body).expect("the report is valid");
    assert_eq!(report.default_branch, None);

    let mut body = valid_report();
    body["default_branch"] = json!("main");
    assert_eq!(
        validated(&body)
            .expect("the report is valid")
            .default_branch,
        Some("main".to_owned())
    );
}

#[test]
fn a_bad_repo_is_a_value_error_with_the_wire_wording() {
    let mut body = valid_report();
    body["repo"] = json!("Not A Repo");
    let issues = validated(&body).expect_err("the report is rejected");
    assert_eq!(
        issues,
        [ValidationIssue {
            r#type: "value_error".to_owned(),
            loc: vec![
                LocSegment::Field("body".to_owned()),
                LocSegment::Field("repo".to_owned())
            ],
            msg: "Value error, repo must look like 'owner/name', got 'Not A Repo'".to_owned(),
            input: Some(Value::from("Not A Repo")),
            ctx: None,
        }]
    );
    let value = serde_json::to_value(&issues[0]).expect("the issue serializes");
    assert_eq!(value["type"], "value_error");
    assert_eq!(value["loc"], json!(["body", "repo"]));
}

#[test]
fn a_bad_provider_is_rejected() {
    let mut body = valid_report();
    body["provider"] = json!("has space");
    let issues = validated(&body).expect_err("the report is rejected");
    assert_eq!(issues[0].r#type, "value_error");
    assert!(issues[0].msg.contains("provider must be 1-32 chars"));
}

#[test]
fn an_empty_job_list_is_too_short() {
    let mut body = valid_report();
    body["jobs"] = json!([]);
    let issues = validated(&body).expect_err("the report is rejected");
    assert_eq!(issues[0].r#type, "too_short");
    assert_eq!(
        issues[0].msg,
        "List should have at least 1 item after validation, not 0"
    );
    assert_eq!(
        issues[0].loc,
        [
            LocSegment::Field("body".to_owned()),
            LocSegment::Field("jobs".to_owned())
        ]
    );
    assert_eq!(issues[0].ctx, Some(json!({"min_length": 1})));
}

#[test]
fn a_blank_job_name_is_too_short() {
    let mut body = valid_report();
    body["jobs"][0]["name"] = json!("   ");
    let issues = validated(&body).expect_err("the report is rejected");
    assert_eq!(issues[0].r#type, "string_too_short");
    assert_eq!(issues[0].msg, "String should have at least 1 character");
    assert_eq!(
        issues[0].loc,
        [
            LocSegment::Field("body".to_owned()),
            LocSegment::Field("jobs".to_owned()),
            LocSegment::Index(0),
            LocSegment::Field("name".to_owned()),
        ]
    );
}

#[test]
fn a_long_url_and_runner_name_are_too_long() {
    let mut body = valid_report();
    body["jobs"][0]["url"] = json!("u".repeat(2049));
    body["jobs"][0]["runner_name"] = json!("r".repeat(256));
    let issues = validated(&body).expect_err("the report is rejected");
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].r#type, "string_too_long");
    assert_eq!(issues[1].r#type, "string_too_long");
    assert_eq!(issues[0].ctx, Some(json!({"max_length": 2048})));
}

#[test]
fn too_many_runner_labels_are_rejected() {
    let mut body = valid_report();
    body["jobs"][0]["runner_labels"] = json!(vec!["ubuntu-latest"; 51]);
    let issues = validated(&body).expect_err("the report is rejected");
    assert_eq!(issues[0].r#type, "too_long");
    assert_eq!(
        issues[0].msg,
        "List should have at most 50 items after validation, not 51"
    );
}

#[test]
fn a_long_runner_label_names_its_position() {
    let mut body = valid_report();
    body["jobs"][0]["runner_labels"] = json!(["ok", "x".repeat(256)]);
    let issues = validated(&body).expect_err("the report is rejected");
    assert_eq!(issues[0].r#type, "string_too_long");
    assert!(issues[0].loc.contains(&LocSegment::Index(1)));
}

#[test]
fn a_zero_attempt_is_below_the_minimum() {
    let mut body = valid_report();
    body["ci_run_attempt"] = json!(0);
    let issues = validated(&body).expect_err("the report is rejected");
    assert_eq!(issues[0].r#type, "greater_than_equal");
    assert_eq!(issues[0].msg, "Input should be greater than or equal to 1");
    assert_eq!(issues[0].ctx, Some(json!({"ge": 1})));
}

#[test]
fn duplicate_job_ids_are_a_model_error() {
    let mut body = valid_report();
    body["jobs"] = json!([
        {"ci_job_id": "1", "name": "a", "status": "passed"},
        {"ci_job_id": "1", "name": "b", "status": "failed"}
    ]);
    let issues = validated(&body).expect_err("the report is rejected");
    assert_eq!(issues[0].r#type, "value_error");
    assert_eq!(issues[0].msg, "Value error, duplicate ci_job_id in jobs");
    assert_eq!(issues[0].loc, [LocSegment::Field("body".to_owned())]);
    // A model-level rule reports no input: there is no one value to point at.
    let value = serde_json::to_value(&issues[0]).expect("the issue serializes");
    assert!(value.get("input").is_none());
}

#[test]
fn duplicate_job_ids_are_not_checked_while_a_field_is_invalid() {
    // A model-level rule is skipped when a field already failed to validate.
    let mut body = valid_report();
    body["pipeline"] = json!("");
    body["jobs"] = json!([
        {"ci_job_id": "1", "name": "a", "status": "passed"},
        {"ci_job_id": "1", "name": "b", "status": "failed"}
    ]);
    let issues = validated(&body).expect_err("the report is rejected");
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].r#type, "string_too_short");
}

#[test]
fn a_too_long_pipeline_and_commit_sha_are_reported_in_field_order() {
    let mut body = valid_report();
    body["pipeline"] = json!("p".repeat(513));
    body["commit_sha"] = json!("c".repeat(65));
    let issues = validated(&body).expect_err("the report is rejected");
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].loc[1], LocSegment::Field("pipeline".to_owned()));
    assert_eq!(issues[1].loc[1], LocSegment::Field("commit_sha".to_owned()));
}

#[test]
fn a_too_long_repo_is_a_length_error_not_a_value_error() {
    let mut body = valid_report();
    body["repo"] = json!(format!("{}/{}", "o".repeat(250), "n".repeat(10)));
    let issues = validated(&body).expect_err("the report is rejected");
    assert_eq!(issues[0].r#type, "string_too_long");
    assert_eq!(issues[0].ctx, Some(json!({"max_length": 255})));
}

#[test]
fn a_too_long_default_branch_and_run_id_are_rejected() {
    let mut body = valid_report();
    body["default_branch"] = json!("b".repeat(256));
    body["ci_run_id"] = json!("r".repeat(256));
    let issues = validated(&body).expect_err("the report is rejected");
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].r#type, "string_too_long");
    assert_eq!(issues[1].r#type, "string_too_long");
}

#[test]
fn more_than_a_thousand_jobs_are_rejected() {
    let mut body = valid_report();
    let jobs: Vec<Value> = (0..1001)
        .map(|index| json!({"ci_job_id": index.to_string(), "name": "a", "status": "passed"}))
        .collect();
    body["jobs"] = Value::Array(jobs);
    let issues = validated(&body).expect_err("the report is rejected");
    assert_eq!(issues[0].r#type, "too_long");
    assert_eq!(
        issues[0].msg,
        "List should have at most 1000 items after validation, not 1001"
    );
}

#[test]
fn fifty_runner_labels_are_allowed() {
    let mut body = valid_report();
    body["jobs"][0]["runner_labels"] = json!(vec!["ubuntu-latest"; 50]);
    let report = validated(&body).expect("fifty labels fit the limit");
    assert_eq!(report.jobs[0].runner_labels.len(), 50);
}

#[test]
fn a_job_timestamp_round_trips_in_the_accepted_shape() {
    let mut body = valid_report();
    body["jobs"][0]["started_at"] = json!("2026-10-20T09:00:00+00:00");
    let report = validated(&body).expect("the report is valid");
    let value = serde_json::to_value(&report).expect("the report serializes");
    assert_eq!(
        value["jobs"][0]["started_at"],
        "2026-10-20T09:00:00.000000Z"
    );
    assert_eq!(value["jobs"][0]["completed_at"], Value::Null);
}

#[test]
fn a_stored_payload_that_names_no_attempt_counts_as_the_first() {
    // The stored body is written by this server, so it always names an attempt;
    // the default is what a payload that names none is read as.
    let mut body = json!({
        "repo": "acme/app",
        "provider": "github",
        "pipeline": "ci.yml",
        "commit_sha": "abc",
        "branch": "main",
        "jobs": [{"ci_job_id": "1", "name": "n", "status": "passed"}]
    });
    let stored: PipelineReportIn =
        serde_json::from_value(body.clone()).expect("a stored payload deserializes");
    assert_eq!(stored.ci_run_attempt, 1);
    body["ci_run_attempt"] = json!(7);
    let stored: PipelineReportIn =
        serde_json::from_value(body).expect("a stored payload deserializes");
    assert_eq!(stored.ci_run_attempt, 7);
}

#[test]
fn a_job_validation_is_usable_on_its_own() {
    let body = json!({"ci_job_id": " 1 ", "name": " build ", "status": "failed"});
    let object = body.as_object().expect("the fixture is a JSON object");
    let job = JobResultIn::from_value(object, vec![LocSegment::Field("body".to_owned())])
        .expect("the job is valid");
    assert_eq!(job.ci_job_id, "1");
    assert_eq!(job.name, "build");
    assert_eq!(job.url, "");
    assert!(!job.summary);
}

#[test]
fn a_quarantine_body_may_omit_the_reason_and_expiry() {
    // `tests/parity/scenarios/quarantine.json` step 10 POSTs exactly this body,
    // and both optional fields have defaults, so serde must
    // accept the body and read the documented defaults.
    let body: QuarantineIn = serde_json::from_value(json!({"quarantined": false}))
        .expect("a body with only `quarantined` deserializes");
    assert!(!body.quarantined);
    assert_eq!((body.reason, body.expires_in_days), (None, None));
}

#[test]
fn a_quarantine_body_keeps_the_optional_fields_when_present() {
    let body: QuarantineIn = serde_json::from_value(json!({
        "quarantined": true,
        "reason": "known race in the tax service",
        "expires_in_days": 7
    }))
    .expect("a full body deserializes");
    assert!(body.quarantined);
    assert_eq!(
        body.reason.as_deref(),
        Some("known race in the tax service")
    );
    assert_eq!(body.expires_in_days, Some(7));

    let explicit_nulls: QuarantineIn = serde_json::from_value(
        json!({"quarantined": false, "reason": null, "expires_in_days": null}),
    )
    .expect("explicit nulls deserialize");
    assert_eq!(
        (explicit_nulls.reason, explicit_nulls.expires_in_days),
        (None, None)
    );
}

/// Reads an acknowledgement body the way the route reads one.
#[expect(
    clippy::panic_in_result_fn,
    reason = "the helper returns the issues but fails loudly on a fixture that is no object"
)]
fn acknowledged(body: &Value) -> Result<AcknowledgeIn, Vec<ValidationIssue>> {
    let object = body
        .as_object()
        .unwrap_or_else(|| panic!("the fixture is a JSON object: {body}"));
    AcknowledgeIn::from_value(object)
}

#[test]
fn an_acknowledge_body_requires_the_flag_and_the_days_it_names() {
    let issues = acknowledged(&json!({"reason": "known"}))
        .expect_err("a body without `acknowledged` is refused");
    assert_eq!(issues[0].r#type, "missing");
    assert_eq!(
        issues[0].loc,
        [
            LocSegment::Field("body".to_owned()),
            LocSegment::Field("acknowledged".to_owned())
        ]
    );

    let issues = acknowledged(&json!({"acknowledged": true}))
        .expect_err("an acknowledgement without `days` is refused");
    assert_eq!(issues[0].r#type, "missing");
    assert_eq!(
        issues[0].loc,
        [
            LocSegment::Field("body".to_owned()),
            LocSegment::Field("days".to_owned())
        ]
    );
}

/// A `days` that is no integer has recorded its own lax issue: the rulebook says
/// so once, with the lax kind, and never adds a range complaint on top.
#[test]
fn an_acknowledge_body_refuses_a_day_count_that_is_no_integer_by_its_lax_kind() {
    let issues = acknowledged(&json!({"acknowledged": true, "days": "3.5"}))
        .expect_err("a fraction is no integer of days");
    assert_eq!(issues.len(), 1, "the range rule adds nothing: {issues:?}");
    assert_eq!(issues[0].r#type, "int_parsing");
    assert_eq!(
        issues[0].loc,
        [
            LocSegment::Field("body".to_owned()),
            LocSegment::Field("days".to_owned())
        ]
    );
}

#[test]
fn an_acknowledge_body_takes_a_day_count_between_one_and_thirty() {
    for days in [1, 30] {
        let body = acknowledged(&json!({"acknowledged": true, "days": days}))
            .unwrap_or_else(|issues| panic!("{days} days is in range: {issues:?}"));
        assert_eq!(body.days, Some(days));
    }

    let issues = acknowledged(&json!({"acknowledged": true, "days": 0}))
        .expect_err("zero days is below the minimum");
    assert_eq!(issues[0].r#type, "greater_than_equal");
    assert_eq!(
        issues[0].loc,
        [
            LocSegment::Field("body".to_owned()),
            LocSegment::Field("days".to_owned())
        ]
    );

    let issues = acknowledged(&json!({"acknowledged": true, "days": 31}))
        .expect_err("31 days is past the maximum");
    assert_eq!(issues[0].r#type, "less_than_equal");
    assert_eq!(
        issues[0].loc,
        [
            LocSegment::Field("body".to_owned()),
            LocSegment::Field("days".to_owned())
        ]
    );
}

#[test]
fn an_acknowledge_body_refuses_a_reason_over_five_hundred_characters() {
    let issues = acknowledged(&json!({"acknowledged": true, "reason": "r".repeat(501)}))
        .expect_err("a 501-character reason is refused");
    assert_eq!(issues[0].r#type, "string_too_long");
    assert_eq!(
        issues[0].loc,
        [
            LocSegment::Field("body".to_owned()),
            LocSegment::Field("reason".to_owned())
        ]
    );
    let body = acknowledged(&json!({"acknowledged": true, "reason": "r".repeat(500), "days": 7}))
        .expect("500 characters is the longest reason");
    assert_eq!(body.reason, Some("r".repeat(500)));
}

#[test]
fn an_acknowledge_body_ignores_the_reason_and_days_when_not_acknowledging() {
    let body = acknowledged(&json!({"acknowledged": false, "reason": "known", "days": 999}))
        .expect("an un-acknowledge stores nothing, so the two fields are ignored");
    assert!(!body.acknowledged);
    assert_eq!((body.reason, body.days), (None, None));
}

#[test]
fn an_issue_link_body_may_omit_the_url() {
    // `tests/parity/scenarios/issue-links.json` step 8 PUTs exactly this body,
    // and `url` defaults to `None`.
    let body: IssueLinkIn = serde_json::from_value(json!({"number": 12}))
        .expect("a body with only `number` deserializes");
    assert_eq!((body.number, body.url), (12, None));
}

#[test]
fn an_issue_link_body_keeps_the_url_when_present() {
    let body: IssueLinkIn = serde_json::from_value(json!({
        "number": 14,
        "url": "https://github.com/o/r/issues/14"
    }))
    .expect("a body with a url deserializes");
    assert_eq!(body.number, 14);
    assert_eq!(
        body.url.as_deref(),
        Some("https://github.com/o/r/issues/14")
    );
}
