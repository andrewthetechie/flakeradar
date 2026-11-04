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
mod identity;
mod migration;
mod report_intake;

/// The `SeaORM` entities, generated from the baseline schema by `sea-orm-cli`.
///
/// Regeneration overwrites every file in the module, so a doc comment in one of
/// them would fail the CI check that regenerates them. The two names
/// `CONTEXT.md` spells differently are therefore noted here: `test_cases` holds
/// Tests, and `confirmed_flake_count` is a Proven flake count.
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
pub use identity::{get_or_create_project, get_or_create_repo};
#[doc(inline)]
pub use migration::{BASELINE_SQL, Migrator, run_migrations, split_statements};
#[doc(inline)]
pub use report_intake::{
    NewJunitReport, NewPipelineReport, REPORT_FAILED, REPORT_KIND_JUNIT, REPORT_KIND_PIPELINE,
    REPORT_PENDING, REPORT_PROCESSED, REPORT_STATUSES, complete_report, fail_report,
    find_junit_duplicate, requeue_report, retry_failed_report, stored_attempt, stored_junit_body,
    stored_pipeline_payload,
};

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
