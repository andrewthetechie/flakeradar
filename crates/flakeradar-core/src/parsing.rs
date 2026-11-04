//! JUnit XML parsing: pure functions, no database.
//!
//! Reads one `<testcase>` at a time and extracts its identity (suite,
//! classname, name), outcome, Location (the file and line attributes when the
//! runner emits them) and, for failures, the Failure message plus Failure
//! details (the traceback body and captured stdout/stderr). Sizes are capped
//! here so storage never sees more. `backend/app/parsing.py` is the reference.

// Rust guideline compliant 2026-07-21

use std::fmt::Write as _;
use std::sync::LazyLock;

use encoding_rs::{Encoding, UTF_8};
use regex::Regex;
use roxmltree::{Document, Node, ParsingOptions};
use sha1::{Digest as _, Sha1};

use crate::model::ExecutionStatus;
use crate::py::{cap, py_splitlines, py_strip};

/// The longest Failure message kept, in code points.
///
/// A rendered progress line plus an assertion message fits; a whole traceback
/// does not, and the Failure details carry that.
pub const MESSAGE_MAX: usize = 2000;

/// The longest Failure details kept, in code points.
pub const DETAILS_MAX: usize = 16384;

/// Retry children that ran before the case's own result.
const RETRY_BEFORE: [&str; 2] = ["flakyFailure", "flakyError"];

/// Retry children that ran after the case's own result.
const RETRY_AFTER: [&str; 2] = ["rerunFailure", "rerunError"];

/// The message `cargo-nextest` with `flaky-result = "fail"` writes for a try
/// that passed.
///
/// It arrives as the case's own `<failure>`, with the failed tries before it as
/// retry children, so a message that fully matches it means the outcome was a
/// pass. A message that merely starts the same way is a real failure.
static NEXTEST_FLAKY_MARKER: LazyLock<Regex> = LazyLock::new(nextest_marker);

/// Compiles the nextest marker, refusing to run with a broken pattern.
#[expect(
    clippy::panic,
    reason = "the pattern is a literal in this file, so one that does not compile is a \
              programming error and must stop the process"
)]
fn nextest_marker() -> Regex {
    Regex::new(r"\Atest passed on attempt \d+/\d+ but is configured to fail when flaky\z")
        .unwrap_or_else(|error| panic!("the nextest marker pattern does not compile: {error}"))
}

/// One `<testcase>`, or one of its retry attempts, after parsing.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCase {
    /// The `<testsuite>` name, or `""` when the suite has none.
    pub suite: String,
    /// The reported classname, or `""` when the runner emitted none.
    pub classname: String,
    /// The case name; a case without one is dropped before this type is built.
    pub name: String,
    /// What this attempt did.
    pub status: ExecutionStatus,
    /// The reported duration in seconds; 0.0 when the runner gave none.
    pub duration: f64,
    /// The Failure message, `""` unless the attempt failed.
    pub message: String,
    /// The Failure details, `""` unless the attempt failed or errored.
    pub details: String,
    /// The reported file path with any leading `./` removed.
    pub file: Option<String>,
    /// The reported line, made 1-based for a `line_base` of 0.
    pub line: Option<i64>,
}

/// The uploaded bytes are not a usable JUnit XML report.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The bytes are not XML, or not a `<testsuites>`/`<testsuite>` document.
    ///
    /// The text after the prefix is the parser's own wording.
    #[error("Not a valid JUnit XML report: {detail}")]
    InvalidXml {
        /// What the parser or the structure check rejected.
        detail: String,
    },
    /// The document parsed, but held no `<testcase>` with a name.
    #[error("Report parsed but contained no test cases.")]
    NoTestCases,
    /// A `<testcase time>` attribute that is not a number.
    ///
    /// The frozen Python server raised an unhandled `ValueError` here and
    /// answered 500; the Rust server rejects the upload with a 400 instead, so
    /// the error carries the offending value.
    #[error("Not a valid JUnit XML report: testcase time '{value}' is not a number")]
    InvalidTime {
        /// The attribute value as it was given.
        value: String,
    },
}

/// The lowercase hex SHA-1 identity of a Test.
///
/// The fingerprint hashes `{suite}::{classname}::{name}` as UTF-8, so it has
/// stayed byte-identical since before the fork.
///
/// # Examples
///
/// ```
/// assert_eq!(
///     flakeradar_core::fingerprint("unit", "tests.test_mod", "t1"),
///     "2b8823c6c55be0a19a34111f08b687028f5273df"
/// );
/// ```
#[must_use]
pub fn fingerprint(suite: &str, classname: &str, name: &str) -> String {
    let raw = format!("{suite}::{classname}::{name}");
    hex_lower(Sha1::digest(raw.as_bytes()))
}

