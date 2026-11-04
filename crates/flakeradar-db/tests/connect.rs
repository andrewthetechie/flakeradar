//! Opening the server's pool: URL spellings, the statement timeout, and the
//! ping that replaces a dead pooled connection.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the connection it just opened misbehaves"
)]

use flakeradar_db::{DbError, DbSettings, connect};
use flakeradar_test_support::TestDb;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

/// The database's URL with `postgresql://` replaced by `scheme://`.
fn with_scheme(url: &str, scheme: &str) -> String {
    url.replacen("postgresql://", &format!("{scheme}://"), 1)
}

/// The `statement_timeout` the server reports for the connection running this.
async fn statement_timeout(db: &impl ConnectionTrait) -> String {
    let row = db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SHOW statement_timeout".to_owned(),
        ))
        .await
        .expect("SHOW runs")
        .expect("SHOW returns a row");
    row.try_get("", "statement_timeout")
        .expect("the setting is text")
}

#[tokio::test]
async fn connect_accepts_every_documented_postgres_url() {
    let db = TestDb::new().await.expect("a migrated test database");
    for scheme in ["postgresql", "postgres", "postgresql+asyncpg"] {
        let url = with_scheme(db.url(), scheme);
        let server = connect(&url, DbSettings::default())
            .await
            .expect("the URL opens a pool");
        let row = server
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT 1 AS one".to_owned(),
            ))
            .await
            .expect("a query runs")
            .expect("a row comes back");
        assert_eq!(row.try_get::<i32>("", "one").ok(), Some(1), "{scheme}");
    }
}

#[tokio::test]
async fn connect_refuses_a_url_that_is_not_postgres() {
    let error = connect("sqlite:///./flakeradar.db", DbSettings::default())
        .await
        .expect_err("SQLite is not supported");
    assert!(matches!(error, DbError::NotPostgres));
}

#[tokio::test]
async fn connect_applies_the_statement_timeout_to_every_connection() {
    let db = TestDb::new().await.expect("a migrated test database");
    let server = connect(
        db.url(),
        DbSettings::new(7.0).expect("seven seconds is a valid timeout"),
    )
    .await
    .expect("the pool opens");

    // Three queries at once borrow three different connections, so this proves
    // the setting reaches each connection, not just the first.
    let (a, b, c) = tokio::join!(
        statement_timeout(&server),
        statement_timeout(&server),
        statement_timeout(&server)
    );
    let timeouts: [&str; 3] = [&a, &b, &c];
    assert_eq!(timeouts, ["7s"; 3]);
}

#[tokio::test]
async fn connect_replaces_a_connection_that_died_while_idle() {
    let db = TestDb::new().await.expect("a migrated test database");
    // Open the pool's only connection, then kill every other backend of this
    // database, so the pooled connection is dead when it is next borrowed.
    assert_eq!(statement_timeout(db.connection()).await, "30s");
    db.connection()
        .execute_unprepared(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE datname = current_database() AND pid <> pg_backend_pid()",
        )
        .await
        .expect("the other backends are terminated");

    let row = db
        .connection()
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT 1 AS one".to_owned(),
        ))
        .await
        .expect("the pool pings and reconnects")
        .expect("a row comes back");
    assert_eq!(row.try_get::<i32>("", "one").ok(), Some(1));
}
