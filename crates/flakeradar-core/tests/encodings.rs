//! Every encoding label and byte decodes as the reference's codecs decode it.
//!
//! `tests/fixtures/encodings.tsv` holds one row per label and byte, generated from
//! the reference's interpreter by
//! `docs/plans/rust-rewrite/review-135b/repro/spec/genencodings.py`.
//! Each row is replayed through the parser with the reference's answer as the
//! expectation, which is what keeps a fingerprint from drifting: the label decides
//! the bytes of a Test's name, and the name decides its fingerprint.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails loudly when a fixture row or the parser is wrong"
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use flakeradar_core::{
    model::ExecutionStatus,
    parsing::{ParseError, fingerprint, parse_junit_xml},
};

/// The generated fixture's path.
fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/encodings.tsv")
}

/// What one fixture row expects of a document that puts its byte in a case name.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expected {
    /// The character the case's name holds.
    Character(char),
    /// The label is one this server refuses.
    Refused,
}

/// One fixture row: a label, the byte it probes, and the answer.
struct Row {
    label: String,
    /// The byte the case's name holds. The row labelled `ascii` probes `A`, the
    /// one printable-ASCII byte the generator writes for each label.
    byte: u8,
    expected: Expected,
}

/// The label's spelling the fixture row proves.
fn rows() -> Vec<Row> {
    let path = fixture_path();
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        let label = fields.next().expect("a label").to_owned();
        let byte = fields.next().expect("a byte");
        let answer = fields.next().expect("an answer");
        let byte = match byte {
            "ascii" => b'A',
            hex => u8::from_str_radix(hex, 16)
                .unwrap_or_else(|error| panic!("{hex} is not a byte: {error}")),
        };
        let expected = match answer {
            "refuse" => Expected::Refused,
            hex => Expected::Character(
                char::from_u32(
                    u32::from_str_radix(hex, 16)
                        .unwrap_or_else(|error| panic!("{hex} is not a code point: {error}")),
                )
                .unwrap_or_else(|| panic!("{hex} is not a character")),
            ),
        };
        rows.push(Row {
            label,
            byte,
            expected,
        });
    }
    rows
}

/// The document a row replays: the label in a declaration, the byte in a case name.
fn document(row: &Row) -> Vec<u8> {
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"{}\"?><testsuite name=\"s\"><testcase classname=\"c\" name=\"n",
        row.label
    )
    .into_bytes();
    xml.push(row.byte);
    xml.extend_from_slice(b"\"/></testsuite>");
    xml
}

#[test]
fn every_label_and_byte_decodes_as_the_reference_says() {
    let rows = rows();
    assert!(!rows.is_empty(), "the fixture holds rows");
    let mut failures: BTreeMap<String, String> = BTreeMap::new();
    for row in &rows {
        let xml = document(row);
        let expected = match &row.expected {
            Expected::Character(char) => {
                let name = format!("n{char}");
                Ok((ExecutionStatus::Passed, name))
            }
            Expected::Refused => Err(()),
        };
        let seen = match parse_junit_xml(&xml, 1) {
            Ok(cases) => Ok((cases[0].status, cases[0].name.clone())),
            Err(ParseError::InvalidXml { .. }) => Err(()),
            Err(other) => {
                failures.insert(
                    format!("{} {:02x}", row.label, row.byte),
                    format!("an unexpected error: {other}"),
                );
                continue;
            }
        };
        if seen != expected {
            failures.insert(
                format!("{} {:02x}", row.label, row.byte),
                format!("expected {expected:?}, got {seen:?}"),
            );
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} fixture rows disagree, first ten: {:?}",
        failures.len(),
        rows.len(),
        failures.iter().take(10).collect::<Vec<_>>()
    );
}

#[test]
fn the_labels_the_fixture_holds_are_the_ones_this_server_knows() {
    // A row per label for the high bytes, and one for the printable-ASCII body,
    // which is what the generator writes and what the reference's own sweep used.
    let rows = rows();
    let mut per_label: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for row in &rows {
        let counts = per_label.entry(row.label.as_str()).or_insert((0, 0));
        if row.byte < 0x80 {
            counts.0 += 1;
        } else {
            counts.1 += 1;
        }
    }
    assert!(
        per_label.len() > 400,
        "the fixture holds {} labels",
        per_label.len()
    );
    for (label, (ascii, high)) in per_label {
        assert_eq!(ascii, 1, "{label} has one printable-ASCII row");
        assert_eq!(high, 128, "{label} has one row per high byte");
    }
}

#[test]
fn a_code_page_that_leaves_a_byte_undefined_falls_back_to_the_lossy_read() {
    // Measured against the reference: `windows-1252` with `0x81` (undefined) and
    // `0x80` (the euro sign) in one name gives two U+FFFD, so the whole document is
    // read again as lossy UTF-8 rather than the one byte being replaced.
    let xml = b"<?xml version=\"1.0\" encoding=\"windows-1252\"?><testsuite name=\"s\"><testcase classname=\"c\" name=\"n\x81\x80z\"/></testsuite>";
    assert_eq!(
        parse_junit_xml(xml, 1).expect("the report parses")[0].name,
        "n\u{fffd}\u{fffd}z"
    );
}

#[test]
fn a_code_page_that_rewrites_ascii_is_read_lossily() {
    // `cp864` is single-byte but does not map `A` to `A`, so the reference cannot
    // build a table from it and reads the whole document lossily. Measured: a
    // `cp864` document with `0xE9` in a name gives U+FFFD, not the code page's
    // character.
    let xml = b"<?xml version=\"1.0\" encoding=\"cp864\"?><testsuite name=\"s\"><testcase classname=\"c\" name=\"n\xe9z\"/></testsuite>";
    assert_eq!(
        parse_junit_xml(xml, 1).expect("the report parses")[0].name,
        "n\u{fffd}z"
    );
}

#[test]
fn three_code_pages_keep_the_fingerprints_the_reference_gives() {
    // The labels whose mapping WHATWG disagrees with: ISO-8859-9 and TIS-620 hold
    // U+0080 at 0x80 where windows-1254 and windows-874 hold the euro sign, and
    // KOI8-U holds `╝` at 0xAE where KOI8-R holds `ў`. The characters and their
    // fingerprints are what the reference gives for the same bytes.
    let cases: [(&str, u8, &str, &str); 3] = [
        (
            "iso-8859-9",
            0x80,
            "na\u{80}me",
            "cb0ac818afbfe66855742dd391f8a4354ac90181",
        ),
        (
            "koi8-u",
            0xae,
            "na\u{255d}me",
            "742ffa3c280b95420d040b2076739022f11d90e7",
        ),
        (
            "tis-620",
            0x80,
            "na\u{80}me",
            "cb0ac818afbfe66855742dd391f8a4354ac90181",
        ),
    ];
    for (label, byte, name, recorded) in cases {
        let mut xml = format!(
            "<?xml version=\"1.0\" encoding=\"{label}\"?><testsuite name=\"s\"><testcase classname=\"c\" name=\"na"
        )
        .into_bytes();
        xml.push(byte);
        xml.extend_from_slice(b"me\"/></testsuite>");
        let case = &parse_junit_xml(&xml, 1).expect("the report parses")[0];
        assert_eq!(case.name, name, "{label}");
        assert_eq!(
            fingerprint(&case.suite, &case.classname, &case.name).as_str(),
            recorded,
            "{label}"
        );
    }
}
