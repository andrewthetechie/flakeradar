//! The multipart request body `POST /api/ingest` reads an upload from.
//!
//! A `multipart/form-data` request is read for the file field named `report`;
//! every other content type is the raw body. The route reads the whole body under
//! its own size limit first, so this only has to find that one file in the
//! buffered bytes: a missing field, a field sent without a filename, or a body
//! whose framing is no multipart framing all read as an empty upload.
//!
//! The scan is linear in the body however the client shapes it. The boundary is
//! the client's to choose and the body can be 20 MiB, so a search that re-reads
//! bytes it has already seen — `slice::windows` and its `position` do — is a
//! quadratic attack on the thread that serves the upload.

/// The form field `POST /api/ingest` reads a JUnit XML file from.
pub const REPORT_FIELD: &str = "report";

/// The bytes that end a part's data and introduce the next delimiter.
const CRLF: &[u8] = b"\r\n";

/// The bytes that separate a part's headers from its data.
const HEADER_END: &[u8] = b"\r\n\r\n";

/// The two dashes that turn a delimiter into the closing one.
const CLOSING: &[u8] = b"--";

/// A part whose header block is no header block.
///
/// Each header line is a field name and a value, and a name may hold only the
/// characters RFC 7230 allows in a token. A line that is no header at all, or
/// whose name holds something else, is a malformed body: a client that cannot
/// frame its own headers sent no upload worth reading, so the route answers 400.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MalformedHeaders;

/// The file field `field` in a buffered multipart body, when the body has one.
///
/// `content_type` is the request's `Content-Type`, whose `boundary` parameter
/// separates the parts. Anything this cannot read — a missing or empty boundary,
/// a preamble, an unterminated part, a body with no such file field — reads as
/// `Ok(None)`. A part whose header block is no header block is
/// [`MalformedHeaders`], which the route answers as a 400.
///
/// A part's data may hold anything, the boundary bytes included: only a CRLF
/// followed by the delimiter and then a CRLF or the closing dashes ends a part, so
/// the scan continues past bytes that merely look like one. When the field arrives
/// more than once, the last part that carries it decides, whatever its kind: a
/// later plain form value leaves no upload at all.
///
/// # Errors
///
/// Returns [`MalformedHeaders`] for a part whose header block is no header block.
pub fn file_field<'a>(
    body: &'a [u8],
    content_type: &str,
    field: &str,
) -> Result<Option<&'a [u8]>, MalformedHeaders> {
    let Some(boundary) = boundary_of(content_type) else {
        return Ok(None);
    };
    let framing = Framing::new(boundary.as_bytes());
    let Some(mut delimiter_at) = framing.opening(body) else {
        return Ok(None);
    };
    let mut found = None;
    loop {
        let Some(data_at) = delimiter_at.checked_add(framing.delimiter.len()) else {
            return Ok(found);
        };
        // A delimiter followed by anything but a CRLF closes the form, or is no
        // delimiter at all; the form's parts carry no field from here on.
        if !body
            .get(data_at..)
            .is_some_and(|rest| rest.starts_with(CRLF))
        {
            return Ok(found);
        }
        let part_start = data_at + CRLF.len();
        // A part whose ending delimiter never arrived is still open when the
        // body ends, and an open part contributes no field.
        let Some(part_end) = framing.next_end(body, part_start) else {
            return Ok(found);
        };
        let Some(part) = body.get(part_start..part_end) else {
            return Ok(found);
        };
        match part_field(part, field)? {
            // A part that names no field, or whose headers cannot be read,
            // leaves the field where the parts before it put it.
            None | Some(FieldPart::Other) => {}
            Some(FieldPart::Value) => found = None,
            Some(FieldPart::File(bytes)) => found = Some(bytes),
        }
        match part_end.checked_add(CRLF.len()) {
            Some(next) => delimiter_at = next,
            None => return Ok(found),
        }
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

/// How one part sends the field the form is read for.
#[derive(Debug, Clone, Copy)]
enum FieldPart<'a> {
    /// A part that names some other field, so it decides nothing here.
    Other,
    /// The field sent as a plain form value, which carries no upload.
    Value,
    /// The field sent as a file, and the bytes it carried.
    File(&'a [u8]),
}

/// The RFC 7230 characters a header field name may hold besides ASCII digits and
/// letters.
const TOKEN_CHARS: &[u8] = b"!#$%&'*+-.^_`|~";

/// How this part sends `field`, when its headers say anything this can read.
///
/// # Errors
///
/// Returns [`MalformedHeaders`] when a header line is no header line, or its name
/// holds a character RFC 7230 does not allow in a token: either way the body is
/// no form and the upload is refused.
fn part_field<'a>(part: &'a [u8], field: &str) -> Result<Option<FieldPart<'a>>, MalformedHeaders> {
    let Some(header_end) = find(part, HEADER_END) else {
        return Ok(None);
    };
    let (Some(headers), Some(payload)) = (
        part.get(..header_end),
        part.get(header_end + HEADER_END.len()..),
    ) else {
        return Ok(None);
    };
    let mut named = false;
    let mut filename = false;
    for line in headers.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let name = match line.iter().position(|byte| *byte == b':') {
            Some(colon) => line.get(..colon).unwrap_or_default(),
            None => return Err(MalformedHeaders),
        };
        let value = line.get(name.len() + 1..).unwrap_or_default();
        if !is_token(name) {
            return Err(MalformedHeaders);
        }
        if name.eq_ignore_ascii_case(b"content-disposition") {
            let (part_name, has_filename) = disposition(value);
            named = part_name.is_some_and(|part_name| part_name == field);
            filename = has_filename;
        }
    }
    Ok(Some(if !named {
        FieldPart::Other
    } else if filename {
        FieldPart::File(payload)
    } else {
        FieldPart::Value
    }))
}

