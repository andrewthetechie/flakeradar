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
//! at most `NODES_MAX` XML nodes; the Test identities copied out of it (the
//! suite, classname, name and file strings) are held to `IDENTITY_BYTES_MAX` in
//! total, so one long attribute repeated across many cases cannot ask for more
//! memory than that budget; a `time` holds at most [`TIME_CHARS_MAX`] code
//! points; and, before the parser reads the text, a pre-scan refuses a body
//! whose internal entities would expand past `EXPANSION_BYTES_MAX` or whose
//! namespace declarations would put more than `NAMESPACE_ENTRIES_MAX` entries in
//! the parser's own table. For a body at the 20 MiB ingest limit, the parser
//! therefore allocates under [`PARSE_BYTES_MAX`], whatever shape the body takes:
//! `every_shape_a_body_can_take_answers_within_the_budget` measures the peak of
//! ten of them with a counting allocator, and of each pair of bounds the one
//! that moves is the pre-scan's, which runs before the parser reads a byte.

// Rust guideline compliant 2026-07-21

use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Range;
use std::sync::LazyLock;

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};
use regex::Regex;
use roxmltree::{Document, Node, ParsingOptions};
use sha1::{Digest as _, Sha1};

use crate::model::ExecutionStatus;
use crate::py::{cap, py_splitlines, strip_wire_whitespace};
use crate::types::Fingerprint;

/// The longest Failure message kept, in code points.
///
/// A rendered progress line plus an assertion message fits; a whole traceback
/// does not, and the Failure details carry that.
pub const MESSAGE_MAX: usize = 2000;

/// The longest Failure details kept, in code points.
///
/// Details are the traceback plus any captured output, and a real one is a few
/// thousand characters: 16,384 holds a deep traceback with its captured stdout
/// and leaves the stored row well inside a `text` column. Widening it stores more
/// of every failure of every Execution; narrowing it cuts the frames a person
/// reads to find the cause. The truncation marker is reserved inside this limit,
/// so a capped value is exactly this long.
pub const DETAILS_MAX: usize = 16384;

/// The longest `<testcase time>` or retry `time` this server reads, in code
/// points.
///
/// A duration is seconds with an optional fraction, and the longest value a CI
/// runner writes is a dozen characters. Every Unicode decimal digit in the value
/// is rewritten before the number is parsed, so the cost is one pass per
/// character of the attribute: a 20 MiB `time` of mathematical digits spent 19.5
/// s of a blocking thread. Past this bound the value is not a number this server
/// reads, so a case's `time` answers the 400 a non-numeric one answers and a
/// retry's reads 0.0, which is what every unreadable retry `time` reads.
pub const TIME_CHARS_MAX: usize = 64;

/// The deepest `<testsuite>` nesting one report may have.
///
/// A recursive walk of nested suites aborted the process at about 995 levels, and
/// an abort is not an answer: the route died rather than answering. This parser
/// holds its pending suites in a `Vec` instead, and refuses anything deeper than
/// this as an upload error, so the depth a report may have is a 422 rather than a
/// crash. 990 is the deepest nesting that still parsed, so every report that used
/// to be processed still is.
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
/// limit, measured with a counting allocator over the ten shapes
/// `every_shape_a_body_can_take_answers_within_the_budget` runs: 20.4 times the
/// body for a body of a million minimal cases (the most cases such an upload can
/// carry), 8 times for one long identity repeated across cases, 7.6 times for a
/// body of two hundred attributes per case, 3.2 times for a failure-heavy
/// report, and about the body's own size for each of the four the pre-scan
/// refuses. The constant is the first round figure with headroom over the worst
/// of those.
pub const PARSE_BYTES_MAX: usize = 512 * 1024 * 1024;

/// The most bytes the entity references in one report may expand to.
///
/// `roxmltree` expands every `&name;` in place and bounds only how deeply
/// references may nest, not how many one attribute or text node may hold: a 1
/// MiB body holding one 512 KiB internal entity referenced 178,000 times asks it
/// for 85 GiB. A real report declares no internal entities at all, so this
/// admits an eighth of [`PARSE_BYTES_MAX`] of expansion and refuses the rest as
/// a bad report, which is the 422 the ingest route answers.
const EXPANSION_BYTES_MAX: usize = 64 * 1024 * 1024;

