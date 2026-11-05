//! The errors the database layer reports.

// Rust guideline compliant 2026-07-21

/// Everything the database layer can fail with.
///
/// Callers branch on the kind: the binary refuses to start on
/// [`DbError::LegacyDatabase`], and a bad setting is reported before any
/// connection is opened.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DbError {
    /// The configured database URL is not a PostgreSQL URL.
    #[error(
        "FLAKERADAR_DATABASE_URL must be a PostgreSQL URL \
         (postgresql+asyncpg://user:pass@host:5432/db)"
    )]
    NotPostgres,
    /// The connection settings hold a value PostgreSQL cannot accept.
    #[error("{variable} must be a positive number of seconds, got {value}")]
    InvalidTimeout {
        /// Name of the environment variable the value came from.
        variable: &'static str,
        /// The rejected value, printed as it was given.
        value: String,
    },
    /// The database holds tables from an older FlakeRadar.
    #[error(
        "this database was not created by FlakeRadar 2: {detail}. FlakeRadar 2 \
         needs a fresh database; see the cutover notes in the README"
    )]
    LegacyDatabase {
        /// What was found: an Alembic version table, or app tables with no
        /// `seaql_migrations` table next to them.
        detail: String,
    },
    /// A stored Pipeline Report body is not the payload it was built from.
    #[error("the stored Pipeline Report body is not its payload: {detail}")]
    StoredPayload {
        /// What the parser rejected.
        detail: String,
    },
    /// The session that held the processor lock is gone, so this process can no longer lead.
    ///
    /// A session lock belongs to the backend that took it. A connection the
    /// server closed is replaced when its pool next hands it out, and the
    /// replacement holds no lock, so the processor reports this rather than
    /// keep leading on a session that no longer holds it.
    #[error("the processor's lock session is gone: {detail}")]
    LockSessionLost {
        /// What the liveness check found instead.
        detail: String,
    },
    /// The driver rejected a statement or could not reach the server.
    #[error(transparent)]
    Database(#[from] sea_orm::DbErr),
}
