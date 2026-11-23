//! The wire readers: the lax boolean and the refusal vocabulary.
//!
//! The readers are what an uploaded body is parsed with, so each case pins the
//! exact values a reader takes and the wording it refuses with.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails loudly when a DTO or its validation is wrong"
)]

use flakeradar_core::dto::lax_bool;
use flakeradar_core::dto::{LocSegment, ValidationIssue};
use serde::Deserialize;
use serde_json::{Value, json};

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
fn a_lax_boolean_takes_exactly_the_documented_values() {
    // Every row is a recorded answer for a `bool` field: `1.0` and `0.0` are
    // booleans, and a value of another JSON type is `bool_type` where a number or
    // string of the right type that spells no boolean is `bool_parsing`.
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
    // `-0.0` is the same float as `0.0`, and it reads as `false`.
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
fn a_refused_lax_boolean_carries_the_kind_and_wording() {
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

/// The loc of a bad field in the request body.
fn body(field: &str) -> Vec<LocSegment> {
    vec![
        LocSegment::Field("body".to_owned()),
        LocSegment::Field(field.to_owned()),
    ]
}

#[test]
fn a_validation_issue_keeps_its_kind_loc_and_wording() {
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