/// The most entries one report's namespace declarations may put in the parser's
/// own table.
///
/// `roxmltree` copies every namespace in scope into each element that declares
/// one, so 60,000 nested declarations followed by siblings at that depth cost it
/// billions of entries and minutes of CPU. An element that declares nothing
/// costs nothing, so a real report's namespaces (one or two, on the root) stay in
/// the low tens; this admits a hundred thousand declaring elements below a few
/// prefixes and refuses the quadratic shape before the parser builds it.
const NAMESPACE_ENTRIES_MAX: usize = 4 * 1024 * 1024;

/// The deepest nesting of entity references the pre-scan measures.
///
/// `roxmltree`'s own loop detector refuses a reference nested more than ten
/// levels deep, so a declaration cycle is a body it refuses as well; stopping
/// here keeps the measurement from walking that cycle forever.
const ENTITY_NESTING_MAX: usize = 16;

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
    /// An unparsable `time` used to reach the process as an unhandled error and
    /// answer 500; it is a property of the upload, so the route refuses it with a
    /// 400 and the error carries the offending value.
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
///     flakeradar_core::parsing::fingerprint("unit", "tests.test_mod", "t1").as_str(),
///     "2b8823c6c55be0a19a34111f08b687028f5273df"
/// );
/// ```
#[must_use]
pub fn fingerprint(suite: &str, classname: &str, name: &str) -> Fingerprint {
    let raw = format!("{suite}::{classname}::{name}");
    Fingerprint::from_digest(hex_lower(Sha1::digest(raw.as_bytes())))
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
/// use flakeradar_core::{model::ExecutionStatus, parsing::parse_junit_xml};
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
            // A document whose declared encoding does not describe its bytes is
            // read again as lossy UTF-8, so a report with a stale declaration is
            // still processed.
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
/// `Ok(None)` means the bytes are not valid in the encoding they declare, and the
/// caller reads them again as lossy UTF-8.
///
/// # Errors
///
/// Returns [`ParseError::InvalidXml`] when the declaration names an encoding this
/// server cannot use at all: one its codec list does not know, or a multi-byte
/// one.
fn strict_decode(content: &[u8]) -> Result<Option<String>, ParseError> {
    // XML settles the encoding before the declaration is read: a byte-order mark
    // decides it, and so does the first byte of a document that starts with `<`.
    // A leading `0x3C 0x00` is `<` in UTF-16LE, which is why a UTF-16 report
    // without a byte-order mark is still read as UTF-16.
    match content {
        [0xFF, 0xFE, ..] | [0x3C, 0x00, ..] => return Ok(decode_utf16(content, UTF_16LE)),
        [0xFE, 0xFF, ..] | [0x00, 0x3C, ..] => return Ok(decode_utf16(content, UTF_16BE)),
        _ => {}
    }
    let Some(label) = declared_encoding(content) else {
        let (text, _, had_errors) = UTF_8.decode(content);
        return Ok((!had_errors).then(|| text.into_owned()));
    };
    let Some(encoding) = encoding_rule(&label)? else {
        return Ok(None);
    };
    match encoding {
        EncodingRule::Latin1 => Ok(Some(content.iter().map(|byte| char::from(*byte)).collect())),
        EncodingRule::Ascii => Ok(std::str::from_utf8(content)
            .ok()
            .filter(|text| text.is_ascii())
            .map(ToOwned::to_owned)),
        EncodingRule::Utf16 => Ok(decode_utf16(content, UTF_16LE)),
        EncodingRule::Other(encoding) => {
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

/// The rule a declared encoding's label is decoded by.
#[derive(Debug, Clone, Copy)]
enum EncodingRule {
    /// True Latin-1: every byte is its own code point, including `0x80`-`0x9F`.
    Latin1,
    /// US-ASCII: a byte past `0x7F` is not valid in it.
    Ascii,
    /// UTF-16, which is read only from a byte-order mark or a leading `<`.
    Utf16,
    /// Any other single-byte encoding the two agree on, decoded by `encoding_rs`.
    Other(&'static Encoding),
}

/// The encoding labels read as true Latin-1, without punctuation.
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

/// The encoding labels read as US-ASCII, without punctuation.
const ASCII_LABELS: [&str; 6] = [
    "usascii",
    "ascii",
    "ansix341968",
    "iso646us",
    "cp367",
    "csascii",
];

/// The rule a declaration's label names.
///
/// The label set is the one an XML reader's own codec list defines, which is why
/// Latin-1 is the real Latin-1 rather than WHATWG's windows-1252 alias for it,
/// and why US-ASCII refuses the bytes Latin-1 accepts. A label the table here
/// does not name goes to [`encoding_rs`]'s registry.
///
/// `Ok(None)` covers a label whose bytes are then read lossily instead, such as
/// the endian-specific UTF-16 spellings: the declaration decides nothing there.
fn encoding_rule(label: &str) -> Result<Option<EncodingRule>, ParseError> {
    let lowercase = label.to_ascii_lowercase();
    match lowercase.as_str() {
        "utf-16" => return Ok(Some(EncodingRule::Utf16)),
        // These spellings name an endianness the byte-order mark already
        // decides, so the declaration decides nothing and the bytes are read
        // lossily.
        "utf-16le" | "utf-16be" => return Ok(None),
        // The unhyphenated spelling names a multi-byte encoding this server
        // cannot read, so the upload is refused rather than decoded wrongly.
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
        return Ok(Some(EncodingRule::Other(UTF_8)));
    }
    if LATIN1_LABELS.contains(&squashed.as_str()) {
        return Ok(Some(EncodingRule::Latin1));
    }
    if ASCII_LABELS.contains(&squashed.as_str()) {
        return Ok(Some(EncodingRule::Ascii));
    }
    match Encoding::for_label(label.as_bytes()) {
        Some(encoding) if encoding == UTF_8 => Ok(Some(EncodingRule::Other(UTF_8))),
        Some(encoding) if encoding.is_single_byte() => Ok(Some(EncodingRule::Other(encoding))),
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
    refuse_expansion_past_the_budget(text)?;
    refuse_namespace_entries_past_the_budget(text)?;
    let options = ParsingOptions {
        // ElementTree parses a document with a `DOCTYPE`, so the reference
        // accepted one; external entities stay unfetched and an entity bomb is
        // refused by the expansion bound above rather than aborted.
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

/// Refuses a report whose internal entities would expand past the budget.
///
/// The bytes a body holds and the bytes it asks the parser for are two
/// different numbers: `roxmltree` copies each `&name;`'s value in place and
/// bounds only how deeply references may nest, so one 512 KiB entity referenced
/// 178,000 times turns a 1 MiB upload into 85 GiB of text. The declared entities
/// are summed with each nested reference at its own expanded length, and the
/// references outside the `DOCTYPE` are counted, before the parser reads the
/// text.
fn refuse_expansion_past_the_budget(text: &str) -> Result<(), ParseError> {
    let Some(doctype) = doctype(text) else {
        return Ok(());
    };
    let entities = entity_values(doctype.subset);
    if entities.is_empty() {
        return Ok(());
    }
    let declared = doctype.end.saturating_sub(doctype.start);
    let cap = text
        .len()
        .saturating_sub(declared)
        .saturating_add(EXPANSION_BYTES_MAX);
    let mut expansion = Expansion {
        entities,
        sizes: HashMap::new(),
        cap,
    };
    let mut expanded = 0_usize;
    for part in [text.get(..doctype.start), text.get(doctype.end..)] {
        expanded = expanded.saturating_add(expansion.length(part.unwrap_or_default(), 0)?);
    }
    if expanded > cap {
        return Err(past_the_expansion_budget());
    }
    Ok(())
}

/// The refusal a body whose entity references expand past the budget gets.
fn past_the_expansion_budget() -> ParseError {
    ParseError::InvalidXml {
        detail: format!(
            "the entity references in this report expand past the \
             {EXPANSION_BYTES_MAX} byte budget"
        ),
    }
}

/// The text one document's entity references expand to.
struct Expansion<'a> {
    /// The internal entities the document's `DOCTYPE` declares, by name.
    entities: HashMap<&'a str, &'a str>,
    /// The expanded byte length of each entity measured so far.
    sizes: HashMap<&'a str, usize>,
    /// The expanded length the document may reach, past which the scan stops.
    cap: usize,
}

impl<'a> Expansion<'a> {
    /// The bytes `text` holds once every entity reference in it is expanded.
    ///
    /// A character reference and a predefined entity (`&lt;`) are counted at
    /// their own spelling, which is never shorter than what they stand for, so
    /// this over-estimates and never under-estimates. The scan stops at the first
    /// total past the cap rather than counting the rest of 178,000 references
    /// that have already passed it.
    fn length(&mut self, text: &'a str, depth: usize) -> Result<usize, ParseError> {
        let mut total = text.len();
        if total > self.cap {
            return Err(past_the_expansion_budget());
        }
        let mut at = 0_usize;
        while let Some(offset) = text.get(at..).and_then(|rest| rest.find('&')) {
            let start = at.saturating_add(offset);
            let tail = text.get(start.saturating_add(1)..).unwrap_or_default();
            let (name, consumed) = reference(tail);
            at = start.saturating_add(1).saturating_add(consumed);
            let Some(name) = name else {
                continue;
            };
            let Some(expansion) = self.entity_length(name, depth)? else {
                continue;
            };
            let spelling = name.len().saturating_add(2);
            total = total.saturating_sub(spelling).saturating_add(expansion);
            if total > self.cap {
                return Err(past_the_expansion_budget());
            }
        }
        Ok(total)
    }

    /// The bytes one entity's value holds once its own references are expanded,
    /// or `None` when the document does not declare that entity.
    fn entity_length(&mut self, name: &'a str, depth: usize) -> Result<Option<usize>, ParseError> {
        if let Some(size) = self.sizes.get(name) {
            return Ok(Some(*size));
        }
        let Some(value) = self.entities.get(name).copied() else {
            return Ok(None);
        };
        if depth > ENTITY_NESTING_MAX {
            return Err(ParseError::InvalidXml {
                detail: "the entity references in this report nest in a loop".to_owned(),
            });
        }
        let size = self.length(value, depth.saturating_add(1))?;
        self.sizes.insert(name, size);
        Ok(Some(size))
    }
}

/// The entity reference `text` starts, and the bytes it spans.
///
/// `None` when `text` does not start a reference this scan reads — a character
/// reference (`&#38;`), a `&` the document leaves bare, or a run of name
/// characters with no `;` — so the caller counts those bytes as literal text. A
/// run is stepped over whole either way, which keeps the scan linear.
fn reference(text: &str) -> (Option<&str>, usize) {
    let mut end = 0_usize;
    for (offset, ch) in text.char_indices() {
        if ch == ';' && end > 0 {
            return (text.get(..end), offset.saturating_add(1));
        }
        if !is_name_char(ch) {
            return (None, end.max(1));
        }
        end = offset.saturating_add(ch.len_utf8());
    }
    (None, end.max(1))
}

/// Whether `ch` may appear in an XML name.
///
/// XML's own set is letters, digits, `.`, `-`, `_`, `:` and the combining and
/// extending characters Unicode adds to them. Every non-ASCII character is
/// allowed here rather than classified: a name this reads that XML does not is
/// only ever counted at its own spelling, and the declaration scan uses the same
/// rule, so no declared name can hide from the reference scan.
fn is_name_char(ch: char) -> bool {
    !ch.is_ascii() || ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':')
}

/// The `DOCTYPE` a document declares, if it declares one.
struct Doctype<'a> {
    /// The internal subset between `[` and `]`, empty when there is none.
    subset: &'a str,
    /// The byte offset `<!DOCTYPE` starts at.
    start: usize,
    /// The byte offset just past the `>` that ends the declaration.
    end: usize,
}

/// The `DOCTYPE` `text` declares, if it declares one.
fn doctype(text: &str) -> Option<Doctype<'_>> {
    let start = text.find("<!DOCTYPE")?;
    let rest = text.get(start..)?;
    let (length, inside) = declaration_extent(rest);
    Some(Doctype {
        subset: inside.and_then(|range| rest.get(range)).unwrap_or_default(),
        start,
        end: start.saturating_add(length),
    })
}

/// The bytes the markup declaration `text` starts span, and where its internal
/// subset sits inside them.
///
/// A `DOCTYPE`'s internal subset may hold `>` and `]` inside a quoted entity
/// value, so the declaration ends at the first `>` outside a quote and outside
/// brackets, and the subset is the text between the first `[` and the `]` that
/// closes it.
fn declaration_extent(text: &str) -> (usize, Option<Range<usize>>) {
    let mut quote: Option<char> = None;
    let mut depth = 0_usize;
    let mut subset_start: Option<usize> = None;
    let mut subset_end: Option<usize> = None;
    for (offset, ch) in text.char_indices() {
        match quote {
            Some(open) if ch == open => quote = None,
            Some(_) => {}
            None => match ch {
                '"' | '\'' => quote = Some(ch),
                '[' => {
                    if subset_start.is_none() {
                        subset_start = Some(offset.saturating_add(1));
                    }
                    depth = depth.saturating_add(1);
                }
                ']' => {
                    if depth == 1 {
                        subset_end = Some(offset);
                    }
                    depth = depth.saturating_sub(1);
                }
                '>' if depth == 0 => {
                    let subset = subset_start.zip(subset_end).map(|(from, to)| from..to);
                    return (offset.saturating_add(1), subset);
                }
                _ => {}
            },
        }
    }
    (text.len(), None)
}

/// The internal entities a `DOCTYPE`'s subset declares, by name.
///
/// An external entity (`SYSTEM` or `PUBLIC`) is never fetched, so the parser
/// holds no value for it and refuses a reference to one itself. A parameter
/// entity's value is read by the same `&name;` expansion as a general one's,
/// because the parser stores every declaration it reads.
fn entity_values(subset: &str) -> HashMap<&str, &str> {
    const OPEN: &str = "<!ENTITY";
    let mut entities = HashMap::new();
    let mut at = 0_usize;
    while let Some(offset) = subset.get(at..).and_then(|rest| rest.find(OPEN)) {
        at = at.saturating_add(offset).saturating_add(OPEN.len());
        let mut rest = subset.get(at..).unwrap_or_default().trim_start();
        rest = rest.strip_prefix('%').map_or(rest, str::trim_start);
        let (name, after_name) = split_name(rest);
        if name.is_empty() {
            continue;
        }
        let Some((quote, body)) = after_name.trim_start().split_at_checked(1) else {
            continue;
        };
        if !matches!(quote, "\"" | "'") {
            continue;
        }
        let Some(end) = body.find(quote) else {
            continue;
        };
        entities.insert(name, body.get(..end).unwrap_or_default());
    }
    entities
}

/// Refuses a report whose namespace declarations would make the parser's table
/// quadratic.
fn refuse_namespace_entries_past_the_budget(text: &str) -> Result<(), ParseError> {
    if !text.contains("xmlns") {
        return Ok(());
    }
    let mut scope = NamespaceScope::default();
    let mut at = 0_usize;
    while let Some(offset) = text.get(at..).and_then(|rest| rest.find('<')) {
        let start = at.saturating_add(offset);
        let tail = text.get(start..).unwrap_or_default();
        if let Some(length) = literal_region_length(tail) {
            at = start.saturating_add(length);
        } else if tail.starts_with("<!") {
            at = start.saturating_add(declaration_extent(tail).0);
        } else if let Some(end) = tail.strip_prefix("</").and_then(|rest| rest.find('>')) {
            scope.close_element();
            at = start.saturating_add(end).saturating_add(3);
        } else if let Some((declarations, self_closing, length)) = start_tag(tail) {
            scope.open_element(declarations, self_closing)?;
            at = start.saturating_add(length);
        } else {
            at = start.saturating_add(1);
        }
    }
    Ok(())
}

/// The namespace declarations a document's elements put in scope.
#[derive(Default)]
struct NamespaceScope {
    /// The declarations each open element made, innermost last.
    open: Vec<usize>,
    /// The declarations in scope, being the sum of `open`.
    in_scope: usize,
    /// The entries the parser's namespace table holds for the document.
    entries: usize,
}

impl NamespaceScope {
    /// Records one element, refusing the document once its table passes the
    /// bound.
    fn open_element(&mut self, declarations: usize, self_closing: bool) -> Result<(), ParseError> {
        let in_scope = self.in_scope.saturating_add(declarations);
        if declarations > 0 {
            self.entries = self.entries.saturating_add(in_scope);
            if self.entries > NAMESPACE_ENTRIES_MAX {
                return Err(ParseError::InvalidXml {
                    detail: format!(
                        "the namespace declarations in this report exceed the \
                         {NAMESPACE_ENTRIES_MAX} entry budget"
                    ),
                });
            }
        }
        if !self_closing {
            self.open.push(declarations);
            self.in_scope = in_scope;
        }
        Ok(())
    }

    /// Records the end of the innermost open element.
    fn close_element(&mut self) {
        if let Some(declarations) = self.open.pop() {
            self.in_scope = self.in_scope.saturating_sub(declarations);
        }
    }
}

/// A start tag's namespace declarations, whether it closes itself, and the bytes
/// it spans.
///
/// `None` when the text is not a start tag this scan reads, which leaves the
/// document to the parser's own refusal.
fn start_tag(text: &str) -> Option<(usize, bool, usize)> {
    let mut rest = text.strip_prefix('<')?;
    rest = split_name(rest).1;
    let mut declarations = 0_usize;
    loop {
        rest = rest.trim_start();
        if rest.starts_with('>') {
            return Some((declarations, false, text.len() - rest.len() + 1));
        }
        if rest.starts_with("/>") {
            return Some((declarations, true, text.len() - rest.len() + 2));
        }
        let (name, after_name) = split_name(rest);
        if name.is_empty() {
            return None;
        }
        rest = after_name.trim_start().strip_prefix('=')?.trim_start();
        let quote = rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let end = rest.get(1..)?.find(quote)?;
        rest = rest.get(end.saturating_add(2)..)?;
        if name == "xmlns" || name.starts_with("xmlns:") {
            declarations = declarations.saturating_add(1);
        }
    }
}

/// The name `text` starts, and the rest after it.
fn split_name(text: &str) -> (&str, &str) {
    let end = text
        .find(|ch: char| !is_name_char(ch))
        .unwrap_or(text.len());
    (
        text.get(..end).unwrap_or_default(),
        text.get(end..).unwrap_or_default(),
    )
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
/// of nested suites, in document order.
///
/// The walk holds the suites still to visit in a `Vec` rather than recursing, so
/// nesting cannot overflow the stack; a suite nested deeper than
/// [`SUITE_DEPTH_MAX`] is refused instead.
fn collect_suite(
    suite: Node<'_, '_>,
    line_base: i64,
    collector: &mut CaseCollector,
) -> Result<(), ParseError> {
    // A case inside a nested suite keeps the outer suite's name: the suite a walk
    // started from is the one its cases are reported under, not the one each case
    // happens to sit in.
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
/// The first `<failure>`, `<error>` or `<skipped>` in document order wins; a case
/// that carries two of them reports the first, because one Execution has one
/// outcome.
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
    // A retry's `time` is read leniently: a value that is no number is 0.0 rather
    // than a refused upload, because a retry's duration is context and not the
    // case's own duration.
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
    // A missing or empty attribute is no duration rather than a bad one.
    if raw.is_empty() {
        return Ok(0.0);
    }
    let Some(readable) = readable_time(raw) else {
        return Err(invalid_time(raw));
    };
    // A thousands separator is not part of a number here, so every comma goes
    // before the value is parsed.
    let without_commas = readable.replace(',', "");
    let duration = parse_seconds(&without_commas).ok_or_else(|| invalid_time(raw))?;
    finite_duration(duration, raw)
}

/// The `time` value this server reads, or `None` when it is past
/// [`TIME_CHARS_MAX`].
///
/// The bound is the first thing every reader of the attribute checks, so a 20
/// MiB `time` never reaches the comma removal or the digit rewriting that made
/// one blocking thread spend 19.5 s on it.
fn readable_time(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    if trimmed.chars().nth(TIME_CHARS_MAX).is_some() {
        return None;
    }
    Some(trimmed)
}

/// The refusal a `time` that is not a number this server reads makes.
///
/// The detail quotes the value, so it quotes at most [`TIME_CHARS_MAX`] code
/// points of it: a 20 MiB attribute is not worth a 20 MiB error message.
fn invalid_time(raw: &str) -> ParseError {
    ParseError::InvalidTime {
        value: strip_wire_whitespace(raw)
            .chars()
            .take(TIME_CHARS_MAX)
            .collect(),
    }
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
    Err(invalid_time(raw))
}

/// Matches one Unicode decimal digit, the characters the reader rewrites.
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

/// Parses a `time` attribute the way this API reads a number.
///
/// Unicode whitespace around the value is trimmed (but not the information
/// separators `\x1c`–`\x1f`), every Unicode decimal digit is rewritten as its
/// ASCII digit, and one underscore is allowed between two digits. Rust's own
/// parser does none of those, so each rule is applied first. A comma is not
/// special here: the testcase path removes them before this function is called,
/// and the retry path does not, which is why a retry with `<retry time="1,5">`
/// reads 0.0.
fn parse_seconds(raw: &str) -> Option<f64> {
    let trimmed = readable_time(raw)?;
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

/// Removes the underscores a number may carry between two digits.
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
