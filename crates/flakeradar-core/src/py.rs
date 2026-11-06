//! Python's string, truncation and rounding semantics.
//!
//! The frozen Python server strips, splits and truncates text with `str`
//! methods, and rounds scores with `round`. Each differs from the obvious Rust
//! equivalent in a way the parity goldens can see, so every rule lives here once
//! instead of being spelled again at each call site.

// Rust guideline compliant 2026-07-21

/// The marker appended to text cut at its limit.
///
/// The `_cap` helper in `backend/app/parsing.py` appends it *inside* the limit,
/// so capped Failure messages and details are never longer than the limit.
pub const TRUNCATION_MARKER: &str = "\n…[truncated]";

/// Strips the characters `str.strip()` strips.
///
/// `char::is_whitespace` covers Unicode's whitespace, but Python also strips the
/// four information separators `\x1c`–`\x1f`, which Rust keeps.
///
/// # Examples
///
/// ```
/// assert_eq!(flakeradar_core::strip_wire_whitespace("\x1c padded \u{85}"), "padded");
/// ```
#[must_use]
pub fn strip_wire_whitespace(text: &str) -> &str {
    text.trim_matches(is_python_whitespace)
}

/// Whether `ch` counts as whitespace to `str.strip()`.
fn is_python_whitespace(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '\u{1c}'..='\u{1f}')
}

/// Whether `ch` ends a line for `str.splitlines()`.
///
/// Python splits on the vertical tab, form feed, the four information
/// separators, NEL and the two Unicode line separators; `str::lines` splits only
/// on `\n` and `\r\n`.
fn is_python_line_break(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{1c}'
            ..='\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// Splits `text` the way `str.splitlines()` does, without the line breaks.
///
/// A trailing line break does not produce a final empty line, so `"a\n"` has one
/// line. Neither does empty text, which has none.
///
/// # Examples
///
/// ```
/// assert_eq!(flakeradar_core::py_splitlines("a\r\nb\u{2028}c"), ["a", "b", "c"]);
/// ```
#[must_use]
pub fn py_splitlines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if is_python_line_break(ch) {
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
/// Python's `_cap` counts code points, not bytes, and reserves the marker's own
/// length inside the limit. Text at or under the limit is returned unchanged.
///
/// # Examples
///
/// ```
/// let text = "x".repeat(30);
/// let capped = flakeradar_core::cap(&text, 20);
/// assert_eq!(capped.chars().count(), 20);
/// assert!(capped.ends_with("…[truncated]"));
/// assert_eq!(flakeradar_core::cap("abc", 20), "abc");
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

/// Rounds a score the way Python's `round(value, 4)` does.
///
/// Python rounds the exact binary value of the double, half to even on a true
/// tie; multiplying by 10 000 and rounding gives different answers. Rust's
/// fixed-precision formatting uses the same exact, ties-to-even conversion, so
/// formatting to four decimals and parsing back reproduces `CPython`.
///
/// # Examples
///
/// ```
/// assert_eq!(flakeradar_core::round4(0.03125), 0.0312);
/// assert_eq!(flakeradar_core::round4(0.09375), 0.0938);
/// ```
#[must_use]
pub fn round4(value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    format!("{value:.4}").parse::<f64>().unwrap_or(value)
}
