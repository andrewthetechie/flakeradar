//! The DTOs' wire shape and the upload validation rules.
//!
//! The shape is what the frontend, the CI actions and Fabro parse, so each test
//! names the field it pins. The validation tests port the rules Pydantic applied
//! to a Pipeline report.

#![expect(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "a test fails loudly when a DTO or its validation is wrong"
)]

use chrono::{DateTime, NaiveDate, TimeZone as _, Utc};
use flakeradar_core::{
    IssueLinkIn, JobResultIn, LocSegment, PipelineReportIn, QuarantineIn, ReportOut, ScorePointOut,
    TestOut, Tier, Trend, ValidationIssue,
};
use serde_json::{Value, json};

/// A Timestamp at a known instant.
fn at(seconds: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 20, 9, 0, seconds)
        .single()
        .expect("the fixture instant exists")
}

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

/// Parses and validates the fixture body.
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test helper returns the validation Result but fails loudly on a fixture that does not parse"
)]
fn validated(body: Value) -> Result<PipelineReportIn, Vec<ValidationIssue>> {
    serde_json::from_value::<PipelineReportIn>(body)
        .unwrap_or_else(|error| panic!("the fixture deserializes: {error}"))
        .validate()
}

#[test]
fn a_report_out_writes_timestamps_in_the_one_accepted_shape() {
    let report = ReportOut {
        id: 1,
        kind: "junit".to_owned(),
        repo: "acme/app".to_owned(),
        project: Some("default".to_owned()),
        commit_sha: "abc".to_owned(),
        branch: "main".to_owned(),
        ci_run_id: String::new(),
        status: "pending".to_owned(),
        error: None,
        counts: None,
        run_id: None,
        retry_count: 0,
        created_at: at(0),
        processed_at: None,
    };
    let value = serde_json::to_value(&report).expect("the report serializes");
    assert_eq!(value["created_at"], "2026-10-20T09:00:00.000000Z");
    assert_eq!(value["processed_at"], Value::Null);
    assert_eq!(value["error"], Value::Null);
    assert_eq!(value["counts"], Value::Null);
    assert_eq!(value["project"], "default");

    let back: ReportOut = serde_json::from_value(value).expect("the report round-trips");
    assert_eq!(back, report);
}

#[test]
fn timestamps_accept_z_and_an_offset_on_the_way_in() {
    let report = json!({
        "id": 1,
        "kind": "junit",
        "repo": "acme/app",
        "project": null,
        "commit_sha": "abc",
        "branch": "main",
        "ci_run_id": "",
        "status": "processed",
        "error": null,
        "counts": null,
        "run_id": null,
        "retry_count": 0,
        "created_at": "2026-10-20T09:00:00+00:00",
        "processed_at": "2026-10-20T11:00:00.5Z",
    });
    let parsed: ReportOut = serde_json::from_value(report).expect("both spellings parse");
    assert_eq!(parsed.created_at, at(0));
    assert_eq!(
        parsed.processed_at,
        Some(at(0) + chrono::Duration::hours(2) + chrono::Duration::milliseconds(500))
    );
}

#[test]
fn a_bad_timestamp_is_a_deserialization_error() {
    let report = json!({
        "id": 1,
        "kind": "junit",
        "repo": "acme/app",
        "project": null,
        "commit_sha": "abc",
        "branch": "main",
        "ci_run_id": "",
        "status": "pending",
        "error": null,
        "counts": null,
        "run_id": null,
        "retry_count": 0,
        "created_at": "yesterday",
        "processed_at": null,
    });
    serde_json::from_value::<ReportOut>(report).expect_err("yesterday is not a timestamp");
}

#[test]
fn dates_and_tiers_keep_their_python_spelling() {
    let point = ScorePointOut {
        day: NaiveDate::from_ymd_opt(2026, 10, 20).expect("the date exists"),
        flakiness_score: 0.5,
        confirmed_flake_count: 1,
        executions: 4,
        failures: 2,
    };
    let value = serde_json::to_value(&point).expect("the point serializes");
    assert_eq!(value["day"], "2026-10-20");
    assert_eq!(value["flakiness_score"], 0.5);
    assert_eq!(serde_json::to_value(Tier::Suspect).unwrap(), "suspect");
    assert_eq!(serde_json::to_value(Trend::Worsening).unwrap(), "worsening");
}

