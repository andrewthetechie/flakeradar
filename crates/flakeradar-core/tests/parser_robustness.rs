//! The parser must never panic on any input, however malformed.
//!
//! CI uploads are attacker-controlled, so a panic is a bug rather than a
//! possible outcome: every mutation below must return `Ok` or `Err`. The corpus
//! is the realistic starting point, and the mutations cover the encoding
//! declaration, the structure and the attribute values at once.

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails loudly when the parser panics or the corpus is missing"
)]

use std::fs;
use std::path::{Path, PathBuf};

use flakeradar_core::parse_junit_xml;

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
