//! JUnit XML parsing: pure functions, no database.
//!
//! Reads one `<testcase>` at a time and extracts its identity (suite,
//! classname, name), outcome, Location (the file and line attributes when the
//! runner emits them) and, for failures, the Failure message plus Failure
//! details (the traceback body and captured stdout/stderr).
//!
//! A report is untrusted input, so every size this module can grow is bounded
//! here rather than left to the upload's own limit. Failure messages keep
//! [`MESSAGE_MAX`] code points and Failure details keep [`DETAILS_MAX`];
//! `<testsuite>` nesting stops at [`SUITE_DEPTH_MAX`] levels; a document holds
//! at most `NODES_MAX` XML nodes; and the Test identities copied out of it (the
//! suite, classname, name and file strings) are held to `IDENTITY_BYTES_MAX` in
//! total, so one long attribute repeated across many cases cannot ask for more
//! memory than that budget. For a body at the 20 MiB ingest limit, the parser
//! therefore allocates under [`PARSE_BYTES_MAX`].

// Rust guideline compliant 2026-07-21

use std::fmt::Write as _;
use std::sync::LazyLock;

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};
use regex::Regex;
use roxmltree::{Document, Node, ParsingOptions};
use sha1::{Digest as _, Sha1};

use crate::model::ExecutionStatus;
use crate::py::{cap, py_splitlines, strip_wire_whitespace};

/// The longest Failure message kept, in code points.
///
/// A rendered progress line plus an assertion message fits; a whole traceback
/// does not, and the Failure details carry that.
pub const MESSAGE_MAX: usize = 2000;

/// The longest Failure details kept, in code points.
pub const DETAILS_MAX: usize = 16384;

/// The deepest `<testsuite>` nesting one report may have.
///
/// The reference walked nested suites by recursion and raised `RecursionError`,
/// which the route answered with 500, at 995 levels under Python's default
/// 1000-frame limit. A crash is not a contract, so the approved rule makes
/// anything that deep a refused upload instead; this limit is the deepest
/// nesting the reference could still parse.
pub const SUITE_DEPTH_MAX: usize = 990;

/// The most XML nodes (elements and text) one report may hold.
///
/// Each node costs the parser a node record, so the arena is bounded by this
/// alone. A failure-heavy report at the 20 MiB ingest limit carries about 200,000
/// nodes; a million small cases would carry a million, and that many Executions
/// is more than any real JUnit report holds.
const NODES_MAX: u32 = 1_000_000;

/// The most bytes of Test identity text one report's cases may hold.
///
/// Suite, classname, name and file are copied once per Execution, and one long
/// attribute repeated across many cases is the quadratic case: a 10 MiB suite
/// name with 500,000 cases asks for terabytes. A real report copies about as
/// many bytes as its own size (a retry copies its case's identity once more per
/// attempt), so three times the ingest limit refuses the amplification and
/// admits every report that fits.
const IDENTITY_BYTES_MAX: usize = 64 * 1024 * 1024;

/// The most heap one report's parse uses, in bytes.
///
/// This is the budget the module doc promises for a body at the 20 MiB ingest
/// limit, measured with a counting allocator over the shapes that reach it: 20.4
/// times the body for a body of a million minimal cases (the most cases such an
/// upload can carry), 8 times for one long identity repeated across cases, and
/// 3.2 times for a failure-heavy report. The constant is the first round figure
/// with headroom over the worst of those.
pub const PARSE_BYTES_MAX: usize = 512 * 1024 * 1024;

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
/// `Ok(None)` means the reference's parser rejected the bytes in the declared
/// encoding and retried on a lenient UTF-8 decode, which is this parser's cue to
/// do the same.
///
/// # Errors
///
/// Returns [`ParseError::InvalidXml`] when the declaration names an encoding the
/// reference's parser could not use at all: one Python's codecs do not know, or
/// a multi-byte one.
fn strict_decode(content: &[u8]) -> Result<Option<String>, ParseError> {
    // XML settles the encoding before the declaration is read: a byte-order mark
    // decides it, and so does the first byte of a document that starts with `<`.
    // Python's parser does the same, which is why a UTF-16 report without a
    // byte-order mark is still read as UTF-16.
    match content {
        [0xFF, 0xFE, ..] | [0x3C, 0x00, ..] => return Ok(decode_utf16(content, UTF_16LE)),
        [0xFE, 0xFF, ..] | [0x00, 0x3C, ..] => return Ok(decode_utf16(content, UTF_16BE)),
        _ => {}
    }
    let Some(label) = declared_encoding(content) else {
        let (text, _, had_errors) = UTF_8.decode(content);
        return Ok((!had_errors).then(|| text.into_owned()));
    };
    let Some(encoding) = python_encoding(&label)? else {
        return Ok(None);
    };
    match encoding {
        PythonEncoding::Latin1 => Ok(Some(content.iter().map(|byte| char::from(*byte)).collect())),
        PythonEncoding::Ascii => Ok(std::str::from_utf8(content)
            .ok()
            .filter(|text| text.is_ascii())
            .map(ToOwned::to_owned)),
        PythonEncoding::Utf16 => Ok(decode_utf16(content, UTF_16LE)),
        PythonEncoding::Other(encoding) => {
            let (text, _, had_errors) = encoding.decode(content);
            Ok((!had_errors).then(|| text.into_owned()))
        }
    }
}

