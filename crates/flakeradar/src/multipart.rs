//! The multipart request body `POST /api/ingest` reads an upload from.
//!
//! The frozen server handed a `multipart/form-data` request to Starlette's form
//! parser and read the file field named `report`; every other content type was
//! the raw body. The route reads the whole body under its own size limit first,
//! so this only has to find that one file in the buffered bytes: a missing
//! field, a field sent without a filename, or a body whose framing the reference
//! parser would refuse all read as an empty upload.
//!
//! The scan is linear in the body however the client shapes it. The boundary is
//! the client's to choose and the body can be 20 MiB, so a search that re-reads
//! bytes it has already seen — `slice::windows` and its `position` do — is a
//! quadratic attack on the thread that serves the upload.

// Rust guideline compliant 2026-07-21

/// The form field `POST /api/ingest` reads a JUnit XML file from.
pub const REPORT_FIELD: &str = "report";

/// The bytes that end a part's data and introduce the next delimiter.
const CRLF: &[u8] = b"\r\n";

/// The bytes that separate a part's headers from its data.
const HEADER_END: &[u8] = b"\r\n\r\n";

/// The two dashes that turn a delimiter into the closing one.
const CLOSING: &[u8] = b"--";

/// The file field `field` in a buffered multipart body, when the body has one.
///
/// `content_type` is the request's `Content-Type`, whose `boundary` parameter
/// separates the parts. Anything this cannot read — a missing or empty boundary,
/// a body that does not open with its delimiter, an unterminated part, a body
/// with no such file field — reads as `None`.
///
/// A part's data may hold anything, the boundary bytes included: only a CRLF
/// followed by the delimiter and then a CRLF or the closing dashes ends a part,
/// so the reference parser keeps scanning past bytes that merely look like one.
/// When the field arrives more than once, the last part that carries it wins,
/// because that is the value Starlette's form answered.
#[must_use]
pub fn file_field<'a>(body: &'a [u8], content_type: &str, field: &str) -> Option<&'a [u8]> {
    let boundary = boundary_of(content_type)?;
    let framing = Framing::new(boundary.as_bytes());
    // The reference parser expects the form to open with its delimiter and
    // refuses a preamble, so a body with something in front of it carries no
    // field. A single leading CRLF is the one thing it skips.
    let opening = if body.starts_with(CRLF) {
        CRLF.len()
    } else {
        0
    };
    let mut delimiter_at = framing.opening(body, opening)?;
    let mut found = None;
    loop {
        let data_at = delimiter_at.checked_add(framing.delimiter.len())?;
        // A delimiter followed by anything but a CRLF closes the form, or is no
        // delimiter at all; the form's parts carry no field from here on.
        if !body.get(data_at..)?.starts_with(CRLF) {
            return found;
        }
        let part_start = data_at.checked_add(CRLF.len())?;
        // A part whose ending delimiter never arrived is still open when the
        // body ends, and the reference parser drops what it read of it.
        let Some(part_end) = framing.next_end(body, part_start) else {
            return found;
        };
        found = part_file(body.get(part_start..part_end)?, field).or(found);
        delimiter_at = part_end.checked_add(CRLF.len())?;
    }
}

