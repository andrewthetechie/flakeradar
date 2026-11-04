//! A private PostgreSQL database per test.
//!
//! Creating and migrating a database for every test would spend most of the
//! suite's time in DDL, so the baseline schema is migrated once into a template
//! database and each test's database is cloned from it. Cloning needs no
//! connection to the template afterwards, and the clone is dropped when the test
//! finishes.

// Rust guideline compliant 2026-07-21

use std::fmt::Write as _;
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use flakeradar_db::{BASELINE_SQL, DbSettings};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
};
use sha2::{Digest, Sha256};
use tokio::runtime::Builder;

use crate::{error::TestError, postgres::with_database};

/// Advisory-lock key that serializes template-database creation.
///
/// Test-only. It is the next key after the server's migration and worker keys,
/// so a test process and a running server never wait on each other.
const TEMPLATE_LOCK_KEY: i64 = 726_300_003;

/// Database a client connects to in order to create and drop other databases.
const MAINTENANCE_DATABASE: &str = "postgres";

/// Most connections one test's pool opens.
///
/// A nextest run has many test processes at once, and every one of them clones
/// its own database on the same server, so a test keeps its share small.
const MAX_CONNECTIONS: u32 = 5;

/// A database that only this test owns.
#[derive(Debug)]
pub struct TestDb {
    connection: Arc<DatabaseConnection>,
    name: String,
    url: String,
    maintenance_url: String,
}

impl TestDb {
    /// Creates a database with the baseline schema migrated into it.
    ///
    /// # Errors
    ///
    /// Returns [`TestError`] when no server is available, the template cannot
    /// be created or migrated, or the clone cannot be opened.
    pub async fn new() -> Result<Self, TestError> {
        let server_url = crate::postgres::ensure_postgres()?;
        let maintenance_url = with_database(&server_url, MAINTENANCE_DATABASE)?;
        let maintenance = connect(&maintenance_url, 1).await?;
        take_lock(&maintenance, TEMPLATE_LOCK_KEY).await?;
        let template = template_database_name();
        if !database_exists(&maintenance, &template).await? {
            create_database(&maintenance, &template).await?;
            let template_url = with_database(&server_url, &template)?;
            let template_db = connect(&template_url, 1).await?;
            flakeradar_db::run_migrations(&template_db).await?;
            template_db.close().await?;
        }
        let name = fresh_database_name();
        clone_database(&maintenance, &name, &template).await?;
        release_lock(&maintenance, TEMPLATE_LOCK_KEY).await?;
        maintenance.close().await?;
        let url = with_database(&server_url, &name)?;
        let connection = connect(&url, MAX_CONNECTIONS).await?;
        Ok(Self {
            connection: Arc::new(connection),
            name,
            url,
            maintenance_url,
        })
    }

    /// Creates a database with no schema at all.
    ///
    /// For a test that compares the migrated schema against a schema script it
    /// applies itself.
    ///
    /// # Errors
    ///
    /// Returns [`TestError`] when no server is available or the database cannot
    /// be created or opened.
    pub async fn empty() -> Result<Self, TestError> {
        let server_url = crate::postgres::ensure_postgres()?;
        let maintenance_url = with_database(&server_url, MAINTENANCE_DATABASE)?;
        let maintenance = connect(&maintenance_url, 1).await?;
        let name = fresh_database_name();
        create_database(&maintenance, &name).await?;
        maintenance.close().await?;
        let url = with_database(&server_url, &name)?;
        let connection = connect(&url, MAX_CONNECTIONS).await?;
        Ok(Self {
            connection: Arc::new(connection),
            name,
            url,
            maintenance_url,
        })
    }

    /// The pool for this test's database.
    #[must_use]
    pub fn connection(&self) -> &DatabaseConnection {
        &self.connection
    }

