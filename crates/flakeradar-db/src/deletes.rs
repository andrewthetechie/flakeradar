//! Admin deletes: one Repo, or one Project of a Repo, with everything it owns.
//!
//! The DELETE removes the Repo or Project row and lets the `ON DELETE CASCADE`
//! foreign keys take the children in the same transaction. The counts and the
//! GitHub issue numbers are read before that, because the rows have to exist for
//! either to be read; the numbers FlakeRadar filed are returned separately so the
//! caller can close those issues after the commit.
//!
//! The lock order with the processor is a two-way wait: the processor holds the
//! pending Report it is processing `FOR UPDATE`, so this delete blocks until that
//! transaction ends, while the processor's later inserts take `FOR KEY SHARE` on
//! the parent Repo or Project row through their foreign keys, which conflicts
//! with this endpoint's `FOR UPDATE`. PostgreSQL may abort either transaction with
//! a deadlock (SQLSTATE 40P01), and this layer reports it so the route can answer
//! a retryable 503.

// Rust guideline compliant 2026-07-21

use flakeradar_core::{DeletedCountsOut, ProjectDeleteOut, RepoDeleteOut};
use sea_orm::sqlx::error::Error as SqlxError;
use sea_orm::{
    ConnectionTrait, DbBackend, DbErr, RuntimeErr, Statement, TransactionSession, TransactionTrait,
    Value,
};

use crate::error::DbError;
use crate::reads::{get, one};

/// Everything a delete can fail with.
#[derive(Debug, thiserror::Error)]
pub enum DeleteError {
    /// PostgreSQL aborted the transaction for a lock cycle the caller may retry.
    #[error("the delete raced with the report processor")]
    Deadlock,
    /// A statement failed.
    #[error(transparent)]
    Database(#[from] DbError),
}

impl From<DbErr> for DeleteError {
    fn from(error: DbErr) -> Self {
        Self::Database(DbError::Database(error))
    }
}

/// What a delete removed, and which of the issue numbers FlakeRadar owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedRepo {
    /// The response: what was removed and every issue number stored on it.
    pub out: RepoDeleteOut,
    /// The numbers of the issues FlakeRadar filed itself, which it may close.
    pub filed: Vec<i64>,
}

/// What a Project delete removed, and which issues FlakeRadar owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedProject {
    /// The response: what was removed and every issue number stored on it.
    pub out: ProjectDeleteOut,
    /// The numbers of the issues FlakeRadar filed itself, which it may close.
    pub filed: Vec<i64>,
}

/// Deletes one Repo and, by cascade, everything in it.
///
/// # Errors
///
/// Returns [`DeleteError::Deadlock`] when PostgreSQL aborted the transaction for
/// a lock cycle, and [`DeleteError::Database`] when another statement fails.
pub async fn delete_repo(
    db: &(impl ConnectionTrait + TransactionTrait),
    repo: &str,
) -> Result<Option<DeletedRepo>, DeleteError> {
    let transaction = db.begin().await?;
    let locked = one(
        &transaction,
        "SELECT id, name FROM repos WHERE name = $1 FOR UPDATE".to_owned(),
        vec![Value::from(repo.to_owned())],
    )
    .await?;
    let Some(row) = locked else {
        transaction.rollback().await?;
        return Ok(None);
    };
    let repo_id: i32 = get(&row, "id")?;
    let repo_name: String = get(&row, "name")?;
    let deleted = repo_counts(&transaction, repo_id).await?;
    let (numbers, filed) = repo_issue_numbers(&transaction, repo_id).await?;
    delete_row(&transaction, "repos", repo_id).await?;
    transaction.commit().await?;
    Ok(Some(DeletedRepo {
        out: RepoDeleteOut {
            repo: repo_name,
            deleted,
            github_issue_numbers: numbers,
        },
        filed,
    }))
}

