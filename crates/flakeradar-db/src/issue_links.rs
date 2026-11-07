//! Issue links: the GitHub issue a Test or Job points at.
//!
//! This module owns the `(github_issue_number, github_issue_checked_at)` pair
//! for both linkable entities. The REST routes and the MCP tools link and unlink
//! it, the GitHub filer stores a number it just filed, and the issue-state sync
//! takes its rotation list from [`linked_for_check`]. A linked entity is treated
//! exactly like a filed one: the filer skips it, and the sync clears the link
//! when the issue closes as completed or is gone.
//!
//! The rules live in the engines, once for both targets; only an intended
//! per-target difference is `github_provider_only` (a Job may link an issue only
//! on a GitHub Pipeline). The two targets select genuinely different row shapes,
//! which is the one place the engines switch on the target.

use chrono::{DateTime, FixedOffset, Utc};
use flakeradar_core::{
    dto::JobOut, dto::MAX_ISSUE_NUMBER, dto::TestOut, helpers::issue_url, model::EntityKind,
    py::strip_wire_whitespace,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionSession, TransactionTrait, Value};

use crate::error::DbError;
use crate::reads::{
    JOBS_FROM, JOBS_SELECT, ReadSettings, TESTS_FROM, TESTS_SELECT, job_out, optional_timestamp,
    test_out,
};
use crate::sql::{get, one, rows};

/// The table that holds `kind`'s link columns.
const fn link_table(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Test => "test_cases",
        EntityKind::Job => "jobs",
    }
}

/// Whether only a GitHub Pipeline's Job can hold a link (ADR 0008).
const fn github_provider_only(kind: EntityKind) -> bool {
    matches!(kind, EntityKind::Job)
}

/// What a link or unlink answered: the target's own response shape.
#[derive(Debug, Clone)]
pub enum LinkedOut {
    /// A Test, in the leaderboard shape.
    Test(TestOut),
    /// A Job, in the leaderboard shape.
    Job(JobOut),
}

/// A refused Issue-link operation; the text is the response `detail`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IssueLinkError {
    /// No such Test or Job.
    #[error("{0}")]
    NotFound(String),
    /// A different issue is already linked and the caller did not pass `replace`.
    #[error("{0}")]
    Conflict(String),
    /// The request breaks a link rule.
    #[error("{0}")]
    NotLinkable(String),
}

impl IssueLinkError {
    /// The response `detail` text.
    #[must_use]
    pub fn detail(&self) -> &str {
        match self {
            Self::NotFound(detail) | Self::Conflict(detail) | Self::NotLinkable(detail) => detail,
        }
    }
}

/// Everything a link engine can fail with.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    /// The link rules refused the request.
    #[error(transparent)]
    Refused(#[from] IssueLinkError),
    /// A statement failed.
    #[error(transparent)]
    Database(#[from] DbError),
}

impl From<sea_orm::DbErr> for LinkError {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::Database(DbError::Database(error))
    }
}

/// One row of the sync's rotation list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedIssue {
    /// The Test's or Job's id.
    pub entity_id: i32,
    /// The Repo name the issue lives in.
    pub repo: String,
    /// The stored issue number.
    pub number: i32,
}

/// The columns of the link pair, as a linked row holds them.
#[derive(Debug, Clone, Copy)]
struct LinkState {
    number: Option<i32>,
    checked_at: Option<DateTime<FixedOffset>>,
    closed: bool,
    fixed_noted_at: Option<DateTime<FixedOffset>>,
}

/// The number, url and conflict rules of a link.
fn check_link(
    target: EntityKind,
    current: Option<i32>,
    number: i64,
    repo: &str,
    url: Option<&str>,
    replace: bool,
) -> Result<(), IssueLinkError> {
    if !(1..=MAX_ISSUE_NUMBER).contains(&number) {
        return Err(IssueLinkError::NotLinkable(format!(
            "number must be between 1 and {MAX_ISSUE_NUMBER}"
        )));
    }
    let expected = issue_url(repo, Some(number)).unwrap_or_default();
    if let Some(url) = url {
        let normalized = strip_wire_whitespace(url)
            .trim_end_matches('/')
            .to_lowercase();
        if normalized != expected {
            return Err(IssueLinkError::NotLinkable(format!(
                "url must be {expected}: the issue must be in the {}'s own Repo",
                target.name()
            )));
        }
    }
    if let Some(current) = current
        && i64::from(current) != number
        && !replace
    {
        return Err(IssueLinkError::Conflict(format!(
            "{} already links issue #{current}; pass replace=true to replace it",
            target.name()
        )));
    }
    Ok(())
}

