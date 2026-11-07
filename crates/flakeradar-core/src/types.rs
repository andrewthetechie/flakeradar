//! The newtypes that keep a fingerprint, a Flakiness score and a CI run id apart.
//!
//! All three travel as the strings and numbers they always did: each newtype is
//! transparent on the wire, and every constructor is fallible, because an
//! invariant the type exists to hold is not one a caller may skip.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

/// A Test's fingerprint: the lowercase hex SHA-1 of its identity.
///
/// The value is the `test_cases.fingerprint` column, so it is exactly 40
/// lowercase hexadecimal characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, ToSchema)]
#[serde(transparent)]
#[schema(value_type = String)]
pub struct Fingerprint(String);

impl Fingerprint {
    /// How many characters a fingerprint has: one SHA-1 digest in lowercase hex.
    pub const LEN: usize = 40;

    /// The fingerprint `text` spells.
    ///
    /// # Errors
    ///
    /// Returns [`FingerprintError`] when `text` is not 40 lowercase hex
    /// characters, which is what the column holds.
    pub fn new(text: &str) -> Result<Self, FingerprintError> {
        if text.len() != Self::LEN {
            return Err(FingerprintError::Length { length: text.len() });
        }
        if !text.bytes().all(is_lowercase_hex) {
            return Err(FingerprintError::NotHex);
        }
        Ok(Self(text.to_owned()))
    }

    /// The fingerprint of a digest this crate just computed.
    ///
    /// Only this crate may skip the check, because the one caller is
    /// [`fingerprint`](crate::parsing::fingerprint): it renders a SHA-1 in lowercase hex,
    /// so its output always satisfies [`Fingerprint::new`].
    pub(crate) fn from_digest(digest: String) -> Self {
        debug_assert!(
            Self::new(&digest).is_ok(),
            "a rendered SHA-1 is a fingerprint"
        );
        Self(digest)
    }

    /// The fingerprint as the column spells it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether `byte` is one of the sixteen lowercase hex digits.
fn is_lowercase_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Fingerprint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::new(&text).map_err(serde::de::Error::custom)
    }
}

/// Why a [`Fingerprint`] cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FingerprintError {
    /// The text is not the fingerprinted length.
    #[error("a fingerprint is 40 characters, got {length}")]
    Length {
        /// How many characters the refused text had.
        length: usize,
    },
    /// The text holds a character that is not a lowercase hex digit.
    #[error("a fingerprint is lowercase hex characters")]
    NotHex,
}

/// A Flakiness score: a finite number from 0 to 1.
///
/// A Test's or Job's score carries this, so a number that is not a score cannot
/// leave the rules that compute one. The column and the wire keep the plain
/// number: `Score` is transparent.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, ToSchema)]
#[serde(transparent)]
#[schema(value_type = f64)]
pub struct Score(f64);

impl Score {
    /// The lowest score there is: a Stable test or Job.
    pub const ZERO: Self = Self(0.0);

    /// The highest score there is: a Test or Job that always flips.
    pub const ONE: Self = Self(1.0);

    /// The score `value` names.
    ///
    /// # Errors
    ///
    /// Returns [`ScoreError`] when `value` is not finite or falls outside 0 to 1,
    /// which no rule can compute and no response may carry.
    pub fn new(value: f64) -> Result<Self, ScoreError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(ScoreError { value });
        }
        Ok(Self(value))
    }

    /// The score as the number the column and the wire carry.
    #[must_use]
    pub fn as_f64(self) -> f64 {
        self.0
    }
}

impl fmt::Display for Score {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl PartialOrd for Score {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(&other.0)
    }
}

impl<'de> Deserialize<'de> for Score {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = f64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Why a [`Score`] cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("a Flakiness score is a finite number from 0 to 1, got {value}")]
pub struct ScoreError {
    /// The refused value.
    pub value: f64,
}

/// A CI run's id, as the provider gave it.
///
/// The `ci_run_id` columns hold at most 255 characters and a JUnit upload that
/// names no CI run stores the empty id, which is a real value here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, ToSchema)]
#[serde(transparent)]
#[schema(value_type = String)]
pub struct CiRunId(String);

impl CiRunId {
    /// The longest id the `ci_run_id` columns accept.
    pub const MAX_LEN: usize = 255;

    /// The id `text` spells.
    ///
    /// # Errors
    ///
    /// Returns [`CiRunIdError`] when `text` is longer than [`CiRunId::MAX_LEN`]
    /// characters, which the column cannot hold.
    pub fn new(text: &str) -> Result<Self, CiRunIdError> {
        let length = text.chars().count();
        if length > Self::MAX_LEN {
            return Err(CiRunIdError { length });
        }
        Ok(Self(text.to_owned()))
    }

    /// An id no upload gave.
    #[must_use]
    pub fn empty() -> Self {
        Self(String::new())
    }

    /// The id as the column and the wire spell it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The id as an owned string, which is what the columns take.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl Default for CiRunId {
    /// The id no upload gave, which is what `#[serde(default)]` reads.
    fn default() -> Self {
        Self::empty()
    }
}

impl fmt::Display for CiRunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CiRunId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::new(&text).map_err(serde::de::Error::custom)
    }
}

/// Why a [`CiRunId`] cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a CI run id is at most 255 characters, got {length}")]
pub struct CiRunIdError {
    /// How many characters the refused id had.
    pub length: usize,
}
