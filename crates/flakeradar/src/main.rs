//! FlakeRadar entry point.
//!
//! The binary starts, does its work and exits successfully. Everything it runs
//! lives in the `flakeradar` library.

// cargo-llvm-cov sets `coverage_nightly` on nightly; see `lib.rs`.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

// Rust guideline compliant 2026-07-21

// Process start-up is outside what a test process can run, so the coverage
// policy excludes it; see `docs/plans/rust-rewrite/coverage-gaps.md`.
#[cfg_attr(coverage_nightly, coverage(off))]
fn main() {}
