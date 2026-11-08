//! The multipart reader the ingest route depends on.
//!
//! The framing cases are real clients' bytes: httpx, curl and writers-app's own
//! upload. Two cases stay in the module because they read the scanner's private
//! needle.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a fixture's framing is not what it says"
)]
//! The framing cases the route depends on, on bytes a real client writes.

use flakeradar::multipart::{MalformedHeaders, REPORT_FIELD, file_field};

/// The file field `body` carries, where the test's framing is well formed.
fn field_of<'a>(body: &'a [u8], content_type: &str) -> Option<&'a [u8]> {
    file_field(body, content_type, REPORT_FIELD).expect("the fixture's headers are headers")
}

/// One part, framed the way httpx and curl frame it.
fn multipart(field: &str, filename: Option<&str>, payload: &[u8]) -> (Vec<u8>, String) {
    let boundary = "-----flakeradar-test-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    match filename {
        Some(filename) => body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{field}\"; filename=\"{filename}\"\r\n\
                 Content-Type: application/xml\r\n\r\n"
            )
            .as_bytes(),
        ),
        None => body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{field}\"\r\n\r\n").as_bytes(),
        ),
    }
    body.extend_from_slice(payload);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (body, format!("multipart/form-data; boundary={boundary}"))
}

/// A part that carries `field` as a file named `r.xml`.
fn file_part(boundary: &str, field: &str, payload: &[u8]) -> Vec<u8> {
    let mut part = format!("--{boundary}\r\n").into_bytes();
    part.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{field}\"; filename=\"r.xml\"\r\n\r\n")
            .as_bytes(),
    );
    part.extend_from_slice(payload);
    part
}

/// A part that carries `field` as a plain form value.
fn value_part(boundary: &str, field: &str, payload: &[u8]) -> Vec<u8> {
    let mut part = format!("--{boundary}\r\n").into_bytes();
    part.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{field}\"\r\n\r\n").as_bytes(),
    );
    part.extend_from_slice(payload);
    part
}

