//! The baseline migration: what it creates, that it runs once, and that it
//! refuses a database an older FlakeRadar created.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails loudly when the fixture it just built is missing"
)]

use flakeradar_db::{
    BASELINE_SQL, DbError, MIGRATIONS, Migrator, get_or_create_project, run_migrations,
    split_statements, sql_migrations,
};
use flakeradar_test_support::{TestDb, make_execution, make_run, make_test_case};
use sea_orm::{ConnectionTrait, DatabaseBackend, EntityTrait, Statement};
use sea_orm_migration::prelude::{MigrationTrait, MigratorTrait};

/// Parses an RFC 3339 instant, so the backfill inputs read as timestamps.
fn parsed(instant: &str) -> chrono::DateTime<chrono::FixedOffset> {
    chrono::DateTime::parse_from_rfc3339(instant).expect("a readable instant")
}

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

/// The versions `seaql_migrations` holds, sorted by name.
async fn applied_versions(db: &impl ConnectionTrait) -> Vec<String> {
    let statement = Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT version FROM seaql_migrations ORDER BY version".to_owned(),
    );
    db.query_all_raw(statement)
        .await
        .expect("the migration table is readable")
        .into_iter()
        .map(|row| row.try_get::<String>("", "version").expect("a version"))
        .collect()
}

/// Whether `public.<table>` holds a `column`.
async fn column_exists(db: &impl ConnectionTrait, table: &str, column: &str) -> bool {
    let statement = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = $1 AND column_name = $2) AS present",
        [table.into(), column.into()],
    );
    db.query_one_raw(statement)
        .await
        .expect("information_schema is readable")
        .and_then(|row| row.try_get::<bool>("", "present").ok())
        .unwrap_or(false)
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
    assert_eq!(
        applied_versions(db.connection()).await,
        [
            "0001_baseline",
            "0002_infra_suspect",
            "0003_flaky_since",
            "0004_acknowledge",
            "0005_issue_reopen",
            "0006_first_seen",
            "0007_digest_state"
        ]
    );
}

/// The column `0002_infra_suspect` adds, as the applied schema records it.
#[tokio::test]
async fn the_added_infra_suspect_column_is_not_null_with_a_false_default() {
    let db = TestDb::new().await.expect("a migrated test database");
    let statement = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT data_type, is_nullable, column_default FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = $1 AND column_name = $2",
        ["test_runs".into(), "infra_suspect".into()],
    );
    let row = db
        .connection()
        .query_one_raw(statement)
        .await
        .expect("information_schema is readable")
        .expect("the migrated schema holds the column");
    assert_eq!(
        (
            row.try_get::<String>("", "data_type").expect("a data type"),
            row.try_get::<String>("", "is_nullable")
                .expect("a nullability"),
            row.try_get::<Option<String>>("", "column_default")
                .expect("a default")
                .as_deref()
        ),
        ("boolean".to_owned(), "NO".to_owned(), Some("false")),
        "test_runs.infra_suspect is boolean, NOT NULL, and defaults to false"
    );
}

/// The data type and nullability `public.<table>.<column>` records, or a panic
/// naming the missing column.
async fn column_shape(db: &impl ConnectionTrait, table: &str, column: &str) -> (String, String) {
    let statement = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT data_type, is_nullable FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = $1 AND column_name = $2",
        [table.into(), column.into()],
    );
    let row = db
        .query_one_raw(statement)
        .await
        .expect("information_schema is readable")
        .expect("the migrated schema holds the column");
    (
        row.try_get::<String>("", "data_type").expect("a data type"),
        row.try_get::<String>("", "is_nullable")
            .expect("a nullability"),
    )
}

/// The columns `0004_acknowledge` adds, as the applied schema records them: both
/// nullable, with the timestamp and text types the migration asks for.
#[tokio::test]
async fn the_added_acknowledge_columns_are_nullable() {
    let db = TestDb::new().await.expect("a migrated test database");
    for (table, column, data_type) in [
        (
            "test_cases",
            "acknowledged_until",
            "timestamp with time zone",
        ),
        ("test_cases", "acknowledge_reason", "text"),
        ("jobs", "acknowledged_until", "timestamp with time zone"),
        ("jobs", "acknowledge_reason", "text"),
    ] {
        assert_eq!(
            column_shape(db.connection(), table, column).await,
            (data_type.to_owned(), "YES".to_owned()),
            "{table}.{column} is {data_type} and nullable"
        );
    }
}

