//! The baseline migration: what it creates, that it runs once, and that it
//! refuses a database an older FlakeRadar created.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails loudly when the fixture it just built is missing"
)]

use flakeradar_db::{DbError, run_migrations, split_statements};
use flakeradar_test_support::TestDb;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

/// Every table the baseline creates, sorted.
const APP_TABLES: [&str; 11] = [
    "job_executions",
    "job_score_history",
    "jobs",
    "pipelines",
    "projects",
    "reports",
    "repos",
    "test_cases",
    "test_executions",
    "test_runs",
    "test_score_history",
];

/// The baseline tables the database holds, sorted.
async fn app_tables(db: &impl ConnectionTrait) -> Vec<String> {
    let statement = Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_type = 'BASE TABLE' ORDER BY table_name"
            .to_owned(),
    );
    db.query_all_raw(statement)
        .await
        .expect("information_schema is readable")
        .into_iter()
        .filter_map(|row| row.try_get::<String>("", "table_name").ok())
        .filter(|name| APP_TABLES.contains(&name.as_str()))
        .collect()
}

/// Whether the migration bookkeeping table exists.
async fn migration_table_exists(db: &impl ConnectionTrait) -> bool {
    let statement = Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT to_regclass('public.seaql_migrations') IS NOT NULL AS present".to_owned(),
    );
    db.query_one_raw(statement)
        .await
        .expect("information_schema is readable")
        .and_then(|row| row.try_get::<bool>("", "present").ok())
        .unwrap_or(false)
}

/// The detail of a legacy-database refusal, or a panic naming the other error.
fn the_refusal_message(error: DbError) -> String {
    let DbError::LegacyDatabase { detail } = error else {
        panic!("expected a legacy-database refusal, got {error:?}");
    };
    detail
}

#[tokio::test]
async fn migrations_create_every_table() {
    let db = TestDb::new().await.expect("a migrated test database");
    assert_eq!(app_tables(db.connection()).await, APP_TABLES);
}

#[tokio::test]
async fn a_second_migration_run_migrates_nothing() {
    let db = TestDb::new().await.expect("a migrated test database");
    run_migrations(db.connection())
        .await
        .expect("a second run finds nothing to do");
    let applied = Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT version FROM seaql_migrations ORDER BY version".to_owned(),
    );
    let versions: Vec<String> = db
        .connection()
        .query_all_raw(applied)
        .await
        .expect("the migration table is readable")
        .into_iter()
        .map(|row| row.try_get::<String>("", "version").expect("a version"))
        .collect();
    assert_eq!(versions, ["0001_baseline"]);
}

#[tokio::test]
async fn concurrent_migration_runs_apply_the_baseline_once() {
    let db = TestDb::empty().await.expect("an empty test database");
    let (first, second) = tokio::join!(
        run_migrations(db.connection()),
        run_migrations(db.connection())
    );
    first.expect("the first run migrates");
    second.expect("the second run waits and migrates nothing");
    assert_eq!(app_tables(db.connection()).await, APP_TABLES);
}

#[tokio::test]
async fn an_alembic_database_is_refused_and_left_alone() {
    let db = TestDb::empty().await.expect("an empty test database");
    db.connection()
        .execute_unprepared("CREATE TABLE alembic_version (version_num varchar(32) NOT NULL)")
        .await
        .expect("the marker table is created");

    let error = run_migrations(db.connection())
        .await
        .expect_err("an Alembic database is refused");
    assert!(
        the_refusal_message(error).contains("alembic_version"),
        "the detail names the table"
    );
    assert_eq!(
        app_tables(db.connection()).await,
        Vec::<String>::new(),
        "the baseline tables were not created"
    );
    assert!(
        !migration_table_exists(db.connection()).await,
        "nothing was recorded as migrated"
    );
}

#[tokio::test]
async fn app_tables_without_a_migration_table_are_refused() {
    let db = TestDb::empty().await.expect("an empty test database");
    db.connection()
        .execute_unprepared("CREATE TABLE repos (id integer NOT NULL)")
        .await
        .expect("an app table is created");

    let error = run_migrations(db.connection())
        .await
        .expect_err("a database with app tables and no migration table is refused");
    assert!(
        the_refusal_message(error).contains("repos"),
        "the detail names the table"
    );
    assert_eq!(
        app_tables(db.connection()).await,
        ["repos"],
        "the refused database keeps only the table the test created"
    );
    assert!(
        !migration_table_exists(db.connection()).await,
        "nothing was recorded as migrated"
    );
}

#[tokio::test]
async fn unrelated_tables_do_not_make_a_database_foreign() {
    let db = TestDb::empty().await.expect("an empty test database");
    db.connection()
        .execute_unprepared("CREATE TABLE notes (id integer NOT NULL)")
        .await
        .expect("an unrelated table is created");

    run_migrations(db.connection())
        .await
        .expect("a database with no app table is migrated");
    assert_eq!(app_tables(db.connection()).await, APP_TABLES);
}

#[tokio::test]
async fn the_refusal_names_the_cutover_notes() {
    let db = TestDb::empty().await.expect("an empty test database");
    db.connection()
        .execute_unprepared("CREATE TABLE alembic_version (version_num varchar(32) NOT NULL)")
        .await
        .expect("the marker table is created");

    let message = run_migrations(db.connection())
        .await
        .expect_err("an Alembic database is refused")
        .to_string();
    assert!(
        message.contains("not created by FlakeRadar 0.2 or later"),
        "{message}"
    );
    assert!(message.contains("needs a fresh database"), "{message}");
    assert!(message.contains("cutover notes in the README"), "{message}");
}

// Statement splitting is not reachable through a database test alone, so its
// quoting and comment rules are checked beside the runs that apply the script.

#[test]
fn splits_on_top_level_semicolons() {
    assert_eq!(
        split_statements("SELECT 1; SELECT 2;\n"),
        ["SELECT 1", " SELECT 2"]
    );
}

#[test]
fn keeps_semicolons_inside_literals_and_identifiers() {
    let script =
        "CREATE TABLE public.t (\n    v text DEFAULT 'a;b' NOT NULL,\n    \"c;d\" integer\n);\n";
    assert_eq!(split_statements(script).len(), 1);
}

#[test]
fn keeps_a_doubled_quote_inside_a_literal_or_an_identifier() {
    assert_eq!(
        split_statements("INSERT INTO t VALUES ('it''s; fine'); -- x; y\nSELECT 1;").len(),
        2
    );
    assert_eq!(split_statements("SELECT 1 AS \"a\"\"b;c\";").len(), 1);
}

#[test]
fn ignores_semicolons_in_line_comments() {
    assert_eq!(
        split_statements("-- a; comment\nSELECT 1;\n-- trailing\n"),
        ["-- a; comment\nSELECT 1"]
    );
}

#[test]
fn skips_blank_statements() {
    assert_eq!(split_statements(";\n ;\n"), Vec::<&str>::new());
}