#[test]
fn a_test_out_keeps_every_field_the_consumers_read() {
    let test = TestOut {
        id: 1,
        repo: "acme/app".to_owned(),
        project: "default".to_owned(),
        fingerprint: "abc".to_owned(),
        suite: "unit".to_owned(),
        classname: "c".to_owned(),
        name: "t".to_owned(),
        file: None,
        line: None,
        flakiness_score: 0.0,
        tier: Tier::Stable,
        confirmed_flake_count: 0,
        failure_category: None,
        clean_streak: 3,
        fail_streak: 0,
        broken: false,
        trend: None,
        last_status: "passed".to_owned(),
        last_seen_at: at(0),
        quarantined: false,
        quarantined_at: None,
        quarantine_reason: None,
        quarantine_expires_at: None,
        github_issue_number: None,
        github_issue_url: None,
        github_issue_closed: false,
    };
    let value = serde_json::to_value(&test).expect("the test serializes");
    for field in [
        "id",
        "repo",
        "project",
        "fingerprint",
        "suite",
        "classname",
        "name",
        "file",
        "line",
        "flakiness_score",
        "tier",
        "confirmed_flake_count",
        "failure_category",
        "clean_streak",
        "fail_streak",
        "broken",
        "trend",
        "last_status",
        "last_seen_at",
        "quarantined",
        "quarantined_at",
        "quarantine_reason",
        "quarantine_expires_at",
        "github_issue_number",
        "github_issue_url",
        "github_issue_closed",
    ] {
        assert!(
            value.get(field).is_some(),
            "{field} is missing from {value}"
        );
    }
    assert_eq!(value["last_seen_at"], "2026-10-20T09:00:00.000000Z");
}

#[test]
fn a_valid_pipeline_report_is_normalized() {
    let report = validated(valid_report()).expect("the report is valid");
    assert_eq!(report.repo, "acme/app");
    assert_eq!(report.provider, "github");
    assert_eq!(report.jobs[0].name, "build");
    assert_eq!(report.ci_run_attempt, 1);
}

#[test]
fn a_blank_default_branch_is_unknown() {
    let mut body = valid_report();
    body["default_branch"] = json!("  ");
    let report = validated(body).expect("the report is valid");
    assert_eq!(report.default_branch, None);

    let mut body = valid_report();
    body["default_branch"] = json!("main");
    assert_eq!(
        validated(body).expect("the report is valid").default_branch,
        Some("main".to_owned())
    );
}

