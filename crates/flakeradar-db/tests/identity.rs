//! Get-or-create for Repos and Projects.
//!
//! The same Repo and Project come back for repeated calls, and the hot path (the
//! row already exists) is a select only, so it never takes a write lock.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when get-or-create misbehaves"
)]

use flakeradar_db::entities::{projects, repos};
use flakeradar_db::{DbError, DbSettings, connect, get_or_create_project, get_or_create_repo};
use flakeradar_test_support::TestDb;
use sea_orm::entity::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DatabaseBackend, EntityTrait, PaginatorTrait as _, Statement};

/// A fixed instant, so a stored `created_at` never depends on the clock.
fn at() -> DateTimeWithTimeZone {
    chrono::DateTime::parse_from_rfc3339("2026-10-20T09:00:00+00:00").expect("a fixed instant")
}

#[tokio::test]
async fn get_or_create_project_is_idempotent() {
    let db = TestDb::new().await.expect("a migrated test database");
    let first = get_or_create_project(db.connection(), "acme/app", "backend", at())
        .await
        .expect("a Project");
    let again = get_or_create_project(db.connection(), "acme/app", "backend", at())
        .await
        .expect("the same Project");
    let elsewhere = get_or_create_project(db.connection(), "other/app", "backend", at())
        .await
        .expect("a Project in another Repo");

    assert_eq!(first.id, again.id);
    assert_ne!(first.id, elsewhere.id);
    assert_eq!(
        repos::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        2
    );
    assert_eq!(
        projects::Entity::find()
            .count(db.connection())
            .await
            .expect("a count"),
        2
    );
}

#[tokio::test]
async fn a_new_project_is_created_at_the_repo_root() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = get_or_create_project(db.connection(), "acme/app", "default", at())
        .await
        .expect("a Project");
    assert_eq!(project.root, "");
    assert_eq!(project.created_at, at());
}

/// Makes every `repos` and `projects` insert fail, so a fast path that
/// attempted one would error instead of quietly succeeding.
async fn block_inserts(db: &impl ConnectionTrait) {
    for statement in [
        "CREATE FUNCTION reject_insert() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN RAISE EXCEPTION 'this table is insert-blocked for this test'; END $$",
        "CREATE TRIGGER reject_repo_insert BEFORE INSERT ON repos \
         FOR EACH ROW EXECUTE FUNCTION reject_insert()",
        "CREATE TRIGGER reject_project_insert BEFORE INSERT ON projects \
         FOR EACH ROW EXECUTE FUNCTION reject_insert()",
    ] {
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            statement.to_owned(),
        ))
        .await
        .expect("the trigger is installed");
    }
}

#[tokio::test]
async fn get_or_create_project_selects_first_when_it_exists() {
    let db = TestDb::new().await.expect("a migrated test database");
    let first = get_or_create_project(db.connection(), "acme/app", "backend", at())
        .await
        .expect("a Project");
    block_inserts(db.connection()).await;

    // Neither the Repo nor the Project is inserted again, so the blocked
    // trigger never fires.
    let again = get_or_create_project(db.connection(), "acme/app", "backend", at())
        .await
        .expect("the existing Project is read, not inserted");
    assert_eq!(again.id, first.id);
}

#[tokio::test]
async fn a_blocked_insert_surfaces_as_a_database_error() {
    let db = TestDb::new().await.expect("a migrated test database");
    // Create the Repo first, so the Project call below fails on its own insert
    // rather than on the Repo's.
    get_or_create_repo(db.connection(), "acme/existing", at())
        .await
        .expect("a Repo");
    block_inserts(db.connection()).await;

    let missing_repo = get_or_create_repo(db.connection(), "acme/new", at())
        .await
        .expect_err("the Repo insert is blocked");
    assert!(matches!(missing_repo, DbError::Database(_)));

    let missing_project = get_or_create_project(db.connection(), "acme/existing", "backend", at())
        .await
        .expect_err("the Project insert is blocked");
    assert!(matches!(missing_project, DbError::Database(_)));
}

/// Makes every row an insert creates vanish again, so the read-back after the
/// insert finds nothing: the race an admin delete running on another connection
/// can win.
async fn vanish_new_rows(db: &impl ConnectionTrait) {
    for statement in [
        "CREATE FUNCTION drop_new_row() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN DELETE FROM repos WHERE id = NEW.id; DELETE FROM projects \
         WHERE id = NEW.id; RETURN NULL; END $$",
        "CREATE TRIGGER drop_new_repo AFTER INSERT ON repos \
         FOR EACH ROW EXECUTE FUNCTION drop_new_row()",
        "CREATE TRIGGER drop_new_project AFTER INSERT ON projects \
         FOR EACH ROW EXECUTE FUNCTION drop_new_row()",
    ] {
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            statement.to_owned(),
        ))
        .await
        .expect("the trigger is installed");
    }
}

#[tokio::test]
async fn a_row_vanished_between_insert_and_read_back_is_an_error() {
    let db = TestDb::new().await.expect("a migrated test database");
    // The Repo exists before the trigger, so the Project call below gets past
    // its Repo select and loses the Project row it inserted instead.
    get_or_create_repo(db.connection(), "acme/app", at())
        .await
        .expect("a Repo");
    vanish_new_rows(db.connection()).await;

    let repo_error = get_or_create_repo(db.connection(), "acme/new", at())
        .await
        .expect_err("the inserted Repo vanished before its read-back");
    assert!(
        repo_error
            .to_string()
            .contains("vanished between its insert and its read-back"),
        "{repo_error}"
    );

    let project_error = get_or_create_project(db.connection(), "acme/app", "backend", at())
        .await
        .expect_err("the inserted Project vanished before its read-back");
    assert!(
        project_error
            .to_string()
            .contains("vanished between its insert and its read-back"),
        "{project_error}"
    );
}

#[tokio::test]
async fn a_closed_connection_surfaces_as_a_database_error() {
    let db = TestDb::new().await.expect("a migrated test database");
    let connection = connect(db.url(), DbSettings::default())
        .await
        .expect("a second pool");
    // Closing a clone closes the pool both handles share.
    connection.clone().close().await.expect("the pool closes");

    let error = get_or_create_repo(&connection, "acme/app", at())
        .await
        .expect_err("a closed pool cannot select");
    assert!(matches!(error, DbError::Database(_)));
}

#[tokio::test]
async fn get_or_create_repo_selects_first_when_it_exists() {
    let db = TestDb::new().await.expect("a migrated test database");
    let first = get_or_create_repo(db.connection(), "acme/app", at())
        .await
        .expect("a Repo");
    block_inserts(db.connection()).await;

    let again = get_or_create_repo(db.connection(), "acme/app", at())
        .await
        .expect("the existing Repo is read, not inserted");
    assert_eq!(again.id, first.id);
}
