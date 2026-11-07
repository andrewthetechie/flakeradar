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

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use flakeradar_core::{
    parsing::ParseError, parsing::SUITE_DEPTH_MAX, parsing::TIME_CHARS_MAX,
    parsing::parse_junit_xml,
};

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

#[test]
fn an_internal_entity_within_the_budget_is_expanded() {
    // The approved rule restores the reference's `DOCTYPE` support, so a body that
    // declares an entity and stays inside the expansion budget parses exactly as
    // it would without one.
    let xml = r#"<?xml version="1.0"?>
<!DOCTYPE testsuite [<!ENTITY name "expanded">]>
<testsuite name="s"><testcase classname="c" name="&name;"><failure message="m">&name;</failure></testcase></testsuite>"#;
    let cases = parse_junit_xml(xml.as_bytes(), 1).expect("a small expansion parses");
    assert_eq!(cases[0].name, "expanded");
    assert_eq!(cases[0].message, "m");
    assert_eq!(cases[0].details, "expanded");
}

#[test]
fn an_entity_that_references_itself_is_refused() {
    // A declared cycle has no expanded length to measure, so the pre-scan refuses
    // it instead of walking it forever: the answer is a bad report, not a hang.
    let xml = br#"<!DOCTYPE testsuite [<!ENTITY a "&b;"><!ENTITY b "&a;">]>
<testsuite name="s"><testcase classname="&a;" name="t"/></testsuite>"#;
    assert_eq!(
        parse_junit_xml(xml, 1),
        Err(ParseError::InvalidXml {
            detail: "the entity references in this report nest in a loop".to_owned()
        })
    );
}

#[test]
fn a_reference_with_no_terminator_is_refused() {
    // `&e` with no `;` is not a reference the scan counts, and the parser refuses
    // the document itself. What matters is that the answer is a refusal rather
    // than a mis-measured expansion.
    let xml = br#"<!DOCTYPE t [<!ENTITY e "aaaa">]><testsuite name="s"><testcase name="a" classname="&e"/></testsuite>"#;
    let error = parse_junit_xml(xml, 1).expect_err("the unterminated reference is refused");
    assert!(
        error
            .to_string()
            .starts_with("Not a valid JUnit XML report: "),
        "{error}"
    );
}

#[test]
fn a_namespace_declaration_on_every_case_is_parsed() {
    // Namespaces are what the second bound is about, and the parser's own answer
    // for them must not change: a declaration under the budget costs nothing and
    // the case is still a case. `<failure xmlns="">` reads as no namespace at all,
    // which is the SP2 case the parser's `is_tag` owns.
    let mut xml = String::from("<testsuite name=\"s\">");
    for index in 0..1000 {
        let _ = write!(xml, "<testcase xmlns:p{index}=\"u\" name=\"a{index}\"/>");
    }
    xml.push_str("</testsuite>");
    let cases = parse_junit_xml(xml.as_bytes(), 1).expect("the declarations parse");
    assert_eq!(cases.len(), 1000);
}

#[test]
fn a_time_past_the_readable_length_is_a_typed_error() {
    // Five million mathematical digits used to spend 19.5 s of a blocking thread
    // rewriting every one of them; past the bound the value is not a number this
    // server reads, so the answer is the same 400 a non-numeric `time` gets.
    let mut xml = String::from("<testsuite name=\"s\"><testcase name=\"a\" time=\"");
    xml.push_str(&"\u{1d7ff}".repeat(100_000));
    xml.push_str("\"/></testsuite>");
    let error = parse_junit_xml(xml.as_bytes(), 1).expect_err("the value is not readable");
    let ParseError::InvalidTime { value } = &error else {
        panic!("a time past the bound is a typed time error: {error}");
    };
    assert_eq!(
        value.chars().count(),
        TIME_CHARS_MAX,
        "the refusal quotes a bounded value"
    );
}

#[test]
fn a_retry_time_past_the_readable_length_reads_zero() {
    // A retry's `time` is context rather than the case's own duration, so an
    // unreadable one reads 0.0, as every other unreadable retry `time` does.
    let mut xml = String::from("<testsuite name=\"s\"><testcase name=\"a\"><rerunFailure time=\"");
    xml.push_str(&"9".repeat(100_000));
    xml.push_str("\"/></testcase></testsuite>");
    let cases = parse_junit_xml(xml.as_bytes(), 1).expect("the case still parses");
    assert_eq!(cases[0].duration, 0.0);
}

#[test]
fn a_namespace_declaration_bound_counts_what_the_parser_copies() {
    // The bound is the parser's own table, not the body's size: 600 nested
    // declarations, and then siblings that inherit them, stay under it.
    let mut xml = String::from("<testsuite name=\"s\">");
    for index in 0..600 {
        let _ = write!(xml, "<x xmlns:p{index}=\"u\">");
    }
    xml.push_str(&"<y xmlns:p0=\"u\"/>".repeat(600));
    xml.push_str(&"</x>".repeat(600));
    xml.push_str("<testcase name=\"a\"/></testsuite>");
    assert_eq!(
        parse_junit_xml(xml.as_bytes(), 1)
            .expect("the declarations parse")
            .len(),
        1
    );
}

#[test]
fn a_reference_at_the_end_of_a_body_is_measured_without_a_terminator() {
    // The scan stops where the text does, so a declared entity referenced in the
    // last bytes is counted as a bare `&` and the parser refuses the document
    // rather than the scan mis-reading an expansion of it.
    let xml =
        br#"<!DOCTYPE t [<!ENTITY e "aaaa">]><testsuite name="s"><testcase name="a" classname="&e"#;
    assert!(
        parse_junit_xml(xml, 1).is_err(),
        "the truncation is refused"
    );
}

#[test]
fn a_declaration_the_prescan_cannot_read_is_left_to_the_parser() {
    // The expansion and namespace bounds read the `DOCTYPE` themselves, so a
    // declaration they cannot read must be the parser's refusal rather than a
    // guessed size. These are also the shapes that make the scan give up: a
    // declaration with no name or no value, an unterminated value, a nested `[`,
    // a stray `]`, and a markup declaration in the document body.
    for subset in [
        "<!ENTITY >",
        "<!ENTITY e",
        "<!ENTITY e>",
        "<!ENTITY e \"unterminated>",
        "[[<!ENTITY e \"x\">]]",
        "]",
    ] {
        let xml = format!(
            "<!DOCTYPE t [{subset}]><testsuite name=\"s\"><testcase name=\"a\"/></testsuite>"
        );
        assert!(
            parse_junit_xml(xml.as_bytes(), 1).is_err(),
            "{subset} must be refused"
        );
    }
    let body = br#"<testsuite xmlns:j="u" name="s"><!ELEMENT x><testcase name="a"/></testsuite>"#;
    assert!(
        parse_junit_xml(body, 1).is_err(),
        "a declaration in the body is refused"
    );
}
