//! JUnit parsing behaviour: identity, outcome, Location and the retry elements.
//!
//! Each test ports one function of `backend/tests/test_parsing.py`, and the
//! names say what the behaviour is rather than what the Python mechanics were.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails loudly when the parser under test is wrong"
)]

use flakeradar_core::{
    model::ExecutionStatus, parsing::DETAILS_MAX, parsing::MESSAGE_MAX, parsing::ParseError,
    parsing::ParsedCase, parsing::fingerprint, parsing::parse_junit_xml,
};

/// Parses a report known to hold exactly one case.
fn one(xml: &str) -> ParsedCase {
    let cases = parse_junit_xml(xml.as_bytes(), 1).expect("the report parses");
    assert_eq!(cases.len(), 1, "expected one case");
    cases.into_iter().next().unwrap()
}

#[test]
fn fingerprint_is_stable() {
    // Byte-identical to the pre-fork fingerprint, so identities never drift.
    assert_eq!(
        fingerprint("unit", "tests.test_mod", "t1").as_str(),
        "2b8823c6c55be0a19a34111f08b687028f5273df"
    );
    assert_eq!(
        fingerprint("s", "c", "n"),
        fingerprint("s", "c", "n"),
        "the same identity hashes the same"
    );
    assert_ne!(
        fingerprint("s", "c", "n"),
        fingerprint("s", "c", "m"),
        "a different name is a different Test"
    );
}

