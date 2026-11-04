//! The multipart request body `POST /api/ingest` reads an upload from.
//!
//! The frozen server handed a `multipart/form-data` request to Starlette's form
//! parser and read the file field named `report`; every other content type was
//! the raw body. The route reads the whole body under its own size limit first,
//! so this only has to find that one file in the buffered bytes: a missing
//! field, or a field sent without a filename, reads as an empty upload.

// Rust guideline compliant 2026-07-21

/// The form field `POST /api/ingest` reads a JUnit XML file from.
pub const REPORT_FIELD: &str = "report";

/// The file field `field` in a buffered multipart body, when the body has one.
///
/// `content_type` is the request's `Content-Type`, whose `boundary` parameter
/// separates the parts. Anything this cannot read — a missing or empty boundary,
/// an unterminated part, a body with no such file field — reads as `None`.
#[must_use]
pub fn file_field<'a>(body: &'a [u8], content_type: &str, field: &str) -> Option<&'a [u8]> {
    let boundary = boundary_of(content_type)?;
    let mut delimiter = Vec::with_capacity(boundary.len() + 2);
    delimiter.extend_from_slice(b"--");
    delimiter.extend_from_slice(boundary.as_bytes());
    // A part's content ends at the CRLF that introduces the next delimiter.
    let mut end_marker = Vec::with_capacity(delimiter.len() + 2);
    end_marker.extend_from_slice(b"\r\n");
    end_marker.extend_from_slice(&delimiter);

    let mut rest = body;
    loop {
        let start = find(rest, &delimiter)?;
        let after = rest.get(start + delimiter.len()..)?;
        // The closing delimiter ends the body: no part carries the field.
        if after.starts_with(b"--") {
            return None;
        }
        let part_start = after.strip_prefix(b"\r\n")?;
        let end = find(part_start, &end_marker)?;
        let part = part_start.get(..end)?;
        if let Some(payload) = part_file(part, field) {
            return Some(payload);
        }
        rest = part_start.get(end..)?;
    }
}

/// The bytes of the boundary parameter in `content_type`.
fn boundary_of(content_type: &str) -> Option<&str> {
    for parameter in content_type.split(';').skip(1) {
        let (key, value) = parameter.split_once('=')?;
        if !key.trim().eq_ignore_ascii_case("boundary") {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap_or(value);
        return (!value.is_empty()).then_some(value);
    }
    None
}

/// The payload of `field` when this part is that field sent as a file.
fn part_file<'a>(part: &'a [u8], field: &str) -> Option<&'a [u8]> {
    let header_end = find(part, b"\r\n\r\n")?;
    let headers = part.get(..header_end)?;
    let payload = part.get(header_end + 4..)?;
    let mut named = false;
    let mut filename = false;
    for line in headers.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            continue;
        };
        let (name, value) = (line.get(..colon)?, line.get(colon + 1..)?);
        if name.eq_ignore_ascii_case(b"content-disposition") {
            let (part_name, has_filename) = disposition(value);
            named = part_name.is_some_and(|part_name| part_name == field);
            filename = has_filename;
        }
    }
    (named && filename).then_some(payload)
}

/// The `name` parameter of a `Content-Disposition` value, and whether it also
/// carries a `filename`, which is what makes the field a file rather than a
/// plain form value.
fn disposition(value: &[u8]) -> (Option<String>, bool) {
    let mut name = None;
    let mut filename = false;
    for parameter in value.split(|byte| *byte == b';').skip(1) {
        let parameter = trim_ascii(parameter);
        let Some(equals) = parameter.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let (key, raw) = (parameter.get(..equals), parameter.get(equals + 1..));
        let (Some(key), Some(raw)) = (key, raw) else {
            continue;
        };
        let raw = trim_ascii(raw);
        let raw = raw
            .strip_prefix(b"\"")
            .and_then(|rest| rest.strip_suffix(b"\""))
            .unwrap_or(raw);
        if key.eq_ignore_ascii_case(b"name") {
            name = String::from_utf8(raw.to_vec()).ok();
        } else if key.eq_ignore_ascii_case(b"filename") {
            filename = true;
        }
    }
    (name, filename)
}

