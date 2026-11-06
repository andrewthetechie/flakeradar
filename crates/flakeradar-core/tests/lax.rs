//! The lax readings a request body's values get, one value at a time.
//!
//! Every expected `type`, `loc` and accepted spelling is a recorded answer:
//! `"1_0"` is 10, `1e300` is `int_parsing_size`, `false` as a boolean is `false`,
//! and a date alone is that day's midnight.

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails loudly when a reading is not the recorded one"
)]

use chrono::{DateTime, TimeZone as _, Utc};
use flakeradar_core::dto::LocSegment;
use flakeradar_core::lax::{self, Fields};
use serde_json::{Value, json};

/// Where the fixture's field sits, so an issue names it.
fn at() -> Vec<LocSegment> {
    vec![
        LocSegment::Field("body".to_owned()),
        LocSegment::Field("value".to_owned()),
    ]
}

/// The `(type, loc)` an expected issue carries.
fn kind_of(
    value: Result<impl std::fmt::Debug, Box<flakeradar_core::dto::ValidationIssue>>,
) -> String {
    match value {
        Ok(reading) => panic!("the value is refused, not read as {reading:?}"),
        Err(issue) => {
            assert_eq!(issue.loc, at(), "the issue names the field");
            issue.r#type
        }
    }
}

#[test]
fn a_string_is_read_only_from_a_json_string() {
    assert_eq!(
        lax::string(&json!("acme/app"), at()).expect("a string reads"),
        "acme/app"
    );
    for value in [json!(5), json!(null), json!(true), json!([]), json!({})] {
        assert_eq!(kind_of(lax::string(&value, at())), "string_type", "{value}");
    }
}

#[test]
fn an_integer_takes_every_number_the_record_took() {
    for (value, expected) in [
        (json!(2), 2),
        (json!(2.0), 2),
        (json!(-0.0), 0),
        (json!(true), 1),
        (json!(false), 0),
        (json!("2"), 2),
        (json!(" 2 "), 2),
        (json!("+2"), 2),
        (json!("-0"), 0),
        (json!("1_0"), 10),
        (json!("10.000"), 10),
        (json!("0.0"), 0),
    ] {
        assert_eq!(
            lax::integer(&value, at()).expect("the value is an integer"),
            expected,
            "{value}"
        );
    }
}

#[test]
fn an_integer_refusal_names_the_kind_the_record_used() {
    for (value, expected) in [
        (json!(2.5), "int_from_float"),
        (json!("2.5"), "int_parsing"),
        (json!("1e3"), "int_parsing"),
        (json!("5."), "int_parsing"),
        (json!(".5"), "int_parsing"),
        (json!("1_0.5"), "int_parsing"),
        (json!("1__0"), "int_parsing"),
        (json!("_1"), "int_parsing"),
        (json!("1_"), "int_parsing"),
        (json!(""), "int_parsing"),
        (json!("0x10"), "int_parsing"),
        // A run past `i64` is no integer here, although a wider integer holds it.
        (json!("9223372036854775808"), "int_parsing"),
        (json!("١٢"), "int_parsing"),
        // A number past `i64` is the size a float is refused with.
        (json!(1e300), "int_parsing_size"),
        (json!(9_223_372_036_854_775_808_u64), "int_parsing_size"),
        (json!(null), "int_type"),
        (json!([]), "int_type"),
        (json!({}), "int_type"),
    ] {
        assert_eq!(kind_of(lax::integer(&value, at())), expected, "{value}");
    }
}

#[test]
fn a_boolean_takes_exactly_the_values_the_wire_reads() {
    for (value, expected) in [
        (json!(true), true),
        (json!(false), false),
        (json!(1), true),
        (json!(0), false),
        (json!(1.0), true),
        (json!("yes"), true),
        (json!("No"), false),
        (json!("T"), true),
        (json!("off"), false),
    ] {
        assert_eq!(
            lax::boolean(&value, at()).expect("the value is a boolean"),
            expected,
            "{value}"
        );
    }
    for (value, expected) in [
        (json!(2), "bool_parsing"),
        (json!("maybe"), "bool_parsing"),
        (json!(1e300), "bool_type"),
        (json!(null), "bool_type"),
        (json!([]), "bool_type"),
    ] {
        assert_eq!(kind_of(lax::boolean(&value, at())), expected, "{value}");
    }
}

