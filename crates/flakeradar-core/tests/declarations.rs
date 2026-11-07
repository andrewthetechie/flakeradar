//! XML declarations, byte-order marks and `DOCTYPE`s: one test per measured case.
//!
//! Each test names one shape from the second review's `SP4` table and asserts the
//! answer the reference gives for it, measured with the reference's own parser.
//! Where the two still differ, the test asserts this server's answer and holds the
//! reference's in a constant, so the difference is visible where the behaviour
//! lives; `differences.md` records why it stands.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the parser under test is wrong"
)]

use flakeradar_core::{
    model::ExecutionStatus,
    parsing::{ParseError, parse_junit_xml},
};

/// The document every declaration case wraps its report in.
const REPORT: &str = "<testsuite name=\"s\"><testcase classname=\"c\" name=\"a\"/></testsuite>";

/// Parses `xml`, expecting one case.
fn one(xml: &[u8]) -> flakeradar_core::parsing::ParsedCase {
    let cases = parse_junit_xml(xml, 1).expect("the report parses");
    assert_eq!(cases.len(), 1, "expected one case");
    cases.into_iter().next().expect("one case")
}

/// The refusal `xml` gets.
fn refused(xml: &[u8]) -> ParseError {
    parse_junit_xml(xml, 1).expect_err("the report is refused")
}

#[test]
fn a_processing_instruction_that_only_starts_with_xml_is_not_a_declaration() {
    // `<?xml-stylesheet href="a" encoding="bogus"?>` is a processing instruction,
    // not the declaration, so its `encoding` is none of this document's business
    // and the reference parses the report. Reading the pseudo-attribute as a
    // declaration refused the upload as an unknown encoding.
    let case = one(format!("<?xml-stylesheet href=\"a\" encoding=\"bogus\"?>{REPORT}").as_bytes());
    assert_eq!(case.name, "a");
    assert_eq!(case.status, ExecutionStatus::Passed);
}

#[test]
fn a_declaration_whose_encoding_name_is_padded_is_refused() {
    // XML's declaration has no space around the value, and the reference reports a
    // declaration that is not well formed for both spellings.
    for declaration in [
        "<?xml version=\"1.0\" encoding=\" utf-8\"?>",
        "<?xml version=\"1.0\" encoding=\"utf-8 \"?>",
    ] {
        let error = refused(format!("{declaration}{REPORT}").as_bytes());
        assert_eq!(
            error,
            ParseError::InvalidXml {
                detail: "XML declaration not well-formed".to_owned()
            },
            "{declaration}"
        );
    }
}

#[test]
fn a_utf8_byte_order_mark_still_leaves_the_declaration_to_decide() {
    // The mark says the document is UTF-8, and the declaration says otherwise: the
    // reference decodes `0xC3 0xA9` as the declared code page reads those bytes,
    // which is `Ã©`, not `é`.
    for label in ["iso-8859-1", "windows-1252"] {
        let mut xml = b"\xEF\xBB\xBF".to_vec();
        xml.extend_from_slice(format!("<?xml version=\"1.0\" encoding=\"{label}\"?>").as_bytes());
        xml.extend_from_slice(b"<testsuite><testcase name=\"\xC3\xA9\"/></testsuite>");
        assert_eq!(one(&xml).name, "\u{c3}\u{a9}", "{label}");
    }
}

#[test]
fn a_utf16_document_whose_declaration_names_another_codec_is_refused() {
    // The bytes are UTF-16 because of their mark, and the declaration names
    // `utf-8`, `iso-8859-1` or `us-ascii`: the reference refuses all three.
    for label in ["utf-8", "iso-8859-1", "us-ascii"] {
        let body = format!("<?xml version=\"1.0\" encoding=\"{label}\"?>");
        let mut xml = b"\xFF\xFE".to_vec();
        xml.extend_from_slice(&utf16le(&format!(
            "{body}<testsuite><testcase name=\"é\"/></testsuite>"
        )));
        assert!(
            matches!(refused(&xml), ParseError::InvalidXml { .. }),
            "{label}"
        );
    }
}

#[test]
fn a_utf16_document_whose_label_is_unknown_is_refused_as_an_unknown_encoding() {
    let mut xml = b"\xFF\xFE".to_vec();
    xml.extend_from_slice(&utf16le(
        "<?xml version=\"1.0\" encoding=\"bogus\"?><testsuite><testcase name=\"é\"/></testsuite>",
    ));
    assert_eq!(
        refused(&xml),
        ParseError::InvalidXml {
            detail: "unknown encoding: bogus".to_owned()
        }
    );
}

#[test]
fn a_utf16_mark_and_a_label_of_the_other_endianness_are_refused() {
    // The mark is big-endian and the label says little-endian; the reference
    // refuses the document, and so does this server.
    let mut xml = b"\xFE\xFF".to_vec();
    xml.extend_from_slice(&utf16be(
        "<?xml version=\"1.0\" encoding=\"utf-16le\"?><testsuite><testcase name=\"é\"/></testsuite>",
    ));
    assert!(matches!(refused(&xml), ParseError::InvalidXml { .. }));
}

