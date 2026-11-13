//! The parity harness's own library, shared by the two tests that use it.
//!
//! [`allowances`] reads `tests/parity/allowed-differences.json` and reconciles it
//! with the goldens; [`additions`] reads `tests/parity/additions.json`, the list
//! of columns, fields, routes and query parameters added after the corpus was
//! recorded. Nothing here runs a server, and
//! every other helper the tests share lives in `flakeradar_test_support`.

// Rust guideline compliant 2026-07-21

pub(crate) mod additions;
pub(crate) mod allowances;