#[test]
fn a_timestamp_takes_every_iso_form_and_epoch_the_record_took() {
    let midnight = Utc
        .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
        .single()
        .expect("a fixed instant");
    for (value, expected) in [
        (json!("2026-01-01T00:00:00Z"), midnight),
        (json!("2026-01-01T00:00:00z"), midnight),
        (json!("2026-01-01t00:00:00Z"), midnight),
        (json!("2026-01-01T00:00:00"), midnight),
        (json!("2026-01-01 00:00:00"), midnight),
        (json!("2026-01-01T00:00"), midnight),
        (json!("2026-01-01 00:00"), midnight),
        (json!("2026-01-01"), midnight),
        (json!("2026-01-01T00:00:00.000000Z"), midnight),
        (json!("2026-01-01T00:00:00,000000Z"), midnight),
        (json!(1_767_225_600_i64), midnight),
        (json!(1_767_225_600.0), midnight),
    ] {
        assert_eq!(
            lax::timestamp(&value, at()).expect("the value is an instant"),
            expected,
            "{value}"
        );
    }
    // An offset is absolute: the same instant, written from two places.
    let with_offset = DateTime::parse_from_rfc3339("2026-01-01T02:00:00+02:00")
        .expect("a fixed instant")
        .with_timezone(&Utc);
    for value in [
        json!("2026-01-01T02:00:00+02:00"),
        json!("2026-01-01T02:00:00+0200"),
    ] {
        assert_eq!(
            lax::timestamp(&value, at()).expect("the value is an instant"),
            with_offset,
            "{value}"
        );
    }
    // A fraction that rounds to the whole of the next second, and a shape
    // `chrono` alone would not read: a comma and a lowercase `z` together.
    for (value, expected) in [
        (json!(0.999_999_999_9), "1970-01-01T00:00:01.000000Z"),
        (
            json!("2026-01-01T00:00:00,5z"),
            "2026-01-01T00:00:00.500000Z",
        ),
    ] {
        let instant = lax::timestamp(&value, at()).expect("the value is an instant");
        assert_eq!(
            instant.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string(),
            expected,
            "{value}"
        );
    }
    // A fraction of an epoch second, on either side of the epoch.
    for (value, expected) in [
        (json!(1_767_225_600.5), "2026-01-01T00:00:00.500000Z"),
        (json!(-1.5), "1969-12-31T23:59:58.500000Z"),
        (json!(-1), "1969-12-31T23:59:59.000000Z"),
        (json!(0), "1970-01-01T00:00:00.000000Z"),
    ] {
        let instant = lax::timestamp(&value, at()).expect("the value is an instant");
        assert_eq!(
            instant.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string(),
            expected,
            "{value}"
        );
    }
}

#[test]
fn a_timestamp_refusal_names_the_kind_the_record_used() {
    for (value, expected) in [
        (json!(""), "datetime_from_date_parsing"),
        (json!("2026-1-1"), "datetime_from_date_parsing"),
        (json!("20260101T000000Z"), "datetime_from_date_parsing"),
        (
            json!("2026-01-01T00:00:00+99:00"),
            "datetime_from_date_parsing",
        ),
        // A tail whose fifth byte from the end sits inside a multi-byte
        // character is refused rather than sliced through.
        (json!("2026-01-01Tð00"), "datetime_from_date_parsing"),
        (json!("2026-01-01T00:00:00é"), "datetime_from_date_parsing"),
        (json!(true), "datetime_type"),
        (json!([]), "datetime_type"),
        (json!({}), "datetime_type"),
        (json!(1e300), "int_parsing_size"),
    ] {
        assert_eq!(kind_of(lax::timestamp(&value, at())), expected, "{value}");
    }
}

