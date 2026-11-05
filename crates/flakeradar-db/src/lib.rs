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

pub mod attribution;
mod batching;
pub mod clock;
mod connect;
mod error;
mod identity;
mod migration;
pub mod processing;
pub mod reads;
mod report_intake;
pub mod retention;
pub mod score_history;

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
pub use attribution::{
    SIBLING_MAX, explained_ci_job_ids, summary_explaining_siblings, unexplained_failure_counts,
};
#[doc(inline)]
pub use batching::{CHUNK, chunks};
#[doc(inline)]
pub use clock::{
    Clock, FixedClock, MonotonicClock, SharedClock, SharedMonotonic, SystemClock, SystemMonotonic,
};
#[doc(inline)]
pub use connect::{DbSettings, connect, connect_session};
#[doc(inline)]
pub use error::DbError;
#[doc(inline)]
pub use identity::{get_or_create_project, get_or_create_repo};
#[doc(inline)]
pub use migration::{BASELINE_SQL, Migrator, run_migrations, split_statements};
#[doc(inline)]
pub use processing::{
    ERROR_MAX, JobHistory, ProcessError, ProcessOutcome, ProcessingSettings, claim_next_report,
    job_history, process_next, rescore, rescore_jobs,
};
#[doc(inline)]
pub use report_intake::{
    NewJunitReport, NewPipelineReport, REPORT_FAILED, REPORT_KIND_JUNIT, REPORT_KIND_PIPELINE,
    REPORT_PENDING, REPORT_PROCESSED, REPORT_STATUSES, complete_report, fail_report,
    find_junit_duplicate, requeue_report, retry_failed_report, stored_attempt, stored_junit_body,
    stored_pipeline_payload,
};
#[doc(inline)]
pub use reads::{
    DEFAULT_EXECUTIONS_LIMIT, FAILURES_LIMIT, ListOptions, MAX_EXECUTIONS_LIMIT, NEW_FLAKY_DAYS,
    ReadError, ReadSettings, Scope, SortKey, TestListFilters, ci_run_failures, find_job_ids,
    find_test_ids, get_job, get_test, job_summary, latest_failure, list_jobs, list_repos,
    list_tests, quarantine_list, search_jobs, search_tests, set_quarantine, summary, test_job_links,
};
#[doc(inline)]
pub use retention::{PruneResult, RetentionSettings, prune, run_prune};
#[doc(inline)]
pub use score_history::{
    HISTORY_DAYS, HistoryKind, HistoryUpdate, ScoreHistoryRow, newest_history_before, past_scores,
    window_rows, write_today_history,
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
