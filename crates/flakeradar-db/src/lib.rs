//! Database layer for FlakeRadar: the baseline migration, entities, queries,
//! processing and retention.
//!
//! PostgreSQL is the only supported database, reached through SeaORM 2. Errors
//! are canonical structs rather than `anyhow`, because more than one crate
//! depends on this one.
//!
//! [`connect`] builds the pool the server and the workers share;
//! [`run_migrations`] brings a fresh database up to the baseline schema. The
//! entities in [`entities`] are generated from that schema, so a query never
//! spells a column name by hand.

// Rust guideline compliant 2026-07-21

mod batching;
mod connect;
mod error;
mod migration;

#[expect(
    missing_docs,
    reason = "sea-orm-cli generate entity writes these files; a doc comment here \
              would be erased by the next generation CI checks"
)]
pub mod entities;

#[doc(inline)]
pub use batching::{CHUNK, chunks};
#[doc(inline)]
pub use connect::{DbSettings, connect};
#[doc(inline)]
pub use error::DbError;
#[doc(inline)]
pub use migration::{BASELINE_SQL, Migrator, run_migrations, split_statements};

/// Advisory-lock key that serializes the startup migration.
///
/// The same key as the frozen Python server's `MIGRATION_LOCK_KEY`, so a
/// database that both servers could reach is still only migrated once at a
/// time. Several server replicas start at once and must not race.
pub const MIGRATION_LOCK_KEY: i64 = 726_300_001;

/// Advisory-lock key that elects the single Report processor.
///
/// The same key as the frozen Python server's `WORKER_LOCK_KEY`, so one
/// processor runs however many FlakeRadar replicas are started.
pub const WORKER_LOCK_KEY: i64 = 726_300_002;
