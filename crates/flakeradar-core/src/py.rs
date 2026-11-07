//! The string, truncation and rounding rules the rest of the workspace shares.
//!
//! Trimming, splitting and capping text looks obvious until a report carries a
//! NEL, an information separator or a combining mark: the wire rules are not
//! `char::is_whitespace` and not `str::lines`, and a score rounds to four
//! decimals half to even. Each rule lives here once rather than being spelled
//! again at each call site, so a Failure message reads the same wherever it is
//! capped. `tests/parity` and the JUnit corpus pin every one of them.

// Rust guideline compliant 2026-07-21

/// The marker appended to text cut at its limit.
///
/// It is appended *inside* the limit rather than after it, so a capped Failure
/// message or Failure detail is never longer than the limit it was capped at.
/// Clients match the marker, so its text is part of the wire.
pub const TRUNCATION_MARKER: &str = "\n…[truncated]";

/// Trims every character the wire counts as whitespace from both ends.
///
/// That is `char::is_whitespace` plus the four information separators
/// `\x1c`–`\x1f`, which `char::is_whitespace` does not cover and a CI runner can
/// still put in a name.
///
/// # Examples
///
/// ```
/// assert_eq!(flakeradar_core::py::strip_wire_whitespace("\x1c padded \u{85}"), "padded");
/// ```
#[must_use]
pub fn strip_wire_whitespace(text: &str) -> &str {
    text.trim_matches(is_wire_whitespace)
}

/// Whether `ch` counts as whitespace at the ends of a value.
fn is_wire_whitespace(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '\u{1c}'..='\u{1f}')
}

/// Whether `ch` ends a line in a multi-line value.
///
/// A line ends at the vertical tab, the form feed, the four information
/// separators, NEL and the two Unicode line separators. `str::lines` splits only
/// on `\n` and `\r\n`, so a traceback that separates its lines with any of these
/// reads as one line through it.
fn is_wire_line_break(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{1c}'
            ..='\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// Splits `text` at every line break, without keeping the breaks.
///
/// A trailing line break does not produce a final empty line, so `"a\n"` has one
/// line. Neither does empty text, which has none.
///
/// # Examples
///
/// ```
/// assert_eq!(flakeradar_core::py::py_splitlines("a\r\nb\u{2028}c"), ["a", "b", "c"]);
/// ```
#[must_use]
pub fn py_splitlines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if is_wire_line_break(ch) {
            lines.push(std::mem::take(&mut current));
            // A CRLF pair is one break, not two.
            if ch == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Truncates `text` to `limit` code points, ending with the marker.
///
/// It counts code points rather than bytes, so a message of multi-byte
/// characters is not cut early, and it reserves the marker's own length inside
/// the limit. Text at or under the limit is returned unchanged.
///
/// # Examples
///
/// ```
/// let text = "x".repeat(30);
/// let capped = flakeradar_core::py::cap(&text, 20);
/// assert_eq!(capped.chars().count(), 20);
/// assert!(capped.ends_with("…[truncated]"));
/// assert_eq!(flakeradar_core::py::cap("abc", 20), "abc");
/// ```
#[must_use]
pub fn cap(text: &str, limit: usize) -> String {
    let marker_len = TRUNCATION_MARKER.chars().count();
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let keep = limit.saturating_sub(marker_len);
    let mut capped: String = text.chars().take(keep).collect();
    capped.push_str(TRUNCATION_MARKER);
    capped
}

/// Rounds a score to four decimals, half to even.
///
/// A double's exact binary value is what rounds, and a true tie goes to the even
/// digit, so `0.12345` is `0.1235` and `0.29995` is `0.2999`. Multiplying by
/// 10 000 and rounding would answer differently on those ties. Fixed-precision
/// formatting uses the same exact, ties-to-even conversion, so formatting to
/// four decimals and parsing back is the rule.
///
/// # Examples
///
/// ```
/// assert_eq!(flakeradar_core::py::round4(0.03125), 0.0312);
/// assert_eq!(flakeradar_core::py::round4(0.09375), 0.0938);
/// ```
#[must_use]
pub fn round4(value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    format!("{value:.4}").parse::<f64>().unwrap_or(value)
}
