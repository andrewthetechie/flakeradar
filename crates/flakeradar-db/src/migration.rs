//! The baseline migration and the old-database check that runs before it.
//!
//! A FlakeRadar 2 database is created in one step: [`run_migrations`] takes the
//! advisory lock, refuses a database an older FlakeRadar created, then applies
//! [`BASELINE_SQL`]. Nothing is imported or renamed, so the schema is the whole
//! history.

// Rust guideline compliant 2026-07-21

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, TransactionTrait};
use sea_orm_migration::prelude::{MigrationName, MigrationTrait, MigratorTrait, SchemaManager};

use crate::{MIGRATION_LOCK_KEY, error::DbError};

/// The baseline schema, as one applyable SQL script.
///
/// It holds today's tables, columns, types, defaults, constraint names and
/// index names, and it is the only migration: a fresh install starts here.
pub const BASELINE_SQL: &str = include_str!("../migrations/0001_baseline.sql");

/// Every table the baseline creates.
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

/// The migration set of a FlakeRadar 2 database.
#[derive(Debug)]
pub struct Migrator;

#[sea_orm_migration::async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(Migration0001Baseline)]
    }
}

/// Creates the baseline schema.
struct Migration0001Baseline;

impl MigrationName for Migration0001Baseline {
    fn name(&self) -> &'static str {
        "0001_baseline"
    }
}

#[sea_orm_migration::async_trait::async_trait]
impl MigrationTrait for Migration0001Baseline {
    #[expect(
        elided_lifetimes_in_paths,
        reason = "the trait method takes `&SchemaManager`; naming its lifetime makes \
                  it early-bound and the signature no longer matches the trait"
    )]
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for statement in split_statements(BASELINE_SQL) {
            manager
                .get_connection()
                .execute_unprepared(statement)
                .await?;
        }
        Ok(())
    }
}

/// Applies the baseline schema to a fresh database, once.
///
/// The whole run is one transaction behind the advisory lock
/// [`MIGRATION_LOCK_KEY`], so replicas that start together migrate once: the
/// first takes the lock, and the rest find nothing to do. A database an older
/// FlakeRadar created is refused before anything is written.
///
/// # Errors
///
/// Returns [`DbError::LegacyDatabase`] when the database was not created by
/// FlakeRadar 2, and [`DbError::Database`] when the server rejects the
/// migration.
pub async fn run_migrations(db: &DatabaseConnection) -> Result<(), DbError> {
    let transaction = db.begin().await?;
    let lock = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock($1)",
        [MIGRATION_LOCK_KEY.into()],
    );
    transaction.execute_raw(lock).await?;
    ensure_created_by_us(&transaction).await?;
    Migrator::up(&transaction, None).await?;
    transaction.commit().await?;
    Ok(())
}

/// Refuses a database that an older FlakeRadar, or a foreign schema, created.
///
/// Alembic left a `alembic_version` table, so its presence identifies an older
/// FlakeRadar; app tables without a `seaql_migrations` table beside them are the
/// same case with the version table gone. Either way FlakeRadar 2 changes
/// nothing and stops.
async fn ensure_created_by_us(db: &impl ConnectionTrait) -> Result<(), DbError> {
    if table_exists(db, "alembic_version").await? {
        return Err(DbError::LegacyDatabase {
            detail: "it holds an alembic_version table".to_owned(),
        });
    }
    if table_exists(db, "seaql_migrations").await? {
        return Ok(());
    }
    if let Some(table) = first_app_table(db).await? {
        return Err(DbError::LegacyDatabase {
            detail: format!("it holds a {table} table but no seaql_migrations table"),
        });
    }
    Ok(())
}

/// Reports whether `public.<table>` exists, as a table or a view.
async fn table_exists(db: &impl ConnectionTrait, table: &str) -> Result<bool, DbErr> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = $1) AS present",
        [table.into()],
    );
    let row = db
        .query_one_raw(statement)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("information_schema.tables".to_owned()))?;
    row.try_get("", "present")
}

/// The name of one baseline table in the database, if any exists.
async fn first_app_table(db: &impl ConnectionTrait) -> Result<Option<String>, DbErr> {
    let statement = Statement::from_string(
        DbBackend::Postgres,
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_type = 'BASE TABLE'"
            .to_owned(),
    );
    Ok(db
        .query_all_raw(statement)
        .await?
        .into_iter()
        .filter_map(|row| row.try_get::<String>("", "table_name").ok())
        .find(|name| APP_TABLES.contains(&name.as_str())))
}

/// Splits an SQL script into statements, one per top-level semicolon.
///
/// Single-quoted literals, double-quoted identifiers and `--` comments are
/// honoured, so a semicolon inside one does not end a statement. A comment stays
/// attached to the statement it precedes, which the server ignores. Dollar-quoted
/// strings are not supported, and the baseline does not use them.
#[must_use]
pub fn split_statements(script: &str) -> Vec<&str> {
    let mut statements = Vec::new();
    let mut start = 0;
    let mut chars = script.char_indices();
    while let Some((index, ch)) = chars.next() {
        match ch {
            '\'' | '"' => skip_quoted(&mut chars, ch),
            '-' if chars.as_str().starts_with('-') => skip_line_comment(&mut chars),
            ';' => {
                push_statement(script, start, index, &mut statements);
                start = index + 1;
            }
            _ => {}
        }
    }
    push_statement(script, start, script.len(), &mut statements);
    statements
}

/// Advances past a quoted run, doubling the quote character to escape it.
fn skip_quoted(chars: &mut std::str::CharIndices<'_>, quote: char) {
    while let Some((_, ch)) = chars.next() {
        if ch == quote {
            if chars.as_str().starts_with(quote) {
                chars.next();
            } else {
                return;
            }
        }
    }
}

/// Advances past the rest of a `--` comment, not including its newline.
fn skip_line_comment(chars: &mut std::str::CharIndices<'_>) {
    for (_, ch) in chars.by_ref() {
        if ch == '\n' {
            return;
        }
    }
}

/// Appends `script[start..end]` unless it holds no SQL.
fn push_statement<'a>(script: &'a str, start: usize, end: usize, out: &mut Vec<&'a str>) {
    if let Some(statement) = script.get(start..end)
        && !is_only_comments(statement)
    {
        out.push(statement);
    }
}

/// Whether `sql` is whitespace and `--` comments with no statement in it.
fn is_only_comments(sql: &str) -> bool {
    sql.lines()
        .all(|line| line.trim().is_empty() || line.trim_start().starts_with("--"))
}

#[cfg(test)]
mod tests {
    //! Statement splitting is not reachable through a database test alone, so
    //! its quoting and comment rules are checked here.

    use super::split_statements;

    #[test]
    fn splits_on_top_level_semicolons() {
        assert_eq!(
            split_statements("SELECT 1; SELECT 2;\n"),
            ["SELECT 1", " SELECT 2"]
        );
    }

    #[test]
    fn keeps_semicolons_inside_literals_and_identifiers() {
        let script = "CREATE TABLE public.t (\n    v text DEFAULT 'a;b' NOT NULL,\n    \"c;d\" integer\n);\n";
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
}
