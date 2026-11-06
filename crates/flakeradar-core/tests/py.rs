//! What this module promises, asserted from outside it.

// Rust guideline compliant 2026-07-21

use flakeradar_core::{py::cap, py::py_splitlines, py::round4, py::strip_wire_whitespace};

#[test]
fn strip_wire_whitespace_removes_the_whole_whitespace_set() {
    assert_eq!(
        strip_wire_whitespace("\x1c\x1d\x1e\x1f\n\t a b \u{85}\u{a0}"),
        "a b"
    );
    assert_eq!(strip_wire_whitespace(""), "");
}

#[test]
fn py_splitlines_splits_on_every_wire_break() {
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
    assert!(capped.ends_with(flakeradar_core::py::TRUNCATION_MARKER));
    assert_eq!(cap("short", 20), "short");
}

#[test]
fn round4_rounds_half_to_even() {
    // Every row is a recorded answer of the reference's rounding.
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