#[test]
fn passed_case_without_location() {
    let case =
        one(r#"<testsuite name="unit"><testcase classname="c" name="t" time="0.5"/></testsuite>"#);
    assert_eq!(
        (
            case.suite.as_str(),
            case.classname.as_str(),
            case.name.as_str(),
            case.status
        ),
        ("unit", "c", "t", ExecutionStatus::Passed)
    );
    assert_eq!(case.duration, 0.5);
    assert_eq!((case.message.as_str(), case.details.as_str()), ("", ""));
    assert_eq!((case.file, case.line), (None, None));
}

#[test]
fn location_attributes_are_read_and_normalized() {
    let case = one(
        r#"<testsuites><testsuite name="s"><testcase classname="c" name="t" file="./tests/test_a.py" line="0"/></testsuite></testsuites>"#,
    );
    assert_eq!(case.file.as_deref(), Some("tests/test_a.py"));
    // Stored as reported; pytest's xunit1 output is 0-based.
    assert_eq!(case.line, Some(0));
}

#[test]
fn bad_line_is_ignored() {
    let case = one(
        r#"<testsuite name="s"><testcase classname="c" name="t" file="a.py" line="abc"/></testsuite>"#,
    );
    assert_eq!((case.file.as_deref(), case.line), (Some("a.py"), None));
}

#[test]
fn line_base_zero_shifts_lines_to_one_based() {
    let xml = r#"<testsuite name="s"><testcase classname="c" name="t" file="tests/a.py" line="11"/></testsuite>"#;
    assert_eq!(
        parse_junit_xml(xml.as_bytes(), 0).unwrap()[0].line,
        Some(12)
    );
    // The default and an explicit 1 store the reported line unchanged.
    assert_eq!(
        parse_junit_xml(xml.as_bytes(), 1).unwrap()[0].line,
        Some(11)
    );
}

#[test]
fn line_base_zero_shifts_line_zero_to_one() {
    // The case that previously lost its `#L` anchor entirely.
    let xml = r#"<testsuite name="s"><testcase classname="c" name="t" file="tests/a.py" line="0"/></testsuite>"#;
    assert_eq!(parse_junit_xml(xml.as_bytes(), 0).unwrap()[0].line, Some(1));
}

#[test]
fn line_base_zero_leaves_non_digit_lines_as_none() {
    let xml = r#"<testsuite name="s"><testcase classname="c" name="t" file="a.py" line="abc"/></testsuite>"#;
    let case = &parse_junit_xml(xml.as_bytes(), 0).unwrap()[0];
    assert_eq!((case.file.as_deref(), case.line), (Some("a.py"), None));
}

#[test]
fn failure_message_and_details_include_traceback_and_output() {
    let case = one(
        r#"<testsuite name="s"><testcase classname="c" name="t"><failure message="assert 1 == 2" type="AssertionError">Traceback...
tests/test_a.py:14: AssertionError</failure><system-out>hello out</system-out><system-err>err here</system-err></testcase></testsuite>"#,
    );
    assert_eq!(case.status, ExecutionStatus::Failed);
    assert_eq!(case.message, "assert 1 == 2");
    assert_eq!(
        case.details,
        "Traceback...\ntests/test_a.py:14: AssertionError\n\n--- stdout ---\nhello out\n\n--- stderr ---\nerr here"
    );
}

#[test]
fn error_without_message_attr_uses_first_body_line() {
    let case = one(
        r#"<testsuite name="s"><testcase classname="c" name="t"><error>
  panic: boom
goroutine 1</error></testcase></testsuite>"#,
    );
    assert_eq!(case.status, ExecutionStatus::Error);
    assert_eq!(case.message, "panic: boom");
    assert_eq!(case.details, "panic: boom\ngoroutine 1");
}

#[test]
fn skipped_keeps_message_but_no_details() {
    let case = one(
        r#"<testsuite name="s"><testcase classname="c" name="t"><skipped message="not on windows"/><system-out>x</system-out></testcase></testsuite>"#,
    );
    assert_eq!(
        (case.status, case.message.as_str(), case.details.as_str()),
        (ExecutionStatus::Skipped, "not on windows", "")
    );
}

#[test]
fn caps_are_applied() {
    let long_msg = "m".repeat(MESSAGE_MAX + 50);
    let long_body = "d".repeat(DETAILS_MAX + 50);
    let case = one(&format!(
        r#"<testsuite name="s"><testcase classname="c" name="t"><failure message="{long_msg}">{long_body}</failure></testcase></testsuite>"#
    ));
    assert_eq!(case.message.chars().count(), MESSAGE_MAX);
    assert_eq!(case.details.chars().count(), DETAILS_MAX);
    assert!(case.message.ends_with("…[truncated]"));
    assert!(case.details.ends_with("…[truncated]"));
}

#[test]
fn duplicate_testcases_are_all_returned() {
    let cases = parse_junit_xml(
        br#"<testsuite name="s"><testcase classname="c" name="t"><failure message="x"/></testcase><testcase classname="c" name="t"/></testsuite>"#,
        1,
    )
    .unwrap();
    assert_eq!(
        cases.iter().map(|case| case.status).collect::<Vec<_>>(),
        [ExecutionStatus::Failed, ExecutionStatus::Passed]
    );
}

#[test]
fn flaky_failure_becomes_a_failed_attempt_before_the_pass() {
    let xml = r#"<testsuite name="s"><testcase classname="c" name="t" time="1.2"><flakyFailure message="Timeout 30000ms exceeded" type="FAILURE" time="30.1"><stackTrace>Error: boom
 at x</stackTrace><system-out>out1</system-out></flakyFailure><system-out>final</system-out></testcase></testsuite>"#;
    let cases = parse_junit_xml(xml.as_bytes(), 1).unwrap();
    assert_eq!(
        cases.iter().map(|case| case.status).collect::<Vec<_>>(),
        [ExecutionStatus::Failed, ExecutionStatus::Passed]
    );
    assert_eq!(cases[0].message, "Timeout 30000ms exceeded");
    assert_eq!(cases[0].duration, 30.1);
    assert_eq!(
        cases[0].details,
        "Error: boom\n at x\n\n--- stdout ---\nout1"
    );
}

/// A `cargo-nextest` report, inlined because the frozen Python tree is deleted
/// at cutover.
const NEXTEST_FLAKY_FAIL: &str = include_str!("fixtures/nextest_flaky_fail.xml");

#[test]
fn nextest_flaky_marker_is_a_passed_attempt() {
    let cases = parse_junit_xml(NEXTEST_FLAKY_FAIL.as_bytes(), 1).unwrap();
    let flaky: Vec<&ParsedCase> = cases
        .iter()
        .filter(|case| {
            case.name
                .ends_with("invalidate_priority_readiness_drops_from_each_driver")
        })
        .collect();
    assert_eq!(
        flaky.iter().map(|case| case.status).collect::<Vec<_>>(),
        [ExecutionStatus::Failed, ExecutionStatus::Passed]
    );
    assert!(flaky[0].message.starts_with("thread 'embedding::reindexer"));
    assert!(flaky[0].message.ends_with("worker.rs:2706:17"));
    assert_eq!(flaky[0].duration, 0.398);
    assert_eq!(
        (
            flaky[1].message.as_str(),
            flaky[1].details.as_str(),
            flaky[1].duration
        ),
        ("", "", 0.412)
    );
    let other: Vec<(&str, ExecutionStatus)> = cases
        .iter()
        .filter(|case| {
            !case
                .name
                .ends_with("invalidate_priority_readiness_drops_from_each_driver")
        })
        .map(|case| (case.name.as_str(), case.status))
        .collect();
    assert_eq!(
        other,
        [(
            "embedding::chunker::tests::splits_on_headings",
            ExecutionStatus::Passed
        )]
    );
}

#[test]
fn nextest_flaky_marker_in_error_is_a_passed_attempt() {
    let xml = r#"<testsuite name="s"><testcase classname="c" name="t"><flakyError message="boom"/><flakyError message="boom"/><error message="test passed on attempt 3/3 but is configured to fail when flaky"/></testcase></testsuite>"#;
    let cases = parse_junit_xml(xml.as_bytes(), 1).unwrap();
    assert_eq!(
        cases.iter().map(|case| case.status).collect::<Vec<_>>(),
        [
            ExecutionStatus::Error,
            ExecutionStatus::Error,
            ExecutionStatus::Passed
        ]
    );
}

#[test]
fn nextest_flaky_marker_without_earlier_tries_stays_failed() {
    let xml = r#"<testsuite name="s"><testcase classname="c" name="t"><failure message="test passed on attempt 2/2 but is configured to fail when flaky"/></testcase></testsuite>"#;
    let cases = parse_junit_xml(xml.as_bytes(), 1).unwrap();
    assert_eq!(
        cases.iter().map(|case| case.status).collect::<Vec<_>>(),
        [ExecutionStatus::Failed]
    );
    assert_eq!(
        cases[0].message,
        "test passed on attempt 2/2 but is configured to fail when flaky"
    );
}

#[test]
fn message_that_only_starts_like_the_marker_stays_failed() {
    let xml = r#"<testsuite name="s"><testcase classname="c" name="t"><flakyFailure message="x"/><failure message="test passed on attempt 2/2 but is configured to fail when flaky, and more"/></testcase></testsuite>"#;
    let cases = parse_junit_xml(xml.as_bytes(), 1).unwrap();
    assert_eq!(
        cases.iter().map(|case| case.status).collect::<Vec<_>>(),
        [ExecutionStatus::Failed, ExecutionStatus::Failed]
    );
}

#[test]
fn rerun_elements_follow_the_failure() {
    let xml = r#"<testsuite name="s"><testcase classname="c" name="t"><failure message="first">t</failure><rerunError message="again"/><rerunError message="again"/></testcase></testsuite>"#;
    let cases = parse_junit_xml(xml.as_bytes(), 1).unwrap();
    assert_eq!(
        cases.iter().map(|case| case.status).collect::<Vec<_>>(),
        [
            ExecutionStatus::Failed,
            ExecutionStatus::Error,
            ExecutionStatus::Error
        ]
    );
}

#[test]
fn retry_without_message_uses_stacktrace_first_line() {
    let xml = r#"<testsuite name="s"><testcase classname="c" name="t"><flakyError><stackTrace>
  TypeError: x
  at y</stackTrace></flakyError></testcase></testsuite>"#;
    let case = &parse_junit_xml(xml.as_bytes(), 1).unwrap()[0];
    assert_eq!(case.message, "TypeError: x");
    assert_eq!(case.status, ExecutionStatus::Error);
    assert_eq!(case.duration, 0.0);
}

#[test]
fn declared_encoding_is_honored() {
    let body = r#"<?xml version="1.0" encoding="ISO-8859-1"?><testsuite name="s"><testcase classname="c" name="café"/></testsuite>"#;
    let cases = parse_junit_xml(&latin1(body), 1).unwrap();
    assert_eq!(cases[0].name, "café");
}

/// Encodes text as ISO-8859-1, which the parser must read from the declaration.
fn latin1(text: &str) -> Vec<u8> {
    text.chars()
        .map(|ch| u8::try_from(u32::from(ch)).expect("the fixture is Latin-1"))
        .collect()
}

#[test]
fn invalid_utf8_bytes_are_replaced_not_rejected() {
    let case = one_bytes(
        b"<testsuite name=\"s\"><testcase classname=\"c\" name=\"bad\xff\"/></testsuite>",
    );
    assert_eq!(case.name, "bad\u{fffd}");
}

/// Parses a report known to hold exactly one case, from raw bytes.
fn one_bytes(xml: &[u8]) -> ParsedCase {
    let cases = parse_junit_xml(xml, 1).expect("the report parses");
    assert_eq!(cases.len(), 1, "expected one case");
    cases.into_iter().next().unwrap()
}

#[test]
fn invalid_or_empty_reports_raise() {
    for body in [
        b"not xml at all".as_slice(),
        b"<testsuites></testsuites>".as_slice(),
    ] {
        assert!(
            matches!(
                parse_junit_xml(body, 1),
                Err(ParseError::InvalidXml { .. } | ParseError::NoTestCases)
            ),
            "{body:?} must be rejected"
        );
    }
}

#[test]
fn a_report_with_no_test_cases_says_so() {
    let error = parse_junit_xml(b"<testsuites name=\"empty\"/>", 1).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Report parsed but contained no test cases."
    );
}

