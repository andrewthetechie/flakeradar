//! Failure categories: the likely cause of a failing Execution (ADR 0006).
//!
//! Fixed, ordered rules, tried from most to least specific. The Failure message
//! is checked first and the Failure details only when the message matches
//! nothing. The result is a heuristic, and the UI calls it "likely cause".

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The likely cause of a failing Execution.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "lowercase")]
#[schema(rename_all = "lowercase")]
pub enum FailureCategory {
    /// A connection, DNS, socket or upstream-service failure.
    Network,
    /// The machine or the checkout was wrong: disk, memory, permissions, files.
    Environment,
    /// Something did not finish in time.
    Timing,
    /// An assertion or expectation failed.
    Assertion,
    /// Nothing matched, or the message and details say nothing useful.
    Other,
}

impl FailureCategory {
    /// Every category, in the order the frozen server's `CATEGORIES` lists them.
    pub const ALL: [Self; 5] = [
        Self::Network,
        Self::Environment,
        Self::Timing,
        Self::Assertion,
        Self::Other,
    ];

    /// The category a stored or requested spelling names, if one does.
    #[must_use]
    pub fn from_wire(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|category| category.as_str() == text)
    }

    /// The category as the Python server spelled it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Environment => "environment",
            Self::Timing => "timing",
            Self::Assertion => "assertion",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for FailureCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A category and the patterns that prove it, in rule order.
///
/// The order is the frozen server's: the most specific cause first, so a
/// network error that also mentions a timeout stays a network error.
static RULES: LazyLock<Vec<(FailureCategory, Regex)>> = LazyLock::new(build_rules);

/// Compiles the rule groups once, refusing to run with a broken rule.
#[expect(
    clippy::panic,
    reason = "the patterns are compiled from constants in this file, so a rule that \
              does not compile is a programming error and must stop the process"
)]
fn build_rules() -> Vec<(FailureCategory, Regex)> {
    let groups: [(FailureCategory, &[&str]); 4] = [
        (
            FailureCategory::Network,
            &[
                "ECONNREFUSED",
                "ECONNRESET",
                "ENOTFOUND",
                "EAI_AGAIN",
                "ETIMEDOUT",
                "EHOSTUNREACH",
                "ENETUNREACH",
                "socket hang up",
                "net::ERR_",
                "connection (?:refused|reset|aborted)",
                "ConnectionError",
                "ConnectionResetError",
                "ConnectionRefusedError",
                "Name or service not known",
                "Temporary failure in name resolution",
                "getaddrinfo",
                r"\b50[234] (?:Bad Gateway|Service Unavailable|Gateway Time-?out)",
            ],
        ),
        (
            FailureCategory::Environment,
            &[
                "ENOSPC",
                "No space left on device",
                "MemoryError",
                "out of memory",
                r"\bOOM\b",
                r"\bKilled\b",
                "SIGKILL",
                r"signal 9\b",
                "Permission denied",
                "EACCES",
                "Target (?:page, context or browser )?(?:has been )?closed",
                "browser has been closed",
                "ModuleNotFoundError",
                "Cannot find module",
                "No such file or directory",
                "ENOENT",
                "command not found",
            ],
        ),
        (
            FailureCategory::Timing,
            &["timeout", "timed out", "times out", "deadline exceeded"],
        ),
        (
            FailureCategory::Assertion,
            &["AssertionError", r"\bassert", r"expect\(", r"\bexpected\b"],
        ),
    ];
    groups
        .into_iter()
        .map(|(category, patterns)| {
            // Alternation of the group's patterns, compiled with the Unicode
            // case folding `re.IGNORECASE` applies to `str` in Python.
            let joined = patterns.join("|");
            let pattern = format!("(?i){joined}");
            let regex = Regex::new(&pattern).unwrap_or_else(|error| {
                panic!("failure-category rule {category} does not compile: {error}")
            });
            (category, regex)
        })
        .collect()
}

/// The first category whose rules match `text`, if any.
fn match_category(text: &str) -> Option<FailureCategory> {
    RULES
        .iter()
        .find(|(_, regex)| regex.is_match(text))
        .map(|(category, _)| *category)
}

/// The Failure category of one failing Execution.
///
/// The Failure message is checked first; the Failure details only when the
/// message matches nothing.
///
/// # Examples
///
/// ```
/// use flakeradar_core::{FailureCategory, classify_failure};
///
/// assert_eq!(classify_failure("connect ECONNREFUSED 127.0.0.1:5432", ""), FailureCategory::Network);
/// assert_eq!(classify_failure("AssertionError: expected 3, got 4", ""), FailureCategory::Assertion);
/// ```
#[must_use]
pub fn classify_failure(message: &str, details: &str) -> FailureCategory {
    match_category(message)
        .or_else(|| match_category(details))
        .unwrap_or(FailureCategory::Other)
}

/// The most common non-`None` category; the newest wins a tie.
///
/// `categories` is newest first, matching the Execution order a Test's history
/// is read in. `None` when every entry is `None`.
///
/// # Examples
///
/// ```
/// use flakeradar_core::{FailureCategory, dominant_category};
///
/// let categories = [Some(FailureCategory::Assertion), Some(FailureCategory::Timing)];
/// assert_eq!(dominant_category(&categories), Some(FailureCategory::Assertion));
/// ```
#[must_use]
pub fn dominant_category(categories: &[Option<FailureCategory>]) -> Option<FailureCategory> {
    let mut counts: BTreeMap<FailureCategory, usize> = BTreeMap::new();
    for category in categories.iter().flatten() {
        *counts.entry(*category).or_default() += 1;
    }
    let best = counts.values().copied().max()?;
    // The newest entry among those with the highest count wins the tie.
    categories
        .iter()
        .flatten()
        .copied()
        .find(|category| counts.get(category) == Some(&best))
}

#[cfg(test)]
mod tests {
    use super::{FailureCategory::*, classify_failure, dominant_category};

    #[test]
    fn classify_failure_matches_the_python_rules() {
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
}