#[test]
fn a_list_is_read_only_from_a_json_list() {
    assert_eq!(
        lax::list_of(&json!(["a", "b"]), at()).expect("a list reads"),
        [json!("a"), json!("b")]
    );
    for value in [json!("x"), json!(null), json!(5)] {
        assert_eq!(kind_of(lax::list_of(&value, at())), "list_type", "{value}");
    }
}

#[test]
fn a_literal_takes_one_of_its_options_and_says_which_it_wanted() {
    let options = ["passed", "failed", "skipped"];
    assert_eq!(
        lax::literal(&json!("failed"), &options, at()).expect("an option"),
        "failed"
    );
    for value in [json!("weird"), json!(5), json!(null)] {
        assert_eq!(
            kind_of(lax::literal(&value, &options, at())),
            "literal_error"
        );
    }
    assert_eq!(
        lax::expected_list(&options),
        "'passed', 'failed' or 'skipped'"
    );
    assert_eq!(lax::expected_list(&["only"]), "'only'");
    assert_eq!(lax::expected_list(&["a", "b"]), "'a' or 'b'");
    assert_eq!(lax::expected_list(&[]), "");
}

#[test]
fn a_field_is_required_or_optional_and_its_loc_names_its_place() {
    let body = json!({"jobs": [{"name": "a"}]});
    let object = body.as_object().expect("an object");
    let fields = Fields::new(object, vec![LocSegment::Field("body".to_owned())]);
    let mut issues = Vec::new();
    assert_eq!(fields.field("jobs"), Some(&json!([{"name": "a"}])));
    assert_eq!(fields.field("missing"), None);
    assert!(fields.required("missing", &mut issues).is_none());
    assert_eq!(
        fields.required("jobs", &mut issues),
        Some(&json!([{"name": "a"}]))
    );
    assert_eq!(issues.len(), 1);
    assert_eq!(issues.first().expect("one issue").r#type, "missing");
    assert_eq!(
        issues.first().expect("one issue").loc,
        [
            LocSegment::Field("body".to_owned()),
            LocSegment::Field("missing".to_owned())
        ]
    );
    assert_eq!(
        fields.item_loc("jobs", 0),
        [
            LocSegment::Field("body".to_owned()),
            LocSegment::Field("jobs".to_owned()),
            LocSegment::Index(0)
        ]
    );
    assert_eq!(
        fields.loc("jobs"),
        [
            LocSegment::Field("body".to_owned()),
            LocSegment::Field("jobs".to_owned())
        ]
    );
}

#[test]
fn a_nul_is_refused_wherever_it_appears_in_text() {
    let mut issues = Vec::new();
    lax::refuse_nul("clean", at(), &mut issues);
    assert_eq!(issues.len(), 0);
    lax::refuse_nul("a\u{0}b", at(), &mut issues);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues.first().expect("one issue").r#type, "value_error");
    assert_eq!(issues.first().expect("one issue").loc, at());
}

#[test]
fn a_number_spelled_as_a_string_is_read_as_the_wire_reads() {
    for (text, expected) in [
        ("0", Some(0)),
        ("-0", Some(0)),
        ("+7", Some(7)),
        ("1_000", Some(1000)),
        ("  -12  ", Some(-12)),
        ("7.0", Some(7)),
        ("-7.000", Some(-7)),
        ("7.5", None),
        ("7.", None),
        (".7", None),
        ("1e3", None),
        ("١", None),
        ("", None),
        ("9223372036854775808", None),
    ] {
        assert_eq!(lax::text_int(text), expected, "{text}");
    }
}

#[test]
fn a_timestamp_that_names_no_instant_is_refused_with_a_detail() {
    let issue = lax::timestamp(&Value::from("2026-1-1"), at()).expect_err("no instant");
    assert_eq!(
        issue.msg,
        "Input should be a valid datetime or date, input is too short"
    );
    assert!(
        issue.ctx.is_some(),
        "the refusal carries the parser's own error"
    );
    let issue = lax::timestamp(&Value::from("not a date at all"), at()).expect_err("no instant");
    assert_eq!(
        issue.msg,
        "Input should be a valid datetime or date, invalid date"
    );
}