#[test]
fn a_bad_repo_is_a_value_error_with_pydantic_wording() {
    let mut body = valid_report();
    body["repo"] = json!("Not A Repo");
    let issues = validated(body).expect_err("the report is rejected");
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
    let issues = validated(body).expect_err("the report is rejected");
    assert_eq!(issues[0].r#type, "value_error");
    assert!(issues[0].msg.contains("provider must be 1-32 chars"));
}

#[test]
fn an_empty_job_list_is_too_short() {
    let mut body = valid_report();
    body["jobs"] = json!([]);
    let issues = validated(body).expect_err("the report is rejected");
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
    let issues = validated(body).expect_err("the report is rejected");
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
    let issues = validated(body).expect_err("the report is rejected");
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].r#type, "string_too_long");
    assert_eq!(issues[1].r#type, "string_too_long");
    assert_eq!(issues[0].ctx, Some(json!({"max_length": 2048})));
}

#[test]
fn too_many_runner_labels_are_rejected() {
    let mut body = valid_report();
    body["jobs"][0]["runner_labels"] = json!(vec!["ubuntu-latest"; 51]);
    let issues = validated(body).expect_err("the report is rejected");
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
    let issues = validated(body).expect_err("the report is rejected");
    assert_eq!(issues[0].r#type, "string_too_long");
    assert!(issues[0].loc.contains(&LocSegment::Index(1)));
}

#[test]
fn a_zero_attempt_is_below_the_minimum() {
    let mut body = valid_report();
    body["ci_run_attempt"] = json!(0);
    let issues = validated(body).expect_err("the report is rejected");
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
    let issues = validated(body).expect_err("the report is rejected");
    assert_eq!(issues[0].r#type, "value_error");
    assert_eq!(issues[0].msg, "Value error, duplicate ci_job_id in jobs");
    assert_eq!(issues[0].loc, [LocSegment::Field("body".to_owned())]);
    // Pydantic reports no input for a model-level rule.
    let value = serde_json::to_value(&issues[0]).expect("the issue serializes");
    assert!(value.get("input").is_none());
}

#[test]
fn duplicate_job_ids_are_not_checked_while_a_field_is_invalid() {
    // Pydantic skips a model validator when field validation already failed.
    let mut body = valid_report();
    body["pipeline"] = json!("");
    body["jobs"] = json!([
        {"ci_job_id": "1", "name": "a", "status": "passed"},
        {"ci_job_id": "1", "name": "b", "status": "failed"}
    ]);
    let issues = validated(body).expect_err("the report is rejected");
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].r#type, "string_too_short");
}

#[test]
fn a_too_long_pipeline_and_commit_sha_are_reported_in_field_order() {
    let mut body = valid_report();
    body["pipeline"] = json!("p".repeat(513));
    body["commit_sha"] = json!("c".repeat(65));
    let issues = validated(body).expect_err("the report is rejected");
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].loc[1], LocSegment::Field("pipeline".to_owned()));
    assert_eq!(issues[1].loc[1], LocSegment::Field("commit_sha".to_owned()));
}

#[test]
fn a_too_long_repo_is_a_length_error_not_a_value_error() {
    let mut body = valid_report();
    body["repo"] = json!(format!("{}/{}", "o".repeat(250), "n".repeat(10)));
    let issues = validated(body).expect_err("the report is rejected");
    assert_eq!(issues[0].r#type, "string_too_long");
    assert_eq!(issues[0].ctx, Some(json!({"max_length": 255})));
}

#[test]
fn a_too_long_default_branch_and_run_id_are_rejected() {
    let mut body = valid_report();
    body["default_branch"] = json!("b".repeat(256));
    body["ci_run_id"] = json!("r".repeat(256));
    let issues = validated(body).expect_err("the report is rejected");
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
    let issues = validated(body).expect_err("the report is rejected");
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
    let report = validated(body).expect("fifty labels fit the limit");
    assert_eq!(report.jobs[0].runner_labels.len(), 50);
}

#[test]
fn a_job_timestamp_round_trips_in_the_accepted_shape() {
    let mut body = valid_report();
    body["jobs"][0]["started_at"] = json!("2026-10-20T09:00:00+00:00");
    let report = validated(body).expect("the report is valid");
    let value = serde_json::to_value(&report).expect("the report serializes");
    assert_eq!(
        value["jobs"][0]["started_at"],
        "2026-10-20T09:00:00.000000Z"
    );
    assert_eq!(value["jobs"][0]["completed_at"], Value::Null);
}

#[test]
fn a_job_validation_is_usable_on_its_own() {
    let job: JobResultIn =
        serde_json::from_value(json!({"ci_job_id": " 1 ", "name": " build ", "status": "failed"}))
            .expect("the job deserializes");
    let job = job.validate(3).expect("the job is valid");
    assert_eq!(job.ci_job_id, "1");
    assert_eq!(job.name, "build");
    assert_eq!(job.url, "");
    assert!(!job.summary);
}

#[test]
fn a_quarantine_body_may_omit_the_reason_and_expiry() {
    // `tests/parity/scenarios/quarantine.json` step 10 POSTs exactly this body,
    // and Python's `QuarantineIn` defaults both optional fields, so serde must
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

#[test]
fn an_issue_link_body_may_omit_the_url() {
    // `tests/parity/scenarios/issue-links.json` step 8 PUTs exactly this body,
    // and Python's `IssueLinkIn` defaults `url` to `None`.
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