#[test]
fn a_root_that_is_not_a_suite_is_an_invalid_report() {
    let error = parse_junit_xml(b"<properties/>", 1).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Not a valid JUnit XML report: Invalid format."
    );
}

#[test]
fn nameless_testcase_is_dropped() {
    let cases = parse_junit_xml(
        br#"<testsuite name="s"><testcase classname="c"/><testcase classname="c" name="n"/></testsuite>"#,
        1,
    )
    .unwrap();
    assert_eq!(
        cases
            .iter()
            .map(|case| (case.name.as_str(), case.status))
            .collect::<Vec<_>>(),
        [("n", ExecutionStatus::Passed)]
    );
}

#[test]
fn retry_time_that_is_not_a_number_is_zero() {
    let cases = parse_junit_xml(
        br#"<testsuite name="s"><testcase classname="c" name="r"><flakyFailure message="m" time="abc"/></testcase></testsuite>"#,
        1,
    )
    .unwrap();
    assert_eq!(
        cases
            .iter()
            .map(|case| (case.status, case.duration))
            .collect::<Vec<_>>(),
        [
            (ExecutionStatus::Failed, 0.0),
            (ExecutionStatus::Passed, 0.0)
        ]
    );
}

#[test]
fn failure_without_message_or_body_is_empty_strings() {
    let case = one(
        r#"<testsuite name="s"><testcase classname="c" name="n"><failure/></testcase></testsuite>"#,
    );
    assert_eq!(
        (case.status, case.message.as_str(), case.details.as_str()),
        (ExecutionStatus::Failed, "", "")
    );
}