/// Reads the link columns of an already-selected row.
fn link_state(row: &sea_orm::QueryResult) -> Result<LinkState, DbError> {
    let number: Option<i32> = get(row, "github_issue_number")?;
    Ok(LinkState {
        number,
        checked_at: optional_timestamp(row, "checked_at")?.map(|at| at.fixed_offset()),
        closed: get(row, "closed")?,
        fixed_noted_at: optional_timestamp(row, "fixed_noted_at")?.map(|at| at.fixed_offset()),
    })
}

/// The response shape of one locked row.
fn out_for(
    target: EntityKind,
    row: &sea_orm::QueryResult,
    settings: &ReadSettings,
) -> Result<LinkedOut, DbError> {
    match target {
        EntityKind::Test => Ok(LinkedOut::Test(test_out(
            row,
            settings.flake_threshold,
            settings.broken_streak,
            None,
        )?)),
        EntityKind::Job => Ok(LinkedOut::Job(job_out(
            row,
            settings.flake_threshold,
            None,
        )?)),
    }
}

/// Applies the written link to the response shape the route answers with.
///
/// Every field comes from the state this call wrote, so a re-link of the same
/// number answers the `github_issue_closed` the row still holds.
fn patched_out(mut out: LinkedOut, repo: &str, state: LinkState) -> LinkedOut {
    let number = state.number.map(i64::from);
    match &mut out {
        LinkedOut::Test(test) => {
            test.github_issue_number = number;
            test.github_issue_url = issue_url(repo, number);
            test.github_issue_closed = state.closed;
        }
        LinkedOut::Job(job) => {
            job.github_issue_number = number;
            job.github_issue_url = if job.provider == "github" {
                issue_url(repo, number)
            } else {
                None
            };
            job.github_issue_closed = state.closed;
        }
    }
    out
}

/// Writes one target's link columns.
async fn write_link(
    db: &impl ConnectionTrait,
    target: EntityKind,
    target_id: i64,
    state: LinkState,
) -> Result<(), DbError> {
    let sql = format!(
        "UPDATE {} SET github_issue_number = $2, github_issue_checked_at = $3, \
         github_issue_filed = $4, github_issue_closed = $5, \
         github_issue_fixed_noted_at = $6 WHERE id = $1",
        link_table(target)
    );
    // A front door linked this one, so FlakeRadar must not close it.
    let filed = state.number.map(|_| false);
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        [
            Value::from(target_id),
            Value::from(state.number),
            Value::from(state.checked_at),
            Value::from(filed),
            Value::from(state.closed),
            Value::from(state.fixed_noted_at),
        ],
    ))
    .await?;
    Ok(())
}

/// Links `number` to one Test or Job: the one link engine.
///
/// Locks the row, applies the number/url/conflict rules, stores the pair and
/// answers the target's response shape.
///
/// # Errors
///
/// Returns [`LinkError::Refused`] for a 404, 409 or 422 the rules produce, and
/// [`LinkError::Database`] when a statement fails.
pub async fn link_issue(
    db: &(impl ConnectionTrait + TransactionTrait),
    target: EntityKind,
    target_id: i64,
    number: i64,
    url: Option<&str>,
    replace: bool,
    settings: &ReadSettings,
) -> Result<LinkedOut, LinkError> {
    let transaction = db.begin().await?;
    let row = one(
        &transaction,
        locked_select_sql(target),
        vec![Value::from(target_id)],
    )
    .await?;
    let Some(row) = row else {
        transaction.rollback().await?;
        return Err(IssueLinkError::NotFound(format!("{} not found", target.name())).into());
    };
    let repo: String = get(&row, "repo_name")?;
    if github_provider_only(target) {
        let provider: String = get(&row, "provider")?;
        if provider != "github" {
            transaction.rollback().await?;
            return Err(IssueLinkError::NotLinkable(
                "Only Jobs of GitHub Pipelines can link a GitHub issue".to_owned(),
            )
            .into());
        }
    }
    let current = link_state(&row)?;
    if let Err(error) = check_link(target, current.number, number, &repo, url, replace) {
        transaction.rollback().await?;
        return Err(error.into());
    }
    let stored = if current.number == Some(stored_number(number)) {
        // The same number keeps its check stamp: it was just checked.
        current
    } else {
        // A new link is checked first, is FlakeRadar's own for a moment, points
        // at an open issue, and has never been noted.
        LinkState {
            number: Some(stored_number(number)),
            checked_at: None,
            closed: false,
            fixed_noted_at: None,
        }
    };
    write_link(&transaction, target, target_id, stored).await?;
    transaction.commit().await?;
    tracing::info!(
        name: "db.issue_link.linked",
        repo = %repo,
        number,
        target = target.as_str(),
        entity_id = target_id,
        previous = ?current.number,
        "linked a GitHub issue"
    );
    Ok(patched_out(out_for(target, &row, settings)?, &repo, stored))
}

