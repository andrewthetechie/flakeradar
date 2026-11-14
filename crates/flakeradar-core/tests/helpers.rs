//! What this module promises, asserted from outside it.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a helper or a score is wrong"
)]

use flakeradar_core::model::{Tier, Trend};
use flakeradar_core::types::Score;
use flakeradar_core::{
    helpers::LocationRef, helpers::body_digest, helpers::escape_like, helpers::is_broken,
    helpers::issue_url, helpers::next_flaky_since, helpers::permalink, helpers::repo_path,
    helpers::tier_for, helpers::trend_for,
};

/// The score a test states as a literal.
fn score(value: f64) -> Score {
    Score::new(value).expect("the test states a score")
}

#[test]
fn tier_thresholds_are_inclusive_at_the_top() {
    assert_eq!(tier_for(score(0.3), 0.3), Tier::Flaky);
    assert_eq!(tier_for(score(f64::MIN_POSITIVE), 0.3), Tier::Suspect);
    assert_eq!(tier_for(Score::ZERO, 0.3), Tier::Stable);
}

#[test]
fn flaky_state_crosses_at_either_threshold_and_never_above_flake() {
    // `T = i32` stands for the stored instant; only the crossings are under test.
    assert_eq!(next_flaky_since(None, score(0.30), 0.30, 0.20, 7), Some(7));
    assert_eq!(next_flaky_since(None, score(0.29), 0.30, 0.20, 7), None);
    assert_eq!(
        next_flaky_since(Some(1), score(0.25), 0.30, 0.20, 7),
        Some(1)
    );
    assert_eq!(
        next_flaky_since(Some(1), score(0.20), 0.30, 0.20, 7),
        Some(1)
    );
    assert_eq!(next_flaky_since(Some(1), score(0.19), 0.30, 0.20, 7), None);
    // A recover threshold above the flake threshold — which a caller that builds
    // its settings directly can state — still lets the state clear at the lower
    // of the two, and holds above it.
    assert_eq!(
        next_flaky_since(Some(1), score(0.10), 0.05, 0.20, 7),
        Some(1)
    );
    assert_eq!(next_flaky_since(Some(1), score(0.04), 0.05, 0.20, 7), None);
}

#[test]
fn broken_needs_a_threshold_and_enough_failures() {
    assert!(is_broken(5, 5));
    assert!(!is_broken(4, 5));
    assert!(!is_broken(0, 5));
    assert!(!is_broken(99, 0));
}

#[test]
fn trend_compares_against_the_delta() {
    assert_eq!(trend_for(0.55, Some(0.45)), Some(Trend::Worsening));
    assert_eq!(trend_for(0.35, Some(0.45)), Some(Trend::Improving));
    assert_eq!(trend_for(0.47, Some(0.45)), Some(Trend::Steady));
    assert_eq!(trend_for(0.5, None), None);
}

#[test]
fn escape_like_escapes_the_escape_first() {
    assert_eq!(escape_like(r"50%_off\"), r"50\%\_off\\");
    assert_eq!(escape_like("plain"), "plain");
}

#[test]
fn issue_url_needs_a_number() {
    assert_eq!(
        issue_url("acme/app", Some(7)).as_deref(),
        Some("https://github.com/acme/app/issues/7")
    );
    assert_eq!(issue_url("acme/app", None), None);
}

#[test]
fn repo_path_joins_only_a_non_empty_root() {
    assert_eq!(repo_path("frontend", "src/a.ts"), "frontend/src/a.ts");
    assert_eq!(repo_path("", "src/a.ts"), "src/a.ts");
}

#[test]
fn permalink_anchors_only_a_positive_line() {
    let at = |project_root: &'static str, line: Option<i64>| {
        permalink(&LocationRef {
            repo: "acme/app",
            commit_sha: "abc",
            project_root,
            file: "src/a.ts",
            line,
        })
    };
    assert_eq!(
        at("frontend", Some(12)),
        "https://github.com/acme/app/blob/abc/frontend/src/a.ts#L12"
    );
    assert_eq!(
        at("", Some(0)),
        "https://github.com/acme/app/blob/abc/src/a.ts"
    );
    assert_eq!(
        at("", None),
        "https://github.com/acme/app/blob/abc/src/a.ts"
    );
}

#[test]
fn body_digest_is_sha256_hex() {
    assert_eq!(
        body_digest(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