/// Renders bytes as lowercase hex.
fn hex_lower(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Parses one JUnit XML report into its Executions, newest last.
///
/// `line_base` is the base the runner used for line numbers: `1` keeps the
/// reported line, `0` shifts it by one so stored lines are always 1-based.
///
/// # Errors
///
/// Returns [`ParseError::InvalidXml`] when the bytes are not a JUnit XML
/// report, [`ParseError::NoTestCases`] when the report holds no named case,
/// and [`ParseError::InvalidTime`] when a `<testcase time>` is not a number.
///
/// # Examples
///
/// ```
/// use flakeradar_core::{ExecutionStatus, parse_junit_xml};
///
/// let xml = br#"<testsuite name="unit"><testcase classname="c" name="t" time="0.5"/></testsuite>"#;
/// let cases = parse_junit_xml(xml, 1).unwrap();
/// assert_eq!(cases[0].status, ExecutionStatus::Passed);
/// assert_eq!(cases[0].duration, 0.5);
/// ```
pub fn parse_junit_xml(content: &[u8], line_base: i64) -> Result<Vec<ParsedCase>, ParseError> {
    let strict = strict_decode(content)?;
    if let Some(text) = strict {
        match parse_text(&text, line_base) {
            // The frozen Python server retried on a lenient UTF-8 decode when
            // the XML parser rejected the declared encoding's bytes.
            Err(ParseError::InvalidXml { .. }) => {
                parse_text(&String::from_utf8_lossy(content), line_base)
            }
            other => other,
        }
    } else {
        parse_text(&String::from_utf8_lossy(content), line_base)
    }
}

/// Decodes the bytes with their declared encoding, or reports that they are not
/// valid in it.
///
/// `Ok(None)` means the bytes had to be replaced, which is the parser's cue to
/// retry on a lossy UTF-8 decode.
fn strict_decode(content: &[u8]) -> Result<Option<String>, ParseError> {
    let encoding = match declared_encoding(content) {
        Some(label) => {
            Encoding::for_label(label.as_bytes()).ok_or_else(|| ParseError::InvalidXml {
                detail: format!("unknown encoding: {label}"),
            })?
        }
        None => UTF_8,
    };
    let (text, _, had_errors) = encoding.decode(content);
    Ok((!had_errors).then(|| text.into_owned()))
}

/// The encoding an XML declaration names, when there is one.
///
/// Reads only the declaration, which XML requires to be ASCII, so a file whose
/// body is in another encoding is still recognized.
fn declared_encoding(content: &[u8]) -> Option<String> {
    let start = if content.starts_with(&[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    };
    let head = content.get(start..).unwrap_or_default();
    if !head.starts_with(b"<?xml") {
        return None;
    }
    let end = head.windows(2).position(|pair| pair == b"?>")?;
    let declaration = std::str::from_utf8(head.get(..end).unwrap_or_default()).ok()?;
    let mut rest = declaration.split_once("encoding")?.1.trim_start();
    rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next().unwrap_or_default();
    if quote != '"' && quote != '\'' {
        return None;
    }
    rest = rest.get(1..).unwrap_or_default();
    let end = rest.find(quote)?;
    rest.get(..end).map(ToOwned::to_owned)
}

/// Parses already-decoded text into Executions.
fn parse_text(text: &str, line_base: i64) -> Result<Vec<ParsedCase>, ParseError> {
    let document =
        Document::parse_with_options(text, ParsingOptions::default()).map_err(|error| {
            ParseError::InvalidXml {
                detail: error.to_string(),
            }
        })?;
    let root = document.root_element();
    let mut cases = Vec::new();
    match root.tag_name().name() {
        "testsuite" => collect_suite(root, line_base, &mut cases)?,
        "testsuites" => {
            for suite in child_elements(root, "testsuite") {
                collect_suite(suite, line_base, &mut cases)?;
            }
        }
        _ => {
            return Err(ParseError::InvalidXml {
                detail: "Invalid format.".to_owned(),
            });
        }
    }
    if cases.is_empty() {
        return Err(ParseError::NoTestCases);
    }
    Ok(cases)
}

/// The direct child elements of `node` with `tag`, in document order.
fn child_elements<'a, 'input>(
    node: Node<'a, 'input>,
    tag: &'static str,
) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children()
        .filter(move |child| child.is_element() && child.tag_name().name() == tag)
}

/// Collects one suite's cases: its own `<testcase>` children first, then those
/// of nested suites, as the Python model iterates them.
fn collect_suite(
    suite: Node<'_, '_>,
    line_base: i64,
    cases: &mut Vec<ParsedCase>,
) -> Result<(), ParseError> {
    collect_cases_named(
        suite,
        suite.attribute("name").unwrap_or_default(),
        line_base,
        cases,
    )
}

/// Collects one suite's cases under `suite_name`.
///
/// A case inside a nested suite keeps the outer suite's name, because the
/// Python model reports the suite it iterated rather than the suite the case
/// sits in.
fn collect_cases_named(
    suite: Node<'_, '_>,
    suite_name: &str,
    line_base: i64,
    cases: &mut Vec<ParsedCase>,
) -> Result<(), ParseError> {
    for case in child_elements(suite, "testcase") {
        collect_case(case, suite_name, line_base, cases)?;
    }
    for nested in child_elements(suite, "testsuite") {
        collect_cases_named(nested, suite_name, line_base, cases)?;
    }
    Ok(())
}

/// Appends one case's retry attempts and its own result.
fn collect_case(
    case: Node<'_, '_>,
    suite_name: &str,
    line_base: i64,
    cases: &mut Vec<ParsedCase>,
) -> Result<(), ParseError> {
    let Some(name) = case.attribute("name") else {
        return Ok(());
    };
    let classname = case.attribute("classname").unwrap_or_default();
    let (file, line) = location(case, line_base);
    let children: Vec<Node<'_, '_>> = case.children().filter(Node::is_element).collect();
    let has_earlier_try = children
        .iter()
        .any(|child| RETRY_BEFORE.contains(&child.tag_name().name()));

    for retry in &children {
        if RETRY_BEFORE.contains(&retry.tag_name().name()) {
            cases.push(retry_case(
                *retry,
                suite_name,
                classname,
                name,
                file.as_deref(),
                line,
            ));
        }
    }

    let (mut status, mut message, mut details) = outcome(case);
    if status.is_failing() && has_earlier_try && NEXTEST_FLAKY_MARKER.is_match(&message) {
        // The nextest flaky marker: that try passed.
        status = ExecutionStatus::Passed;
        message = String::new();
        details = String::new();
    }
    cases.push(ParsedCase {
        suite: suite_name.to_owned(),
        classname: classname.to_owned(),
        name: name.to_owned(),
        status,
        duration: case_duration(case)?,
        message,
        details,
        file: file.clone(),
        line,
    });

    for retry in &children {
        if RETRY_AFTER.contains(&retry.tag_name().name()) {
            cases.push(retry_case(
                *retry,
                suite_name,
                classname,
                name,
                file.as_deref(),
                line,
            ));
        }
    }
    Ok(())
}

/// One retry attempt as a case of its own, inheriting the case's identity.
fn retry_case(
    retry: Node<'_, '_>,
    suite_name: &str,
    classname: &str,
    name: &str,
    file: Option<&str>,
    line: Option<i64>,
) -> ParsedCase {
    let (status, message, details, duration) = retry_outcome(retry);
    ParsedCase {
        suite: suite_name.to_owned(),
        classname: classname.to_owned(),
        name: name.to_owned(),
        status,
        duration,
        message,
        details,
        file: file.map(ToOwned::to_owned),
        line,
    }
}

/// The status, Failure message and Failure details of a case's own result.
///
/// The first `<failure>`, `<error>` or `<skipped>` in document order wins, as
/// the Python model's first result does.
fn outcome(case: Node<'_, '_>) -> (ExecutionStatus, String, String) {
    let Some(result) = case.descendants().find(|node| {
        node.is_element() && matches!(node.tag_name().name(), "failure" | "error" | "skipped")
    }) else {
        return (ExecutionStatus::Passed, String::new(), String::new());
    };
    if result.tag_name().name() == "skipped" {
        let message = py_strip(result.attribute("message").unwrap_or_default());
        return (
            ExecutionStatus::Skipped,
            cap(message, MESSAGE_MAX),
            String::new(),
        );
    }
    let status = if result.tag_name().name() == "failure" {
        ExecutionStatus::Failed
    } else {
        ExecutionStatus::Error
    };
    let body = py_strip(result.text().unwrap_or_default());
    let message = {
        let attribute = py_strip(result.attribute("message").unwrap_or_default());
        if attribute.is_empty() {
            first_line(body)
        } else {
            attribute.to_owned()
        }
    };
    let details = joined_details(
        body,
        child_text(case, "system-out"),
        child_text(case, "system-err"),
    );
    (
        status,
        cap(&message, MESSAGE_MAX),
        cap(&details, DETAILS_MAX),
    )
}

/// One retry element's status, Failure message, Failure details and duration.
fn retry_outcome(elem: Node<'_, '_>) -> (ExecutionStatus, String, String, f64) {
    let failed = elem.tag_name().name().ends_with("Failure");
    let status = if failed {
        ExecutionStatus::Failed
    } else {
        ExecutionStatus::Error
    };
    let stack = py_strip(child_text(elem, "stackTrace").unwrap_or_default());
    let attribute = py_strip(elem.attribute("message").unwrap_or_default());
    let message = if attribute.is_empty() {
        first_line(stack)
    } else {
        attribute.to_owned()
    };
    let details = joined_details(
        stack,
        child_text(elem, "system-out"),
        child_text(elem, "system-err"),
    );
    let duration = python_float(elem.attribute("time").unwrap_or_default()).unwrap_or(0.0);
    (
        status,
        cap(&message, MESSAGE_MAX),
        cap(&details, DETAILS_MAX),
        duration,
    )
}

/// The text of the first direct child element named `tag`.
fn child_text<'a>(node: Node<'a, '_>, tag: &'static str) -> Option<&'a str> {
    let child = child_elements(node, tag).next()?;
    child.text()
}