#[test]
fn properties_child_of_testsuites_is_not_yielded() {
    let cases = parse_junit_xml(
        br#"<testsuites><properties/><testsuite name="s"><testcase classname="c" name="n"/></testsuite></testsuites>"#,
        1,
    )
    .unwrap();
    assert_eq!(
        cases
            .iter()
            .map(|case| (case.name.as_str(), case.status))
            .collect::<Vec<_>>(),
        [("n", ExecutionStatus::Passed)]
    );
}

#[test]
fn nested_suites_report_their_cases_under_the_outer_name() {
    // The Python model yields a suite's own cases before a nested suite's, and
    // the nested cases carry the outer suite's name.
    let cases = parse_junit_xml(
        br#"<testsuites name="nested"><testsuite name="outer"><testsuite name="inner"><testcase classname="c" name="test_inner"/></testsuite><testcase classname="c" name="test_outer"/></testsuite></testsuites>"#,
        1,
    )
    .unwrap();
    assert_eq!(
        cases
            .iter()
            .map(|case| (case.name.as_str(), case.suite.as_str()))
            .collect::<Vec<_>>(),
        [("test_outer", "outer"), ("test_inner", "outer")]
    );
}

#[test]
fn a_testcase_time_that_is_not_a_number_is_a_typed_error() {
    // User decision 2026-11-04: Python crashes with an unhandled ValueError
    // here and answers 500; Rust validates the attribute so the route can
    // answer 400.
    let error = parse_junit_xml(
        br#"<testsuite name="s"><testcase classname="c" name="t" time="abc"/></testsuite>"#,
        1,
    )
    .unwrap_err();
    assert_eq!(
        error,
        ParseError::InvalidTime {
            value: "abc".to_owned()
        }
    );
    assert_eq!(
        error.to_string(),
        "Not a valid JUnit XML report: testcase time 'abc' is not a number"
    );
}

#[test]
fn time_values_python_accepts_keep_their_meaning() {
    // Missing and empty read as 0.0, and a negative or fractional value is the
    // number itself.
    let cases = parse_junit_xml(
        br#"<testsuite name="s"><testcase classname="c" name="a"/><testcase classname="c" name="b" time=""/><testcase classname="c" name="c" time="-1.5"/><testcase classname="c" name="d" time="0.000123456789"/></testsuite>"#,
        1,
    )
    .unwrap();
    assert_eq!(
        cases
            .iter()
            .map(|case| (case.name.as_str(), case.duration))
            .collect::<Vec<_>>(),
        [
            ("a", 0.0),
            ("b", 0.0),
            ("c", -1.5),
            ("d", 0.000_123_456_789)
        ]
    );
}

