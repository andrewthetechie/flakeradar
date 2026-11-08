//! The processor's advisory lock: the session that holds it, and the checks that
//! say whether this process still leads.
//!
//! The lock is what makes exactly one process the Report processor (ADR 0002).
//! [`LockSession`] is the seam a test substitutes; [`AdvisoryLock`] is the real
//! one, held on a dedicated connection of its own.

// Rust guideline compliant 2026-07-21

use std::fmt;

use flakeradar_db::{DbError, DbSettings, WORKER_LOCK_KEY, connect_session};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

use crate::BoxFuture;
use crate::settings::Secret;

/// The advisory lock every process tries to take to become the processor.
///
/// It answers which backend took the lock in the same statement, because a
/// session lock belongs to that backend and to nothing else.
const TRY_LOCK_SQL: &str =
    "SELECT pg_try_advisory_lock($1) AS locked, pg_backend_pid() AS backend_pid";

/// Releases the session lock the leader holds.
const UNLOCK_SQL: &str = "SELECT pg_advisory_unlock($1) AS released";

/// Asks whether the lock connection still answers as the backend that took the lock.
///
/// A replacement session answers a statement just as happily as the one the
/// lock was taken on, so the answer is the backend id rather than `SELECT 1`.
const ALIVE_SQL: &str = "SELECT pg_backend_pid() = $1 AS same_session";

/// The dedicated connection that carries the processor's advisory lock.
///
/// The real one is [`AdvisoryLock`]; a test substitutes its own to make an
/// election fail or to count attempts without a database on cue.
pub trait LockSession: fmt::Debug + Send + Sync {
    /// Tries to take the lock, answering whether this session now holds it.
    fn try_lock(&self) -> BoxFuture<'_, Result<bool, DbError>>;

    /// Proves the session that took the lock is still the one answering.
    ///
    /// A statement that fails, and one answered by any other backend, both mean
    /// the lock is gone: the server releases a session lock with the backend
    /// that held it, and the pool replaces that backend with one that holds
    /// nothing.
    fn check(&self) -> BoxFuture<'_, Result<(), DbError>>;

    /// Releases the lock and closes the session. A broken session is reported.
    fn unlock(&self) -> BoxFuture<'_, Result<(), DbError>>;
}

/// The session lock, held on a pool of exactly one connection.
///
/// The lock is session-scoped and survives a connection being returned to a
/// pool, so the leader keeps one dedicated connection for as long as it leads
/// and closes it after unlocking. The connection is opened lazily, so a standby
/// replica that never wins the election only ever holds the standby connection
/// it needs to try.
///
/// The pool still replaces a connection the server closed when it next hands it
/// out, and the replacement holds no lock, so the session also remembers which
/// backend took the lock and checks every liveness answer against it.
#[derive(Debug)]
pub struct AdvisoryLock {
    url: Secret,
    settings: DbSettings,
    session: tokio::sync::Mutex<Option<LockedSession>>,
}

/// One connection of the lock pool, and the backend that took the lock on it.
#[derive(Debug)]
struct LockedSession {
    connection: DatabaseConnection,
    /// The `pg_backend_pid()` that took the lock; `None` until an election is won.
    backend_pid: Option<i32>,
}

impl AdvisoryLock {
    /// A lock session over the pool `url` names.
    ///
    /// The URL is kept as a [`Secret`], so a `Debug` line on this lock — and on
    /// the worker that holds it — prints no database password.
    #[must_use]
    pub fn new(url: impl Into<String>, settings: DbSettings) -> Self {
        Self {
            url: Secret::new(url),
            settings,
            session: tokio::sync::Mutex::new(None),
        }
    }