/// Decodes bytes with a UTF-16 flavour, or reports that they are not valid in it.
///
/// A byte-order mark is stripped, and one that names the other flavour wins, so
/// the flavour passed here is the default for unmarked bytes.
fn decode_utf16(content: &[u8], encoding: &'static Encoding) -> Option<String> {
    let (text, _, had_errors) = encoding.decode(content);
    (!had_errors).then(|| text.into_owned())
}

/// An encoding the reference's parser decodes bytes with.
#[derive(Debug, Clone, Copy)]
enum PythonEncoding {
    /// True Latin-1: every byte is its own code point, including `0x80`-`0x9F`.
    Latin1,
    /// US-ASCII: a byte past `0x7F` is not valid in it.
    Ascii,
    /// UTF-16, which Python's parser only accepts with a byte-order mark.
    Utf16,
    /// Any other single-byte encoding the two agree on, decoded by `encoding_rs`.
    Other(&'static Encoding),
}

/// The encoding labels Python's codecs read as Latin-1, without punctuation.
const LATIN1_LABELS: [&str; 9] = [
    "iso88591",
    "iso8859",
    "8859",
    "latin1",
    "latin",
    "l1",
    "cp819",
    "ibm819",
    "csisolatin1",
];

/// The encoding labels Python's codecs read as US-ASCII, without punctuation.
const ASCII_LABELS: [&str; 6] = [
    "usascii",
    "ascii",
    "ansix341968",
    "iso646us",
    "cp367",
    "csascii",
];

/// The encoding a declaration's label names, as Python's parser reads it.
///
/// Python hands an unknown label to its codec registry, which is why Latin-1 is
/// the real Latin-1 rather than WHATWG's windows-1252 alias for it, and why
/// US-ASCII refuses the bytes Latin-1 accepts. Labels ASCII agrees with
/// Python's codecs on go through [`encoding_rs`].
///
/// `Ok(None)` covers a label Python's strict parse rejects though its lenient
/// retry still parses the bytes, such as the endian-specific UTF-16 spellings.
fn python_encoding(label: &str) -> Result<Option<PythonEncoding>, ParseError> {
    let lowercase = label.to_ascii_lowercase();
    match lowercase.as_str() {
        "utf-16" => return Ok(Some(PythonEncoding::Utf16)),
        // Python's strict parse refuses these spellings and its lenient retry
        // reads the bytes lossily instead, so the declaration decides nothing.
        "utf-16le" | "utf-16be" => return Ok(None),
        // Python hands the unhyphenated spelling to its codecs, which know it as
        // a multi-byte encoding and refuse it.
        "utf16" => {
            return Err(ParseError::InvalidXml {
                detail: "multi-byte encodings are not supported".to_owned(),
            });
        }
        _ => {}
    }
    let squashed: String = lowercase
        .chars()
        .filter(|ch| !matches!(ch, '-' | '_' | ' '))
        .collect();
    if squashed == "utf8" {
        return Ok(Some(PythonEncoding::Other(UTF_8)));
    }
    if LATIN1_LABELS.contains(&squashed.as_str()) {
        return Ok(Some(PythonEncoding::Latin1));
    }
    if ASCII_LABELS.contains(&squashed.as_str()) {
        return Ok(Some(PythonEncoding::Ascii));
    }
    match Encoding::for_label(label.as_bytes()) {
        Some(encoding) if encoding == UTF_8 => Ok(Some(PythonEncoding::Other(UTF_8))),
        Some(encoding) if encoding.is_single_byte() => Ok(Some(PythonEncoding::Other(encoding))),
        Some(_) => Err(ParseError::InvalidXml {
            detail: "multi-byte encodings are not supported".to_owned(),
        }),
        None => Err(ParseError::InvalidXml {
            detail: format!("unknown encoding: {label}"),
        }),
    }
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
    refuse_unrepresentable_character_references(text)?;
    let options = ParsingOptions {
        // ElementTree parses a document with a `DOCTYPE`, so the reference
        // accepted one; external entities stay unfetched and an entity bomb is
        // refused by the amplification limit rather than aborted.
        allow_dtd: true,
        nodes_limit: NODES_MAX,
        entity_resolver: None,
    };
    let document =
        Document::parse_with_options(text, options).map_err(|error| ParseError::InvalidXml {
            detail: error.to_string(),
        })?;
    let root = document.root_element();
    let mut collector = CaseCollector::default();
    if is_tag(root, "testsuite") {
        collect_suite(root, line_base, &mut collector)?;
    } else if is_tag(root, "testsuites") {
        for suite in child_elements(root, "testsuite") {
            collect_suite(suite, line_base, &mut collector)?;
        }
    } else {
        return Err(ParseError::InvalidXml {
            detail: "Invalid format.".to_owned(),
        });
    }
    if collector.cases.is_empty() {
        return Err(ParseError::NoTestCases);
    }
    Ok(collector.cases)
}

/// Refuses a character reference XML cannot carry.
///
/// `roxmltree` reads `&#xD800;` and `&#x110000;` as U+FFFD, while `ElementTree`
/// raises, so the reference numbers are checked here. A reference inside a
/// comment, a CDATA section, a processing instruction or the `DOCTYPE` is
/// literal text and is left alone.
fn refuse_unrepresentable_character_references(text: &str) -> Result<(), ParseError> {
    if !text.contains("&#") {
        return Ok(());
    }
    let mut rest = text;
    while let Some(ch) = rest.chars().next() {
        if ch == '<' {
            if let Some(length) = literal_region_length(rest) {
                rest = rest.get(length..).unwrap_or_default();
                continue;
            }
        } else if ch == '&'
            && let Some(number) = rest.strip_prefix("&#")
        {
            check_character_reference(number)?;
        }
        rest = rest.get(ch.len_utf8()..).unwrap_or_default();
    }
    Ok(())
}

/// Refuses a numeric character reference XML cannot carry.
fn check_character_reference(number: &str) -> Result<(), ParseError> {
    let (unprefixed, is_hex) = match number.strip_prefix('x') {
        Some(hex) => (hex, true),
        None => (number, false),
    };
    let digits = digits_while(unprefixed, is_hex);
    let terminated = unprefixed
        .get(digits.len()..)
        .is_some_and(|tail| tail.starts_with(';'));
    if digits.is_empty() || !terminated {
        return Ok(());
    }
    let radix = if is_hex { 16 } else { 10 };
    let unrepresentable = u32::from_str_radix(digits, radix)
        .ok()
        .and_then(char::from_u32)
        .is_none();
    if unrepresentable {
        return Err(ParseError::InvalidXml {
            detail: "reference to invalid character number".to_owned(),
        });
    }
    Ok(())
}

/// The byte length of the literal region `tail` starts, if it starts one.
///
/// A comment, a CDATA section and a processing instruction hold text XML does
/// not read as references, so a `&#xD800;` inside one is literal and stays. An
/// unterminated region swallows the rest of the document, which the parser then
/// reports itself. The `DOCTYPE` is deliberately not one of these: the reference
/// parser validates a character reference in an entity declaration.
fn literal_region_length(tail: &str) -> Option<usize> {
    const REGIONS: [(&str, &str); 3] = [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")];
    REGIONS.iter().find_map(|(open, close)| {
        let body = tail.strip_prefix(open)?;
        Some(match body.find(close) {
            Some(end) => open.len() + end + close.len(),
            None => tail.len(),
        })
    })
}

/// The leading digits of `text`: hexadecimal ones when `is_hex`, else decimal.
fn digits_while(text: &str, is_hex: bool) -> &str {
    let len = text
        .bytes()
        .take_while(|byte| {
            if is_hex {
                byte.is_ascii_hexdigit()
            } else {
                byte.is_ascii_digit()
            }
        })
        .count();
    text.get(..len).unwrap_or_default()
}

/// One report's cases and the identity bytes they hold, refused past a budget.
#[derive(Default)]
struct CaseCollector {
    /// The Executions found so far, in report order.
    cases: Vec<ParsedCase>,
    /// The bytes of Test identity text those Executions hold.
    identity_bytes: usize,
}

impl CaseCollector {
    /// Appends one Execution, refusing a report whose identities exceed the
    /// budget.
    fn push(&mut self, case: ParsedCase) -> Result<(), ParseError> {
        self.identity_bytes += case.suite.len()
            + case.classname.len()
            + case.name.len()
            + case.file.as_ref().map_or(0, String::len);
        if self.identity_bytes > IDENTITY_BYTES_MAX {
            return Err(ParseError::InvalidXml {
                detail: format!(
                    "the Test identities in this report exceed the {IDENTITY_BYTES_MAX} byte budget"
                ),
            });
        }
        self.cases.push(case);
        Ok(())
    }
}

/// Whether `node` is an element named `tag` in no namespace.
///
/// `ElementTree` matches a fully qualified tag, so `<j:failure>` and
/// `<failure xmlns="urn:x">` are not JUnit elements and a namespaced suite is
/// not a suite at all.
fn is_tag(node: Node<'_, '_>, tag: &str) -> bool {
    node.is_element() && node.tag_name().namespace().is_none() && node.tag_name().name() == tag
}

/// The un-namespaced attribute `name` of `node`, as `ElementTree`'s `.get` reads it.
fn attribute<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| attribute.namespace().is_none() && attribute.name() == name)
        .map(|attribute| attribute.value())
}

