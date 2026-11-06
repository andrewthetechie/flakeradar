//! What this module promises, asserted from outside it.

// Rust guideline compliant 2026-07-21

use flakeradar_core::{FailureCategory::*, classify_failure, dominant_category};

#[test]
fn classify_failure_matches_the_python_rules() {
    // Each case is one line of the reference classifier's behaviour at 86c98804, so
    // the expected category is the requirement rather than a restatement.
    let cases = [
        ("Test timeout of 30000ms exceeded.", "", Timing),
        (
            "Timed out 5000ms waiting for expect(locator).toBeVisible()",
            "",
            Timing,
        ),
        ("Error: connect ECONNREFUSED 127.0.0.1:5432", "", Network),
        ("Error: connect ETIMEDOUT 10.0.0.1:443", "", Network),
        (
            "Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:3000/",
            "",
            Network,
        ),
        ("expected 200 but got 503 Service Unavailable", "", Network),
        (
            "requests.exceptions.ReadTimeout: read timed out",
            "",
            Timing,
        ),
        (
            "OSError: [Errno 28] No space left on device",
            "",
            Environment,
        ),
        (
            "Error: Target page, context or browser has been closed",
            "",
            Environment,
        ),
        ("AssertionError: expected 3, got 4", "", Assertion),
        ("assert 1 == 2", "", Assertion),
        (
            "",
            "Traceback (most recent call last):\nAssertionError: boom",
            Assertion,
        ),
        // The message wins over the details.
        ("AssertionError: expected 3", "… ECONNRESET …", Assertion),
        ("FAILURE", "", Other),
        ("", "", Other),
    ];
    for (message, details, want) in cases {
        assert_eq!(
            classify_failure(message, details),
            want,
            "message={message:?}"
        );
    }
}

#[test]
fn every_category_prints_its_wire_spelling() {
    // The category names are the wire vocabulary the API serves.

    assert_eq!(Network.as_str(), "network");
    assert_eq!(Environment.as_str(), "environment");
    assert_eq!(Timing.as_str(), "timing");
    assert_eq!(Assertion.as_str(), "assertion");
    assert_eq!(Other.as_str(), "other");
    assert_eq!(Network.to_string(), "network");
}

#[test]
fn dominant_category_majority() {
    assert_eq!(
        dominant_category(&[Some(Timing), Some(Assertion), Some(Timing)]),
        Some(Timing)
    );
}

#[test]
fn dominant_category_tie_newest_wins() {
    assert_eq!(
        dominant_category(&[Some(Assertion), Some(Timing)]),
        Some(Assertion)
    );
}

#[test]
fn dominant_category_none_when_all_null() {
    assert_eq!(dominant_category(&[None, None]), None);
    assert_eq!(dominant_category(&[]), None);
}

#[test]
fn dominant_category_skips_null() {
    assert_eq!(dominant_category(&[None, Some(Network)]), Some(Network));
}
