//! The migrated schema, compared against the frozen Python server's dump.
//!
//! `tests/parity/schema.sql` is the schema the Python server created, recorded
//! before the port. This test applies that dump to one database and compares its
//! columns, constraints and indexes with the migrated database's, so a drift in
//! either direction fails.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a statement or a query it just wrote fails"
)]

use flakeradar_test_support::TestDb;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

/// The Python server's schema, as dumped by `pg_dump`.
const GOLDEN_SCHEMA: &str = include_str!("../../../tests/parity/schema.sql");

/// The statements of an `pg_dump` script, without its comments.
fn statements(script: &str) -> Vec<&str> {
    script
        .split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
        .filter(|statement| {
            !statement
                .lines()
                .all(|line| line.trim().is_empty() || line.trim_start().starts_with("--"))
        })
        .collect()
}

/// One line per column, constraint and index in the `public` schema.
///
/// `seaql_migrations` is the migration bookkeeping table, which only the
/// migrated database has, so it is left out.
async fn schema_snapshot(db: &impl ConnectionTrait) -> Vec<String> {
    let mut lines = Vec::new();
    let queries = [
        "SELECT table_name || '|' || column_name || '|' || data_type || '|' || udt_name \
         || '|' || is_nullable || '|' || coalesce(column_default, '') \
         || '|' || coalesce(character_maximum_length::text, '') AS line \
         FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name <> 'seaql_migrations' \
         ORDER BY table_name, ordinal_position",
        "SELECT c.conname || '|' || c.contype || '|' || pg_get_constraintdef(c.oid) AS line \
         FROM pg_constraint c \
         JOIN pg_class t ON t.oid = c.conrelid \
         JOIN pg_namespace n ON n.oid = t.relnamespace \
         WHERE n.nspname = 'public' AND t.relname <> 'seaql_migrations' \
         ORDER BY c.conname",
        "SELECT indexname || '|' || tablename || '|' || indexdef AS line \
         FROM pg_indexes \
         WHERE schemaname = 'public' AND tablename <> 'seaql_migrations' \
         ORDER BY indexname",
    ];
    for query in queries {
        let rows = db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                query.to_owned(),
            ))
            .await
            .expect("a catalogue query runs");
        for row in rows {
            lines.push(row.try_get::<String>("", "line").expect("a line"));
        }
    }
    lines
}

#[tokio::test]
async fn the_migrated_schema_matches_the_python_schema() {
    let migrated = TestDb::new().await.expect("a migrated test database");
    let golden = TestDb::empty().await.expect("an empty test database");
    for statement in statements(GOLDEN_SCHEMA) {
        golden
            .connection()
            .execute_unprepared(statement)
            .await
            .expect("the golden schema applies");
    }

    let migrated_schema = schema_snapshot(migrated.connection()).await;
    let golden_schema = schema_snapshot(golden.connection()).await;
    assert_eq!(migrated_schema, golden_schema);
}

#[tokio::test]
async fn the_comparison_sees_a_difference() {
    let plain = TestDb::empty().await.expect("an empty test database");
    let golden = TestDb::empty().await.expect("an empty test database");
    for statement in statements(GOLDEN_SCHEMA) {
        golden
            .connection()
            .execute_unprepared(statement)
            .await
            .expect("the golden schema applies");
    }

    assert_ne!(
        schema_snapshot(plain.connection()).await,
        schema_snapshot(golden.connection()).await
    );
}