/// Joins the Failure body with the captured output sections, blank-line separated.
fn joined_details(body: &str, stdout: Option<&str>, stderr: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !body.is_empty() {
        parts.push(body.to_owned());
    }
    if let Some(stdout) = stdout.map(py_strip).filter(|text| !text.is_empty()) {
        parts.push(format!("--- stdout ---\n{stdout}"));
    }
    if let Some(stderr) = stderr.map(py_strip).filter(|text| !text.is_empty()) {
        parts.push(format!("--- stderr ---\n{stderr}"));
    }
    parts.join("\n\n")
}

/// The first line of stripped text, stripped again.
fn first_line(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    py_splitlines(text)
        .first()
        .map_or_else(String::new, |line| py_strip(line).to_owned())
}

/// The case's own duration; a non-numeric `time` is an upload error.
fn case_duration(case: Node<'_, '_>) -> Result<f64, ParseError> {
    let raw = case.attribute("time").unwrap_or_default();
    // Python's `FloatAttr` reads a missing or empty attribute as absent.
    if raw.is_empty() {
        return Ok(0.0);
    }
    python_float(raw).ok_or_else(|| ParseError::InvalidTime {
        value: py_strip(raw).to_owned(),
    })
}

/// Parses a float the way Python's `float(text)` does.
///
/// Python removes no commas (the caller does), ignores surrounding whitespace,
/// and allows an underscore between two digits; Rust's parser does none of
/// those, so each rule is applied first.
fn python_float(raw: &str) -> Option<f64> {
    let cleaned: String = raw.chars().filter(|ch| *ch != ',').collect();
    let trimmed = py_strip(&cleaned);
    if trimmed.is_empty() {
        return None;
    }
    let text = if trimmed.contains('_') {
        strip_numeric_underscores(trimmed)?
    } else {
        trimmed.to_owned()
    };
    text.parse::<f64>().ok()
}

