//! Test databases and factories for FlakeRadar.
//!
//! Only a `[dev-dependencies]` entry in another crate may use this crate, so no
//! test helper can reach a release binary. It gives each test a private
//! PostgreSQL database and builds the rows a test needs.

// Rust guideline compliant 2026-07-21
