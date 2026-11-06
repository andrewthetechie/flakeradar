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
use flakeradar_core::dto::lax_bool;
use flakeradar_core::{
    BodyRefusal, ExecutionStatus, IssueLinkIn, JobResultIn, LocSegment, PipelineReportIn,
    QuarantineIn, ReportKind, ReportOut, ReportStatus, ScorePointOut, TestOut, Tier, Trend,
    ValidationIssue,
};
use serde::Deserialize;
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
fn a_report_out_writes_timestamps_in_the_one_accepted_shape() {
    let report = ReportOut {
        id: 1,
        kind: ReportKind::Junit,
        repo: "acme/app".to_owned(),
        project: Some("default".to_owned()),
        commit_sha: "abc".to_owned(),
        branch: "main".to_owned(),
        ci_run_id: String::new(),
        status: ReportStatus::Pending,
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
        last_status: ExecutionStatus::Passed,
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
fn a_bad_repo_is_a_value_error_with_pydantic_wording() {
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
    // The stored body is written by this port, so it always names an attempt;
    // the default is what Pydantic gave a payload that named none.
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

/// A body whose one field is read as a required lax boolean.
#[derive(Debug, Deserialize)]
struct LaxFlag {
    /// The boolean under test.
    #[serde(deserialize_with = "lax_bool::deserialize")]
    flag: bool,
}

/// A body whose one field is read as an optional lax boolean.
#[derive(Debug, Deserialize)]
struct OptionalLaxFlag {
    /// The boolean under test, absent or `null` when the body gives neither.
    #[serde(default, deserialize_with = "lax_bool::optional")]
    flag: Option<bool>,
}

/// Reads `value` as a required lax boolean.
fn read(value: &Value) -> Result<bool, serde_json::Error> {
    serde_json::from_value::<LaxFlag>(json!({ "flag": value })).map(|body| body.flag)
}

#[test]
fn a_lax_boolean_takes_exactly_the_values_pydantic_reads() {
    // The rows are Pydantic 2.14.2's own answers for a `bool` field, checked
    // against `backend/.venv`: `1.0` and `0.0` are booleans (the Rust port used
    // to refuse them), and a value of another JSON type is `bool_type` where a
    // number or string of the right type it cannot read is `bool_parsing`.
    let accepted: [(&str, Value, bool); 8] = [
        ("true", json!(true), true),
        ("false", json!(false), false),
        ("1", json!(1), true),
        ("0", json!(0), false),
        ("1.0", json!(1.0), true),
        ("0.0", json!(0.0), false),
        ("the JSON string \"1\"", json!("1"), true),
        ("the JSON string \"0\"", json!("0"), false),
    ];
    for (label, value, expected) in accepted {
        match read(&value) {
            Ok(flag) => assert_eq!(flag, expected, "{label}"),
            Err(error) => panic!("{label} was refused: {error}"),
        }
    }
    // `-0.0` is the same float as `0.0`, and Pydantic reads it as `false`.
    match read(&json!(-0.0)) {
        Ok(flag) => assert!(!flag, "-0.0"),
        Err(error) => panic!("-0.0 was refused: {error}"),
    }
    // Every spelling of the word set is accepted in any case.
    let spellings: [(&str, bool); 10] = [
        ("true", true),
        ("false", false),
        ("yes", true),
        ("no", false),
        ("on", true),
        ("off", false),
        ("t", true),
        ("f", false),
        ("y", true),
        ("n", false),
    ];
    for (text, expected) in spellings {
        for spelling in [text.to_lowercase(), text.to_uppercase()] {
            let value = Value::from(spelling.clone());
            match read(&value) {
                Ok(flag) => assert_eq!(flag, expected, "{spelling}"),
                Err(error) => panic!("{spelling} was refused: {error}"),
            }
        }
    }
}

#[test]
fn a_refused_lax_boolean_carries_pydantics_kind_and_wording() {
    let wrong_type = lax_bool::WRONG_TYPE_MESSAGE;
    let unparsable = lax_bool::UNPARSABLE_MESSAGE;
    let refused: [(&str, Value, &str); 18] = [
        ("1.5", json!(1.5), wrong_type),
        ("-1.5", json!(-1.5), wrong_type),
        ("a fraction near one", json!(1.000_000_000_1), wrong_type),
        ("a tiny fraction", json!(1e-20), wrong_type),
        (
            "a whole float past the integer range",
            json!(1e20),
            wrong_type,
        ),
        (
            "a whole float below the integer range",
            json!(-1e20),
            wrong_type,
        ),
        ("null", json!(null), wrong_type),
        ("an array", json!([]), wrong_type),
        ("an object", json!({}), wrong_type),
        (
            "a number past i64",
            json!(18_446_744_073_709_551_615_u64),
            wrong_type,
        ),
        ("2", json!(2), unparsable),
        ("2.0", json!(2.0), unparsable),
        ("-2.0", json!(-2.0), unparsable),
        ("-1", json!(-1), unparsable),
        ("an empty string", json!(""), unparsable),
        ("maybe", json!("maybe"), unparsable),
        ("a float spelled as a string", json!("1.0"), unparsable),
        ("a padded true", json!("  true  "), unparsable),
    ];
    for (label, value, expected) in refused {
        match read(&value) {
            Err(error) => assert_eq!(error.to_string(), expected, "{label}"),
            Ok(flag) => panic!("{label} was read as {flag}"),
        }
    }
}

#[test]
fn an_optional_lax_boolean_reads_null_and_absence_as_none() {
    for body in [json!({}), json!({"flag": null})] {
        let parsed: OptionalLaxFlag =
            serde_json::from_value(body).expect("an absent or null flag deserializes");
        assert_eq!(parsed.flag, None);
    }
    let present: OptionalLaxFlag =
        serde_json::from_value(json!({"flag": "yes"})).expect("a lax spelling deserializes");
    assert_eq!(present.flag, Some(true));
    let refused = serde_json::from_value::<OptionalLaxFlag>(json!({"flag": []}))
        .expect_err("an array is not a boolean");
    assert_eq!(refused.to_string(), lax_bool::WRONG_TYPE_MESSAGE);
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

/// The loc of a bad field in the request body.
fn body(field: &str) -> Vec<LocSegment> {
    vec![
        LocSegment::Field("body".to_owned()),
        LocSegment::Field(field.to_owned()),
    ]
}

#[test]
fn a_validation_issue_keeps_pydantics_kind_loc_and_wording() {
    let cases = [
        (
            ValidationIssue::int_parsing(body("number"), json!("twelve")),
            "int_parsing",
            "Input should be a valid integer, unable to parse string as an integer",
        ),
        (
            ValidationIssue::float_parsing(body("score"), json!("high")),
            "float_parsing",
            "Input should be a valid number, unable to parse string as a number",
        ),
        (
            ValidationIssue::bool_parsing(body("quarantined"), json!("yes")),
            "bool_parsing",
            "Input should be a valid boolean, unable to interpret input",
        ),
        (
            ValidationIssue::model_attributes_type(body(""), json!("not json")),
            "model_attributes_type",
            "Input should be a valid dictionary or object to extract fields from",
        ),
        (
            ValidationIssue::missing(body("quarantined")),
            "missing",
            "Field required",
        ),
        (
            ValidationIssue::less_than_equal(body("expires_in_days"), json!(366), 365),
            "less_than_equal",
            "Input should be less than or equal to 365",
        ),
        (
            ValidationIssue::literal_error(
                body("sort"),
                json!("bogus"),
                "'score', 'last_seen' or 'proven'",
            ),
            "literal_error",
            "Input should be 'score', 'last_seen' or 'proven'",
        ),
        (
            ValidationIssue::json_invalid(
                vec![LocSegment::Field("body".to_owned()), LocSegment::Index(6)],
                "expected value",
            ),
            "json_invalid",
            "JSON decode error",
        ),
    ];
    for (issue, kind, msg) in cases {
        assert_eq!(issue.r#type, kind);
        assert_eq!(issue.msg, msg);
        let written = serde_json::to_value(&issue).expect("the issue serializes");
        assert_eq!(written["type"], json!(kind));
        assert_eq!(written["msg"], json!(msg));
    }
}

#[test]
fn a_validation_issue_reports_the_constraint_it_broke() {
    let le = serde_json::to_value(ValidationIssue::less_than_equal(
        body("expires_in_days"),
        json!(366),
        365,
    ))
    .expect("the issue serializes");
    assert_eq!(le["ctx"], json!({"le": 365}));
    assert_eq!(le["input"], json!(366));
    assert_eq!(le["loc"], json!(["body", "expires_in_days"]));

    let literal = serde_json::to_value(ValidationIssue::literal_error(
        body("sort"),
        json!("bogus"),
        "'score', 'last_seen' or 'proven'",
    ))
    .expect("the issue serializes");
    assert_eq!(
        literal["ctx"],
        json!({"expected": "'score', 'last_seen' or 'proven'"})
    );

    let invalid = serde_json::to_value(ValidationIssue::json_invalid(
        vec![LocSegment::Field("body".to_owned()), LocSegment::Index(6)],
        "expected value",
    ))
    .expect("the issue serializes");
    assert_eq!(invalid["loc"], json!(["body", 6]));
    assert_eq!(invalid["ctx"], json!({"error": "expected value"}));
}