/// Removes the underscores Python's `float` allows between two digits.
fn strip_numeric_underscores(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, ch) in chars.iter().enumerate() {
        if *ch == '_' {
            let between_digits = index
                .checked_sub(1)
                .and_then(|at| chars.get(at))
                .is_some_and(char::is_ascii_digit)
                && chars
                    .get(index.saturating_add(1))
                    .is_some_and(char::is_ascii_digit);
            if !between_digits {
                return None;
            }
        } else {
            out.push(*ch);
        }
    }
    Some(out)
}

/// The reported file and line of a case.
fn location(case: Node<'_, '_>, line_base: i64) -> (Option<String>, Option<i64>) {
    let mut file = py_strip(case.attribute("file").unwrap_or_default());
    while let Some(rest) = file.strip_prefix("./") {
        file = rest;
    }
    let file = (!file.is_empty()).then(|| file.to_owned());
    let raw_line = py_strip(case.attribute("line").unwrap_or_default());
    let line = if raw_line.is_empty() || !raw_line.chars().all(|ch| ch.is_ascii_digit()) {
        None
    } else {
        // A `line_base` of 0 means the runner counted from zero.
        raw_line
            .parse::<i64>()
            .ok()
            .map(|line| line.saturating_add(i64::from(line_base == 0)))
    };
    (file, line)
}
