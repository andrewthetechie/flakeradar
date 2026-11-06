//! Repo, Project and Project-root naming rules.
//!
//! A Repo is a lowercase `owner/name`, a Project is a lowercase name unique
//! within its Repo (`default` when omitted), and a Provider is a short lowercase
//! label such as `github`. See `CONTEXT.md` and ADR 0001.

// Rust guideline compliant 2026-07-21

use std::sync::LazyLock;

use regex::Regex;

use crate::py::strip_wire_whitespace;

/// The Project name used when a Report names none.
pub const DEFAULT_PROJECT: &str = "default";

/// Compiles a naming rule, refusing to run with a broken pattern.
#[expect(
    clippy::expect_used,
    reason = "the patterns are literals in this file, so a pattern that does not \
              compile is a programming error and must stop the process"
)]
fn naming_rule(pattern: &str) -> Regex {
    Regex::new(pattern).expect("naming rule compiles")
}

/// A Repo name must look like a lowercase `owner/name`.
static REPO_RE: LazyLock<Regex> = LazyLock::new(|| naming_rule("^[a-z0-9_.-]+/[a-z0-9_.-]+$"));

/// A Project name is a short lowercase label.
static PROJECT_RE: LazyLock<Regex> = LazyLock::new(|| naming_rule("^[a-z0-9_.-]{1,100}$"));

/// A Provider name is a short lowercase identifier without dots.
static PROVIDER_RE: LazyLock<Regex> = LazyLock::new(|| naming_rule("^[a-z0-9_-]{1,32}$"));

/// A Repo, Project, Project-root or Provider name the server cannot accept.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    /// The Repo is not a lowercase `owner/name`.
    #[error("repo must look like 'owner/name', got '{raw}'")]
    Repo {
        /// The value as it was given, before normalizing.
        raw: String,
    },
    /// The Project name is not 1–100 lowercase characters.
    #[error("project must be 1-100 chars of a-z, 0-9, '.', '_' or '-', got '{raw}'")]
    Project {
        /// The value as it was given, before normalizing.
        raw: String,
    },
    /// The Project root climbs out of the Repo.
    #[error("root must not contain '..', got '{raw}'")]
    Root {
        /// The value as it was given, before normalizing.
        raw: String,
    },
    /// The Provider name is not 1–32 lowercase characters.
    #[error("provider must be 1-32 chars of a-z, 0-9, '_' or '-', got '{raw}'")]
    Provider {
        /// The value as it was given, before normalizing.
        raw: String,
    },
}

/// Normalizes a Repo name to lowercase `owner/name`.
///
/// # Errors
///
/// Returns [`IdentityError::Repo`] when the value is longer than 255
/// characters or does not match `owner/name`.
///
/// # Examples
///
/// ```
/// assert_eq!(
///     flakeradar_core::normalize_repo("  Acme/App ").unwrap(),
///     "acme/app"
/// );
/// ```
pub fn normalize_repo(raw: &str) -> Result<String, IdentityError> {
    let value = strip_wire_whitespace(raw).to_lowercase();
    if value.chars().count() > 255 || !REPO_RE.is_match(&value) {
        return Err(IdentityError::Repo {
            raw: raw.to_owned(),
        });
    }
    Ok(value)
}

/// Normalizes a Project name, falling back to [`DEFAULT_PROJECT`].
///
/// # Errors
///
/// Returns [`IdentityError::Project`] when the value is not 1–100 characters of
/// `a-z`, `0-9`, `.`, `_` or `-`.
///
/// # Examples
///
/// ```
/// assert_eq!(flakeradar_core::normalize_project(None).unwrap(), "default");
/// assert_eq!(flakeradar_core::normalize_project(Some(" Backend ")).unwrap(), "backend");
/// ```
pub fn normalize_project(raw: Option<&str>) -> Result<String, IdentityError> {
    let given = raw.unwrap_or_default();
    let value = strip_wire_whitespace(given).to_lowercase();
    let value = if value.is_empty() {
        DEFAULT_PROJECT.to_owned()
    } else {
        value
    };
    if !PROJECT_RE.is_match(&value) {
        return Err(IdentityError::Project {
            raw: given.to_owned(),
        });
    }
    Ok(value)
}

/// Normalizes a Project root into a Repo-relative path.
///
/// `None` means "not sent", so the Project's root stays as it is; an empty
/// string means the Repo root.
///
/// # Errors
///
/// Returns [`IdentityError::Root`] when a `..` segment would climb out of the
/// Repo.
///
/// # Examples
///
/// ```
/// assert_eq!(flakeradar_core::normalize_root(None).unwrap(), None);
/// assert_eq!(
///     flakeradar_core::normalize_root(Some("./frontend/")).unwrap(),
///     Some("frontend".to_owned())
/// );
/// ```
pub fn normalize_root(raw: Option<&str>) -> Result<Option<String>, IdentityError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let mut value = strip_wire_whitespace(raw);
    while let Some(rest) = value.strip_prefix("./") {
        value = rest;
    }
    let value = value.trim_matches('/');
    if value.split('/').any(|segment| segment == "..") {
        return Err(IdentityError::Root {
            raw: raw.to_owned(),
        });
    }
    Ok(Some(value.to_owned()))
}

/// Normalizes a Provider name to its lowercase label.
///
/// # Errors
///
/// Returns [`IdentityError::Provider`] when the value is not 1–32 characters of
/// `a-z`, `0-9`, `_` or `-`.
///
/// # Examples
///
/// ```
/// assert_eq!(flakeradar_core::normalize_provider(" GitHub ").unwrap(), "github");
/// ```
pub fn normalize_provider(raw: &str) -> Result<String, IdentityError> {
    let value = strip_wire_whitespace(raw).to_lowercase();
    if !PROVIDER_RE.is_match(&value) {
        return Err(IdentityError::Provider {
            raw: raw.to_owned(),
        });
    }
    Ok(value)
}