/// The body of a form from parts, each already framed, and its closing
/// delimiter.
fn form(boundary: &str, parts: &[Vec<u8>]) -> Vec<u8> {
    let mut body = Vec::new();
    for part in parts {
        body.extend_from_slice(part);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

#[test]
fn the_report_file_field_is_the_upload() {
    let (body, content_type) = multipart(REPORT_FIELD, Some("junit.xml"), b"<testsuite/>");
    assert_eq!(field_of(&body, &content_type), Some(&b"<testsuite/>"[..]));
}

#[test]
fn a_field_without_a_filename_is_not_a_file() {
    let (body, content_type) = multipart(REPORT_FIELD, None, b"<testsuite/>");
    assert_eq!(field_of(&body, &content_type), None);
}

#[test]
fn a_body_without_the_field_is_empty() {
    let (body, content_type) = multipart("other", Some("junit.xml"), b"<testsuite/>");
    assert_eq!(field_of(&body, &content_type), None);
    // A boundary that is not in the body reads as no field either.
    let (body, _) = multipart(REPORT_FIELD, Some("junit.xml"), b"<testsuite/>");
    assert_eq!(
        field_of(&body, "multipart/form-data; boundary=absent"),
        None
    );
    assert_eq!(field_of(&body, "multipart/form-data"), None);
    assert_eq!(field_of(&body, "multipart/form-data; boundary="), None);
}

#[test]
fn a_later_part_is_found_after_one_that_is_not_it() {
    let boundary = "sep";
    let body = form(
        boundary,
        &[
            value_part(boundary, "note", b"hello"),
            file_part(boundary, REPORT_FIELD, b"<x/>"),
        ],
    );
    assert_eq!(
        field_of(body.as_slice(), "multipart/form-data; boundary=sep"),
        Some(&b"<x/>"[..])
    );
}

#[test]
fn a_quoted_boundary_reads_the_same() {
    let (body, _) = multipart(REPORT_FIELD, Some("junit.xml"), b"<x/>");
    assert_eq!(
        field_of(
            &body,
            "multipart/form-data; charset=utf-8; boundary=\"-----flakeradar-test-boundary\""
        ),
        Some(&b"<x/>"[..])
    );
}

#[test]
fn an_empty_file_is_a_file_with_no_bytes() {
    let (body, content_type) = multipart(REPORT_FIELD, Some("junit.xml"), b"");
    assert_eq!(field_of(&body, &content_type), Some(&b""[..]));
}

#[test]
fn a_payload_that_looks_like_a_boundary_is_kept_whole() {
    // Only a CRLF plus the delimiter ends a part, so the words alone inside
    // the XML stay part of the upload.
    let payload = b"<testsuite name=\"--x\r\ncontent\"/>";
    let (body, content_type) = multipart(REPORT_FIELD, Some("junit.xml"), payload);
    assert_eq!(field_of(&body, &content_type), Some(&payload[..]));
}

#[test]
fn a_body_that_ends_before_any_part_is_empty() {
    // The closing delimiter on its own: there was never a part to read.
    assert_eq!(
        field_of(b"--b--\r\n", "multipart/form-data; boundary=b"),
        None
    );
}

#[test]
fn a_flag_before_the_boundary_does_not_hide_it() {
    let (body, _) = multipart(REPORT_FIELD, Some("junit.xml"), b"<x/>");
    assert_eq!(
        field_of(
            &body,
            "multipart/form-data; charset; boundary=-----flakeradar-test-boundary"
        ),
        Some(&b"<x/>"[..])
    );
}

#[test]
fn an_unterminated_quote_is_not_the_field_name() {
    let body = b"--b\r\nContent-Disposition: form-data; name=\"report; filename=\"r\"\r\n\r\n<x/>\r\n--b--\r\n";
    assert_eq!(field_of(body, "multipart/form-data; boundary=b"), None);
}

#[test]
fn a_parameter_that_names_nothing_is_skipped() {
    let body = b"--b\r\nContent-Disposition: form-data; flag; size=9; name=\"report\"; \
                 filename=\"r.xml\"\r\nX-Ok: 1\r\n\r\n<x/>\r\n--b--\r\n";
    assert_eq!(
        field_of(body, "multipart/form-data; boundary=b"),
        Some(&b"<x/>"[..])
    );
    // A filename with no name leaves the field unnamed.
    let unnamed =
        b"--b\r\nContent-Disposition: form-data; filename=\"r.xml\"\r\n\r\n<x/>\r\n--b--\r\n";
    assert_eq!(field_of(unnamed, "multipart/form-data; boundary=b"), None);
}

#[test]
fn a_header_line_that_is_no_header_is_a_malformed_body() {
    // A header line with no colon is `Invalid multipart data.`, before or after a
    // valid header, and so is a field name holding a character RFC 7230 does not
    // allow.
    for headers in [
        &b"Content-Disposition: form-data; name=\"report\"; filename=\"r.xml\"\r\nX-NoColon"[..],
        &b"X-NoColon\r\nContent-Disposition: form-data; name=\"report\"; filename=\"r.xml\""[..],
        &b"Content-Disposition: form-data; name=\"report\"; filename=\"r.xml\"\r\nX Bad: 1"[..],
    ] {
        let mut body = b"--b\r\n".to_vec();
        body.extend_from_slice(headers);
        body.extend_from_slice(b"\r\n\r\n<x/>\r\n--b--\r\n");
        assert_eq!(
            file_field(&body, "multipart/form-data; boundary=b", REPORT_FIELD),
            Err(MalformedHeaders),
            "{}",
            String::from_utf8_lossy(headers)
        );
    }
}

#[test]
fn a_truncated_body_is_empty() {
    // The opening delimiter with no part after it, and a part with no headers.
    assert_eq!(
        field_of(b"--b\r\n", "multipart/form-data; boundary=b"),
        None
    );
    assert_eq!(
        field_of(b"--b\r\nno headers", "multipart/form-data; boundary=b"),
        None
    );
    // A part that closed but never ended its headers carries no field.
    assert_eq!(
        field_of(
            b"--b\r\nContent-Disposition: form-data; name=\"report\"; filename=\"r.xml\"\r\n--b--\r\n",
            "multipart/form-data; boundary=b"),
        None
    );
    // The delimiter's own bytes run on into something that is no delimiter.
    assert_eq!(
        field_of(b"--bX\r\n", "multipart/form-data; boundary=b"),
        None
    );
    assert_eq!(field_of(b"", "multipart/form-data; boundary=b"), None);
}

#[test]
fn only_a_file_field_understood_as_utf8_names_the_field() {
    let body = b"--b\r\nContent-Disposition: form-data; name=\"\xff\"; filename=\"r\"\r\n\r\nx\r\n--b--\r\n";
    assert_eq!(field_of(body, "multipart/form-data; boundary=b"), None);
}

#[test]
fn a_preamble_is_not_a_form() {
    // CR, LF or the delimiter is the first byte and nothing else: a preamble that
    // starts with any other byte is a malformed body, which answers 400. The route
    // reads that as an empty upload.
    let boundary = "b";
    let framed = form(boundary, &[file_part(boundary, REPORT_FIELD, b"<x/>")]);
    for preamble in [&b"preamble in front of the form\r\n"[..], b" ", b"x\r\n"] {
        let mut body = preamble.to_vec();
        body.extend_from_slice(&framed);
        assert_eq!(
            field_of(&body, "multipart/form-data; boundary=b"),
            None,
            "{preamble:?} in front of the form"
        );
    }
}

#[test]
fn a_preamble_of_leading_newlines_is_skipped() {
    // Any run of leading CR and LF before the delimiter is skipped, so a form
    // behind one still carries its field.
    let boundary = "b";
    let framed = form(boundary, &[file_part(boundary, REPORT_FIELD, b"<x/>")]);
    for preamble in [&b"\n"[..], b"\r\n", b"\r\n\r\n"] {
        let mut body = preamble.to_vec();
        body.extend_from_slice(&framed);
        assert_eq!(
            field_of(&body, "multipart/form-data; boundary=b"),
            Some(&b"<x/>"[..]),
            "{preamble:?} in front of the form"
        );
    }
}

#[test]
fn a_skipped_newline_lets_the_parser_look_past_what_follows_it() {
    // After a leading CR or LF the search jumps to the next `-` instead of reading
    // the bytes between them, so words there still leave a form this parser reads.
    let boundary = "b";
    let framed = form(boundary, &[file_part(boundary, REPORT_FIELD, b"<x/>")]);
    let mut body = b"\r\nwords\r\n".to_vec();
    body.extend_from_slice(&framed);
    assert_eq!(
        field_of(&body, "multipart/form-data; boundary=b"),
        Some(&b"<x/>"[..])
    );
    // A `-` that opens no delimiter is where that search stops, and a body
    // with no `-` after its newlines carries no field either.
    let mut body = b"\r\n-junk".to_vec();
    body.extend_from_slice(&framed);
    assert_eq!(field_of(&body, "multipart/form-data; boundary=b"), None);
    assert_eq!(
        field_of(b"\r\njunk", "multipart/form-data; boundary=b"),
        None
    );
}

#[test]
fn a_false_delimiter_in_a_preamble_is_not_the_form() {
    // The boundary bytes in front of the form are a false delimiter: they are not
    // the delimiter that opens it, and the body is refused.
    let boundary = "b";
    let mut body = b"junk--bX junk\r\n".to_vec();
    body.extend_from_slice(&form(
        boundary,
        &[file_part(boundary, REPORT_FIELD, b"<x/>")],
    ));
    assert_eq!(field_of(&body, "multipart/form-data; boundary=b"), None);
}

#[test]
fn a_false_delimiter_inside_a_part_is_kept_and_the_scan_goes_on() {
    // A part's data can hold the boundary bytes: they stay data — the `note` field
    // is still `hello\r\n--bXworld` — and the part after them is read.
    let boundary = "b";
    let body = form(
        boundary,
        &[
            value_part(boundary, "note", b"hello\r\n--bXworld"),
            file_part(boundary, REPORT_FIELD, b"<x/>"),
        ],
    );
    assert_eq!(
        field_of(&body, "multipart/form-data; boundary=b"),
        Some(&b"<x/>"[..])
    );
}

#[test]
fn a_boundary_that_prefixes_another_stays_in_the_part() {
    // After `--b` comes `b`, so this is not a delimiter at all: the whole run,
    // closing bytes and all, is the part's data.
    let boundary = "b";
    let payload = b"<x/>\r\n--bb\r\nmore";
    let body = form(
        boundary,
        &[file_part(boundary, REPORT_FIELD, payload.as_slice())],
    );
    assert_eq!(
        field_of(&body, "multipart/form-data; boundary=b"),
        Some(&payload[..])
    );
}

#[test]
fn lf_only_framing_is_not_a_form() {
    // The framing requires CRLF: bare LF framing is a malformed body, so the route
    // answers 400 and stores nothing.
    let body = b"--b\nContent-Disposition: form-data; name=\"report\"; filename=\"r.xml\"\n\n<x/>\n--b--\n";
    assert_eq!(field_of(body, "multipart/form-data; boundary=b"), None);
}

#[test]
fn the_last_report_part_wins_whatever_its_kind() {
    // The last value a name carried is the answer, so a duplicated field is the
    // later part. A plain value is a string rather than an upload, which leaves the
    // route with nothing to read: a 400, not the earlier file.
    let boundary = "b";
    let two_files = form(
        boundary,
        &[
            file_part(boundary, REPORT_FIELD, b"<first/>"),
            file_part(boundary, REPORT_FIELD, b"<second/>"),
        ],
    );
    assert_eq!(
        field_of(&two_files, "multipart/form-data; boundary=b"),
        Some(&b"<second/>"[..])
    );
    let file_then_text = form(
        boundary,
        &[
            file_part(boundary, REPORT_FIELD, b"<first/>"),
            value_part(boundary, REPORT_FIELD, b"<second/>"),
        ],
    );
    assert_eq!(
        field_of(&file_then_text, "multipart/form-data; boundary=b"),
        None
    );
    let text_then_file = form(
        boundary,
        &[
            value_part(boundary, REPORT_FIELD, b"<first/>"),
            file_part(boundary, REPORT_FIELD, b"<second/>"),
        ],
    );
    assert_eq!(
        field_of(&text_then_file, "multipart/form-data; boundary=b"),
        Some(&b"<second/>"[..])
    );
    // A part under another name decides nothing at all, here and anywhere.
    let file_then_other = form(
        boundary,
        &[
            value_part(boundary, REPORT_FIELD, b"<first/>"),
            value_part(boundary, "note", b"hello"),
            file_part(boundary, REPORT_FIELD, b"<second/>"),
            file_part(boundary, "note", b"r.xml"),
        ],
    );
    assert_eq!(
        field_of(&file_then_other, "multipart/form-data; boundary=b"),
        Some(&b"<second/>"[..])
    );
}

#[test]
fn a_part_left_open_at_the_end_carries_nothing() {
    // A part whose ending delimiter never arrived carries nothing, and the parts
    // that did close still count.
    let boundary = "b";
    let mut body = form(boundary, &[file_part(boundary, REPORT_FIELD, b"<x/>")]);
    body.extend_from_slice(&file_part(boundary, "note", b"no end"));
    assert_eq!(
        field_of(&body, "multipart/form-data; boundary=b"),
        Some(&b"<x/>"[..])
    );
    // A body whose only part is still open carries no field.
    let body = file_part(boundary, REPORT_FIELD, b"<x/>");
    assert_eq!(field_of(&body, "multipart/form-data; boundary=b"), None);
}
