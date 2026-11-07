//! The migrated schema, compared against the recorded baseline.
//!
//! `tests/parity/schema.sql` is the schema a 1.x database has, recorded before
//! the rewrite. This test applies that dump to one database and compares its
//! columns, constraints and indexes with the migrated database's, so a drift in
//! either direction fails.
//!
//! The comparison is [`snapshot_difference`], and the drift cases below change
//! one field of a real snapshot at a time, so that comparison is shown to report
//! a single altered column by name instead of only a wholly absent schema.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a statement or a query it just wrote fails"
)]

use flakeradar_db::split_statements;
use flakeradar_test_support::TestDb;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};

/// The recorded 1.x schema, as dumped by `pg_dump`.
const GOLDEN_SCHEMA: &str = include_str!("../../../tests/parity/schema.sql");

/// The start of the snapshot line for the column the drift cases change.
///
/// `test_cases.failure_category` is nullable and `character varying`, so both its
/// type and its nullability can be changed from the value the schema has.
const COLUMN_LINE_PREFIX: &str = "test_cases|failure_category|";

/// Position of a column's `data_type` in its snapshot line.
const DATA_TYPE_FIELD: usize = 2;

/// Position of a column's `is_nullable` in its snapshot line.
const NULLABILITY_FIELD: usize = 4;

/// Fields in a column snapshot line: table, column, data type, udt name,
/// nullability, default and character length.
const COLUMN_FIELDS: usize = 7;

/// Fields in a constraint snapshot line: name, type and definition.
const CONSTRAINT_FIELDS: usize = 3;

/// Fields in an index snapshot line: name, table and definition.
const INDEX_FIELDS: usize = 3;

/// The lines only one of two snapshots holds, in each snapshot's order.
#[derive(Debug)]
struct SnapshotDifference {
    /// Lines the left snapshot holds and the right one does not.
    only_in_left: Vec<String>,
    /// Lines the right snapshot holds and the left one does not.
    only_in_right: Vec<String>,
}

impl SnapshotDifference {
    /// Whether both snapshots hold the same lines.
    fn is_empty(&self) -> bool {
        self.only_in_left.is_empty() && self.only_in_right.is_empty()
    }
}

/// The lines two snapshots do not have in common, counted once per line.
///
/// A missing, extra or altered column, constraint or index lands here as the
/// snapshot line that names it, which reports a drift in the terms of the schema
/// rather than as two whole snapshots.
fn snapshot_difference(left: &[String], right: &[String]) -> SnapshotDifference {
    let mut only_in_left = left.to_vec();
    let mut only_in_right = Vec::new();
    for line in right {
        match only_in_left.iter().position(|candidate| candidate == line) {
            Some(index) => {
                only_in_left.remove(index);
            }
            None => only_in_right.push(line.clone()),
        }
    }
    SnapshotDifference {
        only_in_left,
        only_in_right,
    }
}