/// Removes one Test's or Job's link: the one unlink engine.
///
/// # Errors
///
/// Returns [`LinkError::Refused`] for an unknown Test or Job and
/// [`LinkError::Database`] when a statement fails.
pub async fn unlink_issue(
    db: &(impl ConnectionTrait + TransactionTrait),
    target: EntityKind,
    target_id: i64,
    settings: &ReadSettings,
) -> Result<LinkedOut, LinkError> {
    let transaction = db.begin().await?;
    let row = one(
        &transaction,
        locked_select_sql(target),
        vec![Value::from(target_id)],
    )
    .await?;
    let Some(row) = row else {
        transaction.rollback().await?;
        return Err(IssueLinkError::NotFound(format!("{} not found", target.name())).into());
    };
    let repo: String = get(&row, "repo_name")?;
    let previous = link_state(&row)?.number;
    // Both halves go unconditionally: with no number there is nothing for the
    // rotation to look up, so a stamp left behind only misleads.
    let cleared = LinkState {
        number: None,
        checked_at: None,
        closed: false,
        fixed_noted_at: None,
    };
    write_link(&transaction, target, target_id, cleared).await?;
    transaction.commit().await?;
    if let Some(previous) = previous {
        tracing::info!(
            name: "db.issue_link.unlinked",
            repo = %repo,
            number = previous,
            target = target.as_str(),
            entity_id = target_id,
            "unlinked a GitHub issue"
        );
    }
    Ok(patched_out(
        out_for(target, &row, settings)?,
        &repo,
        cleared,
    ))
}

/// A number the `INTEGER` column can hold.
fn stored_number(number: i64) -> i32 {
    // The range rule above narrowed this to 1..=MAX_ISSUE_NUMBER, and
    // MAX_ISSUE_NUMBER is i32::MAX.
    i32::try_from(number).unwrap_or(i32::MAX)
}

/// The locked select of one target, as SQL.
fn locked_select_sql(target: EntityKind) -> String {
    match target {
        EntityKind::Test => format!(
            "{TESTS_SELECT}, t.github_issue_checked_at AS checked_at, \
             t.github_issue_closed AS closed, \
             t.github_issue_fixed_noted_at AS fixed_noted_at \
             {TESTS_FROM} WHERE t.id = $1 FOR UPDATE OF t"
        ),
        EntityKind::Job => format!(
            "{JOBS_SELECT}, j.github_issue_checked_at AS checked_at, \
             j.github_issue_closed AS closed, \
             j.github_issue_fixed_noted_at AS fixed_noted_at \
             {JOBS_FROM} WHERE j.id = $1 FOR UPDATE OF j"
        ),
    }
}

/// This target's linked rows in check order, with their Repo names.
///
/// One statement of the rule the two partial indexes encode: never checked
/// (or just re-linked) first, then least recently checked. No row lock, because
/// the sync is leader-only maintenance.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn linked_for_check(
    db: &impl ConnectionTrait,
    target: EntityKind,
    batch: i64,
) -> Result<Vec<LinkedIssue>, DbError> {
    let sql = match target {
        EntityKind::Test => {
            "SELECT t.id AS id, t.github_issue_number AS number, r.name AS repo_name \
             FROM test_cases t JOIN projects p ON p.id = t.project_id \
             JOIN repos r ON r.id = p.repo_id \
             WHERE t.github_issue_number IS NOT NULL \
             ORDER BY t.github_issue_checked_at ASC NULLS FIRST, t.id LIMIT $1"
        }
        EntityKind::Job => {
            "SELECT j.id AS id, j.github_issue_number AS number, r.name AS repo_name \
             FROM jobs j JOIN pipelines p ON p.id = j.pipeline_id \
             JOIN repos r ON r.id = p.repo_id \
             WHERE j.github_issue_number IS NOT NULL \
             ORDER BY j.github_issue_checked_at ASC NULLS FIRST, j.id LIMIT $1"
        }
    };
    let mut linked = Vec::new();
    for row in rows(db, sql, vec![Value::from(batch)]).await? {
        linked.push(LinkedIssue {
            entity_id: get(&row, "id")?,
            repo: get(&row, "repo_name")?,
            number: get(&row, "number")?,
        });
    }
    Ok(linked)
}

