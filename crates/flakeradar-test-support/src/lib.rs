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

mod db;
mod error;
mod factories;
mod junit;
mod postgres;

#[doc(inline)]
pub use db::TestDb;
#[doc(inline)]
pub use error::TestError;
#[doc(inline)]
pub use factories::{
    JobBuilder, JobExecutionBuilder, JobScoreBuilder, PipelineBuilder, ProjectBuilder,
    ReportBuilder, RunBuilder, TestCaseBuilder, TestExecutionBuilder, TestScoreBuilder,
    make_execution, make_job, make_job_execution, make_job_score, make_pipeline, make_project,
    make_report, make_run, make_test_case, make_test_score,
};
#[doc(inline)]
pub use junit::make_junit;
#[doc(inline)]
pub use postgres::ensure_postgres;
