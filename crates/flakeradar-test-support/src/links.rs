//! The Issue link a Test or a Job row carries, and the stamps a test stages.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a fixture fails loudly when the row it reads is not there"
)]

use chrono::{DateTime, FixedOffset};
use flakeradar_core::EntityKind;
use sea_orm::{ConnectionTrait as _, DbBackend, Statement, Value};

use crate::TestDb;

/// The `github_issue_*` columns of one Test or Job.
///
/// One shape for either entity, so an assertion says which column it reads
/// instead of counting a tuple's positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssueLink {
    /// The linked issue, if any.
    pub number: Option<i32>,
    /// Who filed the issue: FlakeRadar or somebody else.
    pub filed: Option<bool>,
    /// When the issue-state sync last checked the issue, if it has.
    pub checked_at: Option<DateTime<FixedOffset>>,
    /// Whether GitHub reported the issue closed as not planned.
    pub closed: bool,
    /// When a fixed-fix comment was stamped, if it was.
    pub noted_at: Option<DateTime<FixedOffset>>,
}

impl IssueLink {
    /// Whether the issue-state sync has checked the issue.
    #[must_use]
    pub fn checked(&self) -> bool {
        self.checked_at.is_some()
    }

    /// Whether the fix appears to hold, so the issue was noted once.
    #[must_use]
    pub fn noted(&self) -> bool {
        self.noted_at.is_some()
    }
}

/// The table that holds `kind`'s rows.
#[must_use]
pub fn link_table(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Test => "test_cases",
        EntityKind::Job => "jobs",
    }
}

/// Reads one Test's or Job's Issue link.
pub async fn link_of(db: &TestDb, kind: EntityKind, id: i32) -> IssueLink {
    let sql = format!(
        "SELECT github_issue_number, github_issue_filed, github_issue_checked_at, \
                github_issue_closed, github_issue_fixed_noted_at \
           FROM {} WHERE id = $1",
        link_table(kind)
    );
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [Value::from(id)],
        ))
        .await
        .expect("the row reads")
        .expect("the row exists");
    IssueLink {
        number: row.try_get("", "github_issue_number").expect("a number"),
        filed: row.try_get("", "github_issue_filed").expect("filed"),
        checked_at: row.try_get("", "github_issue_checked_at").expect("a stamp"),
        closed: row.try_get("", "github_issue_closed").expect("closed"),
        noted_at: row
            .try_get("", "github_issue_fixed_noted_at")
            .expect("a stamp"),
    }
}

/// The link a test stages, built one column at a time.
///
/// It starts unlinked and unchecked, so a test states only what the case under
/// it is about: `stamp().number(11).checked()` is a link the sync has looked at.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stamp {
    number: Option<i32>,
    filed: Option<bool>,
    checked: bool,
    closed: bool,
}

/// A stamp with no link and no check.
#[must_use]
pub fn stamp() -> Stamp {
    Stamp::default()
}

impl Stamp {
    /// Links issue `number`, filed by FlakeRadar.
    #[must_use]
    pub fn number(mut self, number: i32) -> Self {
        self.number = Some(number);
        self.filed = Some(true);
        self
    }

    /// Links issue `number`, filed by somebody else.
    #[must_use]
    pub fn number_filed_elsewhere(mut self, number: i32) -> Self {
        self.number = Some(number);
        self.filed = Some(false);
        self
    }

    /// Records that the issue-state sync has checked the issue.
    #[must_use]
    pub fn checked(mut self) -> Self {
        self.checked = true;
        self
    }

    /// Records that GitHub closed the issue as not planned.
    #[must_use]
    pub fn closed(mut self) -> Self {
        self.closed = true;
        self
    }
}

/// Writes `stamp` over one Test's or Job's link columns, as an earlier pass would have.
pub async fn stamp_link(db: &TestDb, kind: EntityKind, id: i32, stamp: Stamp) {
    let sql = format!(
        "UPDATE {} SET github_issue_number = $2, github_issue_filed = $3, \
         github_issue_checked_at = CASE WHEN $4 THEN now() ELSE NULL END, \
         github_issue_closed = $5 WHERE id = $1",
        link_table(kind)
    );
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [
                Value::from(id),
                Value::from(stamp.number),
                Value::from(stamp.filed),
                Value::from(stamp.checked),
                Value::from(stamp.closed),
            ],
        ))
        .await
        .expect("the stamp writes");
}

/// Clears one Test's or Job's fixed-note stamp, as a failing Run would.
pub async fn clear_note(db: &TestDb, kind: EntityKind, id: i32) {
    let sql = format!(
        "UPDATE {} SET github_issue_fixed_noted_at = NULL WHERE id = $1",
        link_table(kind)
    );
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [Value::from(id)],
        ))
        .await
        .expect("the stamp clears");
}

/// Stamps one Test's or Job's fixed-note column, as a posted comment would.
pub async fn stamp_note(db: &TestDb, kind: EntityKind, id: i32) {
    let sql = format!(
        "UPDATE {} SET github_issue_fixed_noted_at = now() WHERE id = $1",
        link_table(kind)
    );
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [Value::from(id)],
        ))
        .await
        .expect("the fixed-note stamp writes");
}