/// One line per column, constraint and index in the `public` schema.
///
/// `seaql_migrations` is the migration bookkeeping table, which only the
/// migrated database has, so it is left out.
///
/// The snapshot runs in one transaction with `search_path` pinned to `public`.
/// `pg_get_constraintdef` and `pg_get_indexdef` leave the schema off a name only
/// when the schema is in the search path, and a `pg_dump` script sets the search
/// path it was dumped with, which would otherwise make the two sides differ in
/// punctuation rather than in schema.
async fn schema_snapshot(db: &DatabaseConnection) -> Vec<String> {
    let transaction = db.begin().await.expect("a transaction");
    transaction
        .execute_unprepared("SET LOCAL search_path = public")
        .await
        .expect("the search path is pinned");
    let mut lines = Vec::new();
    let queries = [
        "SELECT table_name::text || '|' || column_name::text || '|' || data_type::text \
         || '|' || udt_name::text || '|' || is_nullable::text || '|' \
         || coalesce(column_default::text, '') || '|' \
         || coalesce(character_maximum_length::text, '') AS line \
         FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name <> 'seaql_migrations' \
         ORDER BY table_name, ordinal_position",
        "SELECT c.conname::text || '|' || c.contype::text || '|' \
         || pg_get_constraintdef(c.oid) AS line \
         FROM pg_constraint c \
         JOIN pg_class t ON t.oid = c.conrelid \
         JOIN pg_namespace n ON n.oid = t.relnamespace \
         WHERE n.nspname = 'public' AND t.relname <> 'seaql_migrations' \
         ORDER BY c.conname",
        "SELECT indexname::text || '|' || tablename::text || '|' || indexdef AS line \
         FROM pg_indexes \
         WHERE schemaname = 'public' AND tablename <> 'seaql_migrations' \
         ORDER BY indexname",
    ];
    for query in queries {
        let rows = transaction
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
    transaction.rollback().await.expect("the transaction ends");
    lines
}

/// The migrated snapshot and the golden one, which the parity test expects to
/// match.
async fn compared_snapshots() -> (Vec<String>, Vec<String>) {
    let migrated = TestDb::new().await.expect("a migrated test database");
    let golden = TestDb::empty().await.expect("an empty test database");
    for statement in split_statements(GOLDEN_SCHEMA) {
        golden
            .connection()
            .execute_unprepared(statement)
            .await
            .expect("the golden schema applies");
    }

    (
        schema_snapshot(migrated.connection()).await,
        schema_snapshot(golden.connection()).await,
    )
}

/// The snapshot line for [`COLUMN_LINE_PREFIX`], when the snapshot emits it.
fn column_line(snapshot: &[String]) -> Option<&String> {
    snapshot
        .iter()
        .find(|line| line.starts_with(COLUMN_LINE_PREFIX))
}

/// `line` with the field at `position` replaced by `value`.
fn replace_field(line: &str, position: usize, value: &str) -> String {
    line.split('|')
        .enumerate()
        .map(|(index, field)| if index == position { value } else { field })
        .collect::<Vec<_>>()
        .join("|")
}

/// `snapshot` with the line `from` replaced by `to`.
fn replace_line(snapshot: &[String], from: &str, to: &str) -> Vec<String> {
    snapshot
        .iter()
        .map(|line| {
            if line == from {
                to.to_owned()
            } else {
                line.clone()
            }
        })
        .collect()
}

/// `snapshot` without the line `dropped`.
fn without_line(snapshot: &[String], dropped: &str) -> Vec<String> {
    snapshot
        .iter()
        .filter(|line| line.as_str() != dropped)
        .cloned()
        .collect()
}

#[tokio::test]
async fn the_migrated_schema_matches_the_recorded_baseline() {
    let (migrated_schema, golden_schema) = compared_snapshots().await;

    let difference = snapshot_difference(&migrated_schema, &golden_schema);
    assert!(
        difference.is_empty(),
        "the migrated schema drifted from the recorded baseline: {difference:#?}"
    );
    // The difference ignores the order of the lines, and the snapshots are
    // ordered too, so a reordered line is still a drift.
    assert_eq!(migrated_schema, golden_schema);
}

#[tokio::test]
async fn the_comparison_sees_a_difference() {
    let plain = TestDb::empty().await.expect("an empty test database");
    let golden = TestDb::empty().await.expect("an empty test database");
    for statement in split_statements(GOLDEN_SCHEMA) {
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

#[tokio::test]
async fn the_comparison_sees_no_difference_between_matching_snapshots() {
    let (migrated, golden) = compared_snapshots().await;

    let difference = snapshot_difference(&migrated, &golden);

    assert!(difference.is_empty(), "{difference:#?}");
}

#[tokio::test]
async fn the_comparison_names_a_dropped_column() {
    let (migrated, golden) = compared_snapshots().await;
    let dropped = column_line(&golden)
        .expect("the golden snapshot emits the column the test drops")
        .clone();

    let difference = snapshot_difference(&migrated, &without_line(&golden, &dropped));

    assert_eq!(difference.only_in_left, vec![dropped], "{difference:#?}");
    assert!(
        difference
            .only_in_left
            .iter()
            .all(|line| line.starts_with(COLUMN_LINE_PREFIX)),
        "the difference does not name the dropped column: {difference:#?}"
    );
    assert!(difference.only_in_right.is_empty(), "{difference:#?}");
}

#[tokio::test]
async fn the_comparison_names_a_changed_column_type() {
    let (migrated, golden) = compared_snapshots().await;
    let original =
        column_line(&golden).expect("the golden snapshot emits the column the test changes");
    let changed = replace_field(original, DATA_TYPE_FIELD, "text");

    let difference = snapshot_difference(&migrated, &replace_line(&golden, original, &changed));

    assert_eq!(
        difference.only_in_left,
        vec![original.clone()],
        "{difference:#?}"
    );
    assert_eq!(difference.only_in_right, vec![changed], "{difference:#?}");
    assert!(
        difference
            .only_in_right
            .iter()
            .all(|line| line.starts_with(COLUMN_LINE_PREFIX)),
        "the difference does not name the column whose type changed: {difference:#?}"
    );
}

#[tokio::test]
async fn the_comparison_names_a_changed_nullability() {
    let (migrated, golden) = compared_snapshots().await;
    let original =
        column_line(&golden).expect("the golden snapshot emits the column the test changes");
    assert_eq!(
        original.split('|').nth(NULLABILITY_FIELD),
        Some("YES"),
        "the column the test changes is expected to be nullable: {original}"
    );
    let changed = replace_field(original, NULLABILITY_FIELD, "NO");

    let difference = snapshot_difference(&migrated, &replace_line(&golden, original, &changed));

    assert_eq!(
        difference.only_in_left,
        vec![original.clone()],
        "{difference:#?}"
    );
    assert_eq!(difference.only_in_right, vec![changed], "{difference:#?}");
    assert!(
        difference
            .only_in_right
            .iter()
            .all(|line| line.starts_with(COLUMN_LINE_PREFIX)),
        "the difference does not name the column whose nullability changed: {difference:#?}"
    );
}

#[tokio::test]
async fn the_snapshot_keeps_every_field_of_each_catalogue() {
    let migrated = TestDb::new().await.expect("a migrated test database");
    let snapshot = schema_snapshot(migrated.connection()).await;

    let column = column_line(&snapshot).expect("the snapshot emits the test_cases column");
    assert_eq!(
        column.split('|').count(),
        COLUMN_FIELDS,
        "the column line lost a field: {column}"
    );

    let constraint = snapshot
        .iter()
        .find(|line| line.starts_with("test_cases_pkey|p|"))
        .expect("the snapshot emits the test_cases primary key");
    assert_eq!(
        constraint.split('|').count(),
        CONSTRAINT_FIELDS,
        "the constraint line lost a field: {constraint}"
    );

    let index = snapshot
        .iter()
        .find(|line| line.starts_with("ix_test_cases_issue_check|"))
        .expect("the snapshot emits the partial index on test_cases");
    assert_eq!(
        index.split('|').count(),
        INDEX_FIELDS,
        "the index line lost a field: {index}"
    );
}