/// `slice` without leading or trailing ASCII whitespace.
fn trim_ascii(slice: &[u8]) -> &[u8] {
    let start = slice
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(slice.len());
    let end = slice
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |last| last + 1);
    slice.get(start..end).unwrap_or_default()
}

/// Where `needle` first appears in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    //! The framing cases the route depends on, on bytes a real client writes.

    use super::{REPORT_FIELD, file_field};

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

    #[test]
    fn the_report_file_field_is_the_upload() {
        let (body, content_type) = multipart(REPORT_FIELD, Some("junit.xml"), b"<testsuite/>");
        assert_eq!(
            file_field(&body, &content_type, REPORT_FIELD),
            Some(&b"<testsuite/>"[..])
        );
    }

    #[test]
    fn a_field_without_a_filename_is_not_a_file() {
        let (body, content_type) = multipart(REPORT_FIELD, None, b"<testsuite/>");
        assert_eq!(file_field(&body, &content_type, REPORT_FIELD), None);
    }

    #[test]
    fn a_body_without_the_field_is_empty() {
        let (body, content_type) = multipart("other", Some("junit.xml"), b"<testsuite/>");
        assert_eq!(file_field(&body, &content_type, REPORT_FIELD), None);
        // A boundary that is not in the body reads as no field either.
        let (body, _) = multipart(REPORT_FIELD, Some("junit.xml"), b"<testsuite/>");
        assert_eq!(
            file_field(&body, "multipart/form-data; boundary=absent", REPORT_FIELD),
            None
        );
        assert_eq!(file_field(&body, "multipart/form-data", REPORT_FIELD), None);
        assert_eq!(
            file_field(&body, "multipart/form-data; boundary=", REPORT_FIELD),
            None
        );
    }

    #[test]
    fn a_later_part_is_found_after_one_that_is_not_it() {
        let boundary = "sep";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhello\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"report\"; filename=\"r.xml\"\r\n\r\n<x/>\r\n\
             --{boundary}--\r\n"
        );
        assert_eq!(
            file_field(
                body.as_bytes(),
                "multipart/form-data; boundary=sep",
                REPORT_FIELD
            ),
            Some(&b"<x/>"[..])
        );
    }

    #[test]
    fn a_quoted_boundary_reads_the_same() {
        let (body, _) = multipart(REPORT_FIELD, Some("junit.xml"), b"<x/>");
        assert_eq!(
            file_field(
                &body,
                "multipart/form-data; charset=utf-8; boundary=\"-----flakeradar-test-boundary\"",
                REPORT_FIELD
            ),
            Some(&b"<x/>"[..])
        );
    }

    #[test]
    fn an_empty_file_is_a_file_with_no_bytes() {
        let (body, content_type) = multipart(REPORT_FIELD, Some("junit.xml"), b"");
        assert_eq!(
            file_field(&body, &content_type, REPORT_FIELD),
            Some(&b""[..])
        );
    }

    #[test]
    fn a_payload_that_looks_like_a_boundary_is_kept_whole() {
        // Only a CRLF plus the delimiter ends a part, so the words alone inside
        // the XML stay part of the upload.
        let payload = b"<testsuite name=\"--x\r\ncontent\"/>";
        let (body, content_type) = multipart(REPORT_FIELD, Some("junit.xml"), payload);
        assert_eq!(
            file_field(&body, &content_type, REPORT_FIELD),
            Some(&payload[..])
        );
    }

    #[test]
    fn a_truncated_body_is_empty() {
        // The opening delimiter with no part after it, and a part with no headers.
        assert_eq!(
            file_field(b"--b\r\n", "multipart/form-data; boundary=b", REPORT_FIELD),
            None
        );
        assert_eq!(
            file_field(
                b"--b\r\nno headers",
                "multipart/form-data; boundary=b",
                REPORT_FIELD
            ),
            None
        );
        assert_eq!(
            file_field(b"", "multipart/form-data; boundary=b", REPORT_FIELD),
            None
        );
    }

    #[test]
    fn only_a_file_field_understood_as_utf8_names_the_field() {
        let body = b"--b\r\nContent-Disposition: form-data; name=\"\xff\"; filename=\"r\"\r\n\r\nx\r\n--b--\r\n";
        assert_eq!(
            file_field(body, "multipart/form-data; boundary=b", REPORT_FIELD),
            None
        );
    }
}