/// Whether `name` is one HTTP header field name may hold.
fn is_token(name: &[u8]) -> bool {
    !name.is_empty()
        && name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || TOKEN_CHARS.contains(byte))
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
    ///
    /// A body may open with any run of CR and LF, after which the delimiter has
    /// to start at the next `-`: `"\n--b…"` and `"\r\n\r\n--b…"` are forms, and
    /// so is `"\r\nwords--b…"`, whose delimiter is found by jumping to that `-`.
    /// A body that leads nowhere carries no field.
    fn opening(&self, body: &[u8]) -> Option<usize> {
        let skipped = body
            .iter()
            .position(|byte| *byte != b'\r' && *byte != b'\n')
            .unwrap_or(body.len());
        let rest = body.get(skipped..)?;
        if rest.starts_with(&self.delimiter) {
            return Some(skipped);
        }
        // Only a leading CR or LF moves the search, and there the next `-` is
        // where the delimiter has to start.
        if skipped == 0 {
            return None;
        }
        let at = skipped.checked_add(rest.iter().position(|byte| *byte == b'-')?)?;
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
        self.search(haystack, from).0
    }

    /// [`Needle::find`], with the byte comparisons the scan made.
    ///
    /// The count is what lets a test assert the scan stays linear without timing
    /// anything. `find` drops it, so a build that never asks pays nothing: the
    /// counter is a value the compiler removes with the rest of the unused tuple.
    fn search(&self, haystack: &[u8], from: usize) -> (Option<usize>, usize) {
        let mut matched = 0;
        let mut at = from;
        let mut comparisons = 0;
        while let Some(byte) = haystack.get(at).copied() {
            comparisons += 1;
            if self.bytes.get(matched).copied() == Some(byte) {
                matched += 1;
                if matched == self.bytes.len() {
                    return (Some(at + 1 - matched), comparisons);
                }
            } else if matched == 0 {
                // Nothing of the needle is matched, so this byte is a plain miss.
            } else {
                // Resume inside the needle, and try this byte against the
                // shorter prefix before moving on.
                matched = self.failure.get(matched - 1).copied().unwrap_or_default();
                comparisons += 1;
                continue;
            }
            at += 1;
        }
        (None, comparisons)
    }
}

#[cfg(test)]
mod tests {
    //! The scanner's own rules, which no public item exposes.

    use super::{Framing, Needle, REPORT_FIELD};

    /// The file field `body` carries, where the test's framing is well formed.
    fn field_of<'a>(body: &'a [u8], content_type: &str) -> Option<&'a [u8]> {
        super::file_field(body, content_type, REPORT_FIELD)
            .expect("the fixture's headers are headers")
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

    #[test]
    fn a_needle_that_repeats_its_own_prefix_is_found() {
        // A boundary the parser is handed never repeats its own CRLF prefix, so
        // the part of the KMP search that falls back to a shorter prefix is
        // exercised here, on a needle that does repeat its prefix.
        let needle = Needle::new(b"aabaa".to_vec());
        assert_eq!(needle.find(b"aaaaabaabaa", 0), Some(3));
        assert_eq!(needle.find(b"aaaaabaabaa", 6), Some(6));
        assert_eq!(needle.find(b"ababab", 0), None);
        // A needle longer than the haystack, and an empty one, are never found.
        assert_eq!(needle.find(b"aab", 0), None);
        assert_eq!(Needle::new(Vec::new()).find(b"aab", 0), None);
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

        // The scan the parser runs for every part, over the body as it arrives.
        let end_marker = Framing::new(boundary.as_bytes()).end_marker;
        let (found, near_misses) = end_marker.search(&carrying, 0);
        assert_eq!(found, None, "no end marker is in the body of near misses");
        assert!(
            near_misses < 3 * carrying.len(),
            "{near_misses} comparisons for a {} byte body of near misses",
            carrying.len()
        );

        // The same shape, with the upload behind it, still finds the field.
        carrying.extend_from_slice(b"\r\n");
        carrying.extend_from_slice(&file_part(&boundary, REPORT_FIELD, b"<x/>"));
        carrying.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        assert_eq!(field_of(&carrying, &content_type), Some(&b"<x/>"[..]));
        let (found, comparisons) = end_marker.search(&carrying, 0);
        assert!(found.is_some(), "the end marker is in the carrying body");
        assert!(
            comparisons < 3 * carrying.len(),
            "{comparisons} comparisons for a {} byte body that carries the field",
            carrying.len()
        );
    }
}
