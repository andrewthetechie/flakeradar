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
    /// The configured database URL is not a URL the driver can parse.
    ///
    /// The message carries a redacted copy, because the URL holds the database
    /// password and the driver's own error would quote all of it.
    #[error("FLAKERADAR_DATABASE_URL is not a URL: {url}")]
    UnusableUrl {
        /// The URL with its password replaced by `***`.
        url: String,
    },
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
    /// A leaderboard page whose `OFFSET` would not fit an `i64`.
    ///
    /// Request validation bounds `page` so the product fits, so a caller that
    /// builds the options itself is the only way here: it gets an error rather
    /// than a wrapped offset that would answer another page's rows.
    #[error("page {page} at page size {page_size} is past the largest offset an i64 holds")]
    PageOffsetTooLarge {
        /// The page that was asked for.
        page: i64,
        /// The page size it was asked for with.
        page_size: i64,
    },
    /// A stored Pipeline Report body is not the Report it was built from.
    #[error("the stored Pipeline Report body is not the Report it was built from: {detail}")]
    StoredReportBody {
        /// What the parser rejected.
        detail: String,
    },
    /// A stored Execution or Job execution holds a status this version does not write.
    ///
    /// The status column is text, and this server only ever writes `passed`,
    /// `failed`, `error` or `skipped`: an unknown value means something else
    /// wrote the row, and the rule that reads it cannot say what it flaked.
    #[error("unknown Execution status {value}")]
    UnknownExecutionStatus {
        /// The status the row holds, printed as it was read.
        value: String,
    },
    /// A stored Report holds a kind this version does not write.
    ///
    /// A Report row is only ever written by this server, so an unknown kind
    /// means something else wrote the row.
    #[error("stored Report kind {value:?} is not one of junit or pipeline")]
    UnknownReportKind {
        /// The kind the row holds, printed as it was read.
        value: String,
    },
    /// A stored Report holds a status this version does not write.
    ///
    /// A Report row is only ever written by this server, so an unknown status
    /// means something else wrote the row: serving it as a guessed status would
    /// hide that.
    #[error("stored Report status {value:?} is not one of pending, processed or failed")]
    UnknownReportStatus {
        /// The status the row holds, printed as it was read.
        value: String,
    },
    /// A stored value is outside the type this version writes into that column.
    ///
    /// A score is a `double precision` and a fingerprint or a CI run id is text, so
    /// a row written by hand can hold a number that is no score or a word that is
    /// no digest. The newtype is what keeps such a value out of a response, and
    /// there is no truthful value to serve in its place, so the read refuses the row
    /// the same way an unknown stored status does.
    #[error("stored {column} is not {what}: {detail}")]
    UnusableStoredValue {
        /// The column the row holds.
        column: &'static str,
        /// The type the column's own rule says it holds.
        what: &'static str,
        /// What the refused value was, as the newtype's own error states it.
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