#[test]
fn a_testcase_time_with_thousands_commas_is_read_as_junitparser_reads_it() {
    // junitparser's FloatAttr removes every comma before calling float(), so the
    // testcase's own time keeps Python's behaviour.
    let case =
        one(r#"<testsuite name="s"><testcase classname="c" name="t" time="1,234.5"/></testsuite>"#);
    assert_eq!(case.duration, 1234.5);
}

#[test]
fn a_retry_time_with_a_comma_is_not_a_number_so_it_is_zero() {
    // `_retry_outcome` calls plain float(), which rejects "1,5", so Python's
    // except branch stores 0.0. Only the testcase's own time is comma-stripped.
    let cases = parse_junit_xml(
        br#"<testsuite name="s"><testcase classname="c" name="t"><flakyFailure message="m" time="1,5"/></testcase></testsuite>"#,
        1,
    )
    .unwrap();
    assert_eq!(cases[0].duration, 0.0);
}

#[test]
fn underscores_between_digits_are_read_as_python_reads_them() {
    let cases = parse_junit_xml(
        br#"<testsuite name="s"><testcase classname="c" name="a" time="1_000.5"/><testcase classname="c" name="b"><flakyFailure message="m" time="1_0"/></testcase></testsuite>"#,
        1,
    )
    .unwrap();
    assert_eq!(cases[0].duration, 1000.5);
    assert_eq!(cases[1].duration, 10.0);
}

#[test]
fn unicode_decimal_digits_are_read_as_python_reads_them() {
    // Python's float() rewrites Arabic-Indic digits as ASCII before parsing.
    let xml = "<testsuite name=\"s\"><testcase classname=\"c\" name=\"a\" time=\"١٢٣.٥٩\"/><testcase classname=\"c\" name=\"b\"><flakyFailure message=\"m\" time=\"١_٢\"/></testcase></testsuite>";
    let cases = parse_junit_xml(xml.as_bytes(), 1).unwrap();
    assert_eq!(cases[0].duration, 123.59);
    assert_eq!(cases[1].duration, 12.0);
}

#[test]
fn surrounding_whitespace_is_ignored_as_python_ignores_it() {
    let cases = parse_junit_xml(
        br#"<testsuite name="s"><testcase classname="c" name="a" time=" 1.5 "/><testcase classname="c" name="b"><flakyFailure message="m" time=" 2.5 "/></testcase></testsuite>"#,
        1,
    )
    .unwrap();
    assert_eq!(cases[0].duration, 1.5);
    assert_eq!(cases[1].duration, 2.5);
}

#[test]
fn a_time_with_a_misplaced_underscore_is_an_error() {
    // Python's float() rejects this too, so the upload is not a number.
    for value in ["_1", "1_", "1__0"] {
        let xml = format!(
            r#"<testsuite name="s"><testcase classname="c" name="t" time="{value}"/></testsuite>"#
        );
        assert!(
            matches!(
                parse_junit_xml(xml.as_bytes(), 1),
                Err(ParseError::InvalidTime { .. })
            ),
            "time={value:?} must be rejected"
        );
    }
}

#[test]
fn an_unknown_declared_encoding_is_an_invalid_report() {
    let error = parse_junit_xml(
        br#"<?xml version="1.0" encoding="bogus-enc"?><testsuite name="s"><testcase classname="c" name="t"/></testsuite>"#,
        1,
    )
    .unwrap_err();
    assert_eq!(
        error,
        ParseError::InvalidXml {
            detail: "unknown encoding: bogus-enc".to_owned()
        }
    );
}

#[test]
fn a_declaration_without_an_encoding_reads_as_utf8() {
    let xml = r#"<?xml version="1.0"?><testsuite name="s"><testcase classname="c" name="café"/></testsuite>"#;
    assert_eq!(parse_junit_xml(xml.as_bytes(), 1).unwrap()[0].name, "café");
}

#[test]
fn a_declaration_may_quote_the_encoding_with_single_quotes() {
    let xml = r#"<?xml version="1.0" encoding='UTF-8'?><testsuite name="s"><testcase classname="c" name="t"/></testsuite>"#;
    assert_eq!(parse_junit_xml(xml.as_bytes(), 1).unwrap()[0].name, "t");
}

#[test]
fn a_declaration_with_a_malformed_encoding_attribute_falls_back_to_utf8() {
    // Each of these declarations cannot be read, so the bytes are decoded as
    // UTF-8 and the document parses normally.
    for declaration in [
        r#"<?xml version="1.0" encoding=UTF-8?>"#,
        r#"<?xml version="1.0" encoding="UTF-8?>"#,
        r#"<?xml version="1.0" encoding="?>"#,
        "<?xml version=\"1.0\" encoding=\"UTF-8\u{fffd}\"?>",
    ] {
        let xml = format!(
            "{declaration}<testsuite name=\"s\"><testcase classname=\"c\" name=\"t\"/></testsuite>"
        );
        // The declaration is not read as an encoding, so the bytes are decoded
        // as UTF-8 and the XML parser reports the malformed declaration.
        assert!(
            matches!(
                parse_junit_xml(xml.as_bytes(), 1),
                Err(ParseError::InvalidXml { .. })
            ),
            "{declaration} must be rejected by the XML parser"
        );
    }
}

#[test]
fn a_nested_suite_reports_its_bad_time_too() {
    let xml = r#"<testsuites><testsuite name="outer"><testsuite name="inner"><testcase classname="c" name="t" time="soon"/></testsuite></testsuite></testsuites>"#;
    assert_eq!(
        parse_junit_xml(xml.as_bytes(), 1),
        Err(ParseError::InvalidTime {
            value: "soon".to_owned()
        })
    );
}

#[test]
fn an_external_entity_is_never_fetched() {
    let error = parse_junit_xml(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE testsuite [<!ENTITY secret SYSTEM "file:///etc/passwd">]>
<testsuite name="external"><testcase classname="c" name="test_x"><failure message="leak">&secret;</failure></testcase></testsuite>"#,
        1,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("Not a valid JUnit XML report: "),
        "{error}"
    );
}

// The review of PR #135 found the reference and this parser disagreeing on the
// inputs below. Each expected value was recorded by running the frozen parser
// (`backend/app/parsing.py` at 86c98804) over the same bytes.

/// A failing case's details, as the reference parses them.
fn details(xml: &str) -> String {
    one(xml).details
}

#[test]
fn a_comment_inside_a_failure_body_merges_its_text() {
    // ElementTree drops the comment, so `<failure>a<!-- c -->b` is the body `ab`.
    assert_eq!(
        details(
            r#"<testsuite name="s"><testcase classname="c" name="t"><failure message="m">a<!-- c -->b</failure></testcase></testsuite>"#
        ),
        "ab"
    );
}

#[test]
fn a_comment_before_a_failure_body_keeps_the_body() {
    assert_eq!(
        details(
            r#"<testsuite name="s"><testcase classname="c" name="t"><failure message="m"><!-- c -->body</failure></testcase></testsuite>"#
        ),
        "body"
    );
}

#[test]
fn a_processing_instruction_inside_a_failure_body_merges_its_text() {
    assert_eq!(
        details(
            r#"<testsuite name="s"><testcase classname="c" name="t"><failure message="m">a<?pi x?>b</failure></testcase></testsuite>"#
        ),
        "ab"
    );
}

#[test]
fn a_comment_before_captured_output_keeps_the_output() {
    assert_eq!(
        details(
            r#"<testsuite name="s"><testcase classname="c" name="t"><failure message="m">body</failure><system-out><!-- c -->out text</system-out></testcase></testsuite>"#
        ),
        "body\n\n--- stdout ---\nout text"
    );
}

#[test]
fn a_comment_before_a_stack_trace_keeps_the_message_and_details() {
    let cases = parse_junit_xml(
        br#"<testsuite name="s"><testcase classname="c" name="t"><rerunFailure time="1"><stackTrace><!-- c -->stack line</stackTrace></rerunFailure></testcase></testsuite>"#,
        1,
    )
    .unwrap();
    assert_eq!(
        (cases[1].message.as_str(), cases[1].details.as_str()),
        ("stack line", "stack line")
    );
}

#[test]
fn iso_8859_1_is_true_latin_1() {
    // Python decodes the label with its own codec, so 0x80 is U+0080 rather than
    // the euro sign WHATWG's alias for the label would give it. The fingerprint
    // is the reference's.
    let xml = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><testsuite name=\"s\"><testcase classname=\"c\" name=\"na\x80me\"/></testsuite>";
    let case = &parse_junit_xml(xml, 1).unwrap()[0];
    assert_eq!(case.name.as_str(), "na\u{80}me");
    assert_eq!(
        fingerprint(&case.suite, &case.classname, &case.name).as_str(),
        "cb0ac818afbfe66855742dd391f8a4354ac90181"
    );
}

#[test]
fn us_ascii_falls_back_to_utf_8_as_python_does() {
    // Python's ASCII parse refuses the byte, and its lenient UTF-8 retry then
    // reads the name as `té`.
    let xml = b"<?xml version=\"1.0\" encoding=\"US-ASCII\"?><testsuite name=\"s\"><testcase classname=\"c\" name=\"t\xc3\xa9\"/></testsuite>";
    let case = &parse_junit_xml(xml, 1).unwrap()[0];
    assert_eq!(case.name.as_str(), "té");
    assert_eq!(
        fingerprint(&case.suite, &case.classname, &case.name).as_str(),
        "d88f048c1c68cc494cd4fd9997accfda5166e2fb"
    );
}

#[test]
fn a_namespaced_attribute_is_not_the_identity() {
    // ElementTree looks `name` up by its exact key, so `j:name` is not the name.
    let xml = r#"<testsuite name="s" xmlns:j="urn:x"><testcase j:name="x" name="u" classname="c"/></testsuite>"#;
    let case = &parse_junit_xml(xml.as_bytes(), 1).unwrap()[0];
    assert_eq!(case.name.as_str(), "u");
    assert_eq!(
        fingerprint(&case.suite, &case.classname, &case.name).as_str(),
        "f747ebcc1a2706ed5a71f59cd5aa984eb2f411e2"
    );
}

#[test]
fn a_namespaced_element_is_not_a_junit_element() {
    // `<j:failure>` is not a `<failure>`, and neither a `<j:testcase>` nor a
    // `<j:flakyFailure>` is the JUnit element of that name.
    let cases = parse_junit_xml(
        br#"<testsuite name="s" xmlns:j="urn:x"><testcase classname="c" name="t"><j:failure message="m">body</j:failure></testcase><j:testcase classname="c" name="skipped"/><testcase classname="c" name="r"><j:flakyFailure message="m"/></testcase></testsuite>"#,
        1,
    )
    .unwrap();
    assert_eq!(
        cases
            .iter()
            .map(|case| (case.name.as_str(), case.status))
            .collect::<Vec<_>>(),
        [
            ("t", ExecutionStatus::Passed),
            ("r", ExecutionStatus::Passed)
        ]
    );
}

#[test]
fn a_namespaced_suite_is_refused() {
    // Python's parser compares the fully qualified root tag, so a default
    // namespace on `<testsuite>` is `Invalid format.` rather than a report.
    for xml in [
        r#"<testsuite xmlns="urn:x" name="s"><testcase classname="c" name="t"/></testsuite>"#,
        r#"<testsuites xmlns="urn:x"><testsuite name="s"><testcase classname="c" name="t"/></testsuite></testsuites>"#,
    ] {
        assert_eq!(
            parse_junit_xml(xml.as_bytes(), 1),
            Err(ParseError::InvalidXml {
                detail: "Invalid format.".to_owned()
            })
        );
    }
}

#[test]
fn a_doctype_is_accepted() {
    // The reference's parser accepts a `DOCTYPE`; the approved rule restores it.
    for xml in [
        "<!DOCTYPE testsuite>\n<testsuite name=\"s\"><testcase classname=\"c\" name=\"t\"/></testsuite>",
        "<!DOCTYPE testsuite SYSTEM \"http://example.invalid/x.dtd\">\n<testsuite name=\"s\"><testcase classname=\"c\" name=\"t\"/></testsuite>",
        "<!DOCTYPE testsuite []>\n<testsuite name=\"s\"><testcase classname=\"c\" name=\"t\"/></testsuite>",
    ] {
        assert_eq!(
            parse_junit_xml(xml.as_bytes(), 1).unwrap()[0].status,
            ExecutionStatus::Passed,
            "{xml}"
        );
    }
}

#[test]
fn an_internal_entity_is_expanded() {
    assert_eq!(
        details(
            "<!DOCTYPE testsuite [<!ENTITY e \"expanded\">]>\n<testsuite name=\"s\"><testcase classname=\"c\" name=\"t\"><failure message=\"m\">&e;</failure></testcase></testsuite>"
        ),
        "expanded"
    );
}

#[test]
fn a_utf16_report_without_a_byte_order_mark_is_read_as_utf16() {
    // XML settles the encoding from the document's own bytes before the
    // declaration is read, and Python's parser does the same.
    let document = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><testsuite name=\"s\"><testcase classname=\"c\" name=\"t\" time=\"1.5\"/></testsuite>";
    let units: Vec<u16> = document.encode_utf16().collect();
    for bytes in [
        units
            .iter()
            .flat_map(|unit| unit.to_le_bytes())
            .collect::<Vec<u8>>(),
        units
            .iter()
            .flat_map(|unit| unit.to_be_bytes())
            .collect::<Vec<u8>>(),
    ] {
        let case = &parse_junit_xml(&bytes, 1).unwrap()[0];
        assert_eq!((case.name.as_str(), case.duration), ("t", 1.5));
    }
}

#[test]
fn an_invalid_character_reference_is_refused() {
    // ElementTree raises on both of these; `roxmltree` reads them as U+FFFD.
    for reference in ["&#xD800;", "&#x110000;", "&#55296;"] {
        let xml = format!(
            r#"<testsuite name="s"><testcase classname="c" name="t"><failure message="m">{reference}</failure></testcase></testsuite>"#
        );
        assert_eq!(
            parse_junit_xml(xml.as_bytes(), 1),
            Err(ParseError::InvalidXml {
                detail: "reference to invalid character number".to_owned()
            }),
            "{reference}"
        );
    }
}

#[test]
fn a_character_reference_in_cdata_or_a_comment_is_literal_text() {
    // Neither is a reference to the parser, so the same text is allowed there.
    assert_eq!(
        details(
            r#"<testsuite name="s"><testcase classname="c" name="t"><failure message="m"><![CDATA[&#xD800;]]></failure></testcase></testsuite>"#
        ),
        "&#xD800;"
    );
    assert_eq!(
        details(
            r#"<testsuite name="s"><testcase classname="c" name="t"><failure message="m"><!-- &#xD800; -->ok</failure></testcase></testsuite>"#
        ),
        "ok"
    );
    // A reference XML can carry keeps its character.
    assert_eq!(
        details(
            r#"<testsuite name="s"><testcase classname="c" name="t"><failure message="m">&#65;&#x42;</failure></testcase></testsuite>"#
        ),
        "AB"
    );
}

#[test]
fn an_encoding_python_does_not_know_is_refused() {
    let xml = b"<?xml version=\"1.0\" encoding=\"bogus\"?><testsuite name=\"s\"><testcase classname=\"c\" name=\"t\"/></testsuite>";
    assert_eq!(
        parse_junit_xml(xml, 1),
        Err(ParseError::InvalidXml {
            detail: "unknown encoding: bogus".to_owned()
        })
    );
}

#[test]
fn a_multi_byte_declared_encoding_is_refused_as_python_refuses_it() {
    // Python's parser refuses a multi-byte codec, and so does this one, whether
    // it names one outright or not at all. Only the wording after the prefix
    // differs, which is the parse-error difference the plan already records.
    for label in ["shift_jis", "UTF16", "utf-32"] {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"{label}\"?><testsuite name=\"s\"><testcase classname=\"c\" name=\"t\"/></testsuite>"
        );
        let error = parse_junit_xml(xml.as_bytes(), 1).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Not a valid JUnit XML report: "),
            "{label}: {error}"
        );
    }
}

