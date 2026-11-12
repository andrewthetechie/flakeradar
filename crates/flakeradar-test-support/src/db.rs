//! A private PostgreSQL database per test.
//!
//! Creating and migrating a database for every test would spend most of the
//! suite's time in DDL, so the baseline schema is migrated once into a template
//! database and each test's database is cloned from it. Cloning needs no
//! connection to the template afterwards, and the clone is dropped when the test
//! finishes.
//!
//! The template is built under a staging name and published only once the
//! migration has succeeded, so the template never exists half-built: a run that
//! is killed mid-migration, or a migration that fails, leaves no template for a
//! later run to clone.

// Rust guideline compliant 2026-07-21

use std::fmt::Write as _;
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use flakeradar_db::{DbSettings, MIGRATIONS};
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
            build_template(&maintenance, &server_url, &template).await?;
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

    /// The URL of the server's maintenance database.
    ///
    /// For a test that needs a second database on the same server, such as one
    /// proving a cluster-wide view is scoped to this instance's database.
    #[must_use]
    pub fn maintenance_url(&self) -> &str {
        &self.maintenance_url
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

/// Creates the template database, with the baseline schema migrated into it.
///
/// # Errors
///
/// Returns [`TestError`] when the staging database cannot be created, migrated
/// or published. No template is left behind in that case.
async fn build_template(
    maintenance: &DatabaseConnection,
    server_url: &str,
    template: &str,
) -> Result<(), TestError> {
    publish_template(maintenance, server_url, template, |database| async move {
        flakeradar_db::run_migrations(&database).await?;
        Ok::<(), TestError>(())
    })
    .await
}

/// Migrates a staging database with `migrate`, then renames it to `template`.
///
/// The template name therefore only ever holds a database the migration
/// finished on: an interrupted or failed build is dropped instead of published,
/// because a later run would clone it and every database test would then fail
/// against an empty schema.
async fn publish_template<F, Fut>(
    maintenance: &DatabaseConnection,
    server_url: &str,
    template: &str,
    migrate: F,
) -> Result<(), TestError>
where
    F: FnOnce(DatabaseConnection) -> Fut,
    Fut: Future<Output = Result<(), TestError>>,
{
    let staging = staging_database_name(template);
    // A run killed between creating the staging database and publishing it
    // leaves one behind. The template lock is what keeps two builds apart, so
    // this leftover is nobody's and can go.
    drop_database(maintenance, &staging).await?;
    create_database(maintenance, &staging).await?;
    let published = match migrate_staging(server_url, &staging, migrate).await {
        Ok(()) => rename_database(maintenance, &staging, template).await,
        Err(error) => Err(error),
    };
    if published.is_err() {
        // Best effort: a staging database that is not dropped wastes disk space
        // but never becomes a template, and the error that got here is the one
        // worth reporting.
        let _ = drop_database(maintenance, &staging).await;
    }
    published
}

/// Migrates the staging database and closes the pool it was migrated through.
async fn migrate_staging<F, Fut>(
    server_url: &str,
    staging: &str,
    migrate: F,
) -> Result<(), TestError>
where
    F: FnOnce(DatabaseConnection) -> Fut,
    Fut: Future<Output = Result<(), TestError>>,
{
    let staging_url = with_database(server_url, staging)?;
    let database = connect(&staging_url, 1).await?;
    let migrated = migrate(database.clone()).await;
    // The pool is closed before the rename: PostgreSQL refuses to rename a
    // database that another session is connected to.
    let closed = database.close().await.map_err(TestError::from);
    migrated.and(closed)
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

/// Renames the database `from` to `to`.
///
/// PostgreSQL refuses this while a session is connected to `from`, and when `to`
/// already names a database, so the caller closes the migration pool first and
/// publishes only a name that is free.
async fn rename_database(db: &DatabaseConnection, from: &str, to: &str) -> Result<(), TestError> {
    db.execute_unprepared(&format!("ALTER DATABASE \"{from}\" RENAME TO \"{to}\""))
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

/// The name a template is built under before it is published.
///
/// A fixed suffix rather than a random name: the template lock is what keeps two
/// builds apart, so a leftover from a killed run is the only other database that
/// can hold this name, and it is dropped before the build starts.
fn staging_database_name(template: &str) -> String {
    format!("{template}_building")
}

/// The template database's name, keyed on the content of every migration.
///
/// A changed migration therefore gets a new template instead of cloning the
/// previous schema.
fn template_database_name() -> String {
    template_name_for(MIGRATIONS)
}

/// The template name for these migrations, keyed on their SQL in order.
fn template_name_for(migrations: &[(&str, &str)]) -> String {
    let mut hasher = Sha256::new();
    for (_, sql) in migrations {
        hasher.update(sql.as_bytes());
    }
    let suffix = hex(hasher.finalize().iter().take(6).copied());
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
    //! Names are computed without a server, so they are checked here. Whether a
    //! template is published only once it is complete is a property of the
    //! server, so those cases run against a real one.

    use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

    use super::{
        MAINTENANCE_DATABASE, TEMPLATE_LOCK_KEY, TestDb, build_template, clone_database, connect,
        create_database, database_exists, drop_database, fresh_database_name, publish_template,
        release_lock, staging_database_name, take_lock, template_database_name, template_name_for,
        with_database,
    };
    use crate::error::TestError;
    use crate::postgres::ensure_postgres;

    /// The server's URL and a pool of one connection to its `postgres` database.
    async fn server() -> (String, DatabaseConnection) {
        let server = ensure_postgres().expect("a PostgreSQL server for the suite");
        let url = with_database(&server, MAINTENANCE_DATABASE).expect("a maintenance URL");
        let maintenance = connect(&url, 1).await.expect("a maintenance connection");
        (server, maintenance)
    }

    /// The OID of the database named `name`, or `None` when there is none.
    ///
    /// A rename keeps the OID, so an unchanged one proves the database was not
    /// dropped and built again.
    async fn database_oid(
        maintenance: &DatabaseConnection,
        name: &str,
    ) -> Result<Option<i64>, TestError> {
        let statement = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT oid::bigint AS oid FROM pg_database WHERE datname = $1",
            [name.into()],
        );
        let row = maintenance.query_one_raw(statement).await?;
        row.map(|row| row.try_get::<i64>("", "oid"))
            .transpose()
            .map_err(TestError::from)
    }

    /// Whether `db` has a `public` table named `table`.
    async fn table_exists(db: &DatabaseConnection, table: &str) -> Result<bool, TestError> {
        let statement = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_name = $1",
            [table.into()],
        );
        Ok(db.query_one_raw(statement).await?.is_some())
    }

    #[test]
    fn template_name_follows_the_baseline() {
        let name = template_database_name();
        assert!(name.starts_with("flakeradar_template_"), "{name}");
        assert_eq!(name.len(), "flakeradar_template_".len() + 12);
    }

    #[test]
    fn the_template_name_follows_every_migration() {
        let one = template_name_for(&[("0001_baseline", "A")]);
        let two = template_name_for(&[("0001_baseline", "A"), ("0002_x", "B")]);
        assert_ne!(one, two, "a later migration must not reuse the template");
        for name in [one, two] {
            assert!(name.starts_with("flakeradar_template_"), "{name}");
            assert_eq!(name.len(), "flakeradar_template_".len() + 12, "{name}");
        }
    }

    #[test]
    fn fresh_names_differ() {
        assert_ne!(fresh_database_name(), fresh_database_name());
    }

    #[test]
    fn the_staging_name_fits_a_postgresql_identifier() {
        let template = template_database_name();
        let staging = staging_database_name(&template);
        assert_ne!(
            staging, template,
            "the staging database is not the template"
        );
        assert!(staging.starts_with(&template), "{staging}");
        assert!(
            staging.len() <= 63,
            "{staging} is longer than PostgreSQL allows before it truncates"
        );
    }

    #[tokio::test]
    async fn a_failed_migration_leaves_no_template() {
        let (server, maintenance) = server().await;
        let template = format!("t_failed_build_{}", fresh_database_name());
        let staging = staging_database_name(&template);

        take_lock(&maintenance, TEMPLATE_LOCK_KEY)
            .await
            .expect("the template lock");
        let outcome = publish_template(&maintenance, &server, &template, |_database| async {
            Err(TestError::Fixture("the migration failed".to_owned()))
        })
        .await;
        release_lock(&maintenance, TEMPLATE_LOCK_KEY)
            .await
            .expect("the lock is released");

        assert!(matches!(outcome, Err(TestError::Fixture(_))), "{outcome:?}");
        assert!(
            !database_exists(&maintenance, &template)
                .await
                .expect("the template lookup"),
            "{template} was published from a failed migration"
        );
        assert!(
            !database_exists(&maintenance, &staging)
                .await
                .expect("the staging lookup"),
            "{staging} was left behind"
        );
    }

    #[tokio::test]
    async fn a_template_that_cannot_be_published_leaves_no_staging_database() {
        let (server, maintenance) = server().await;
        let template = format!("t_taken_name_{}", fresh_database_name());
        let staging = staging_database_name(&template);
        create_database(&maintenance, &template)
            .await
            .expect("the template name is taken");

        let outcome = publish_template(&maintenance, &server, &template, |_database| async {
            Ok::<(), TestError>(())
        })
        .await;

        assert!(
            matches!(outcome, Err(TestError::Database(_))),
            "a migration that succeeded against a taken name reported {outcome:?}"
        );
        assert!(
            !database_exists(&maintenance, &staging)
                .await
                .expect("the staging lookup"),
            "{staging} was left behind"
        );
        drop_database(&maintenance, &template)
            .await
            .expect("the placeholder is dropped");
    }

    #[tokio::test]
    async fn a_built_template_is_published_with_the_baseline() {
        let (server, maintenance) = server().await;
        let template = format!("t_built_{}", fresh_database_name());
        let staging = staging_database_name(&template);

        take_lock(&maintenance, TEMPLATE_LOCK_KEY)
            .await
            .expect("the template lock");
        build_template(&maintenance, &server, &template)
            .await
            .expect("the template is built");
        release_lock(&maintenance, TEMPLATE_LOCK_KEY)
            .await
            .expect("the lock is released");

        assert!(
            !database_exists(&maintenance, &staging)
                .await
                .expect("the staging lookup"),
            "{staging} was left behind after a build that succeeded"
        );

        // The check opens a clone rather than the template: a session on the
        // template would block the next clone of it.
        let clone = format!("{template}_clone");
        clone_database(&maintenance, &clone, &template)
            .await
            .expect("the published template clones");
        let clone_url = with_database(&server, &clone).expect("a clone URL");
        let clone_db = connect(&clone_url, 1).await.expect("a clone connection");
        assert!(
            table_exists(&clone_db, "test_cases")
                .await
                .expect("the catalogue query"),
            "the published template has no baseline schema"
        );
        clone_db.close().await.expect("the clone connection closes");

        drop_database(&maintenance, &clone)
            .await
            .expect("the clone is dropped");
        drop_database(&maintenance, &template)
            .await
            .expect("the template is dropped");
    }

    #[tokio::test]
    async fn an_existing_migrated_template_is_reused() {
        let (_, maintenance) = server().await;
        let template = template_database_name();

        let first = TestDb::new().await.expect("a first test database");
        let built = database_oid(&maintenance, &template)
            .await
            .expect("the template lookup");
        let second = TestDb::new().await.expect("a second test database");
        let reused = database_oid(&maintenance, &template)
            .await
            .expect("the template lookup");

        assert!(
            built.is_some(),
            "the first clone left no template at {template}"
        );
        assert_eq!(built, reused, "{template} was rebuilt instead of reused");
        assert!(
            table_exists(second.connection(), "test_cases")
                .await
                .expect("the catalogue query"),
            "the clone of the reused template has no baseline schema"
        );
        drop(first);
    }
}
