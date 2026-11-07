//! Builds the small JUnit reports tests upload.

use std::fmt::Write as _;

/// Builds a minimal JUnit XML report for a test to upload.
///
/// `cases` is one `(name, status)` pair per Test, where the status is `passed`,
/// `failed`, `error` or `skipped`. Every Test is in the suite `unit` and the
/// classname `tests.test_mod`, which is enough for ingest and processing tests.
#[must_use]
pub fn make_junit(cases: &[(&str, &str)]) -> Vec<u8> {
    let suite = "unit";
    let classname = "tests.test_mod";
    let mut inner = String::new();
    for (name, status) in cases {
        let body = match *status {
            "failed" => "<failure message=\"assert 1 == 2\">trace</failure>",
            "error" => "<error message=\"boom\">trace</error>",
            "skipped" => "<skipped message=\"not on windows\"/>",
            _ => "",
        };
        let _ = write!(
            inner,
            "<testcase classname=\"{classname}\" name=\"{name}\" time=\"0.01\">{body}</testcase>"
        );
    }
    format!(
        "<testsuites><testsuite name=\"{suite}\" tests=\"{}\">{inner}</testsuite></testsuites>",
        cases.len()
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    //! The fixture's shape is checked here so every suite can rely on it.

    use super::make_junit;

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
}