/// Deletes one Project of a Repo and, by cascade, its Tests, Runs and Reports.
///
/// # Errors
///
/// Returns [`DeleteError::Deadlock`] when PostgreSQL aborted the transaction for
/// a lock cycle, and [`DeleteError::Database`] when another statement fails.
pub async fn delete_project(
    db: &(impl ConnectionTrait + TransactionTrait),
    repo: &str,
    project: &str,
) -> Result<Option<DeletedProject>, DeleteError> {
    let transaction = db.begin().await?;
    let repo_row = one(
        &transaction,
        "SELECT id, name FROM repos WHERE name = $1".to_owned(),
        vec![Value::from(repo.to_owned())],
    )
    .await?;
    let Some(row) = repo_row else {
        transaction.rollback().await?;
        return Ok(None);
    };
    let repo_id: i32 = get(&row, "id")?;
    let repo_name: String = get(&row, "name")?;
    let project_row = one(
        &transaction,
        "SELECT id, name FROM projects WHERE repo_id = $1 AND name = $2 FOR UPDATE".to_owned(),
        vec![Value::from(repo_id), Value::from(project.to_owned())],
    )
    .await?;
    let Some(row) = project_row else {
        transaction.rollback().await?;
        return Ok(None);
    };
    let project_id: i32 = get(&row, "id")?;
    let project_name: String = get(&row, "name")?;
    let deleted = project_counts(&transaction, project_id).await?;
    let (numbers, filed) = project_issue_numbers(&transaction, project_id).await?;
    delete_row(&transaction, "projects", project_id).await?;
    transaction.commit().await?;
    Ok(Some(DeletedProject {
        out: ProjectDeleteOut {
            repo: repo_name,
            project: project_name,
            deleted,
            github_issue_numbers: numbers,
        },
        filed,
    }))
}

/// Deletes one row by id, reporting a lock cycle as a deadlock.
async fn delete_row(
    db: &impl ConnectionTrait,
    table: &'static str,
    id: i32,
) -> Result<(), DeleteError> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("DELETE FROM {table} WHERE id = $1"),
        [Value::from(id)],
    );
    match db.execute_raw(statement).await {
        Ok(_) => Ok(()),
        Err(error) if is_deadlock(&error) => Err(DeleteError::Deadlock),
        Err(error) => Err(DeleteError::Database(DbError::Database(error))),
    }
}

/// SQLSTATEs an admin retry can resolve: deadlock detected, serialization failure.
fn is_deadlock(error: &DbErr) -> bool {
    let runtime = match error {
        DbErr::Conn(error) | DbErr::Exec(error) | DbErr::Query(error) => error,
        _ => return false,
    };
    let RuntimeErr::SqlxError(error) = runtime else {
        return false;
    };
    match error.as_ref() {
        SqlxError::Database(database) => database
            .code()
            .is_some_and(|code| matches!(code.as_ref(), "40P01" | "40001")),
        _ => false,
    }
}

/// The four counts one Repo delete reports, read before the cascade.
async fn repo_counts(
    db: &impl ConnectionTrait,
    repo_id: i32,
) -> Result<DeletedCountsOut, DeleteError> {
    let tests = count(
        db,
        "SELECT count(*) AS value FROM test_cases t \
         JOIN projects p ON p.id = t.project_id WHERE p.repo_id = $1",
        repo_id,
    )
    .await?;
    let executions = count(
        db,
        "SELECT count(*) AS value FROM test_executions e \
         JOIN test_cases t ON t.id = e.test_case_id \
         JOIN projects p ON p.id = t.project_id WHERE p.repo_id = $1",
        repo_id,
    )
    .await?;
    let jobs = count(
        db,
        "SELECT count(*) AS value FROM jobs j \
         JOIN pipelines p ON p.id = j.pipeline_id WHERE p.repo_id = $1",
        repo_id,
    )
    .await?;
    let reports = count(
        db,
        "SELECT count(*) AS value FROM reports WHERE repo_id = $1",
        repo_id,
    )
    .await?;
    Ok(DeletedCountsOut {
        tests,
        executions,
        jobs,
        reports,
    })
}