#[test]
fn a_hyphenated_utf16_label_is_a_multi_byte_codec_the_reference_refuses() {
    // `utf-16-le` is not expat's own name for the endianness; it is a codec name,
    // and the reference refuses a multi-byte codec whatever the bytes are.
    let mut xml = b"\xFF\xFE".to_vec();
    xml.extend_from_slice(&utf16le(
        "<?xml version=\"1.0\" encoding=\"utf-16-le\"?><testsuite><testcase name=\"é\"/></testsuite>",
    ));
    assert!(matches!(refused(&xml), ParseError::InvalidXml { .. }));
}

#[test]
fn a_utf16_mark_with_a_label_that_names_its_endianness_is_read() {
    // `utf-16le` is expat's own name, and the mark agrees with it.
    let mut xml = b"\xFF\xFE".to_vec();
    xml.extend_from_slice(&utf16le(
        "<?xml version=\"1.0\" encoding=\"utf-16le\"?><testsuite><testcase name=\"é\"/></testsuite>",
    ));
    assert_eq!(one(&xml).name, "é");
}

#[test]
fn a_utf16_mark_with_the_plain_utf16_label_is_read() {
    let mut xml = b"\xFF\xFE".to_vec();
    xml.extend_from_slice(&utf16le(
        "<?xml version=\"1.0\" encoding=\"utf-16\"?><testsuite><testcase name=\"é\"/></testsuite>",
    ));
    assert_eq!(one(&xml).name, "é");
}

#[test]
fn an_attribute_reference_to_an_entity_an_external_subset_would_declare_is_skipped() {
    // The reference never fetches `x.dtd`, so it skips `&nope;` rather than
    // refusing it: the attribute holds the text around the reference.
    let cases = parse_junit_xml(
        br#"<!DOCTYPE t SYSTEM "x.dtd"><testsuite><testcase name="a&nope;b"/><testcase name="&nope;"/></testsuite>"#,
        1,
    )
    .expect("the report parses");
    assert_eq!(
        cases
            .iter()
            .map(|case| case.name.as_str())
            .collect::<Vec<_>>(),
        ["ab", ""]
    );
}

#[test]
fn a_skipped_reference_is_skipped_with_a_declaration_and_a_comment_around_it() {
    // The skip reads past a declaration that says nothing about `standalone`, and
    // past a comment, which holds no reference of its own.
    let case = one(
        br#"<?xml version="1.0"?><!-- c --><!DOCTYPE t SYSTEM "x.dtd"><testsuite><testcase name="a&nope;b" classname="c&amp;d"/></testsuite>"#,
    );
    assert_eq!(case.name, "ab");
    // A reference the document does resolve is left alone, here a predefined one.
    assert_eq!(case.classname, "c&d");
}

#[test]
fn a_text_reference_to_an_undefined_entity_is_refused() {
    // The skip is an attribute's rule only: the reference refuses an undefined
    // entity in the document's text, `&nope;` in a Failure body included.
    let error = refused(
        br#"<!DOCTYPE t SYSTEM "x.dtd"><testsuite><testcase name="a"><failure>&nope;</failure></testcase></testsuite>"#,
    );
    assert!(matches!(error, ParseError::InvalidXml { .. }), "{error}");
}

#[test]
fn a_standalone_document_keeps_its_undefined_entity_refused() {
    // `standalone="yes"` promises the document needs no external declaration, so
    // the reference refuses the reference it cannot resolve.
    let error = refused(
        br#"<?xml version="1.0" standalone="yes"?><!DOCTYPE t SYSTEM "x.dtd"><testsuite><testcase name="a&nope;b"/></testsuite>"#,
    );
    assert!(matches!(error, ParseError::InvalidXml { .. }), "{error}");
}

#[test]
fn an_attlist_default_classname_is_not_read() {
    // The reference reads the `ATTLIST` default and gives the case the classname
    // `dflt`; attribute defaults are one of the two things `roxmltree` does not
    // model, so the classname is empty here. `differences.md` records it.
    const REFERENCE: &str = "dflt";
    let case = one(format!(
        "<!DOCTYPE t [<!ATTLIST testcase classname CDATA \"{REFERENCE}\">]>\
             <testsuite name=\"s\"><testcase name=\"a\"/></testsuite>"
    )
    .as_bytes());
    assert_eq!(case.classname, "", "the reference gives {REFERENCE} here");
}

#[test]
fn an_attlist_default_file_is_not_read() {
    // As above, for the Location a default would add.
    const REFERENCE: &str = "dflt.py";
    let case = one(format!(
        "<!DOCTYPE t [<!ATTLIST testcase file CDATA \"{REFERENCE}\">]>{REPORT}"
    )
    .as_bytes());
    assert_eq!(case.file, None, "the reference gives {REFERENCE} here");
}