#[test]
fn a_declared_encoding_python_reads_only_leniently_falls_back() {
    // Python's strict parse refuses the endian-specific UTF-16 spellings, and its
    // lenient retry then reads the bytes as UTF-8.
    let xml = b"<?xml version=\"1.0\" encoding=\"utf-16le\"?><testsuite name=\"s\"><testcase classname=\"c\" name=\"t\"/></testsuite>";
    assert_eq!(parse_junit_xml(xml, 1).unwrap()[0].name, "t");
}

#[test]
fn a_digit_from_a_wide_decimal_block_keeps_its_value() {
    // U+1D7CE..U+1D7FF is five adjacent `Nd` blocks, so the value is the digit's
    // position in the run modulo ten; Python reads each of these as named.
    let xml = "<testsuite name=\"s\"><testcase classname=\"c\" name=\"a\" time=\"\u{1D7D9}\"/><testcase classname=\"c\" name=\"b\" time=\"\u{1D7CE}\"/><testcase classname=\"c\" name=\"c\" time=\"\u{1D7FF}\"/><testcase classname=\"c\" name=\"d\" time=\"\u{FF15}\"/><testcase classname=\"c\" name=\"e\" time=\"\u{967}\u{968}\"/><testcase classname=\"c\" name=\"f\" time=\"\u{1D7D8}\"/></testsuite>";
    assert_eq!(
        parse_junit_xml(xml.as_bytes(), 1)
            .unwrap()
            .iter()
            .map(|case| (case.name.as_str(), case.duration))
            .collect::<Vec<_>>(),
        [
            ("a", 1.0),
            ("b", 0.0),
            ("c", 9.0),
            ("d", 5.0),
            ("e", 12.0),
            ("f", 0.0)
        ]
    );
}

