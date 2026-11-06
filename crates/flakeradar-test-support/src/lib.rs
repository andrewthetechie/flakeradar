//! Test databases and factories for FlakeRadar.
//!
//! Only a `[dev-dependencies]` entry in another crate may use this crate, so no
//! test helper can reach a release binary. It gives each test a private
//! PostgreSQL database and builds the rows a test needs.
//!
//! [`ensure_postgres`] finds a server: `FLAKERADAR_TEST_DATABASE_URL` when it is
//! set, otherwise one reused Docker container. [`TestDb`] then clones a
//! migrated template database per test, and [`make_project`] and the other
//! builders write rows with the frozen Python server's defaults.

// Rust guideline compliant 2026-07-21

mod app;
mod clock;
mod db;
mod doubles;
mod error;
mod factories;
mod junit;
mod links;
mod monotonic;
mod postgres;
mod wait;

#[doc(inline)]
pub use app::{
    MemoryAssets, Reply, Unanswered, app, app_serving, app_with, get, post, request,
    request_with_headers, send, settings,
};
#[doc(inline)]
pub use clock::{FixedClock, system_clock};
#[doc(inline)]
pub use db::TestDb;
#[doc(inline)]
pub use doubles::{
    CapturedLog, CountingLock, CountingPruner, FailingLock, FailingOnceProcessor,
    FailingUnlockLock, GateWaiter, ManualMonotonic, RecordingTask, RecordingWaiter,
    ScriptedProcessor, SharedWriter, StopProbe, database_name, end_session, lock_holder,
    lock_holder_pid, lock_is_free, set_connections_allowed, terminate_lock_session,
};
#[doc(inline)]
pub use error::TestError;
#[doc(inline)]
pub use factories::{
    FIXTURE_INSTANT, JobBuilder, JobExecutionBuilder, JobScoreBuilder, PipelineBuilder,
    ProjectBuilder, ReportBuilder, RunBuilder, TestCaseBuilder, TestExecutionBuilder,
    TestScoreBuilder, make_execution, make_job, make_job_execution, make_job_score, make_pipeline,
    make_project, make_report, make_run, make_test_case, make_test_score,
};
#[doc(inline)]
pub use junit::make_junit;
#[doc(inline)]
pub use links::{IssueLink, Stamp, clear_note, link_of, link_table, stamp, stamp_link, stamp_note};
#[doc(inline)]
pub use monotonic::ScriptedMonotonic;
#[doc(inline)]
pub use postgres::ensure_postgres;
#[doc(inline)]
pub use wait::{WAIT_DEADLINE, WAIT_INTERVAL, dropped, queued, until_finished, wait_until, within};