#[test]
fn an_attlist_declared_nmtoken_is_not_normalized() {
    // `NMTOKEN` attributes are normalized: a run of spaces becomes one and the
    // ends are trimmed, so the reference reads the name `a b`.
    const REFERENCE: &str = "a b";
    let case = one(
        br#"<!DOCTYPE t [<!ATTLIST testcase name NMTOKEN #IMPLIED>]><testsuite><testcase name="  a  b "/></testsuite>"#,
    );
    assert_eq!(case.name, "  a  b ", "the reference gives {REFERENCE} here");
}

#[test]
fn a_parameter_entity_reference_in_the_internal_subset_is_refused() {
    // The reference accepts the reference to the parameter entity, and declares
    // the general entity through it, so its one case has an empty name.
    const REFERENCE: &str = "";
    let error = refused(
        br#"<!DOCTYPE t [<!ENTITY % p "<!ENTITY e 'x'>"> %p;]><testsuite><testcase name="&e;"/></testsuite>"#,
    );
    assert!(
        matches!(error, ParseError::InvalidXml { .. }),
        "the reference gives a case named {REFERENCE:?} here"
    );
}

#[test]
fn a_parameter_entity_system_reference_in_the_internal_subset_is_refused() {
    // As above, for a parameter entity that names an external subset.
    const REFERENCE: &str = "";
    let error = refused(
        br#"<!DOCTYPE t [<!ENTITY % p SYSTEM "x"> %p; <!ENTITY e "y">]><testsuite><testcase name="&e;"/></testsuite>"#,
    );
    assert!(
        matches!(error, ParseError::InvalidXml { .. }),
        "the reference gives a case named {REFERENCE:?} here"
    );
}

#[test]
fn a_processing_instruction_whose_target_is_xml_in_another_case_is_read() {
    // XML reserves the target `xml` in every spelling, so the reference refuses
    // `<?XML version="1.0"?>`; `roxmltree` reads it as an ordinary instruction.
    const REFERENCE: &str = "not well-formed (invalid token)";
    let case = one(format!("<?XML version=\"1.0\"?>{REPORT}").as_bytes());
    assert_eq!(case.name, "a", "the reference refuses it: {REFERENCE}");
}

#[test]
fn a_processing_instruction_named_xml_inside_the_document_is_read() {
    const REFERENCE: &str = "not well-formed (invalid token)";
    let case = one(br#"<testsuite><?xMl x?><testcase name="a"/></testsuite>"#);
    assert_eq!(case.name, "a", "the reference refuses it: {REFERENCE}");
}

#[test]
fn a_standalone_value_that_is_not_yes_or_no_is_read() {
    // XML allows `yes` and `no` only, so the reference reports a declaration that
    // is not well formed; `roxmltree` does not check the value.
    const REFERENCE: &str = "XML declaration not well-formed";
    let case = one(format!("<?xml version=\"1.0\" standalone=\"maybe\"?>{REPORT}").as_bytes());
    assert_eq!(case.name, "a", "the reference refuses it: {REFERENCE}");
}

#[test]
fn a_namespace_declaration_that_undeclares_a_prefix_is_read() {
    // XML 1.0 has no way to undeclare a prefix, so the reference refuses
    // `xmlns:a=""`; `roxmltree` accepts it.
    const REFERENCE: &str = "must not undeclare prefix";
    let case = one(br#"<testsuite xmlns:a=""><testcase name="b"/></testsuite>"#);
    assert_eq!(case.name, "b", "the reference refuses it: {REFERENCE}");
}

#[test]
fn a_declaration_of_the_reserved_xmlns_prefix_is_read() {
    const REFERENCE: &str = "reserved prefix (xmlns) must not be declared or undeclared";
    let case = one(br#"<testsuite xmlns:xmlns="u"><testcase name="b"/></testsuite>"#);
    assert_eq!(case.name, "b", "the reference refuses it: {REFERENCE}");
}

#[test]
fn an_element_name_that_starts_with_a_colon_is_read() {
    const REFERENCE: &str = "not well-formed (invalid token)";
    let case = one(br#"<testsuite><:a/><testcase name="b"/></testsuite>"#);
    assert_eq!(case.name, "b", "the reference refuses it: {REFERENCE}");
}

#[test]
fn an_attribute_name_that_starts_with_a_colon_is_read() {
    const REFERENCE: &str = "not well-formed (invalid token)";
    let case = one(br#"<testsuite><testcase :a="1" name="b"/></testsuite>"#);
    assert_eq!(case.name, "b", "the reference refuses it: {REFERENCE}");
}

/// Encodes text as UTF-16LE, as a byte-order-marked report would hold it.
fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<u8>>()
}

/// Encodes text as UTF-16BE.
fn utf16be(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect::<Vec<u8>>()
}
