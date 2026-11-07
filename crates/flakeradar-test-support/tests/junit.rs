//! The JUnit fixture every suite uploads is one well-formed report.
//!
//! A suite that uploads this cannot tell a parser bug from a broken fixture, so the
//! shape each outcome takes is pinned here.

use flakeradar_test_support::make_junit;

#[test]
fn writes_one_testcase_per_case() {
    let report = make_junit(&[
        ("test_a", "passed"),
        ("test_b", "failed"),
        ("test_c", "error"),
        ("test_d", "skipped"),
    ]);
    let text = String::from_utf8_lossy(&report);
    assert!(
        text.contains(r#"<testsuite name="unit" tests="4">"#),
        "{text}"
    );
    assert!(
        text.contains("name=\"test_a\" time=\"0.01\"></testcase>"),
        "{text}"
    );
    assert!(
        text.contains(r#"<failure message="assert 1 == 2">"#),
        "{text}"
    );
    assert!(text.contains(r#"<error message="boom">"#), "{text}");
    assert!(
        text.contains(r#"<skipped message="not on windows"/>"#),
        "{text}"
    );
}
