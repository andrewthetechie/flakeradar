//! The newtypes that keep a fingerprint, a Flakiness score and a CI run id apart.
//!
//! All three travel as the strings and numbers they always did: each newtype is
//! transparent on the wire, and every constructor is fallible, because an
//! invariant the type exists to hold is not one a caller may skip.

// Rust guideline compliant 2026-07-21

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
    /// [`fingerprint`](crate::fingerprint): it renders a SHA-1 in lowercase hex,
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

#[cfg(test)]
mod tests {
    use super::{Fingerprint, FingerprintError};

    const EXAMPLE: &str = "2b8823c6c55be0a19a34111f08b687028f5273df";

    /// The first `count` characters of [`EXAMPLE`], without slicing the string.
    fn prefix(count: usize) -> String {
        EXAMPLE.chars().take(count).collect()
    }

    #[test]
    fn a_fingerprint_is_forty_lowercase_hex_characters() {
        assert_eq!(
            Fingerprint::new(EXAMPLE).expect("a fingerprint").as_str(),
            EXAMPLE
        );
        assert_eq!(
            Fingerprint::new("").expect_err("empty"),
            FingerprintError::Length { length: 0 }
        );
        assert_eq!(
            Fingerprint::new(&prefix(39)).expect_err("short"),
            FingerprintError::Length { length: 39 }
        );
        assert_eq!(
            Fingerprint::new(&format!("{EXAMPLE}0")).expect_err("long"),
            FingerprintError::Length { length: 41 }
        );
        assert_eq!(
            Fingerprint::new(&EXAMPLE.to_uppercase()).expect_err("uppercase"),
            FingerprintError::NotHex
        );
        assert_eq!(
            Fingerprint::new(&format!("{}z", prefix(39))).expect_err("not hex"),
            FingerprintError::NotHex
        );
    }

    #[test]
    fn a_fingerprint_travels_as_the_string_it_wraps() {
        let fingerprint = Fingerprint::new(EXAMPLE).expect("a fingerprint");
        assert_eq!(
            serde_json::to_string(&fingerprint).expect("a string"),
            format!("\"{EXAMPLE}\"")
        );
        assert_eq!(fingerprint.to_string(), EXAMPLE);
        let back: Fingerprint = serde_json::from_str(&format!("\"{EXAMPLE}\"")).expect("back");
        assert_eq!(back, fingerprint);
        assert!(
            serde_json::from_str::<Fingerprint>("\"abc\"").is_err(),
            "a short fingerprint never deserializes"
        );
    }
}