/// The text of `node` before its first child element.
///
/// `ElementTree` merges the text around comments and processing instructions,
/// because its parser drops them, and its `.text` is everything before the
/// first child element. `roxmltree` keeps both nodes, so they are skipped here
/// and the surrounding text is joined back together.
fn leading_text(node: Node<'_, '_>) -> Option<String> {
    let mut merged: Option<String> = None;
    for child in node.children() {
        if child.is_element() {
            break;
        }
        if child.is_text() {
            merged
                .get_or_insert_with(String::new)
                .push_str(child.text().unwrap_or_default());
        }
    }
    merged
}

/// The direct child elements of `node` with `tag`, in document order.
fn child_elements<'a, 'input>(
    node: Node<'a, 'input>,
    tag: &'static str,
) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children().filter(move |child| is_tag(*child, tag))
}

/// Collects one suite's cases: its own `<testcase>` children first, then those
/// of nested suites, as the Python model iterates them.
///
/// The walk holds the suites still to visit in a `Vec` rather than recursing, so
/// nesting cannot overflow the stack; a suite nested deeper than
/// [`SUITE_DEPTH_MAX`] is refused instead.
fn collect_suite(
    suite: Node<'_, '_>,
    line_base: i64,
    collector: &mut CaseCollector,
) -> Result<(), ParseError> {
    // A case inside a nested suite keeps the outer suite's name, because the
    // Python model reports the suite it iterated rather than the suite the case
    // sits in.
    let suite_name = attribute(suite, "name").unwrap_or_default();
    let mut pending = vec![(suite, 1_usize)];
    while let Some((node, depth)) = pending.pop() {
        if depth > SUITE_DEPTH_MAX {
            return Err(ParseError::InvalidXml {
                detail: format!(
                    "the report nests `<testsuite>` deeper than {SUITE_DEPTH_MAX} levels"
                ),
            });
        }
        for case in child_elements(node, "testcase") {
            collect_case(case, suite_name, line_base, collector)?;
        }
        // Pushed in reverse so the suites are visited in document order, which
        // is the order the Executions are stored in.
        let mut nested: Vec<Node<'_, '_>> = child_elements(node, "testsuite").collect();
        nested.reverse();
        pending.extend(nested.into_iter().map(|suite| (suite, depth + 1)));
    }
    Ok(())
}