/// The columns `0005_issue_reopen` adds, as the applied schema records them: both
/// nullable, with the integer and timestamp types the migration asks for.
#[tokio::test]
async fn the_added_issue_reopen_columns_are_nullable() {
    let db = TestDb::new().await.expect("a migrated test database");
    for (table, column, data_type) in [
        ("test_cases", "github_issue_last_closed_number", "integer"),
        (
            "test_cases",
            "github_issue_last_closed_at",
            "timestamp with time zone",
        ),
        ("jobs", "github_issue_last_closed_number", "integer"),
        (
            "jobs",
            "github_issue_last_closed_at",
            "timestamp with time zone",
        ),
    ] {
        assert_eq!(
            column_shape(db.connection(), table, column).await,
            (data_type.to_owned(), "YES".to_owned()),
            "{table}.{column} is {data_type} and nullable"
        );
    }
}

/// The column `0006_first_seen` adds, as the applied schema records it: NOT NULL,
/// backfilled from each Test's earliest Execution, or from `last_seen_at` when
/// the Test has none.
#[tokio::test]
async fn the_added_first_seen_column_is_not_null_and_backfilled() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = get_or_create_project(
        db.connection(),
        "acme/app",
        "backend",
        parsed("2026-09-01T00:00:00+00:00"),
    )
    .await
    .expect("a Project");
    let a = make_test_case(&project).create(&db).await;
    let with_no_executions = make_test_case(&project)
        .name("t2")
        .last_seen_at(parsed("2026-09-10T00:00:00+00:00").fixed_offset())
        .create(&db)
        .await;
    let run = make_run(&project).create(&db).await;
    make_execution(&a, &run)
        .created_at(parsed("2026-09-05T00:00:00+00:00"))
        .create(&db)
        .await;
    make_execution(&a, &run)
        .created_at(parsed("2026-09-01T00:00:00+00:00"))
        .create(&db)
        .await;

    let (_, sql) = MIGRATIONS
        .iter()
        .find(|(name, _)| *name == "0006_first_seen")
        .expect("the first-seen migration is in the set");
    let backfill = split_statements(sql)
        .into_iter()
        .find(|statement| statement.contains("UPDATE test_cases"))
        .expect("the migration holds the backfill");
    db.connection()
        .execute_unprepared(backfill)
        .await
        .expect("the backfill runs");

    let a_row = flakeradar_db::entities::test_cases::Entity::find_by_id(a.id)
        .one(db.connection())
        .await
        .expect("the Tests are readable")
        .expect("the Test exists");
    let b_row = flakeradar_db::entities::test_cases::Entity::find_by_id(with_no_executions.id)
        .one(db.connection())
        .await
        .expect("the Tests are readable")
        .expect("the Test exists");
    assert_eq!(
        a_row.first_seen_at,
        parsed("2026-09-01T00:00:00+00:00"),
        "the backfill takes the earliest Execution"
    );
    assert_eq!(
        b_row.first_seen_at,
        parsed("2026-09-10T00:00:00+00:00"),
        "a Test with no Executions falls back to last_seen_at"
    );
    assert_eq!(
        column_shape(db.connection(), "test_cases", "first_seen_at").await,
        ("timestamp with time zone".to_owned(), "NO".to_owned()),
        "test_cases.first_seen_at is a timestamp and NOT NULL"
    );
}

