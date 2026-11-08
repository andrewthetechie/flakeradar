//! The errors the test helpers report.

// Rust guideline compliant 2026-07-21

/// Everything the test helpers can fail with.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TestError {
    /// Docker is missing, or the container could not be started or read.
    #[error("docker: {0}")]
    Docker(String),
    /// The test database could not be reached or written.
    #[error("database: {0}")]
    Database(#[from] sea_orm::DbErr),
    /// The database layer refused a migration or a connection.
    #[error("flakeradar database: {0}")]
    Flakeradar(#[from] flakeradar_db::DbError),
    /// A variable or fixture value is unusable.
    #[error("fixture: {0}")]
    Fixture(String),
}