/// Appends one case's retry attempts and its own result.
fn collect_case(
    case: Node<'_, '_>,
    suite_name: &str,
    line_base: i64,
    collector: &mut CaseCollector,
) -> Result<(), ParseError> {
    let Some(name) = attribute(case, "name") else {
        return Ok(());
    };
    let classname = attribute(case, "classname").unwrap_or_default();
    let (file, line) = location(case, line_base);
    let children: Vec<Node<'_, '_>> = case.children().filter(Node::is_element).collect();
    let has_earlier_try = children.iter().any(|child| is_retry(*child, &RETRY_BEFORE));

    for retry in &children {
        if is_retry(*retry, &RETRY_BEFORE) {
            collector.push(retry_case(
                *retry,
                suite_name,
                classname,
                name,
                file.as_deref(),
                line,
            )?)?;
        }
    }

    let (mut status, mut message, mut details) = outcome(case);
    if status.is_failing() && has_earlier_try && NEXTEST_FLAKY_MARKER.is_match(&message) {
        // The nextest flaky marker: that try passed.
        status = ExecutionStatus::Passed;
        message = String::new();
        details = String::new();
    }
    collector.push(ParsedCase {
        suite: suite_name.to_owned(),
        classname: classname.to_owned(),
        name: name.to_owned(),
        status,
        duration: case_duration(case)?,
        message,
        details,
        file: file.clone(),
        line,
    })?;

    for retry in &children {
        if is_retry(*retry, &RETRY_AFTER) {
            collector.push(retry_case(
                *retry,
                suite_name,
                classname,
                name,
                file.as_deref(),
                line,
            )?)?;
        }
    }
    Ok(())
}