/// The table `0007_digest_state` creates: one singleton row, whose id the check
/// constraint pins to 1, holding the last digest slot that was sent.
#[tokio::test]
async fn the_digest_state_table_holds_only_id_one() {
    let db = TestDb::new().await.expect("a migrated test database");
    assert_eq!(
        column_shape(db.connection(), "digest_state", "id").await,
        ("smallint".to_owned(), "NO".to_owned()),
        "digest_state.id is a smallint and NOT NULL"
    );
    assert_eq!(
        column_shape(db.connection(), "digest_state", "last_sent_at").await,
        ("timestamp with time zone".to_owned(), "NO".to_owned()),
        "digest_state.last_sent_at is a timestamp and NOT NULL"
    );

    let error = db
        .connection()
        .execute_unprepared("INSERT INTO digest_state VALUES (2, now())")
        .await
        .expect_err("the check constraint refuses an id other than 1");
    assert!(
        error.to_string().contains("check constraint"),
        "the refusal names the check constraint: {error}"
    );

    db.connection()
        .execute_unprepared("INSERT INTO digest_state VALUES (1, now())")
        .await
        .expect("(1, now()) inserts once");
    let row = flakeradar_db::entities::digest_state::Entity::find_by_id(1_i16)
        .one(db.connection())
        .await
        .expect("the table is readable through the entity")
        .expect("the row with id 1 exists");
    assert_eq!(row.id, 1_i16, "the entity reads back the singleton row");

    let error = db
        .connection()
        .execute_unprepared("INSERT INTO digest_state VALUES (1, now())")
        .await
        .expect_err("the primary key refuses a second row");
    assert!(
        error.to_string().contains("duplicate key"),
        "the second insert is refused as a duplicate: {error}"
    );
}

/// A migration set with two entries after the baseline, one depending on the other.
const ORDERED: &[(&str, &str)] = &[
    ("0001_baseline", BASELINE_SQL),
    ("0002_first", "CREATE TABLE follow_on (id integer NOT NULL)"),
    ("0003_second", "ALTER TABLE follow_on ADD COLUMN note text"),
];

/// A migrator with two migrations after the baseline.
#[derive(Debug)]
struct Ordered;

#[sea_orm_migration::async_trait::async_trait]
impl MigratorTrait for Ordered {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        sql_migrations(ORDERED)
    }
}

#[tokio::test]
async fn migrations_after_the_baseline_apply_in_listed_order() {
    let db = TestDb::empty().await.expect("an empty test database");
    for _ in 0..2 {
        Ordered::up(db.connection(), None)
            .await
            .expect("a run applies every listed migration");
    }
    assert_eq!(
        applied_versions(db.connection()).await,
        ["0001_baseline", "0002_first", "0003_second"],
        "each migration is recorded once, in listed order"
    );
    assert!(
        column_exists(db.connection(), "follow_on", "note").await,
        "0003_second ran after 0002_first created its table"
    );
}

#[test]
fn the_migrator_lists_the_baseline_and_the_added_migration_by_name() {
    let migrations = Migrator::migrations();
    assert_eq!(
        migrations.len(),
        7,
        "the shipped set holds the baseline and every migration after it"
    );
    assert_eq!(
        migrations.first().expect("a set with seven entries").name(),
        "0001_baseline",
        "the recorded name is unchanged, so an existing install still matches"
    );
    assert_eq!(
        migrations.get(1).expect("a set with seven entries").name(),
        "0002_infra_suspect",
        "the added migration is recorded under its own name"
    );
    assert_eq!(
        migrations.get(2).expect("a set with seven entries").name(),
        "0003_flaky_since",
        "the added migration is recorded under its own name"
    );
    assert_eq!(
        migrations.get(3).expect("a set with seven entries").name(),
        "0004_acknowledge",
        "the added migration is recorded under its own name"
    );
    assert_eq!(
        migrations.get(4).expect("a set with seven entries").name(),
        "0005_issue_reopen",
        "the added migration is recorded under its own name"
    );
    assert_eq!(
        migrations.get(5).expect("a set with seven entries").name(),
        "0006_first_seen",
        "the added migration is recorded under its own name"
    );
    assert_eq!(
        migrations.get(6).expect("a set with seven entries").name(),
        "0007_digest_state",
        "the added migration is recorded under its own name"
    );
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
fn a_dash_that_does_not_open_a_comment_is_sql() {
    // Only `--` starts a comment, so arithmetic keeps its own dash.
    assert_eq!(split_statements("SELECT 1 - 2;"), ["SELECT 1 - 2"]);
}

#[test]
fn an_unterminated_quote_keeps_the_rest_of_the_script() {
    // The quote never closes before the script ends, so the semicolons after it
    // stay inside the one statement the script holds.
    assert_eq!(
        split_statements("SELECT 'open; SELECT 2;"),
        ["SELECT 'open; SELECT 2;"]
    );
}

#[test]
fn skips_blank_statements() {
    assert_eq!(split_statements(";\n ;\n"), Vec::<&str>::new());
}
