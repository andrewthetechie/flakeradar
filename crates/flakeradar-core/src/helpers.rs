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

/// A Test's Location and the commit to link it at.
///
/// Every field is named because a permalink joins a Repo, a commit and a path:
/// four bare strings in a row would let a caller pass the root where the file
/// belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocationRef<'a> {
    /// The Repo's `owner/name`.
    pub repo: &'a str,
    /// The commit SHA the path is read at.
    pub commit_sha: &'a str,
    /// The Project root the file is relative to.
    pub project_root: &'a str,
    /// The file, relative to the Project root.
    pub file: &'a str,
    /// The line, when the report gave one.
    pub line: Option<i64>,
}

/// A GitHub permalink to a Test's Location at one commit SHA.
///
/// The line anchor is added for a 1-based line only.
#[must_use]
pub fn permalink(location: &LocationRef<'_>) -> String {
    let url = format!(
        "https://github.com/{}/blob/{}/{}",
        location.repo,
        location.commit_sha,
        repo_path(location.project_root, location.file)
    );
    match location.line {
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