/// Whether `elem` is one of the retry elements `tags` names.
fn is_retry(elem: Node<'_, '_>, tags: &[&str; 2]) -> bool {
    tags.iter().any(|tag| is_tag(elem, tag))
}

/// One retry attempt as a case of its own, inheriting the case's identity.
fn retry_case(
    retry: Node<'_, '_>,
    suite_name: &str,
    classname: &str,
    name: &str,
    file: Option<&str>,
    line: Option<i64>,
) -> Result<ParsedCase, ParseError> {
    let (status, message, details, duration) = retry_outcome(retry)?;
    Ok(ParsedCase {
        suite: suite_name.to_owned(),
        classname: classname.to_owned(),
        name: name.to_owned(),
        status,
        duration,
        message,
        details,
        file: file.map(ToOwned::to_owned),
        line,
    })
}

/// The status, Failure message and Failure details of a case's own result.
///
/// The first `<failure>`, `<error>` or `<skipped>` in document order wins, as
/// the Python model's first result does.
fn outcome(case: Node<'_, '_>) -> (ExecutionStatus, String, String) {
    let Some(result) = case.descendants().find(|node| {
        ["failure", "error", "skipped"]
            .iter()
            .any(|tag| is_tag(*node, tag))
    }) else {
        return (ExecutionStatus::Passed, String::new(), String::new());
    };
    let message_attribute = strip_wire_whitespace(attribute(result, "message").unwrap_or_default());
    if is_tag(result, "skipped") {
        return (
            ExecutionStatus::Skipped,
            cap(message_attribute, MESSAGE_MAX),
            String::new(),
        );
    }
    let status = if is_tag(result, "failure") {
        ExecutionStatus::Failed
    } else {
        ExecutionStatus::Error
    };
    let body = leading_text(result).unwrap_or_default();
    let body = strip_wire_whitespace(&body);
    let message = if message_attribute.is_empty() {
        first_line(body)
    } else {
        message_attribute.to_owned()
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
fn retry_outcome(elem: Node<'_, '_>) -> Result<(ExecutionStatus, String, String, f64), ParseError> {
    let status = if is_tag(elem, "flakyFailure") || is_tag(elem, "rerunFailure") {
        ExecutionStatus::Failed
    } else {
        ExecutionStatus::Error
    };
    let stack = child_text(elem, "stackTrace").unwrap_or_default();
    let stack = strip_wire_whitespace(&stack);
    let message_attribute = strip_wire_whitespace(attribute(elem, "message").unwrap_or_default());
    let message = if message_attribute.is_empty() {
        first_line(stack)
    } else {
        message_attribute.to_owned()
    };
    let details = joined_details(
        stack,
        child_text(elem, "system-out"),
        child_text(elem, "system-err"),
    );
    let raw = attribute(elem, "time").unwrap_or_default();
    // Python reads a retry's `time` with a bare `float()`, so a value it cannot
    // parse is 0.0 rather than an error.
    let duration = parse_seconds(raw).unwrap_or(0.0);
    Ok((
        status,
        cap(&message, MESSAGE_MAX),
        cap(&details, DETAILS_MAX),
        finite_duration(duration, raw)?,
    ))
}

/// The text of the first direct child element named `tag`.
fn child_text(node: Node<'_, '_>, tag: &'static str) -> Option<String> {
    leading_text(child_elements(node, tag).next()?)
}

/// Joins the Failure body with the captured output sections, blank-line separated.
fn joined_details(body: &str, stdout: Option<String>, stderr: Option<String>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !body.is_empty() {
        parts.push(body.to_owned());
    }
    for (label, section) in [("stdout", stdout), ("stderr", stderr)] {
        if let Some(text) = section {
            let text = strip_wire_whitespace(&text);
            if !text.is_empty() {
                parts.push(format!("--- {label} ---\n{text}"));
            }
        }
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
        .map_or_else(String::new, |line| strip_wire_whitespace(line).to_owned())
}

/// The case's own duration; a `time` that is not a finite number is an upload
/// error.
fn case_duration(case: Node<'_, '_>) -> Result<f64, ParseError> {
    let raw = attribute(case, "time").unwrap_or_default();
    // junitparser's `FloatAttr` reads a missing or empty attribute as absent.
    if raw.is_empty() {
        return Ok(0.0);
    }
    // junitparser removes every comma before Python's `float()` sees the value.
    let without_commas = raw.replace(',', "");
    let duration = parse_seconds(&without_commas).ok_or_else(|| ParseError::InvalidTime {
        value: strip_wire_whitespace(raw).to_owned(),
    })?;
    finite_duration(duration, raw)
}

/// A duration the database and the read API can carry.
///
/// The reference stored a `NaN` or infinite `time` as it read it, which the read
/// API then served as JSON `null` against a schema that says number; the
/// approved rule makes a non-finite `time` an upload error like a non-numeric
/// one.
fn finite_duration(duration: f64, raw: &str) -> Result<f64, ParseError> {
    if duration.is_finite() {
        return Ok(duration);
    }
    Err(ParseError::InvalidTime {
        value: strip_wire_whitespace(raw).to_owned(),
    })
}

/// Matches one Unicode decimal digit, the characters Python's `float` rewrites.
static DECIMAL_DIGIT: LazyLock<Regex> = LazyLock::new(decimal_digit);

/// Compiles the decimal-digit pattern, refusing to run with a broken pattern.
#[expect(
    clippy::expect_used,
    reason = "the pattern is a literal in this file, so one that does not compile is a \
              programming error and must stop the process"
)]
fn decimal_digit() -> Regex {
    Regex::new(r"\A\p{Nd}\z").expect("the decimal-digit pattern compiles")
}

/// Parses a float the way Python's `float(text)` does.
///
/// Python trims Unicode whitespace (but not the information separators
/// `\x1c`–`\x1f`, which `str.strip` would remove), rewrites every Unicode
/// decimal digit as ASCII, and allows one underscore between two digits. Rust's
/// parser does none of those, so each rule is applied first. Commas are not
/// special here: the retry path calls plain `float()` in Python, while the
/// testcase path removes them before this function, as junitparser does.
fn parse_seconds(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let ascii = ascii_decimal_digits(trimmed);
    let text = strip_numeric_underscores(&ascii)?;
    text.parse::<f64>().ok()
}

/// Rewrites every Unicode decimal digit as its ASCII digit.
fn ascii_decimal_digits(text: &str) -> String {
    text.chars()
        .map(
            |ch| match decimal_value(ch).and_then(|value| char::from_digit(value, 10)) {
                Some(digit) => digit,
                None => ch,
            },
        )
        .collect()
}

/// The ASCII value of one Unicode decimal digit.
///
/// Every run of `Nd` code points in Unicode is a whole number of ten-character
/// blocks, one per digit 0-9, so the value is the digit's position within its
/// block. The block's own zero is not always the start of the run: U+1D7CE..U+1D7FF
/// is five adjacent blocks (bold, double-struck, sans-serif, sans-serif bold and
/// monospace), so the value is the position in the run modulo ten.
fn decimal_value(ch: char) -> Option<u32> {
    if let Some(value) = ch.to_digit(10) {
        return Some(value);
    }
    if !is_decimal_digit(ch) {
        return None;
    }
    let mut start = u32::from(ch);
    let mut position = 0;
    while let Some(previous) =
        char::from_u32(start - 1).filter(|previous| is_decimal_digit(*previous))
    {
        start = u32::from(previous);
        position += 1;
    }
    Some(position % 10)
}

/// Whether `ch` is a Unicode decimal digit (`Nd`).
fn is_decimal_digit(ch: char) -> bool {
    let mut encoded = [0_u8; 4];
    DECIMAL_DIGIT.is_match(ch.encode_utf8(&mut encoded))
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
    let mut file = strip_wire_whitespace(attribute(case, "file").unwrap_or_default());
    while let Some(rest) = file.strip_prefix("./") {
        file = rest;
    }
    let file = (!file.is_empty()).then(|| file.to_owned());
    let raw_line = strip_wire_whitespace(attribute(case, "line").unwrap_or_default());
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
