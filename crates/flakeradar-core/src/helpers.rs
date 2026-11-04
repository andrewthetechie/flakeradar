//! The small pure helpers the queries and the DTOs share.
//!
//! Each one is a rule the frozen Python server applied inline: the tier a score
//! falls in, whether a Test is broken, the trend arrow, LIKE escaping, GitHub
//! URLs and the Report body digest.

// Rust guideline compliant 2026-07-21

use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};

use crate::model::{Tier, Trend};

/// The age, in days, at which a stored Score history point is the trend baseline.
pub const TREND_DAYS: i64 = 14;

/// The score movement that counts as a trend rather than noise.
pub const TREND_DELTA: f64 = 0.05;

/// The Flakiness tier a score falls in.
///
/// A score at or above the Flake threshold is flaky; above zero is suspect;
/// zero is stable.
///
/// # Examples
///
/// ```
/// use flakeradar_core::{Tier, tier_for};
///
/// assert_eq!(tier_for(0.5, 0.5), Tier::Flaky);
/// assert_eq!(tier_for(0.1, 0.5), Tier::Suspect);
/// assert_eq!(tier_for(0.0, 0.5), Tier::Stable);
/// ```
#[must_use]
pub fn tier_for(score: f64, threshold: f64) -> Tier {
    if score >= threshold {
        Tier::Flaky
    } else if score > 0.0 {
        Tier::Suspect
    } else {
        Tier::Stable
    }
}

/// Whether a Test is broken: it failed `broken_streak` times in a row.
///
/// A `broken_streak` of zero disables broken detection.
///
/// # Examples
///
/// ```
/// assert!(flakeradar_core::is_broken(5, 5));
/// assert!(!flakeradar_core::is_broken(5, 0));
/// ```
#[must_use]
pub fn is_broken(fail_streak: i64, broken_streak: i64) -> bool {
    broken_streak > 0 && fail_streak >= broken_streak
}

/// The trend between a score now and its baseline [`TREND_DAYS`] ago.
///
/// `None` means the baseline is unknown, which is also what a Test or Job with
/// no Score history that old reports.
#[must_use]
pub fn trend_for(current: f64, past: Option<f64>) -> Option<Trend> {
    let past = past?;
    let delta = current - past;
    if delta >= TREND_DELTA {
        Some(Trend::Worsening)
    } else if delta <= -TREND_DELTA {
        Some(Trend::Improving)
    } else {
        Some(Trend::Steady)
    }
}

/// Escapes the LIKE wildcards so user text matches literally.
///
/// Pair it with `ESCAPE '\'`; the backslash is escaped first so it cannot
/// escape the escapes.
///
/// # Examples
///
/// ```
/// assert_eq!(flakeradar_core::escape_like(r"50%_off\"), r"50\%\_off\\");
/// ```
#[must_use]
pub fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// The GitHub issue a Test or Job was filed as, if any.
#[must_use]
pub fn issue_url(repo: &str, number: Option<i64>) -> Option<String> {
    let number = number?;
    Some(format!("https://github.com/{repo}/issues/{number}"))
}

/// A reported file path made relative to the Repo root.
///
/// # Examples
///
/// ```
/// assert_eq!(flakeradar_core::repo_path("frontend", "src/a.ts"), "frontend/src/a.ts");
/// assert_eq!(flakeradar_core::repo_path("", "src/a.ts"), "src/a.ts");
/// ```
#[must_use]
pub fn repo_path(root: &str, file: &str) -> String {
    if root.is_empty() {
        file.to_owned()
    } else {
        format!("{root}/{file}")
    }
}

/// A GitHub permalink to a Test's Location at one commit SHA.
///
/// The line anchor is added for a 1-based line only.
#[must_use]
pub fn permalink(repo: &str, sha: &str, root: &str, file: &str, line: Option<i64>) -> String {
    let url = format!(
        "https://github.com/{repo}/blob/{sha}/{}",
        repo_path(root, file)
    );
    match line {
        Some(line) if line >= 1 => format!("{url}#L{line}"),
        _ => url,
    }
}

/// The SHA-256 digest of a Report body, as lowercase hex.
///
/// A Report's duplicate identity includes this digest, so the digest and the
/// stored row cannot disagree.
#[must_use]
pub fn body_digest(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::{
        body_digest, escape_like, is_broken, issue_url, permalink, repo_path, tier_for, trend_for,
    };
    use crate::model::{Tier, Trend};

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
        assert!(!is_broken(50, 0));
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
        assert_eq!(
            permalink("acme/app", "abc", "frontend", "src/a.ts", Some(12)),
            "https://github.com/acme/app/blob/abc/frontend/src/a.ts#L12"
        );
        assert_eq!(
            permalink("acme/app", "abc", "", "src/a.ts", Some(0)),
            "https://github.com/acme/app/blob/abc/src/a.ts"
        );
        assert_eq!(
            permalink("acme/app", "abc", "", "src/a.ts", None),
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
}
