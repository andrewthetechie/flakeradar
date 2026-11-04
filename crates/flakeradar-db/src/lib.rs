//! Database layer for FlakeRadar: the baseline migration, entities, queries,
//! processing and retention.
//!
//! PostgreSQL is the only supported database, reached through SeaORM 2. Errors
//! are canonical structs rather than `anyhow`, because more than one crate
//! depends on this one.

// Rust guideline compliant 2026-07-21