/// The four counts one Project delete reports. A Project removes no Job.
async fn project_counts(
    db: &impl ConnectionTrait,
    project_id: i32,
) -> Result<DeletedCountsOut, DeleteError> {
    let tests = count(
        db,
        "SELECT count(*) AS value FROM test_cases WHERE project_id = $1",
        project_id,
    )
    .await?;
    let executions = count(
        db,
        "SELECT count(*) AS value FROM test_executions e \
         JOIN test_cases t ON t.id = e.test_case_id WHERE t.project_id = $1",
        project_id,
    )
    .await?;
    let reports = count(
        db,
        "SELECT count(*) AS value FROM reports WHERE project_id = $1",
        project_id,
    )
    .await?;
    Ok(DeletedCountsOut {
        tests,
        executions,
        jobs: 0,
        reports,
    })
}

/// One `count(*)` statement.
async fn count(db: &impl ConnectionTrait, sql: &str, parameter: i32) -> Result<i64, DeleteError> {
    let Some(row) = one(db, sql.to_owned(), vec![Value::from(parameter)]).await? else {
        return Ok(0);
    };
    let value: i64 = get(&row, "value")?;
    Ok(value)
}

/// Every stored issue number of the Repo, and the FlakeRadar-filed subset.
async fn repo_issue_numbers(
    db: &impl ConnectionTrait,
    repo_id: i32,
) -> Result<(Vec<i64>, Vec<i64>), DeleteError> {
    let tests = "SELECT t.github_issue_number AS number FROM test_cases t \
                 JOIN projects p ON p.id = t.project_id \
                 WHERE p.repo_id = $1 AND t.github_issue_number IS NOT NULL";
    let tests_filed = format!("{tests} AND t.github_issue_filed IS TRUE");
    let jobs = "SELECT j.github_issue_number AS number FROM jobs j \
                JOIN pipelines p ON p.id = j.pipeline_id \
                WHERE p.repo_id = $1 AND j.github_issue_number IS NOT NULL";
    let jobs_filed = format!("{jobs} AND j.github_issue_filed IS TRUE");
    let numbers = issue_numbers(db, &[tests, jobs], repo_id).await?;
    let filed = issue_numbers(db, &[&tests_filed, &jobs_filed], repo_id).await?;
    Ok((numbers, filed))
}

/// Every stored issue number of the Project, and the FlakeRadar-filed subset.
///
/// Jobs hang off Pipelines, which belong to the Repo, so a Project delete strands
/// no Job issue. A human-linked issue is reported but never auto-closed.
async fn project_issue_numbers(
    db: &impl ConnectionTrait,
    project_id: i32,
) -> Result<(Vec<i64>, Vec<i64>), DeleteError> {
    let tests = "SELECT github_issue_number AS number FROM test_cases \
                 WHERE project_id = $1 AND github_issue_number IS NOT NULL";
    let filed = format!("{tests} AND github_issue_filed IS TRUE");
    let numbers = issue_numbers(db, &[tests], project_id).await?;
    let filed = issue_numbers(db, &[&filed], project_id).await?;
    Ok((numbers, filed))
}

/// The sorted, deduplicated issue numbers the given statements select.
async fn issue_numbers(
    db: &impl ConnectionTrait,
    statements: &[&str],
    parameter: i32,
) -> Result<Vec<i64>, DeleteError> {
    let mut numbers: Vec<i64> = Vec::new();
    for sql in statements {
        let Some(rows) = one_many(db, sql, parameter).await? else {
            continue;
        };
        numbers.extend(rows);
    }
    numbers.sort_unstable();
    numbers.dedup();
    Ok(numbers)
}

/// Reads every row of a one-column statement.
async fn one_many(
    db: &impl ConnectionTrait,
    sql: &str,
    parameter: i32,
) -> Result<Option<Vec<i64>>, DeleteError> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql.to_owned(),
            [Value::from(parameter)],
        ))
        .await
        .map_err(|error| DeleteError::Database(DbError::Database(error)))?;
    let mut numbers = Vec::new();
    for row in rows {
        let number: i32 = get(&row, "number")?;
        numbers.push(i64::from(number));
    }
    Ok(Some(numbers))
}