/// Stores the number GitHub just created: the one write of a brand-new link.
///
/// The check stamp is cleared, the link is marked as FlakeRadar's own, the
/// not-planned flag reset and the fixed-note stamp cleared, because a fresh
/// Issue points at an open issue with no comment on it yet.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn store_filed_link(
    db: &impl ConnectionTrait,
    target: EntityKind,
    target_id: i64,
    number: i32,
) -> Result<u64, DbError> {
    let sql = format!(
        "UPDATE {} SET github_issue_number = $2, github_issue_checked_at = NULL, \
         github_issue_filed = TRUE, github_issue_closed = FALSE, \
         github_issue_fixed_noted_at = NULL WHERE id = $1",
        link_table(target)
    );
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [Value::from(target_id), Value::from(number)],
        ))
        .await?;
    Ok(result.rows_affected())
}

/// What one answered sync check decided about the row's link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// The answer changed nothing: the link keeps its number and its flags.
    Kept,
    /// The issue is closed or gone: clear the link so the next Report files again.
    Rearmed,
    /// The issue was closed as not planned: keep the link and flag the row.
    Declined,
    /// A declined issue was reopened, so filing is live again.
    Reopened,
}

/// Records one sync check in a single statement, guarded on the number it checked.
///
/// The `GET` that produced the decision runs with no statement open, so the row can
/// change under it: a re-link, an unlink or a fresh filing clears or replaces
/// `github_issue_number`. This write therefore applies only while the row still
/// holds the number that was checked, so a link replaced mid-`GET` survives, and the
/// replacement is not stamped as checked on the old number's answer.
///
/// The stamp lands for every answer, whatever it said, which is what keeps a link
/// whose answer stays unusable rotating instead of pinning the front of the queue;
/// a caller leaves a rate-limited or unanswered row unwritten so it is retried first.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn record_check(
    db: &impl ConnectionTrait,
    target: EntityKind,
    target_id: i64,
    number: i32,
    check: Check,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let mut set = vec!["github_issue_checked_at = $3"];
    match check {
        Check::Kept => {}
        Check::Rearmed => set.extend([
            "github_issue_number = NULL",
            "github_issue_filed = NULL",
            "github_issue_closed = FALSE",
            "github_issue_fixed_noted_at = NULL",
        ]),
        Check::Declined => set.push("github_issue_closed = TRUE"),
        Check::Reopened => set.push("github_issue_closed = FALSE"),
    }
    let sql = format!(
        "UPDATE {} SET {} WHERE id = $1 AND github_issue_number = $2",
        link_table(target),
        set.join(", ")
    );
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [Value::from(target_id), Value::from(number), Value::from(at)],
        ))
        .await?;
    Ok(result.rows_affected() > 0)
}

/// What a conditional fixed-note stamp found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteStamp {
    /// Whether the stamp landed.
    pub stamped: bool,
    /// The row's state when the stamp did not land, or `None` when the row is gone.
    pub state: Option<NoteRowState>,
}

/// The two row values a refused fixed-note stamp reports.
///
/// A named pair, so a caller cannot read the issue number as the streak: the two
/// are both integers, so swapping them in a tuple would compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteRowState {
    /// The Clean streak the row held when the stamp was refused.
    pub clean_streak: i32,
    /// The issue the row is linked to, when it still holds one.
    pub issue_number: Option<i32>,
}

/// Lands the fixed-note stamp only while the row is still noteable.
///
/// The comment POST hands the row over to whoever can change it first: a failure
/// the processor handles mid-POST resets the Clean streak, and a re-armed or
/// replaced link clears the stamp. Stamping unconditionally would undo that
/// reset, so the write is one conditional UPDATE on the state the note was
/// selected for, which PostgreSQL re-checks against the row as of the write.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn stamp_fixed_note(
    db: &impl ConnectionTrait,
    target: EntityKind,
    target_id: i64,
    number: i32,
    threshold: i32,
    at: DateTime<Utc>,
) -> Result<NoteStamp, DbError> {
    let sql = format!(
        "UPDATE {} SET github_issue_fixed_noted_at = $4 WHERE id = $1 \
         AND github_issue_fixed_noted_at IS NULL AND github_issue_number = $2 \
         AND clean_streak >= $3",
        link_table(target)
    );
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [
                Value::from(target_id),
                Value::from(number),
                Value::from(threshold),
                Value::from(at),
            ],
        ))
        .await?;
    if result.rows_affected() > 0 {
        return Ok(NoteStamp {
            stamped: true,
            state: None,
        });
    }
    let sql = format!(
        "SELECT clean_streak, github_issue_number FROM {} WHERE id = $1",
        link_table(target)
    );
    let state = one(db, sql, vec![Value::from(target_id)]).await?;
    let state = match state {
        Some(row) => Some(NoteRowState {
            clean_streak: get(&row, "clean_streak")?,
            issue_number: get(&row, "github_issue_number")?,
        }),
        None => None,
    };
    Ok(NoteStamp {
        stamped: false,
        state,
    })
}
