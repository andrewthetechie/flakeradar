//! What this module promises, asserted from outside it.

// Rust guideline compliant 2026-07-21

use flakeradar_core::model::{Tier, Trend};
use flakeradar_core::{
    helpers::LocationRef, helpers::body_digest, helpers::escape_like, helpers::is_broken,
    helpers::issue_url, helpers::permalink, helpers::repo_path, helpers::tier_for,
    helpers::trend_for,
};

#[test]
fn tier_thresholds_are_inclusive_at_the_top() {
    assert_eq!(tier_for(0.3, 0.3), Tier::Flaky);
    assert_eq!(tier_for(f64::MIN_POSITIVE, 0.3), Tier::Suspect);
    assert_eq!(tier_for(0.0, 0.3), Tier::Stable);
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
