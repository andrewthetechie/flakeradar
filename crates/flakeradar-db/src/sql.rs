//! The raw-SQL helpers every statement in this crate shares.
//!
//! PostgreSQL numbers a bind by its position, so a statement and its values
//! have to agree on one scheme. [`Binds`] is that scheme: every placeholder it
//! hands out comes with the value it names, in bind order. [`rows`], [`one`] and
//! [`required_one`] run a statement built that way, and [`get`] reads one column
//! of a row. A caller that needs a bind of its own takes the number from
//! [`Binds::bind`] rather than counting `$n` by hand.

// Rust guideline compliant 2026-07-21

use flakeradar_core::ExecutionStatus;
use sea_orm::{ConnectionTrait, DbBackend, DbErr, QueryResult, Statement, TryGetable, Value};

use crate::error::DbError;

/// Reads one stored Execution status.
///
/// # Errors
///
/// Returns [`DbError::UnknownExecutionStatus`] for a spelling the four
/// [`ExecutionStatus`] variants do not cover.
pub(crate) fn execution_status(text: &str) -> Result<ExecutionStatus, DbError> {
    ExecutionStatus::from_wire(text).ok_or_else(|| DbError::UnknownExecutionStatus {
        value: text.to_owned(),
    })
}

/// Reads one column of a raw row.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the column is missing or has another type.
pub fn get<T: TryGetable>(row: &QueryResult, column: &str) -> Result<T, DbError> {
    row.try_get("", column)
        .map_err(|error: DbErr| DbError::Database(error))
}

/// Runs a statement and reads every row.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn rows(
    db: &impl ConnectionTrait,
    sql: impl Into<String>,
    values: Vec<Value>,
) -> Result<Vec<QueryResult>, DbError> {
    Ok(db
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await?)
}

/// Runs a statement and reads its first row, when it has one.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn one(
    db: &impl ConnectionTrait,
    sql: impl Into<String>,
    values: Vec<Value>,
) -> Result<Option<QueryResult>, DbError> {
    Ok(db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await?)
}

/// Runs a statement that always answers one row, such as an aggregate.
///
/// `what` names the query in the error a statement that answered nothing gives.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails or answers no row.
pub async fn required_one(
    db: &impl ConnectionTrait,
    sql: impl Into<String>,
    values: Vec<Value>,
    what: &str,
) -> Result<QueryResult, DbError> {
    one(db, sql, values).await?.ok_or_else(|| {
        DbError::Database(DbErr::RecordNotFound(format!(
            "the {what} query returned no row"
        )))
    })
}

/// `ids` as one `BIGINT[]` bind, for a statement that takes a batch of ids.
#[must_use]
pub fn bigint_array(ids: &[i64]) -> Value {
    Value::Array(
        sea_orm::sea_query::ArrayType::BigInt,
        Some(Box::new(ids.iter().copied().map(Value::from).collect())),
    )
}

/// The `$n` placeholders from `start`, one per bind: `$3, $4, $5`.
#[must_use]
pub fn placeholder_list(start: usize, count: usize) -> String {
    (start..start + count)
        .map(|n| format!("${n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The placeholder list of one `VALUES` row: `($3, $4, $5)` for a start of 3.
#[must_use]
pub fn value_row(start: usize, width: usize) -> String {
    format!("({})", placeholder_list(start, width))
}

/// The conditions of one statement, with the binds they need.
///
/// The first bind takes `$start`, so a statement can reserve the placeholders it
/// spells out itself and let the builder number the rest.
#[derive(Debug, Clone)]
pub struct Binds {
    conditions: Vec<String>,
    values: Vec<Value>,
    next: usize,
}

impl Default for Binds {
    fn default() -> Self {
        Self::new()
    }
}

impl Binds {
    /// A builder whose first placeholder is `$1`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            conditions: Vec::new(),
            values: Vec::new(),
            next: 1,
        }
    }

    /// A builder whose first placeholder is `$start`.
    #[must_use]
    pub fn after(start: usize) -> Self {
        Self {
            conditions: Vec::new(),
            values: Vec::new(),
            next: start,
        }
    }

    /// The placeholder the next bind takes, without binding a value.
    #[must_use]
    pub fn next(&self) -> String {
        format!("${}", self.next)
    }

    /// Binds `value` and answers the placeholder that names it.
    pub fn bind(&mut self, value: Value) -> String {
        let placeholder = self.next();
        self.values.push(value);
        self.next += 1;
        placeholder
    }

    /// Adds `column = $n` and binds the value.
    pub fn eq(&mut self, column: &str, value: Value) {
        let placeholder = self.bind(value);
        self.raw(format!("{column} = {placeholder}"));
    }

    /// Adds a condition with no bind of its own.
    pub fn raw(&mut self, condition: impl Into<String>) {
        self.conditions.push(condition.into());
    }

    /// The values bound so far, in bind order.
    #[must_use]
    pub fn values(&self) -> &[Value] {
        &self.values
    }

    /// The bound values, in bind order.
    #[must_use]
    pub fn into_values(self) -> Vec<Value> {
        self.values
    }

    /// The ` WHERE ...` suffix, or nothing when no condition was added.
    #[must_use]
    pub fn suffix(&self) -> String {
        if self.conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", self.conditions.join(" AND "))
        }
    }
}

#[cfg(test)]
mod tests {
    //! Placeholder numbering needs no database.

    use sea_orm::Value;

    use super::{Binds, placeholder_list, value_row};

    #[test]
    fn a_condition_suffix_is_empty_without_conditions() {
        assert_eq!(Binds::new().suffix(), "");
    }

    #[test]
    fn every_bound_value_comes_with_its_own_placeholder() {
        let mut binds = Binds::new();
        binds.eq("r.name", Value::from("acme/app"));
        binds.raw("t.flakiness_score > 0");
        let placeholder = binds.bind(Value::from(0.3));
        binds.raw(format!("t.flakiness_score >= {placeholder}"));
        assert_eq!(
            binds.suffix(),
            " WHERE r.name = $1 AND t.flakiness_score > 0 AND t.flakiness_score >= $2"
        );
        assert_eq!(binds.values().len(), 2);
    }

    #[test]
    fn a_builder_that_starts_later_keeps_the_numbers_a_statement_reserved() {
        let mut binds = Binds::after(3);
        assert_eq!(binds.next(), "$3");
        binds.eq("p.name", Value::from("backend"));
        assert_eq!(binds.suffix(), " WHERE p.name = $3");
        assert_eq!(binds.into_values().len(), 1);
    }

    #[test]
    fn a_value_row_numbers_every_column_from_its_start() {
        assert_eq!(value_row(1, 3), "($1, $2, $3)");
        assert_eq!(value_row(7, 2), "($7, $8)");
        assert_eq!(placeholder_list(4, 2), "$4, $5");
        assert_eq!(placeholder_list(3, 0), "");
    }
}
