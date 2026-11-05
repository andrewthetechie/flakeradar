//! Builds the PostgreSQL connection pool.
//!
//! The URL is accepted in the three spellings FlakeRadar has ever documented,
//! and the pool is configured the way the frozen Python server configured
//! asyncpg: a statement timeout on every connection, and a ping before a pooled
//! connection is handed out.

// Rust guideline compliant 2026-07-21

use std::time::Duration;

use sea_orm::{ConnectOptions, Database, DatabaseConnection};

use crate::error::DbError;

/// Seconds a statement may run before PostgreSQL cancels it.
///
/// The frozen Python server's `db_command_timeout_seconds` default. A long
/// statement would otherwise hold a pooled connection until the client gives
/// up, so the server cancels it instead.
const DEFAULT_COMMAND_TIMEOUT_SECONDS: f64 = 30.0;

/// Settings that shape the connection pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbSettings {
    command_timeout: Duration,
}

impl DbSettings {
    /// Builds settings from the configured command timeout in seconds.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::InvalidTimeout`] when the value is not a finite,
    /// positive number of seconds, which
    /// [`Duration::from_secs_f64`] cannot represent.
    pub fn new(command_timeout_seconds: f64) -> Result<Self, DbError> {
        if !command_timeout_seconds.is_finite() || command_timeout_seconds <= 0.0 {
            return Err(DbError::InvalidTimeout {
                variable: "FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS",
                value: command_timeout_seconds.to_string(),
            });
        }
        Ok(Self {
            command_timeout: Duration::from_secs_f64(command_timeout_seconds),
        })
    }

    /// How long a statement may run before PostgreSQL cancels it.
    #[must_use]
    pub fn command_timeout(self) -> Duration {
        self.command_timeout
    }
}

impl Default for DbSettings {
    fn default() -> Self {
        Self {
            command_timeout: Duration::from_secs_f64(DEFAULT_COMMAND_TIMEOUT_SECONDS),
        }
    }
}

/// Opens a PostgreSQL pool with `url`, using `settings`.
///
/// A `postgresql+asyncpg://` URL is accepted and rewritten for `SQLx`, so a
/// configuration written for FlakeRadar 1 keeps working. Other schemes are
/// refused with the message FlakeRadar 1 gave.
///
/// # Errors
///
/// Returns [`DbError::NotPostgres`] for any URL that is not PostgreSQL, and
/// [`DbError::Database`] when the server cannot be reached.
pub async fn connect(url: &str, settings: DbSettings) -> Result<DatabaseConnection, DbError> {
    let mut options = ConnectOptions::new(postgres_url(url)?);
    // Match asyncpg's `pool_pre_ping`: a connection that died while idle is
    // replaced instead of failing the request that borrowed it.
    options.test_before_acquire(true);
    options.statement_timeout(settings.command_timeout());
    Ok(Database::connect(options).await?)
}

/// Opens a pool of exactly one connection, for a session-scoped advisory lock.
///
/// A session lock lives and dies with the connection that took it, so the
/// connection must stay the same one for as long as the lock is held. A pool of
/// one guarantees that, and both pool reapers are turned off: an idle or
/// over-age connection closed behind the lock's back would leave this process
/// believing it still leads while the next process takes the lock. A dead
/// connection is therefore reported rather than silently replaced.
///
/// # Errors
///
/// Returns [`DbError::NotPostgres`] for any URL that is not PostgreSQL, and
/// [`DbError::Database`] when the server cannot be reached.
pub async fn connect_session(
    url: &str,
    settings: DbSettings,
) -> Result<DatabaseConnection, DbError> {
    let mut options = ConnectOptions::new(postgres_url(url)?);
    options.max_connections(1);
    options.min_connections(1);
    options.test_before_acquire(false);
    options.idle_timeout(None::<Duration>);
    options.max_lifetime(None::<Duration>);
    options.statement_timeout(settings.command_timeout());
    Ok(Database::connect(options).await?)
}

/// Rewrites the accepted PostgreSQL spellings to the one `SQLx` parses.
fn postgres_url(url: &str) -> Result<String, DbError> {
    if let Some(rest) = url.strip_prefix("postgresql+asyncpg://") {
        return Ok(format!("postgresql://{rest}"));
    }
    if url.starts_with("postgresql://") || url.starts_with("postgres://") {
        return Ok(url.to_owned());
    }
    Err(DbError::NotPostgres)
}

#[cfg(test)]
mod tests {
    //! URL and setting parsing need no database, so they are checked here.

    use super::{DbError, DbSettings, postgres_url};

    #[test]
    fn accepts_every_documented_postgres_url() {
        assert_eq!(
            postgres_url("postgresql://u:p@h:5432/d").ok(),
            Some("postgresql://u:p@h:5432/d".to_owned())
        );
        assert_eq!(
            postgres_url("postgres://u:p@h:5432/d").ok(),
            Some("postgres://u:p@h:5432/d".to_owned())
        );
        assert_eq!(
            postgres_url("postgresql+asyncpg://u:p@h:5432/d").ok(),
            Some("postgresql://u:p@h:5432/d".to_owned())
        );
    }

    #[test]
    fn refuses_other_schemes() {
        assert!(matches!(
            postgres_url("sqlite:///./x.db"),
            Err(DbError::NotPostgres)
        ));
        assert!(matches!(postgres_url(""), Err(DbError::NotPostgres)));
    }

    #[test]
    fn rejects_a_timeout_postgres_cannot_take() {
        for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                DbSettings::new(value),
                Err(DbError::InvalidTimeout { .. })
            ));
        }
    }

    #[test]
    fn keeps_a_valid_timeout() {
        let timeout = DbSettings::new(1.5)
            .ok()
            .map(|settings| settings.command_timeout().as_secs_f64());
        assert_eq!(timeout, Some(1.5));
    }
}