/// The bytes of the boundary parameter in `content_type`.
fn boundary_of(content_type: &str) -> Option<&str> {
    for parameter in content_type.split(';').skip(1) {
        // A parameter without a value, such as a bare `charset`, is not the
        // boundary, and does not stop the search for it.
        let Some((key, value)) = parameter.split_once('=') else {
            continue;
        };
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
    let header_end = find(part, HEADER_END)?;
    let headers = part.get(..header_end)?;
    let payload = part.get(header_end + HEADER_END.len()..)?;
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
        // A parameter without a value is not one this reads, such as a bare flag
        // a client added.
        let mut key = Vec::new();
        let mut raw = Vec::new();
        let mut valued = false;
        for byte in parameter {
            if valued {
                raw.push(*byte);
            } else if *byte == b'=' {
                valued = true;
            } else {
                key.push(*byte);
            }
        }
        if !valued {
            continue;
        }
        let (key, raw) = (key.as_slice(), trim_ascii(&raw));
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
    Needle::new(needle.to_vec()).find(haystack, 0)
}

/// One form's boundary, and the search for the delimiter that ends a part.
#[derive(Debug)]
struct Framing {
    /// The delimiter that introduces a part: two dashes and the boundary.
    delimiter: Vec<u8>,
    /// The delimiter preceded by the CRLF that ends the part before it.
    end_marker: Needle,
}

impl Framing {
    /// The framing of one body, from the boundary its `Content-Type` named.
    fn new(boundary: &[u8]) -> Self {
        let mut delimiter = Vec::with_capacity(boundary.len() + CLOSING.len());
        delimiter.extend_from_slice(CLOSING);
        delimiter.extend_from_slice(boundary);
        let mut end_marker = Vec::with_capacity(delimiter.len() + CRLF.len());
        end_marker.extend_from_slice(CRLF);
        end_marker.extend_from_slice(&delimiter);
        Self {
            delimiter,
            end_marker: Needle::new(end_marker),
        }
    }

    /// Where the body's opening delimiter starts, when it opens with one.
    fn opening(&self, body: &[u8], at: usize) -> Option<usize> {
        body.get(at..)?.starts_with(&self.delimiter).then_some(at)
    }

    /// Where the part that starts at `from` ends: the CRLF in front of the next
    /// delimiter that really introduces a part or closes the form.
    ///
    /// A part's data can hold the boundary bytes, so an occurrence that is not
    /// followed by a CRLF or the closing dashes is data. The scan goes on from
    /// the byte after it, and every byte it reads is read a bounded number of
    /// times.
    fn next_end(&self, body: &[u8], from: usize) -> Option<usize> {
        let mut at = from;
        loop {
            let end = self.end_marker.find(body, at)?;
            let after_delimiter = end.checked_add(self.end_marker.bytes.len())?;
            if body.get(after_delimiter..).is_some_and(starts_or_closes) {
                return Some(end);
            }
            at = end.checked_add(1)?;
        }
    }
}

/// Whether `after` continues a delimiter into a part's CRLF or the form's
/// closing dashes.
fn starts_or_closes(after: &[u8]) -> bool {
    after.starts_with(CRLF) || after.starts_with(CLOSING)
}

/// A byte needle with the failure table that makes searching linear.
///
/// The table says where to resume inside the needle after a mismatch, so no
/// haystack byte is compared twice: a search costs at most two comparisons per
/// byte it reads, whatever the needle and the haystack look like.
#[derive(Debug)]
struct Needle {
    /// The bytes to find.
    bytes: Vec<u8>,
    /// For each prefix of `bytes`, the length of its longest proper border.
    failure: Vec<usize>,
}

impl Needle {
    /// The needle `bytes`, with its failure table built once.
    fn new(bytes: Vec<u8>) -> Self {
        let mut failure = Vec::with_capacity(bytes.len());
        // The table has one entry per needle byte, so the first byte's border is
        // zero and each byte after it gets the border of the prefix up to it.
        failure.extend(bytes.first().map(|_| 0));
        let mut border = 0;
        for at in 1..bytes.len() {
            let byte = bytes.get(at).copied().unwrap_or_default();
            while border > 0 && bytes.get(border).copied() != Some(byte) {
                border = failure.get(border - 1).copied().unwrap_or_default();
            }
            if bytes.get(border).copied() == Some(byte) {
                border += 1;
            }
            failure.push(border);
        }
        Self { bytes, failure }
    }

    /// Where this needle first appears in `haystack` at or after `from`.
    ///
    /// An empty needle is never found, and neither is one longer than what is
    /// left of the haystack.
    fn find(&self, haystack: &[u8], from: usize) -> Option<usize> {
        let mut matched = 0;
        let mut at = from;
        while let Some(byte) = haystack.get(at).copied() {
            #[cfg(test)]
            comparisons::record();
            if self.bytes.get(matched).copied() == Some(byte) {
                matched += 1;
                if matched == self.bytes.len() {
                    return Some(at + 1 - matched);
                }
            } else if matched == 0 {
                // Nothing of the needle is matched, so this byte is a plain miss.
            } else {
                // Resume inside the needle, and try this byte against the
                // shorter prefix before moving on.
                matched = self.failure.get(matched - 1).copied().unwrap_or_default();
                #[cfg(test)]
                comparisons::record();
                continue;
            }
            at += 1;
        }
        None
    }
}

/// A count of the byte comparisons the scan made, so a test can assert its work
/// stays linear without timing anything.
///
/// Every search the parser runs goes through [`Needle::find`], which is the one
/// place a byte of the body is compared with a byte of the boundary.
#[cfg(test)]
mod comparisons {
    use std::cell::Cell;

    thread_local! {
        /// The comparisons the current test thread's scans made.
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }

    /// Adds one comparison to the current thread's count.
    pub(super) fn record() {
        COUNT.with(|count| count.set(count.get() + 1));
    }

    /// The comparisons the current thread made since [`reset`].
    pub(super) fn count() -> usize {
        COUNT.with(Cell::get)
    }

    /// Sets the current thread's count back to zero.
    pub(super) fn reset() {
        COUNT.with(|count| count.set(0));
    }
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
        let body = form(
            boundary,
            &[
                value_part(boundary, "note", b"hello"),
                file_part(boundary, REPORT_FIELD, b"<x/>"),
            ],
        );
        assert_eq!(
            file_field(
                body.as_slice(),
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
    fn a_body_that_ends_before_any_part_is_empty() {
        // The closing delimiter on its own: there was never a part to read.
        assert_eq!(
            file_field(
                b"--b--\r\n",
                "multipart/form-data; boundary=b",
                REPORT_FIELD
            ),
            None
        );
    }

    #[test]
    fn a_flag_before_the_boundary_does_not_hide_it() {
        let (body, _) = multipart(REPORT_FIELD, Some("junit.xml"), b"<x/>");
        assert_eq!(
            file_field(
                &body,
                "multipart/form-data; charset; boundary=-----flakeradar-test-boundary",
                REPORT_FIELD
            ),
            Some(&b"<x/>"[..])
        );
    }

    #[test]
    fn an_unterminated_quote_is_not_the_field_name() {
        let body = b"--b\r\nContent-Disposition: form-data; name=\"report; filename=\"r\"\r\n\r\n<x/>\r\n--b--\r\n";
        assert_eq!(
            file_field(body, "multipart/form-data; boundary=b", REPORT_FIELD),
            None
        );
    }

    #[test]
    fn a_parameter_that_names_nothing_is_skipped() {
        let body = b"--b\r\nContent-Disposition: form-data; flag; size=9; name=\"report\"; \
                     filename=\"r.xml\"\r\nX-NoColon\r\n\r\n<x/>\r\n--b--\r\n";
        assert_eq!(
            file_field(body, "multipart/form-data; boundary=b", REPORT_FIELD),
            Some(&b"<x/>"[..])
        );
        // A filename with no name leaves the field unnamed.
        let unnamed =
            b"--b\r\nContent-Disposition: form-data; filename=\"r.xml\"\r\n\r\n<x/>\r\n--b--\r\n";
        assert_eq!(
            file_field(unnamed, "multipart/form-data; boundary=b", REPORT_FIELD),
            None
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
        // The delimiter's own bytes run on into something that is no delimiter.
        assert_eq!(
            file_field(b"--bX\r\n", "multipart/form-data; boundary=b", REPORT_FIELD),
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

    #[test]
    fn a_preamble_is_not_a_form() {
        // python-multipart expects the body to open with its delimiter and
        // refuses a preamble: Starlette's `request.form()` raises
        // MultiPartException, which FastAPI answers with 400. The route reads
        // that as an empty upload, so a preamble hides the field.
        let boundary = "b";
        let mut body = b"preamble in front of the form\r\n".to_vec();
        body.extend_from_slice(&form(
            boundary,
            &[file_part(boundary, REPORT_FIELD, b"<x/>")],
        ));
        assert_eq!(
            file_field(&body, "multipart/form-data; boundary=b", REPORT_FIELD),
            None
        );
        // A single leading CRLF is the one preamble the reference parser skips.
        let mut body = b"\r\n".to_vec();
        body.extend_from_slice(&form(
            boundary,
            &[file_part(boundary, REPORT_FIELD, b"<x/>")],
        ));
        assert_eq!(
            file_field(&body, "multipart/form-data; boundary=b", REPORT_FIELD),
            Some(&b"<x/>"[..])
        );
    }

    #[test]
    fn a_false_delimiter_in_a_preamble_is_not_the_form() {
        // The boundary bytes in front of the form are a false delimiter: the
        // reference parser refuses the body, and they must not be read as the
        // delimiter that opens it.
        let boundary = "b";
        let mut body = b"junk--bX junk\r\n".to_vec();
        body.extend_from_slice(&form(
            boundary,
            &[file_part(boundary, REPORT_FIELD, b"<x/>")],
        ));
        assert_eq!(
            file_field(&body, "multipart/form-data; boundary=b", REPORT_FIELD),
            None
        );
    }

    #[test]
    fn a_false_delimiter_inside_a_part_is_kept_and_the_scan_goes_on() {
        // A part's data can hold the boundary bytes. python-multipart keeps them
        // as data — its `note` field is still `hello\r\n--bXworld` — and reads
        // the part after them.
        let boundary = "b";
        let body = form(
            boundary,
            &[
                value_part(boundary, "note", b"hello\r\n--bXworld"),
                file_part(boundary, REPORT_FIELD, b"<x/>"),
            ],
        );
        assert_eq!(
            file_field(&body, "multipart/form-data; boundary=b", REPORT_FIELD),
            Some(&b"<x/>"[..])
        );
    }

    #[test]
    fn a_boundary_that_prefixes_another_stays_in_the_part() {
        // After `--b` comes `b`, so this is not a delimiter at all: the whole
        // run, closing bytes and all, is the part's data, and python-multipart
        // reports the same 16 bytes.
        let boundary = "b";
        let payload = b"<x/>\r\n--bb\r\nmore";
        let body = form(
            boundary,
            &[file_part(boundary, REPORT_FIELD, payload.as_slice())],
        );
        assert_eq!(
            file_field(&body, "multipart/form-data; boundary=b", REPORT_FIELD),
            Some(&payload[..])
        );
    }

    #[test]
    fn lf_only_framing_is_not_a_form() {
        // python-multipart requires CRLF: bare LF framing raises
        // MultiPartException, so the route answers 400 and stores nothing.
        let body = b"--b\nContent-Disposition: form-data; name=\"report\"; filename=\"r.xml\"\n\n<x/>\n--b--\n";
        assert_eq!(
            file_field(body, "multipart/form-data; boundary=b", REPORT_FIELD),
            None
        );
    }

    #[test]
    fn the_last_report_part_wins() {
        // Starlette's form answers the last value a name carried, so a
        // duplicated field is the later upload.
        let boundary = "b";
        let body = form(
            boundary,
            &[
                file_part(boundary, REPORT_FIELD, b"<first/>"),
                file_part(boundary, REPORT_FIELD, b"<second/>"),
            ],
        );
        assert_eq!(
            file_field(&body, "multipart/form-data; boundary=b", REPORT_FIELD),
            Some(&b"<second/>"[..])
        );
    }

    #[test]
    fn a_part_left_open_at_the_end_carries_nothing() {
        // python-multipart drops a part whose ending delimiter never arrived,
        // and the parts that did close still count.
        let boundary = "b";
        let mut body = form(boundary, &[file_part(boundary, REPORT_FIELD, b"<x/>")]);
        body.extend_from_slice(&file_part(boundary, "note", b"no end"));
        assert_eq!(
            file_field(&body, "multipart/form-data; boundary=b", REPORT_FIELD),
            Some(&b"<x/>"[..])
        );
        // A body whose only part is still open carries no field.
        let body = file_part(boundary, REPORT_FIELD, b"<x/>");
        assert_eq!(
            file_field(&body, "multipart/form-data; boundary=b", REPORT_FIELD),
            None
        );
    }

    #[test]
    fn a_needle_that_repeats_its_own_prefix_is_found() {
        // A boundary the parser is handed never repeats its own CRLF prefix, so
        // the part of the KMP search that falls back to a shorter prefix is
        // exercised here, on a needle that does repeat its prefix.
        let needle = super::Needle::new(b"aabaa".to_vec());
        assert_eq!(needle.find(b"aaaaabaabaa", 0), Some(3));
        assert_eq!(needle.find(b"aaaaabaabaa", 6), Some(6));
        assert_eq!(needle.find(b"ababab", 0), None);
        // A needle longer than the haystack, and an empty one, are never found.
        assert_eq!(needle.find(b"aab", 0), None);
        assert_eq!(super::Needle::new(Vec::new()).find(b"aab", 0), None);
    }

    #[test]
    fn a_crafted_body_is_scanned_in_one_pass() {
        // The boundary is the client's to choose and the body can be 20 MiB, so
        // a scan that re-reads its haystack is a cheap attack. Here every block
        // of the body matches all but the last byte of the end marker — a CRLF,
        // two dashes and 63 of the boundary's 64 `a`s — which is what makes a
        // scan that compares from the start at every offset do the most work it
        // can on a megabyte of input.
        let boundary = "a".repeat(64);
        let content_type = format!("multipart/form-data; boundary={boundary}");
        let mut body = format!("--{boundary}\r\n").into_bytes();
        while body.len() < (1 << 20) {
            body.extend_from_slice(b"\r\n--");
            body.extend_from_slice(b"a".repeat(63).as_slice());
            body.push(b'b');
        }
        let mut carrying = body.clone();
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        super::comparisons::reset();
        assert_eq!(file_field(&body, &content_type, REPORT_FIELD), None);
        let near_misses = super::comparisons::count();
        assert!(
            near_misses < 3 * body.len(),
            "{near_misses} comparisons for a {} byte body of near misses",
            body.len()
        );

        // The same shape, with the upload behind it, still finds the field.
        carrying.extend_from_slice(b"\r\n");
        carrying.extend_from_slice(&file_part(&boundary, REPORT_FIELD, b"<x/>"));
        carrying.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        super::comparisons::reset();
        assert_eq!(
            file_field(&carrying, &content_type, REPORT_FIELD),
            Some(&b"<x/>"[..])
        );
        let comparisons = super::comparisons::count();
        assert!(
            comparisons < 3 * carrying.len(),
            "{comparisons} comparisons for a {} byte body that carries the field",
            carrying.len()
        );
    }
}
