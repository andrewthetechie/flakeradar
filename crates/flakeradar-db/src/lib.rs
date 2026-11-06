//! Database layer for FlakeRadar: the baseline migration, entities, queries,
//! processing and retention.
//!
//! PostgreSQL is the only supported database, reached through SeaORM 2. Errors
//! are canonical structs rather than `anyhow`, because more than one crate
//! depends on this one.
//!
//! [`connect()`] builds the pool the server and the workers share;
//! [`run_migrations`] brings a fresh database up to the baseline schema. The
//! entities in [`entities`] are generated from that schema, so a query never
//! spells a column name by hand.

// Rust guideline compliant 2026-07-21

#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        reason = "a unit test fails loudly when the rule under test is wrong"
    )
)]

mod attribution;
mod batching;
pub mod clock;
mod connect;
mod deletes;
mod error;
pub mod filing;
mod identity;
pub mod issue_links;
mod metrics;
mod migration;
pub mod processing;
pub mod reads;
mod report_intake;
pub mod retention;
pub mod score_history;
pub mod sql;

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

// Items from the modules this crate keeps private. Each has exactly this one
// path: the module it is written in is not public, so `flakeradar_db::<item>`
// and `flakeradar_db::<module>::<item>` can never both resolve.
#[doc(inline)]
pub use attribution::{
    SIBLING_MAX, explained_ci_job_ids, summary_explaining_siblings, unexplained_failure_counts,
};
#[doc(inline)]
pub use batching::{CHUNK, chunks, owned_chunks};
#[doc(inline)]
pub use connect::{DbSettings, connect, connect_session};
#[doc(inline)]
pub use deletes::{
    DeleteError, DeletedProject, DeletedRepo, ProjectDelete, delete_project, delete_repo,
};
#[doc(inline)]
pub use error::DbError;
#[doc(inline)]
pub use identity::{get_or_create_project, get_or_create_repo};
#[doc(inline)]
pub use metrics::{
    QueueFacts, job_tier_counts, last_processed_at, queue_facts, reachable, report_status_counts,
    test_tier_counts,
};
#[doc(inline)]
pub use migration::{BASELINE_SQL, Migrator, run_migrations, split_statements};
#[doc(inline)]
pub use report_intake::{
    NewJunitReport, NewPipelineReport, StoredReport, complete_report, fail_report,
    find_junit_duplicate, list_reports, report_kind, report_status, report_summary, requeue_report,
    reset_for_retry, stored_ci_run_attempt, stored_junit_body, stored_pipeline_report,
    stored_report,
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