    /// The dedicated session, opened on first use.
    ///
    /// A standby replica that never wins holds the one connection it tried
    /// with, so the next attempt is a query rather than a new session. The
    /// session is taken out of the guard: the caller puts it back only when it
    /// is still usable, and dropping it is what ends a broken one for good.
    async fn open(
        &self,
        guard: &mut tokio::sync::MutexGuard<'_, Option<LockedSession>>,
    ) -> Result<LockedSession, DbError> {
        match guard.take() {
            Some(session) => Ok(session),
            None => Ok(LockedSession {
                connection: connect_session(self.url.expose(), self.settings).await?,
                backend_pid: None,
            }),
        }
    }
}

impl LockSession for AdvisoryLock {
    fn try_lock(&self) -> BoxFuture<'_, Result<bool, DbError>> {
        Box::pin(async move {
            let mut guard = self.session.lock().await;
            let mut session = self.open(&mut guard).await?;
            let statement = Statement::from_sql_and_values(
                DbBackend::Postgres,
                TRY_LOCK_SQL,
                [WORKER_LOCK_KEY.into()],
            );
            // A session the server closed has already lost the lock, so it
            // must not be reused: dropping the pool ends it for good.
            let row = session
                .connection
                .query_one_raw(statement)
                .await
                .map_err(DbError::Database)?;
            // A query with no FROM clause always answers one row; the arm for a
            // missing one means "not locked".
            let Some(row) = row else {
                *guard = Some(session);
                return Ok(false);
            };
            let locked: bool = row.try_get("", "locked")?;
            if locked {
                // The lock belongs to the backend that answered this statement,
                // and every later check is compared against it.
                session.backend_pid = Some(row.try_get("", "backend_pid")?);
            }
            *guard = Some(session);
            Ok(locked)
        })
    }

    fn check(&self) -> BoxFuture<'_, Result<(), DbError>> {
        Box::pin(async move {
            let mut guard = self.session.lock().await;
            let Some(session) = guard.take() else {
                return Err(DbError::LockSessionLost {
                    detail: "this process holds no lock session".to_owned(),
                });
            };
            let Some(locked) = session.backend_pid else {
                // The session is still usable, so it stays; it just never won an
                // election, and no session that did not take the lock leads.
                *guard = Some(session);
                return Err(DbError::LockSessionLost {
                    detail: "no lock was taken on this session".to_owned(),
                });
            };
            let statement =
                Statement::from_sql_and_values(DbBackend::Postgres, ALIVE_SQL, [locked.into()]);
            match session.connection.query_one_raw(statement).await {
                // The statement answered as the backend that took the lock, so
                // this session still holds it and is kept.
                Ok(Some(row))
                    if row
                        .try_get::<bool>("", "same_session")
                        .is_ok_and(|same| same) =>
                {
                    *guard = Some(session);
                    Ok(())
                }
                // The statement failed, so the lock is lost either way. The pool
                // is not what broke: sqlx replaces a connection the server
                // closed, and it is that replacement's answer to the next check
                // which names the backend that no longer holds the lock.
                Err(error) => {
                    *guard = Some(session);
                    Err(DbError::Database(error))
                }
                // A statement that answered as another backend is a replacement
                // the pool opened over a session the server had already closed,
                // and no answer at all proves nothing either. Neither holds the
                // lock, so the session is dropped rather than kept.
                Ok(_) => Err(DbError::LockSessionLost {
                    detail: format!(
                        "the connection no longer answers as backend {locked}, which took the lock"
                    ),
                }),
            }
        })
    }

    fn unlock(&self) -> BoxFuture<'_, Result<(), DbError>> {
        Box::pin(async move {
            let Some(session) = self.session.lock().await.take() else {
                return Ok(());
            };
            let statement = Statement::from_sql_and_values(
                DbBackend::Postgres,
                UNLOCK_SQL,
                [WORKER_LOCK_KEY.into()],
            );
            // Whether the unlock worked or the session was broken, this session
            // is over; dropping it is what closes it.
            session
                .connection
                .query_one_raw(statement)
                .await
                .map(|_row| ())
                .map_err(DbError::Database)
        })
    }
}
