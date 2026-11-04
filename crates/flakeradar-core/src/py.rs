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
/// assert_eq!(flakeradar_core::py_strip("\x1c padded \u{85}"), "padded");
/// ```
#[must_use]
pub fn py_strip(text: &str) -> &str {
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

#[cfg(test)]
mod tests {
    use super::{cap, py_splitlines, py_strip, round4};

    #[test]
    fn py_strip_removes_pythons_whitespace_set() {
        assert_eq!(py_strip("\x1c\x1d\x1e\x1f\n\t a b \u{85}\u{a0}"), "a b");
        assert_eq!(py_strip(""), "");
    }

    #[test]
    fn py_splitlines_splits_on_every_python_break() {
        assert_eq!(
            py_splitlines("a\nb\rc\r\nd\u{0b}e\u{0c}f\u{1c}g"),
            ["a", "b", "c", "d", "e", "f", "g"]
        );
        assert_eq!(py_splitlines("a\u{85}b\u{2028}c\u{2029}"), ["a", "b", "c"]);
    }

    #[test]
    fn py_splitlines_drops_the_trailing_break_only() {
        assert_eq!(py_splitlines("a\n"), ["a"]);
        assert_eq!(py_splitlines("a\n\n"), ["a", ""]);
        assert!(py_splitlines("").is_empty(), "empty input has no lines");
        assert_eq!(py_splitlines("\n"), [""]);
    }

    #[test]
    fn cap_counts_code_points_and_keeps_the_marker_inside_the_limit() {
        let text = "é".repeat(30);
        let capped = cap(&text, 20);
        assert_eq!(capped.chars().count(), 20);
        assert!(capped.ends_with(super::TRUNCATION_MARKER));
        assert_eq!(cap("short", 20), "short");
    }

    #[test]
    fn round4_matches_cpython_half_to_even() {
        // The table from 00-shared-context.md, recorded from CPython.
        let table = [
            (0.03125, 0.0312),
            (0.09375, 0.0938),
            (0.15625, 0.1562),
            (0.00005, 0.0001),
            (0.12345, 0.1235),
            (0.29995, 0.2999),
            (0.30005, 0.3),
            (1.0 / 3.0, 0.3333),
            (2.0 / 3.0, 0.6667),
        ];
        for (input, want) in table {
            assert_eq!(round4(input), want, "round4({input})");
        }
        assert_eq!(round4(1.0), 1.0);
        assert_eq!(round4(0.0), 0.0);
    }

    #[test]
    fn round4_passes_non_finite_values_through() {
        assert!(round4(f64::NAN).is_nan());
        assert_eq!(round4(f64::INFINITY), f64::INFINITY);
    }
}