#[test]
fn a_non_finite_time_is_an_upload_error() {
    // The reference stored `NaN` and infinities as it read them, and the read API
    // then served JSON `null` against a schema that says number; the approved
    // rule makes them upload errors like a non-numeric `time`.
    for time in ["NaN", "inf", "-inf", "1e999"] {
        let xml = format!(
            r#"<testsuite name="s"><testcase classname="c" name="t" time="{time}"/></testsuite>"#
        );
        assert_eq!(
            parse_junit_xml(xml.as_bytes(), 1),
            Err(ParseError::InvalidTime {
                value: time.to_owned()
            }),
            "{time}"
        );
    }
}

#[test]
fn a_non_finite_retry_time_is_an_upload_error() {
    let xml = r#"<testsuite name="s"><testcase classname="c" name="t"><rerunFailure message="m" time="NaN"/></testcase></testsuite>"#;
    assert_eq!(
        parse_junit_xml(xml.as_bytes(), 1),
        Err(ParseError::InvalidTime {
            value: "NaN".to_owned()
        })
    );
}

#[test]
fn text_after_a_child_element_is_not_the_failure_body() {
    // ElementTree's `.text` is what comes before the first child element, so the
    // body here is empty and the message falls back to it (also empty).
    let case = one(
        r#"<testsuite name="s"><testcase classname="c" name="t"><failure message=""><child/>after</failure></testcase></testsuite>"#,
    );
    assert_eq!((case.message.as_str(), case.details.as_str()), ("", ""));
}

