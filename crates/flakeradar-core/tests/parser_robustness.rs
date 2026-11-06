//! The parser must never panic on any input, however malformed.
//!
//! CI uploads are attacker-controlled, so a panic is a bug rather than a
//! possible outcome: every mutation below must return `Ok` or `Err`. The corpus
//! is the realistic starting point, and the mutations cover the encoding
//! declaration, the structure and the attribute values at once.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails loudly when the parser panics or the corpus is missing"
)]

use std::fs;
use std::path::{Path, PathBuf};

use flakeradar_core::{parsing::ParseError, parsing::SUITE_DEPTH_MAX, parsing::parse_junit_xml};

/// Every corpus file, sorted so a failure names a stable file.
fn corpus() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/junit");
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("cannot list {}: {error}", dir.display()))
        .map(|entry| entry.expect("the directory is readable").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "xml"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no corpus files under {}", dir.display());
    paths
}

#[test]
fn no_byte_mutation_of_the_corpus_panics() {
    // Every 61st byte: a stride that samples the declaration, the structure and
    // the attribute values of every file while keeping the suite fast.
    let mut mutations = 0_usize;
    for path in corpus() {
        let bytes = fs::read(&path).expect("the corpus file is readable");
        let mut index = 0;
        while index < bytes.len() {
            let mut mutated = bytes.clone();
            mutated[index] ^= 0b1010_1010;
            let _ = parse_junit_xml(&mutated, 1);
            let _ = parse_junit_xml(&mutated, 0);
            mutations += 1;
            index += 61;
        }
    }
    assert!(mutations > 500, "only {mutations} mutations ran");
}

#[test]
fn no_truncation_of_the_corpus_panics() {
    let mut truncations = 0_usize;
    for path in corpus() {
        let bytes = fs::read(&path).expect("the corpus file is readable");
        let mut length = 0;
        while length < bytes.len() {
            let prefix = bytes.get(..length).expect("the length stays in bounds");
            let _ = parse_junit_xml(prefix, 1);
            let _ = parse_junit_xml(prefix, 0);
            truncations += 1;
            length += 211;
        }
    }
    assert!(truncations > 200, "only {truncations} truncations ran");
}

#[test]
fn arbitrary_bytes_return_an_error_rather_than_panicking() {
    let cases: [&[u8]; 8] = [
        b"",
        b"\xef\xbb\xbf",
        b"\xff\xfe<\x00t\x00",
        b"<?xml",
        b"<?xml encoding=\"",
        b"<?xml encoding='utf-16'?>\xff\xfe",
        b"<testsuite><testcase name=\"\"/></testsuite>",
        b"<testsuite><testcase classname=\"c\" name=\"t\" time=\"\xff\"/></testsuite>",
    ];
    for bytes in cases {
        let _ = parse_junit_xml(bytes, 1);
    }
}

/// A report of `<testsuite>` nested `depth` levels around one passing case.
fn nested_suites(depth: usize) -> Vec<u8> {
    let mut xml = String::with_capacity(depth * 40 + 40);
    for _ in 0..depth {
        xml.push_str("<testsuite name=\"s\">");
    }
    xml.push_str("<testcase classname=\"c\" name=\"t\"/>");
    for _ in 0..depth {
        xml.push_str("</testsuite>");
    }
    xml.into_bytes()
}

#[test]
fn deep_nesting_is_refused_rather_than_overflowing_the_stack() {
    // The reference recursed once per level and aborted the process at 5,000
    // (H0); the walk here is a `Vec`, so the answer is a refusal at any depth.
    // Each parse runs on a 2 MiB stack, which is what a blocking thread gets.
    for depth in [5_000, 200_000] {
        let body = nested_suites(depth);
        let parsed = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || parse_junit_xml(&body, 1))
            .expect("the thread starts")
            .join()
            .expect("the parse returns");
        assert_eq!(
            parsed,
            Err(ParseError::InvalidXml {
                detail: format!(
                    "the report nests `<testsuite>` deeper than {SUITE_DEPTH_MAX} levels"
                )
            }),
            "depth {depth}"
        );
    }
}

#[test]
fn the_deepest_allowed_nesting_is_parsed() {
    let body = nested_suites(SUITE_DEPTH_MAX);
    assert_eq!(
        parse_junit_xml(&body, 1).expect("the deepest allowed nesting parses"),
        parse_junit_xml(&nested_suites(1), 1).expect("a single suite parses")
    );
}

#[test]
fn an_entity_bomb_is_refused() {
    // Nine levels of ten-fold expansion: the reference's parser refuses the
    // amplification, and so does this one, without aborting.
    let bomb = br#"<?xml version="1.0"?>
<!DOCTYPE testsuite [
<!ENTITY a "aaaaaaaaaa">
<!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">
<!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;">
<!ENTITY d "&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;">
<!ENTITY e "&d;&d;&d;&d;&d;&d;&d;&d;&d;&d;">
<!ENTITY f "&e;&e;&e;&e;&e;&e;&e;&e;&e;&e;">
<!ENTITY g "&f;&f;&f;&f;&f;&f;&f;&f;&f;&f;">
<!ENTITY h "&g;&g;&g;&g;&g;&g;&g;&g;&g;&g;">
<!ENTITY i "&h;&h;&h;&h;&h;&h;&h;&h;&h;&h;">
<!ENTITY j "&i;&i;&i;&i;&i;&i;&i;&i;&i;&i;">
]>
<testsuite name="s"><testcase classname="c" name="t"><failure message="m">&j;</failure></testcase></testsuite>"#;
    let error = parse_junit_xml(bomb, 1).expect_err("the bomb is refused");
    assert!(
        error
            .to_string()
            .starts_with("Not a valid JUnit XML report: "),
        "{error}"
    );
}
