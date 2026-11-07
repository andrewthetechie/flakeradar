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
