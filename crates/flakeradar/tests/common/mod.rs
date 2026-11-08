//! The parity harness's own library, shared by the two tests that use it.
//!
//! [`allowances`] reads `tests/parity/allowed-differences.json` and reconciles it
//! with the goldens. Nothing here runs a server, and
//! every other helper the tests share lives in `flakeradar_test_support`.

// Rust guideline compliant 2026-07-21

pub(crate) mod allowances;