    /// The URL of this test's database.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let maintenance_url = self.maintenance_url.clone();
        let name = self.name.clone();
        // Dropping the database needs a connection, and the test's own runtime is
        // parked while this runs, so the cleanup gets its own thread and runtime.
        // The test's pool is not closed here: a pool only finishes closing when
        // the runtime that drives it polls again, and this value drops while that
        // runtime is waiting for this drop. `DROP DATABASE ... WITH (FORCE)`
        // terminates the pool's connection from the server side instead.
        let cleanup = std::thread::spawn(move || {
            let Ok(runtime) = Builder::new_current_thread().enable_all().build() else {
                return;
            };
            runtime.block_on(async move {
                if let Ok(maintenance) = connect(&maintenance_url, 1).await {
                    let _ = drop_database(&maintenance, &name).await;
                }
            });
        });
        let _ = cleanup.join();
    }
}

/// Opens a pool of at most `max_connections` connections to `url`.
async fn connect(url: &str, max_connections: u32) -> Result<DatabaseConnection, TestError> {
    let mut options = ConnectOptions::new(postgres_url(url));
    options.max_connections(max_connections);
    options.test_before_acquire(true);
    // The same statement timeout the server applies, so a test cannot hold a
    // connection through a statement the server would have cancelled.
    options.statement_timeout(DbSettings::default().command_timeout());
    Ok(Database::connect(options).await?)
}

/// Rewrites the PostgreSQL URL spellings the server accepts for `SQLx`.
fn postgres_url(url: &str) -> String {
    url.strip_prefix("postgresql+asyncpg://")
        .map_or_else(|| url.to_owned(), |rest| format!("postgresql://{rest}"))
}

/// Whether a database named `name` exists on the server.
async fn database_exists(db: &DatabaseConnection, name: &str) -> Result<bool, TestError> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT 1 FROM pg_database WHERE datname = $1",
        [name.into()],
    );
    Ok(db.query_one_raw(statement).await?.is_some())
}

/// Creates an empty database named `name`.
async fn create_database(db: &DatabaseConnection, name: &str) -> Result<(), TestError> {
    db.execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
        .await?;
    Ok(())
}

/// Clones `template` into a new database named `name`.
async fn clone_database(
    db: &DatabaseConnection,
    name: &str,
    template: &str,
) -> Result<(), TestError> {
    db.execute_unprepared(&format!(
        "CREATE DATABASE \"{name}\" TEMPLATE \"{template}\""
    ))
    .await?;
    Ok(())
}

/// Drops the database named `name`, terminating any session still on it.
async fn drop_database(db: &DatabaseConnection, name: &str) -> Result<(), TestError> {
    db.execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .await?;
    Ok(())
}

/// Takes a session-level advisory lock on this pool's only connection.
async fn take_lock(db: &DatabaseConnection, key: i64) -> Result<(), TestError> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_lock($1)",
        [key.into()],
    );
    db.execute_raw(statement).await?;
    Ok(())
}

/// Releases the advisory lock taken by [`take_lock`].
async fn release_lock(db: &DatabaseConnection, key: i64) -> Result<(), TestError> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_unlock($1)",
        [key.into()],
    );
    db.execute_raw(statement).await?;
    Ok(())
}

/// The template database's name, keyed on the baseline schema's content.
///
/// A changed baseline therefore gets a new template instead of cloning the
/// previous schema.
fn template_database_name() -> String {
    let digest = Sha256::digest(BASELINE_SQL.as_bytes());
    let suffix = hex(digest.iter().take(6).copied());
    format!("flakeradar_template_{suffix}")
}

/// The lowercase hexadecimal form of `bytes`.
pub(crate) fn hex(bytes: impl IntoIterator<Item = u8>) -> String {
    let mut out = String::new();
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// A name no other database on the server has.
fn fresh_database_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!(
        "t_{}_{nanos}_{}",
        process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    //! Names are computed without a server, so they are checked here.

    use super::{fresh_database_name, template_database_name};

    #[test]
    fn template_name_follows_the_baseline() {
        let name = template_database_name();
        assert!(name.starts_with("flakeradar_template_"), "{name}");
        assert_eq!(name.len(), "flakeradar_template_".len() + 12);
    }

    #[test]
    fn fresh_names_differ() {
        assert_ne!(fresh_database_name(), fresh_database_name());
    }
}
