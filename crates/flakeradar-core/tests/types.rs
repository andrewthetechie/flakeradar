//! The newtypes that carry an invariant: a Fingerprint, a Flakiness score, a CI run id.
//!
//! Each has a fallible constructor, no infallible `From`, and a `Deserialize` that
//! refuses what the column could not hold.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a constructor accepts what it must refuse"
)]

use flakeradar_core::types::{CiRunId, CiRunIdError, Fingerprint, FingerprintError, Score};

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

#[test]
fn a_score_refuses_what_is_not_a_score() {
    for value in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -0.000_001,
        1.000_001,
        f64::MAX,
    ] {
        let error = Score::new(value).expect_err("a value outside 0 to 1 is refused");
        assert_eq!(error.value.to_bits(), value.to_bits(), "value {value}");
    }
    assert_eq!(Score::new(0.0).expect("zero is a score").as_f64(), 0.0);
    assert_eq!(Score::new(1.0).expect("one is a score").as_f64(), 1.0);
    assert_eq!(Score::new(0.6).expect("a score").to_string(), "0.6");
    assert_eq!(Score::ZERO.as_f64(), 0.0);
    assert_eq!(Score::ONE.as_f64(), 1.0);
}

#[test]
fn a_score_travels_as_the_number_it_wraps() {
    let score = Score::new(0.6).expect("a score");
    assert_eq!(
        serde_json::to_string(&score).expect("a number"),
        "0.6".to_owned()
    );
    let back: Score = serde_json::from_str("0.6").expect("a score");
    assert_eq!(back, score);
    assert!(
        serde_json::from_str::<Score>("1.5").is_err(),
        "a score above one never deserializes"
    );
    assert!(
        serde_json::from_str::<Score>("null").is_err(),
        "a missing score is not zero"
    );
}

#[test]
fn a_ci_run_id_is_at_most_two_hundred_and_fifty_five_characters() {
    assert_eq!(
        CiRunId::new(&"a".repeat(CiRunId::MAX_LEN))
            .expect("the longest id")
            .as_str()
            .len(),
        CiRunId::MAX_LEN
    );
    assert_eq!(
        CiRunId::new(&"a".repeat(CiRunId::MAX_LEN + 1)).expect_err("too long"),
        CiRunIdError {
            length: CiRunId::MAX_LEN + 1
        }
    );
    // The column counts characters, so 255 two-byte characters fit.
    assert!(
        CiRunId::new(&"é".repeat(CiRunId::MAX_LEN)).is_ok(),
        "the column counts characters, not bytes"
    );
    assert_eq!(CiRunId::empty().as_str(), "");
    assert_eq!(CiRunId::new("42-1").expect("an id").into_string(), "42-1");
    assert_eq!(CiRunId::new("42-1").expect("an id").to_string(), "42-1");
}

#[test]
fn a_ci_run_id_travels_as_the_string_it_wraps() {
    let id = CiRunId::new("42-1").expect("an id");
    assert_eq!(
        serde_json::to_string(&id).expect("a string"),
        "\"42-1\"".to_owned()
    );
    let back: CiRunId = serde_json::from_str("\"42-1\"").expect("an id");
    assert_eq!(back, id);
    assert!(
        serde_json::from_str::<CiRunId>(&format!("\"{}\"", "a".repeat(256))).is_err(),
        "an id past the column never deserializes"
    );
}
