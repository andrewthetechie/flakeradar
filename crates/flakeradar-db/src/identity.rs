//! Race-safe get-or-create for Repo and Project rows.
//!
//! Both helpers select first, so the hot path (the row already exists) is a pure
//! read that never takes a write lock or waits on a concurrent writer. The
//! insert fallback is still race-safe: a losing `ON CONFLICT DO NOTHING` no-ops
//! and both callers then read the winning row.

// Rust guideline compliant 2026-07-21

use sea_orm::entity::prelude::DateTimeWithTimeZone;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, Set};

use crate::entities::{projects, repos};
use crate::error::DbError;

/// The Repo named `repo`, inserted when the database has none.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the select or insert fails, or when the
/// row cannot be read back after a concurrent insert.
pub async fn get_or_create_repo(
    db: &impl ConnectionTrait,
    repo: &str,
    now: DateTimeWithTimeZone,
) -> Result<repos::Model, DbError> {
    if let Some(existing) = repos::Entity::find()
        .filter(repos::Column::Name.eq(repo))
        .one(db)
        .await?
    {
        return Ok(existing);
    }
    repos::Entity::insert(repos::ActiveModel {
        name: Set(repo.to_owned()),
        created_at: Set(now),
        ..Default::default()
    })
    .on_conflict(
        OnConflict::column(repos::Column::Name)
            .do_nothing()
            .to_owned(),
    )
    .exec(db)
    .await?;
    repos::Entity::find()
        .filter(repos::Column::Name.eq(repo))
        .one(db)
        .await?
        .ok_or_else(|| {
            DbError::Database(DbErr::RecordNotFound(format!(
                "Repo {repo} vanished between its insert and its read-back"
            )))
        })
}

/// The Project named `project` in the Repo `repo`, created with the Repo when
/// either is missing.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a select or insert fails, or when a row
/// cannot be read back after a concurrent insert.
pub async fn get_or_create_project(
    db: &impl ConnectionTrait,
    repo: &str,
    project: &str,
    now: DateTimeWithTimeZone,
) -> Result<projects::Model, DbError> {
    let repo_row = get_or_create_repo(db, repo, now).await?;
    if let Some(existing) = projects::Entity::find()
        .filter(projects::Column::RepoId.eq(repo_row.id))
        .filter(projects::Column::Name.eq(project))
        .one(db)
        .await?
    {
        return Ok(existing);
    }
    projects::Entity::insert(projects::ActiveModel {
        repo_id: Set(repo_row.id),
        name: Set(project.to_owned()),
        root: Set(String::new()),
        created_at: Set(now),
        ..Default::default()
    })
    .on_conflict(
        OnConflict::columns([projects::Column::RepoId, projects::Column::Name])
            .do_nothing()
            .to_owned(),
    )
    .exec(db)
    .await?;
    projects::Entity::find()
        .filter(projects::Column::RepoId.eq(repo_row.id))
        .filter(projects::Column::Name.eq(project))
        .one(db)
        .await?
        .ok_or_else(|| {
            DbError::Database(DbErr::RecordNotFound(format!(
                "Project {project} in {repo} vanished between its insert and its read-back"
            )))
        })
}