#[test]
fn a_whitespace_only_section_is_dropped() {
    let case = one(
        r#"<testsuite name="s"><testcase classname="c" name="t"><failure message="m">b</failure><system-out></system-out><system-err>   </system-err></testcase></testsuite>"#,
    );
    assert_eq!(case.details, "b");
}

#[test]
fn an_unterminated_comment_is_the_parsers_error() {
    // The character-reference check leaves an unterminated comment alone, so the
    // answer is the XML parser's own refusal.
    let xml = r#"<testsuite name="s"><testcase classname="c" name="t"><failure message="m">body</failure></testcase><!-- &#xD800; "#;
    let error = parse_junit_xml(xml.as_bytes(), 1).unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("Not a valid JUnit XML report: "),
        "{error}"
    );
    assert!(
        !error
            .to_string()
            .contains("reference to invalid character number")
    );
}

#[test]
fn a_single_byte_code_page_label_is_decoded_by_its_own_codec() {
    // Python hands the label to its codec registry, so a single-byte code page
    // that its parser can use decodes exactly as that code page says.
    let cases: [(&str, &[u8], &str); 4] = [
        ("windows-1252", b"\x80", "n€"),
        ("iso-8859-15", b"\xa4", "n€"),
        ("koi8-r", b"\xc1", "nа"),
        ("unicode-1-1-utf-8", b"\xc3\xa9", "né"),
    ];
    for (label, bytes, expected) in cases {
        let mut xml = format!(
            "<?xml version=\"1.0\" encoding=\"{label}\"?><testsuite name=\"s\"><testcase classname=\"c\" name=\"n"
        )
        .into_bytes();
        xml.extend_from_slice(bytes);
        xml.extend_from_slice(b"\"/></testsuite>");
        let case = &parse_junit_xml(&xml, 1).unwrap()[0];
        assert_eq!(case.name.as_str(), expected, "{label}");
    }
}
