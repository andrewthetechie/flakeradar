//! The read services the REST routes and the MCP server share.
//!
//! Every function takes the connection and returns a DTO from `flakeradar-core`,
//! so both front doors expose exactly the same data. The rules are the frozen
//! Python server's `queries.py`, `score_history.py` and `attribution.py`; the
//! statements keep the shapes recorded in `spike-notes.md`, including the
//! `LATERAL ... LIMIT` lookups the leaderboard filters with.
//!
//! The leaderboard is one engine with a descriptor per entity, exactly as
//! `_list_leaderboard` is: the threshold rule, the stable-hiding default, search,
//! the issue filter and the new-flaky window are stated once, and only an
//! intended per-entity difference is descriptor data.

// Rust guideline compliant 2026-07-21

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use flakeradar_core::{
    CiRunFailedJobOut, CiRunFailuresOut, CiRunTestFailureOut, ExecutionOut, ExplainingTestOut,
    FailedJobOutcome, FailureCategory, HistoryOut, JobExecutionOut, JobHistoryOut, JobOut,
    JobOutcome, JobPage, JobSummaryOut, LocationOut, QuarantineItem, RepoOut, ReportCountsOut,
    RunnerStatOut, ScorePointOut, SummaryOut, TestJobLinkOut, TestOut, TestPage, Trend,
    escape_like, is_broken, issue_url, permalink, repo_path, tier_for, trend_for,
};
use sea_orm::{ConnectionTrait, DbBackend, DbErr, QueryResult, Statement, TryGetable, Value};
use serde_json::Value as JsonValue;

use crate::attribution::{
    explained_ci_job_ids, int_array, job_execution_explained_sql, job_window_ids_overall_sql,
    summary_explaining_siblings,
};
use crate::clock::SharedClock;
use crate::error::DbError;
use crate::score_history::{self, HistoryKind};

/// The dashboard tiles' "became flaky recently" window.
pub const NEW_FLAKY_DAYS: i64 = 7;

/// The Score-history window the dashboard tiles sum over.
pub const SUMMARY_WINDOW_DAYS: i64 = 30;

/// Groups `get_job` returns, worst unexplained rate first.
pub const RUNNER_STATS_LIMIT: usize = 10;

/// How many failing Tests one failed Job execution lists.
pub const EXPLAINED_TESTS_MAX: usize = 20;

/// The most failures one CI run attempt lists.
pub const FAILURES_LIMIT: i64 = 500;

/// The most characters of a failure message a CI run listing keeps.
pub const CI_RUN_MESSAGE_MAX: usize = 1024;

/// How many history rows a detail response embeds by default.
pub const DEFAULT_EXECUTIONS_LIMIT: i64 = 60;

/// The largest `limit` a detail response accepts.
pub const MAX_EXECUTIONS_LIMIT: i64 = 500;

/// The values a read needs that the configuration owns.
#[derive(Clone)]
pub struct ReadSettings {
    /// The Flakiness score at or above which a Test or Job counts as flaky.
    pub flake_threshold: f64,
    /// Consecutive failures at which a Test counts as broken; `0` disables it.
    pub broken_streak: i64,
    /// How many recent Executions the runner statistics weigh.
    pub score_window: usize,
    /// Where "today" comes from, so a test controls the history cut-offs.
    pub clock: SharedClock,
}

impl std::fmt::Debug for ReadSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadSettings")
            .field("flake_threshold", &self.flake_threshold)
            .field("broken_streak", &self.broken_streak)
            .field("score_window", &self.score_window)
            .finish_non_exhaustive()
    }
}

impl Default for ReadSettings {
    fn default() -> Self {
        Self {
            flake_threshold: 0.30,
            broken_streak: 5,
            score_window: 50,
            clock: std::sync::Arc::new(crate::clock::SystemClock),
        }
    }
}

impl ReadSettings {
    /// The UTC day the history reads are relative to.
    fn today(&self) -> NaiveDate {
        self.clock.today()
    }
}

/// Which Tests a query covers.
///
/// `project` requires `repo`, because the same Project name exists in other
/// Repos; a caller that gives only a Project is refused before any query runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    /// The Repo's `owner/name`, when the scope names one.
    pub repo: Option<String>,
    /// The Project's name, which needs `repo`.
    pub project: Option<String>,
}

impl Scope {
    /// A scope for a Repo and optionally a Project.
    ///
    /// # Errors
    ///
    /// Returns the message the frozen server's `Scope.__post_init__` raised when
    /// a Project is given without a Repo.
    pub fn new(repo: Option<String>, project: Option<String>) -> Result<Self, String> {
        if project.is_some() && repo.is_none() {
            return Err("project filter requires repo".to_owned());
        }
        Ok(Self { repo, project })
    }
}

/// The sort orders both leaderboards share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    /// Worst score first.
    #[default]
    Score,
    /// Most recent first.
    LastSeen,
    /// Most Proven flakes first.
    Proven,
}

impl SortKey {
    /// The spellings the query parameter takes, in the order the frozen server's
    /// `Literal` listed them.
    pub const SPELLINGS: [&'static str; 3] = ["score", "last_seen", "proven"];

    /// Parses the query parameter's value.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "score" => Some(Self::Score),
            "last_seen" => Some(Self::LastSeen),
            "proven" => Some(Self::Proven),
            _ => None,
        }
    }
}

/// The errors a read can answer with.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    /// The statement failed.
    #[error(transparent)]
    Database(#[from] DbError),
    /// The Repo, CI run or attempt is unknown; the text is the 404 detail.
    #[error("{0}")]
    NotFound(String),
}

/// Reads one column of a raw row.
fn get<T: TryGetable>(row: &QueryResult, column: &str) -> Result<T, DbError> {
    row.try_get("", column)
        .map_err(|error: DbErr| DbError::Database(error))
}

/// Runs a statement and reads every row.
async fn rows(
    db: &impl ConnectionTrait,
    sql: String,
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

/// Runs a statement and reads its first row.
async fn one(
    db: &impl ConnectionTrait,
    sql: String,
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

/// The WHERE conditions of one statement, with the binds they need.
#[derive(Debug, Default)]
struct Where {
    conditions: Vec<String>,
    values: Vec<Value>,
}

impl Where {
    /// The placeholder the next bind uses.
    fn next(&self) -> String {
        format!("${}", self.values.len() + 1)
    }

    /// Adds a condition with no bind of its own.
    fn raw(&mut self, condition: impl Into<String>) {
        self.conditions.push(condition.into());
    }

    /// Adds `column = $n` and binds the value.
    fn eq(&mut self, column: &str, value: Value) {
        let placeholder = self.next();
        self.conditions.push(format!("{column} = {placeholder}"));
        self.values.push(value);
    }

    /// The ` WHERE ...` suffix, or nothing when no condition was added.
    fn suffix(&self) -> String {
        if self.conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", self.conditions.join(" AND "))
        }
    }
}

/// Adds the Scope's conditions against the given Repo and Project aliases.
fn scope_conditions(scope: &Scope, repo_column: &str, project_column: &str, filter: &mut Where) {
    if let Some(repo) = &scope.repo {
        filter.eq(repo_column, Value::from(repo.clone()));
    }
    if let Some(project) = &scope.project {
        filter.eq(project_column, Value::from(project.clone()));
    }
}

/// The SQL a `new_within_days` filter adds: the per-row newest old history point.
fn newest_history_join(kind: HistoryKind, alias: &str, cutoff: NaiveDate) -> String {
    let lateral = score_history::newest_history_before(kind, alias)
        .replace("$1::DATE", &format!("DATE '{}'", cutoff.format("%Y-%m-%d")));
    format!(" LEFT JOIN LATERAL ({lateral}) AS past ON true")
}

/// A timestamp column read as UTC.
fn timestamp(row: &QueryResult, column: &str) -> Result<DateTime<Utc>, DbError> {
    let value: DateTime<FixedOffset> = get(row, column)?;
    Ok(value.with_timezone(&Utc))
}

/// An optional timestamp column read as UTC.
fn optional_timestamp(row: &QueryResult, column: &str) -> Result<Option<DateTime<Utc>>, DbError> {
    let value: Option<DateTime<FixedOffset>> = get(row, column)?;
    Ok(value.map(|value| value.with_timezone(&Utc)))
}

/// Converts a stored `INTEGER` id to the DTO's `i64`.
fn id(value: i32) -> i64 {
    i64::from(value)
}

/// The leaderboard descriptor: what the one engine needs to know about an entity.
struct Board<'a> {
    /// The Score-history table the past-score lookups use.
    kind: HistoryKind,
    /// The entity's table alias in `select`.
    alias: &'static str,
    /// The `SELECT` list, ending with the entity's own columns.
    select: &'static str,
    /// The `FROM` clause, joining the entity to its parents.
    from: &'static str,
    /// The conditions the wrapper scoped and filtered with.
    filter: Where,
    /// The search condition, with `$P` standing for the pattern bind.
    search: &'static str,
    /// The entity's GitHub issue column.
    issue_column: &'static str,
    /// Whether the entity's listing keeps stable rows for its own filters.
    keeps_stable_rows: bool,
    /// Whether the caller asked for stable rows.
    include_stable: bool,
    /// Whether only flaky rows may be returned (the MCP "flaky only" listing).
    flaky_only: bool,
    /// The sort order.
    sort: SortKey,
    /// The page number, from 1.
    page: i64,
    /// How many rows a page holds.
    page_size: i64,
    /// The search text, when the caller gave a non-empty one.
    q: Option<&'a str>,
    /// Whether the caller filtered on having a linked issue.
    has_issue: Option<bool>,
    /// The window, in days, of the "became flaky recently" filter.
    new_within_days: Option<i64>,
    /// The Flake threshold.
    threshold: f64,
    /// The UTC day the history cut-offs are relative to.
    today: NaiveDate,
}

/// What one page of the engine returned.
struct BoardPage {
    rows: Vec<QueryResult>,
    total: i64,
}

impl Board<'_> {
    /// Runs the count and the page over the filters, in the one place the shared
    /// rules are stated.
    async fn run(self, db: &impl ConnectionTrait) -> Result<BoardPage, DbError> {
        let mut conditions = self.filter;

        let mut joins = String::new();
        if self.flaky_only || self.new_within_days.is_some() {
            let placeholder = conditions.next();
            conditions.raw(format!("{}.flakiness_score >= {placeholder}", self.alias));
            conditions.values.push(Value::from(self.threshold));
        } else if !self.include_stable && self.q.is_none() && !self.keeps_stable_rows {
            // The stable-hiding default. Kept as descriptor data by the caller.
            conditions.raw(format!("{}.flakiness_score > 0", self.alias));
        }

        if let Some(text) = self.q {
            let placeholder = conditions.next();
            conditions.raw(self.search.replace("$P", &placeholder));
            conditions
                .values
                .push(Value::from(format!("%{}%", escape_like(text))));
        }

        if let Some(wanted) = self.has_issue {
            conditions.raw(if wanted {
                format!("{} IS NOT NULL", self.issue_column)
            } else {
                format!("{} IS NULL", self.issue_column)
            });
        }

        if let Some(days) = self.new_within_days {
            joins = newest_history_join(self.kind, self.alias, self.today - Duration::days(days));
            let placeholder = conditions.next();
            conditions.raw(format!(
                "(past.score IS NULL OR past.score < {placeholder})"
            ));
            conditions.values.push(Value::from(self.threshold));
        }

        let base = format!(
            "{} {}{}{}",
            self.select,
            self.from,
            joins,
            conditions.suffix()
        );
        let count_sql = format!("SELECT count(*) AS total FROM ({base}) AS matches");
        let count = one(db, count_sql, conditions.values.clone()).await?;
        let total: i64 = match count {
            Some(row) => get(&row, "total")?,
            None => 0,
        };

        let order = match self.sort {
            SortKey::Score => "flakiness_score DESC, last_seen_at DESC, id",
            SortKey::LastSeen => "last_seen_at DESC, id DESC",
            SortKey::Proven => "confirmed_flake_count DESC, flakiness_score DESC, id",
        };
        let limit = conditions.next();
        let mut values = conditions.values;
        values.push(Value::from(self.page_size));
        let offset = format!("${}", values.len() + 1);
        values.push(Value::from((self.page - 1) * self.page_size));
        let page_sql = format!("{base} ORDER BY {order} LIMIT {limit} OFFSET {offset}");
        let page = rows(db, page_sql, values).await?;
        Ok(BoardPage { rows: page, total })
    }
}

/// The row shape both leaderboards select: the entity's columns, then the names.
const TESTS_SELECT: &str = "SELECT t.*, p.name AS project_name, r.name AS repo_name";
const TESTS_FROM: &str = "FROM test_cases t \
     JOIN projects p ON p.id = t.project_id \
     JOIN repos r ON r.id = p.repo_id";

const JOBS_SELECT: &str = "SELECT j.*, p.name AS pipeline_name, p.provider AS provider, \
     p.repo_id AS repo_id, r.name AS repo_name";
const JOBS_FROM: &str = "FROM jobs j \
     JOIN pipelines p ON p.id = j.pipeline_id \
     JOIN repos r ON r.id = p.repo_id";

/// The case-insensitive literal substring search over a Test's identity.
const TEST_SEARCH: &str = "(t.name ILIKE $P ESCAPE '\\' \
     OR t.classname ILIKE $P ESCAPE '\\' \
     OR t.file ILIKE $P ESCAPE '\\')";

/// The case-insensitive literal substring search over a Job's name and Pipeline.
const JOB_SEARCH: &str = "(j.name ILIKE $P ESCAPE '\\' OR p.name ILIKE $P ESCAPE '\\')";

/// Every Repo with its Projects, ordered by Repo name then Project name.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn list_repos(db: &impl ConnectionTrait) -> Result<Vec<RepoOut>, DbError> {
    let sql = "SELECT r.name AS repo, p.name AS project, p.root AS root \
                 FROM projects p JOIN repos r ON r.id = p.repo_id \
                ORDER BY r.name, p.name";
    let mut repos: Vec<RepoOut> = Vec::new();
    for row in rows(db, sql.to_owned(), Vec::new()).await? {
        let repo: String = get(&row, "repo")?;
        let project = flakeradar_core::ProjectOut {
            name: get(&row, "project")?,
            root: get(&row, "root")?,
        };
        match repos.last_mut() {
            Some(last) if last.name == repo => last.projects.push(project),
            _ => repos.push(RepoOut {
                name: repo,
                projects: vec![project],
            }),
        }
    }
    Ok(repos)
}

/// Reads one Test row into its response shape.
fn test_out(
    row: &QueryResult,
    threshold: f64,
    broken_streak: i64,
    trend: Option<Trend>,
) -> Result<TestOut, DbError> {
    let score: f64 = get(row, "flakiness_score")?;
    let repo: String = get(row, "repo_name")?;
    let issue_number: Option<i32> = get(row, "github_issue_number")?;
    let issue_number = issue_number.map(id);
    let fail_streak = i64::from(get::<i32>(row, "fail_streak")?);
    let source_url = issue_url(&repo, issue_number);
    Ok(TestOut {
        id: id(get(row, "id")?),
        repo,
        project: get(row, "project_name")?,
        fingerprint: get(row, "fingerprint")?,
        suite: get(row, "suite")?,
        classname: get(row, "classname")?,
        name: get(row, "name")?,
        file: get(row, "file")?,
        line: get::<Option<i32>>(row, "line")?.map(id),
        flakiness_score: score,
        tier: tier_for(score, threshold),
        confirmed_flake_count: i64::from(get::<i32>(row, "confirmed_flake_count")?),
        failure_category: get(row, "failure_category")?,
        clean_streak: i64::from(get::<i32>(row, "clean_streak")?),
        fail_streak,
        broken: is_broken(fail_streak, broken_streak),
        trend,
        last_status: get(row, "last_status")?,
        last_seen_at: timestamp(row, "last_seen_at")?,
        quarantined: get(row, "quarantined")?,
        quarantined_at: optional_timestamp(row, "quarantined_at")?,
        quarantine_reason: get(row, "quarantine_reason")?,
        quarantine_expires_at: optional_timestamp(row, "quarantine_expires_at")?,
        github_issue_number: issue_number,
        github_issue_url: source_url,
        github_issue_closed: get(row, "github_issue_closed")?,
    })
}

/// Reads one Job row into its response shape.
fn job_out(row: &QueryResult, threshold: f64, trend: Option<Trend>) -> Result<JobOut, DbError> {
    let score: f64 = get(row, "flakiness_score")?;
    let repo: String = get(row, "repo_name")?;
    let provider: String = get(row, "provider")?;
    let issue_number: Option<i32> = get(row, "github_issue_number")?;
    let issue_number = issue_number.map(id);
    let github_issue_url = job_issue_url(&repo, &provider, issue_number);
    Ok(JobOut {
        id: id(get(row, "id")?),
        repo,
        provider,
        pipeline: get(row, "pipeline_name")?,
        name: get(row, "name")?,
        flakiness_score: score,
        tier: tier_for(score, threshold),
        confirmed_flake_count: i64::from(get::<i32>(row, "confirmed_flake_count")?),
        clean_streak: i64::from(get::<i32>(row, "clean_streak")?),
        trend,
        last_status: get(row, "last_status")?,
        last_seen_at: timestamp(row, "last_seen_at")?,
        summary: get(row, "summary")?,
        github_issue_number: issue_number,
        github_issue_url,
        github_issue_closed: get(row, "github_issue_closed")?,
    })
}

/// The GitHub issue a Job was filed as; only GitHub Pipelines file issues.
fn job_issue_url(repo: &str, provider: &str, number: Option<i64>) -> Option<String> {
    if provider == "github" {
        issue_url(repo, number)
    } else {
        None
    }
}

/// The Tests-only listing filters the engine must not know about.
#[derive(Debug, Clone, Default)]
pub struct TestListFilters {
    /// A literal substring of the Test's file.
    pub file: Option<String>,
    /// The Failure category to list.
    pub category: Option<FailureCategory>,
    /// The quarantine state to list; `None` lists both.
    pub quarantined: Option<bool>,
    /// Whether to list only broken Tests.
    pub broken: bool,
}

/// The Jobs-only listing filters the engine must not know about.
#[derive(Debug, Clone, Default)]
pub struct JobListFilters {
    /// The exact Pipeline name to list.
    pub pipeline: Option<String>,
}

/// The paging, sorting and searching a leaderboard request asks for.
#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    /// Whether to include rows with a zero score.
    pub include_stable: bool,
    /// Whether only flaky rows may be returned.
    pub flaky_only: bool,
    /// The sort order.
    pub sort: SortKey,
    /// The page number, from 1.
    pub page: i64,
    /// How many rows a page holds.
    pub page_size: i64,
    /// The search text; an empty string behaves as none.
    pub q: Option<String>,
    /// Whether the entity has a linked issue.
    pub has_issue: Option<bool>,
    /// The window, in days, of the "became flaky recently" filter.
    pub new_within_days: Option<i64>,
}

/// The Tests leaderboard.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn list_tests(
    db: &impl ConnectionTrait,
    scope: &Scope,
    filters: &TestListFilters,
    options: &ListOptions,
    settings: &ReadSettings,
) -> Result<TestPage, DbError> {
    let mut filter = Where::default();
    scope_conditions(scope, "r.name", "p.name", &mut filter);
    if let Some(file) = filters.file.as_deref().filter(|file| !file.is_empty()) {
        let placeholder = filter.next();
        filter.raw(format!("t.file ILIKE {placeholder} ESCAPE '\\'"));
        filter
            .values
            .push(Value::from(format!("%{}%", escape_like(file))));
    }
    if let Some(category) = filters.category {
        filter.eq(
            "t.failure_category",
            Value::from(category.as_str().to_owned()),
        );
    }
    if let Some(quarantined) = filters.quarantined {
        filter.eq("t.quarantined", Value::from(quarantined));
    }
    if filters.broken {
        if settings.broken_streak > 0 {
            let placeholder = filter.next();
            filter.raw(format!("t.fail_streak >= {placeholder}"));
            filter.values.push(Value::from(settings.broken_streak));
        } else {
            // Detection disabled: `is_broken` is false for every row, so nothing is broken.
            filter.raw("FALSE");
        }
    }
    let board = Board {
        kind: HistoryKind::Test,
        alias: "t",
        select: TESTS_SELECT,
        from: TESTS_FROM,
        filter,
        search: TEST_SEARCH,
        issue_column: "t.github_issue_number",
        keeps_stable_rows: filters.quarantined == Some(true) || filters.broken,
        include_stable: options.include_stable,
        flaky_only: options.flaky_only,
        sort: options.sort,
        page: options.page,
        page_size: options.page_size,
        q: options.q.as_deref().filter(|text| !text.is_empty()),
        has_issue: options.has_issue,
        new_within_days: options.new_within_days,
        threshold: settings.flake_threshold,
        today: settings.today(),
    };
    let page = board.run(db).await?;
    let ids: Vec<i32> = page
        .rows
        .iter()
        .map(|row| get::<i32>(row, "id"))
        .collect::<Result<_, _>>()?;
    let past = score_history::past_scores(db, HistoryKind::Test, &ids, settings.today()).await?;
    let mut items = Vec::with_capacity(page.rows.len());
    for row in &page.rows {
        let test_id: i32 = get(row, "id")?;
        let row_score: f64 = get(row, "flakiness_score")?;
        items.push(test_out(
            row,
            settings.flake_threshold,
            settings.broken_streak,
            trend_for(row_score, past.get(&test_id).copied()),
        )?);
    }
    Ok(TestPage {
        items,
        total: page.total,
        page: options.page,
        page_size: options.page_size,
    })
}

/// The Jobs leaderboard.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn list_jobs(
    db: &impl ConnectionTrait,
    repo: Option<&str>,
    filters: &JobListFilters,
    options: &ListOptions,
    settings: &ReadSettings,
) -> Result<JobPage, DbError> {
    let mut filter = Where::default();
    if let Some(repo) = repo {
        filter.eq("r.name", Value::from(repo.to_owned()));
    }
    if let Some(pipeline) = &filters.pipeline {
        filter.eq("p.name", Value::from(pipeline.clone()));
    }
    let board = Board {
        kind: HistoryKind::Job,
        alias: "j",
        select: JOBS_SELECT,
        from: JOBS_FROM,
        filter,
        search: JOB_SEARCH,
        issue_column: "j.github_issue_number",
        // ADR 0004: Jobs have no Quarantine, so no Job listing keeps stable rows.
        keeps_stable_rows: false,
        include_stable: options.include_stable,
        flaky_only: options.flaky_only,
        sort: options.sort,
        page: options.page,
        page_size: options.page_size,
        q: options.q.as_deref().filter(|text| !text.is_empty()),
        has_issue: options.has_issue,
        new_within_days: options.new_within_days,
        threshold: settings.flake_threshold,
        today: settings.today(),
    };
    let page = board.run(db).await?;
    let ids: Vec<i32> = page
        .rows
        .iter()
        .map(|row| get::<i32>(row, "id"))
        .collect::<Result<_, _>>()?;
    let past = score_history::past_scores(db, HistoryKind::Job, &ids, settings.today()).await?;
    let mut items = Vec::with_capacity(page.rows.len());
    for row in &page.rows {
        let job_id: i32 = get(row, "id")?;
        let score: f64 = get(row, "flakiness_score")?;
        items.push(job_out(
            row,
            settings.flake_threshold,
            trend_for(score, past.get(&job_id).copied()),
        )?);
    }
    Ok(JobPage {
        items,
        total: page.total,
        page: options.page,
        page_size: options.page_size,
    })
}

/// The scope's conditions for one statement, with placeholders from `start`.
fn scope_filters(
    scope: &Scope,
    repo_column: &str,
    project_column: &str,
    start: usize,
) -> (Vec<String>, Vec<Value>) {
    let mut conditions = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    if let Some(repo) = &scope.repo {
        conditions.push(format!("{repo_column} = ${}", start + values.len()));
        values.push(Value::from(repo.clone()));
    }
    if let Some(project) = &scope.project {
        conditions.push(format!("{project_column} = ${}", start + values.len()));
        values.push(Value::from(project.clone()));
    }
    (conditions, values)
}

/// A `WHERE` suffix over already-built conditions.
fn where_suffix(conditions: &[String]) -> String {
    if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    }
}

/// The dashboard's headline counts for the Tests in scope.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn summary(
    db: &impl ConnectionTrait,
    scope: &Scope,
    settings: &ReadSettings,
) -> Result<SummaryOut, DbError> {
    let threshold = settings.flake_threshold;
    let today = settings.today();
    let counts = test_counts(db, scope, threshold).await?;
    let new_flaky = new_flaky_count(db, scope, threshold, today).await?;
    let (executions_30d, failures_30d) = window_sums(db, scope, today).await?;
    Ok(SummaryOut {
        total_tests: counts.0,
        flaky_tests: counts.1,
        suspect_tests: counts.2,
        confirmed_flaky_tests: counts.3,
        new_flaky_7d: new_flaky,
        total_runs: counts.4,
        executions_30d,
        failures_30d,
        flake_threshold: threshold,
        category_counts: category_counts(db, scope).await?,
    })
}

/// The five dashboard tile counts and the Run count, in one statement.
async fn test_counts(
    db: &impl ConnectionTrait,
    scope: &Scope,
    threshold: f64,
) -> Result<(i64, i64, i64, i64, i64), DbError> {
    // `$1` is the threshold everywhere; the Runs subquery and the scope follow.
    let (runs_conditions, runs_values) = scope_filters(scope, "trr.name", "tp.name", 2);
    let runs_sql = format!(
        "SELECT count(*) FROM test_runs tr \
           JOIN projects tp ON tp.id = tr.project_id \
           JOIN repos trr ON trr.id = tp.repo_id{}",
        where_suffix(&runs_conditions)
    );
    let (scope_sql, scope_values) = scope_filters(scope, "r.name", "p.name", 2 + runs_values.len());
    let mut values = vec![Value::from(threshold)];
    values.extend(runs_values);
    values.extend(scope_values);
    let counts_sql = format!(
        "SELECT count(*) AS total, \
                count(*) FILTER (WHERE t.flakiness_score >= $1) AS flaky, \
                count(*) FILTER (WHERE t.flakiness_score > 0 AND t.flakiness_score < $1) AS suspect, \
                count(*) FILTER (WHERE t.confirmed_flake_count > 0) AS confirmed, \
                ({runs_sql}) AS total_runs \
           FROM test_cases t \
           JOIN projects p ON p.id = t.project_id \
           JOIN repos r ON r.id = p.repo_id{}",
        where_suffix(&scope_sql)
    );
    let row = one(db, counts_sql, values).await?.ok_or_else(|| {
        DbError::Database(DbErr::RecordNotFound(
            "the summary query returned no row".to_owned(),
        ))
    })?;
    Ok((
        get(&row, "total")?,
        get(&row, "flaky")?,
        get(&row, "suspect")?,
        get(&row, "confirmed")?,
        get(&row, "total_runs")?,
    ))
}

/// How many Tests in scope became flaky inside [`NEW_FLAKY_DAYS`].
async fn new_flaky_count(
    db: &impl ConnectionTrait,
    scope: &Scope,
    threshold: f64,
    today: NaiveDate,
) -> Result<i64, DbError> {
    let cutoff = today - Duration::days(NEW_FLAKY_DAYS);
    // The lateral subquery's cut-off is a literal day, so the binds stay simple.
    let old_point = score_history::newest_history_before(HistoryKind::Test, "t")
        .replace("$1::DATE", &format!("DATE '{}'", cutoff.format("%Y-%m-%d")));
    let (new_conditions, new_values) = scope_filters(scope, "r.name", "p.name", 2);
    let mut new_where = new_conditions;
    new_where.push("(past.score IS NULL OR past.score < $1)".to_owned());
    let new_sql = format!(
        "SELECT count(*) FILTER (WHERE t.flakiness_score >= $1) AS new_flaky \
           FROM test_cases t \
           JOIN projects p ON p.id = t.project_id \
           JOIN repos r ON r.id = p.repo_id \
           LEFT JOIN LATERAL ({old_point}) AS past ON true{}",
        where_suffix(&new_where)
    );
    let mut new_binds = vec![Value::from(threshold)];
    new_binds.extend(new_values);
    let row = one(db, new_sql, new_binds).await?.ok_or_else(|| {
        DbError::Database(DbErr::RecordNotFound(
            "the new-flaky query returned no row".to_owned(),
        ))
    })?;
    get(&row, "new_flaky")
}

/// The Executions and failures of the last [`SUMMARY_WINDOW_DAYS`] in scope.
async fn window_sums(
    db: &impl ConnectionTrait,
    scope: &Scope,
    today: NaiveDate,
) -> Result<(i64, i64), DbError> {
    let cutoff = today - Duration::days(SUMMARY_WINDOW_DAYS);
    let (sum_conditions, sum_values) = scope_filters(scope, "r.name", "p.name", 2);
    let mut sum_where = vec!["h.day > $1".to_owned()];
    sum_where.extend(sum_conditions);
    let sums_sql = format!(
        "SELECT coalesce(sum(h.executions), 0) AS executions, \
                coalesce(sum(h.failures), 0) AS failures \
           FROM test_score_history h \
           JOIN test_cases t ON t.id = h.test_case_id \
           JOIN projects p ON p.id = t.project_id \
           JOIN repos r ON r.id = p.repo_id{}",
        where_suffix(&sum_where)
    );
    let mut sum_binds = vec![Value::from(cutoff)];
    sum_binds.extend(sum_values);
    let row = one(db, sums_sql, sum_binds).await?.ok_or_else(|| {
        DbError::Database(DbErr::RecordNotFound(
            "the thirty-day sums query returned no row".to_owned(),
        ))
    })?;
    Ok((get(&row, "executions")?, get(&row, "failures")?))
}

/// Failing Executions per Failure category, for the flaky Tests in scope.
async fn category_counts(
    db: &impl ConnectionTrait,
    scope: &Scope,
) -> Result<BTreeMap<String, i64>, DbError> {
    let (category_conditions, category_values) = scope_filters(scope, "r.name", "p.name", 1);
    let mut category_where = vec![
        "t.flakiness_score > 0".to_owned(),
        "t.failure_category IS NOT NULL".to_owned(),
    ];
    category_where.extend(category_conditions);
    let categories_sql = format!(
        "SELECT t.failure_category AS category, count(*) AS count \
           FROM test_cases t \
           JOIN projects p ON p.id = t.project_id \
           JOIN repos r ON r.id = p.repo_id{} \
          GROUP BY t.failure_category",
        where_suffix(&category_where)
    );
    let mut category_counts: BTreeMap<String, i64> = BTreeMap::new();
    for category in FailureCategory::ALL {
        let _ = category_counts.insert(category.as_str().to_owned(), 0);
    }
    for row in rows(db, categories_sql, category_values).await? {
        let category: String = get(&row, "category")?;
        if FailureCategory::ALL
            .iter()
            .any(|known| known.as_str() == category)
        {
            let _ = category_counts.insert(category, get(&row, "count")?);
        }
    }
    Ok(category_counts)
}

/// One Execution in a Test's history, with the Run facts it carries.
fn execution_out(row: &QueryResult) -> Result<ExecutionOut, DbError> {
    Ok(ExecutionOut {
        id: get(row, "id")?,
        status: get(row, "status")?,
        duration: get(row, "duration")?,
        message: get(row, "message")?,
        details: get(row, "details")?,
        created_at: timestamp(row, "created_at")?,
        commit_sha: get(row, "commit_sha")?,
        branch: get(row, "branch")?,
        ci_run_id: get(row, "ci_run_id")?,
        attempt: i64::from(get::<i32>(row, "attempt")?),
        failure_category: get(row, "failure_category")?,
    })
}

/// The SQL selecting an Execution with its Run's facts.
const EXECUTION_WITH_RUN: &str = "SELECT e.id, e.status, e.duration, e.message, e.details, \
        e.created_at, r.commit_sha, r.branch, r.ci_run_id, e.attempt, e.failure_category, \
        r.ci_job_id \
   FROM test_executions e JOIN test_runs r ON r.id = e.test_run_id";

/// One day of history as the detail responses serialize it.
fn score_point(row: &score_history::ScoreHistoryRow) -> ScorePointOut {
    ScorePointOut {
        day: row.day,
        flakiness_score: row.flakiness_score,
        confirmed_flake_count: i64::from(row.confirmed_flake_count),
        executions: i64::from(row.executions),
        failures: i64::from(row.failures),
    }
}

/// One Test's detail page.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn get_test(
    db: &impl ConnectionTrait,
    test_id: i64,
    settings: &ReadSettings,
    executions_limit: i64,
) -> Result<Option<HistoryOut>, DbError> {
    let sql = format!("{TESTS_SELECT}, p.root AS project_root {TESTS_FROM} WHERE t.id = $1");
    let Some(row) = one(db, sql, vec![Value::from(test_id)]).await? else {
        return Ok(None);
    };
    let stored_id: i32 = get(&row, "id")?;
    let today = settings.today();
    let past = score_history::past_scores(db, HistoryKind::Test, &[stored_id], today).await?;
    let score: f64 = get(&row, "flakiness_score")?;
    let test = test_out(
        &row,
        settings.flake_threshold,
        settings.broken_streak,
        trend_for(score, past.get(&stored_id).copied()),
    )?;

    let exec_sql = format!(
        "{EXECUTION_WITH_RUN} \
         WHERE e.test_case_id = $1 ORDER BY e.id DESC LIMIT $2"
    );
    let execution_rows = rows(
        db,
        exec_sql,
        vec![Value::from(stored_id), Value::from(executions_limit)],
    )
    .await?;
    let mut executions = Vec::with_capacity(execution_rows.len());
    let mut ci_job_ids: BTreeSet<String> = BTreeSet::new();
    for execution in &execution_rows {
        if let Some(ci_job_id) = get::<Option<String>>(execution, "ci_job_id")?
            && !ci_job_id.is_empty()
        {
            let _ = ci_job_ids.insert(ci_job_id);
        }
        executions.push(execution_out(execution)?);
    }
    let jobs = test_job_links(db, &test.repo, &ci_job_ids).await?;

    let failing_sql = "SELECT r.commit_sha, r.branch \
         FROM test_executions e JOIN test_runs r ON r.id = e.test_run_id \
        WHERE e.test_case_id = $1 AND e.status IN ('failed', 'error') \
        ORDER BY e.id DESC LIMIT 1";
    let failing = one(db, failing_sql.to_owned(), vec![Value::from(stored_id)]).await?;
    let (last_failing_sha, last_failing_branch) = match failing {
        Some(row) => (
            Some(get::<String>(&row, "commit_sha")?),
            Some(get::<String>(&row, "branch")?),
        ),
        None => (None, None),
    };

    let root: String = get(&row, "project_root")?;
    let location = match test.file.as_deref().filter(|file| !file.is_empty()) {
        Some(file) => Some(LocationOut {
            path: repo_path(&root, file),
            line: test.line,
            url: last_failing_sha
                .as_deref()
                .map(|sha| permalink(&test.repo, sha, &root, file, test.line)),
        }),
        None => None,
    };

    let history = score_history::window_rows(db, HistoryKind::Test, stored_id, today).await?;
    Ok(Some(HistoryOut {
        test,
        location,
        last_failing_sha,
        last_failing_branch,
        executions,
        jobs,
        score_history: history.iter().map(score_point).collect(),
    }))
}

/// The distinct Jobs whose executions share a `ci_job_id` with these Runs, in this Repo.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn test_job_links(
    db: &impl ConnectionTrait,
    repo: &str,
    ci_job_ids: &BTreeSet<String>,
) -> Result<Vec<TestJobLinkOut>, DbError> {
    if ci_job_ids.is_empty() {
        return Ok(Vec::new());
    }
    let wanted: Vec<String> = ci_job_ids.iter().cloned().collect();
    let placeholders = (2..2 + wanted.len())
        .map(|index| format!("${index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT DISTINCT j.id AS job_id, p.name AS pipeline, j.name AS name \
           FROM job_executions e \
           JOIN jobs j ON j.id = e.job_id \
           JOIN pipelines p ON p.id = j.pipeline_id \
           JOIN repos ro ON ro.id = p.repo_id \
          WHERE ro.name = $1 AND e.ci_job_id IN ({placeholders}) \
          ORDER BY j.name, j.id"
    );
    let mut values = vec![Value::from(repo.to_owned())];
    values.extend(wanted.into_iter().map(Value::from));
    let mut links = Vec::new();
    for row in rows(db, sql, values).await? {
        links.push(TestJobLinkOut {
            job_id: id(get(&row, "job_id")?),
            pipeline: get(&row, "pipeline")?,
            name: get(&row, "name")?,
        });
    }
    Ok(links)
}

/// Stores a human's quarantine decision for one Test.
///
/// Every write stores the body's values, so re-quarantining without a reason or
/// an expiry clears the old ones instead of keeping a stale decision.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn set_quarantine(
    db: &impl ConnectionTrait,
    test_id: i64,
    quarantined: bool,
    reason: Option<&str>,
    expires_in_days: Option<i64>,
    settings: &ReadSettings,
) -> Result<Option<TestOut>, DbError> {
    let sql = format!("{TESTS_SELECT} {TESTS_FROM} WHERE t.id = $1");
    let Some(row) = one(db, sql, vec![Value::from(test_id)]).await? else {
        return Ok(None);
    };
    let stored_id: i32 = get(&row, "id")?;
    let now = settings.clock.now();
    let quarantined_at = quarantined.then_some(now);
    let reason = if quarantined { reason } else { None };
    let expires_at = match (quarantined, expires_in_days) {
        (true, Some(days)) => Some(now + Duration::days(days)),
        _ => None,
    };
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE test_cases SET quarantined = $2, quarantined_at = $3, \
            quarantine_reason = $4, quarantine_expires_at = $5 WHERE id = $1",
        [
            Value::from(stored_id),
            Value::from(quarantined),
            Value::from(quarantined_at),
            Value::from(reason.map(str::to_owned)),
            Value::from(expires_at),
        ],
    );
    db.execute_raw(statement).await?;
    let mut test = test_out(&row, settings.flake_threshold, settings.broken_streak, None)?;
    test.quarantined = quarantined;
    test.quarantined_at = quarantined_at.map(|at| at.with_timezone(&Utc));
    test.quarantine_reason = reason.map(str::to_owned);
    test.quarantine_expires_at = expires_at.map(|at| at.with_timezone(&Utc));
    Ok(Some(test))
}

/// For test runners: the quarantined Tests to skip in one Repo and Project.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn quarantine_list(
    db: &impl ConnectionTrait,
    repo: &str,
    project: &str,
) -> Result<Vec<QuarantineItem>, DbError> {
    let sql = "SELECT t.* FROM test_cases t \
           JOIN projects p ON p.id = t.project_id \
           JOIN repos r ON r.id = p.repo_id \
          WHERE r.name = $1 AND p.name = $2 AND t.quarantined \
          ORDER BY t.name, t.id";
    let mut items = Vec::new();
    for row in rows(
        db,
        sql.to_owned(),
        vec![
            Value::from(repo.to_owned()),
            Value::from(project.to_owned()),
        ],
    )
    .await?
    {
        items.push(QuarantineItem {
            suite: get(&row, "suite")?,
            classname: get(&row, "classname")?,
            name: get(&row, "name")?,
            fingerprint: get(&row, "fingerprint")?,
            file: get(&row, "file")?,
            line: get::<Option<i32>>(&row, "line")?.map(id),
            quarantined_at: optional_timestamp(&row, "quarantined_at")?,
            reason: get(&row, "quarantine_reason")?,
            expires_at: optional_timestamp(&row, "quarantine_expires_at")?,
        });
    }
    Ok(items)
}

/// The newest failing Execution of one Test, in the Execution shape.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn latest_failure(
    db: &impl ConnectionTrait,
    test_id: i64,
) -> Result<Option<ExecutionOut>, DbError> {
    let sql = format!(
        "{EXECUTION_WITH_RUN} \
         WHERE e.test_case_id = $1 AND e.status IN ('failed', 'error') \
         ORDER BY e.id DESC LIMIT 1"
    );
    let Some(row) = one(db, sql, vec![Value::from(test_id)]).await? else {
        return Ok(None);
    };
    Ok(Some(execution_out(&row)?))
}

/// Case-insensitive literal substring over a Test's name, classname and file; worst first.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn search_tests(
    db: &impl ConnectionTrait,
    scope: &Scope,
    query: &str,
    settings: &ReadSettings,
    limit: i64,
) -> Result<Vec<TestOut>, DbError> {
    let (scope_sql, scope_values) = scope_filters(scope, "r.name", "p.name", 2);
    let mut conditions = scope_sql;
    conditions.push(TEST_SEARCH.replace("$P", "$1"));
    let mut values = vec![Value::from(format!("%{}%", escape_like(query)))];
    values.extend(scope_values);
    let limit_placeholder = format!("${}", values.len() + 1);
    values.push(Value::from(limit));
    let sql = format!(
        "{TESTS_SELECT} {TESTS_FROM}{} ORDER BY t.flakiness_score DESC, t.id LIMIT {limit_placeholder}",
        where_suffix(&conditions)
    );
    let rows = rows(db, sql, values).await?;
    let ids: Vec<i32> = rows
        .iter()
        .map(|row| get::<i32>(row, "id"))
        .collect::<Result<_, _>>()?;
    let past = score_history::past_scores(db, HistoryKind::Test, &ids, settings.today()).await?;
    let mut found = Vec::with_capacity(rows.len());
    for row in &rows {
        let test_id: i32 = get(row, "id")?;
        let row_score: f64 = get(row, "flakiness_score")?;
        found.push(test_out(
            row,
            settings.flake_threshold,
            settings.broken_streak,
            trend_for(row_score, past.get(&test_id).copied()),
        )?);
    }
    Ok(found)
}

/// Exact-match lookup of a Test by name, and optionally classname.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn find_test_ids(
    db: &impl ConnectionTrait,
    repo: &str,
    project: &str,
    name: &str,
    classname: Option<&str>,
) -> Result<Vec<i64>, DbError> {
    let mut conditions = vec![
        "r.name = $1".to_owned(),
        "p.name = $2".to_owned(),
        "t.name = $3".to_owned(),
    ];
    let mut values = vec![
        Value::from(repo.to_owned()),
        Value::from(project.to_owned()),
        Value::from(name.to_owned()),
    ];
    if let Some(classname) = classname {
        conditions.push(format!("t.classname = ${}", values.len() + 1));
        values.push(Value::from(classname.to_owned()));
    }
    let sql = format!(
        "SELECT t.id FROM test_cases t \
           JOIN projects p ON p.id = t.project_id \
           JOIN repos r ON r.id = p.repo_id{} \
          ORDER BY t.id",
        where_suffix(&conditions)
    );
    let mut found = Vec::new();
    for row in rows(db, sql, values).await? {
        found.push(id(get::<i32>(&row, "id")?));
    }
    Ok(found)
}

/// One Job's detail page.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn get_job(
    db: &impl ConnectionTrait,
    job_id: i64,
    settings: &ReadSettings,
    executions_limit: i64,
) -> Result<Option<JobHistoryOut>, DbError> {
    let sql = format!("{JOBS_SELECT} {JOBS_FROM} WHERE j.id = $1");
    let Some(row) = one(db, sql, vec![Value::from(job_id)]).await? else {
        return Ok(None);
    };
    let stored_id: i32 = get(&row, "id")?;
    let repo_id: i32 = get(&row, "repo_id")?;
    let summary_job: bool = get(&row, "summary")?;
    let today = settings.today();
    let past = score_history::past_scores(db, HistoryKind::Job, &[stored_id], today).await?;
    let score: f64 = get(&row, "flakiness_score")?;
    let job = job_out(
        &row,
        settings.flake_threshold,
        trend_for(score, past.get(&stored_id).copied()),
    )?;

    let exec_sql = "SELECT * FROM job_executions WHERE job_id = $1 \
         ORDER BY id DESC LIMIT $2";
    let exec_rows = rows(
        db,
        exec_sql.to_owned(),
        vec![Value::from(stored_id), Value::from(executions_limit)],
    )
    .await?;
    let ci_job_ids: Vec<String> = exec_rows
        .iter()
        .map(|row| get::<String>(row, "ci_job_id"))
        .collect::<Result<_, _>>()?;
    let explained = explained_ci_job_ids(db, repo_id, &ci_job_ids).await?;
    let runners = runner_stats(db, stored_id, settings.score_window).await?;

    let mut histories = job_executions_out(&exec_rows, &explained)?;
    if !histories.failed_ci_to_exec.is_empty() {
        let mut by_exec = explaining_tests(db, repo_id, &histories.failed_ci_to_exec).await?;
        for execution in &mut histories.executions {
            if let Some(found) = by_exec.remove(&execution.id) {
                execution.explained_by = found;
            }
        }
    }
    if summary_job {
        attach_explaining_siblings(db, repo_id, &mut histories.executions).await?;
    }

    let history = score_history::window_rows(db, HistoryKind::Job, stored_id, today).await?;
    Ok(Some(JobHistoryOut {
        job,
        unexplained_failures: histories.unexplained,
        explained_failures: histories.explained,
        executions: histories.executions,
        runners,
        score_history: history.iter().map(score_point).collect(),
    }))
}

/// The mapped Job executions, the failure counts and the explained map.
struct JobExecutions {
    /// The executions, newest first.
    executions: Vec<JobExecutionOut>,
    /// Failures nothing explained.
    unexplained: i64,
    /// Failures a failing Test or a failed sibling explained.
    explained: i64,
    /// Each explained `ci_job_id` and the execution it belongs to.
    failed_ci_to_exec: BTreeMap<String, i64>,
}

/// Maps the stored Job executions, labelling each failure's outcome.
fn job_executions_out(
    exec_rows: &[QueryResult],
    explained: &BTreeSet<String>,
) -> Result<JobExecutions, DbError> {
    let mut failed_ci_to_exec: BTreeMap<String, i64> = BTreeMap::new();
    let mut executions = Vec::with_capacity(exec_rows.len());
    let mut unexplained: i64 = 0;
    let mut explained_failures: i64 = 0;
    for execution in exec_rows {
        let status: String = get(execution, "status")?;
        let ci_job_id: String = get(execution, "ci_job_id")?;
        let outcome = if status == "failed" {
            if explained.contains(&ci_job_id) {
                explained_failures += 1;
                let _ = failed_ci_to_exec.insert(ci_job_id.clone(), get::<i64>(execution, "id")?);
                JobOutcome::Explained
            } else {
                unexplained += 1;
                JobOutcome::Failed
            }
        } else if status == "passed" {
            JobOutcome::Passed
        } else {
            JobOutcome::Skipped
        };
        executions.push(JobExecutionOut {
            id: get(execution, "id")?,
            status,
            outcome,
            commit_sha: get(execution, "commit_sha")?,
            branch: get(execution, "branch")?,
            ci_run_id: get(execution, "ci_run_id")?,
            ci_run_attempt: i64::from(get::<i32>(execution, "ci_run_attempt")?),
            ci_job_id,
            url: get(execution, "url")?,
            runner_name: get(execution, "runner_name")?,
            runner_labels: runner_labels(execution)?,
            started_at: optional_timestamp(execution, "started_at")?,
            completed_at: optional_timestamp(execution, "completed_at")?,
            created_at: timestamp(execution, "created_at")?,
            explained_by: Vec::new(),
            explained_by_jobs: Vec::new(),
        });
    }
    Ok(JobExecutions {
        executions,
        unexplained,
        explained: explained_failures,
        failed_ci_to_exec,
    })
}

/// Lists the sibling Jobs that explain a Summary Job's failed executions.
async fn attach_explaining_siblings(
    db: &impl ConnectionTrait,
    repo_id: i32,
    executions: &mut [JobExecutionOut],
) -> Result<(), DbError> {
    let pairs: Vec<(String, i32)> = executions
        .iter()
        .filter(|execution| {
            execution.outcome == JobOutcome::Explained && execution.status == "failed"
        })
        .map(|execution| {
            (
                execution.ci_run_id.clone(),
                i32::try_from(execution.ci_run_attempt).unwrap_or(i32::MAX),
            )
        })
        .collect();
    let by_pair = summary_explaining_siblings(db, repo_id, &pairs).await?;
    for execution in executions {
        if execution.outcome == JobOutcome::Explained
            && let Some(siblings) = by_pair.get(&(
                execution.ci_run_id.clone(),
                i32::try_from(execution.ci_run_attempt).unwrap_or(i32::MAX),
            ))
        {
            execution.explained_by_jobs.clone_from(siblings);
        }
    }
    Ok(())
}

/// The `runner_labels` column as the sorted, deduplicated label set.
fn runner_labels(row: &QueryResult) -> Result<Vec<String>, DbError> {
    let raw: JsonValue = get(row, "runner_labels")?;
    let mut labels: Vec<String> = raw
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    labels.sort_unstable();
    labels.dedup();
    Ok(labels)
}

/// The failing Tests that explain each failed Job execution, capped per execution.
async fn explaining_tests(
    db: &impl ConnectionTrait,
    repo_id: i32,
    failed_ci_to_exec: &BTreeMap<String, i64>,
) -> Result<BTreeMap<i64, Vec<ExplainingTestOut>>, DbError> {
    let wanted: Vec<String> = failed_ci_to_exec.keys().cloned().collect();
    let placeholders = (2..2 + wanted.len())
        .map(|index| format!("${index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT e.test_case_id, e.status, r.ci_job_id, p.name AS project, \
                t.classname, t.name \
           FROM test_executions e \
           JOIN test_runs r ON r.id = e.test_run_id \
           JOIN projects p ON p.id = r.project_id \
           JOIN test_cases t ON t.id = e.test_case_id \
          WHERE r.ci_job_id IN ({placeholders}) AND p.repo_id = $1 \
            AND e.status IN ('failed', 'error') \
          ORDER BY e.id DESC"
    );
    let mut values = vec![Value::from(repo_id)];
    values.extend(wanted.into_iter().map(Value::from));
    let mut by_exec: BTreeMap<i64, Vec<ExplainingTestOut>> = failed_ci_to_exec
        .values()
        .map(|exec_id| (*exec_id, Vec::new()))
        .collect();
    for row in rows(db, sql, values).await? {
        let ci_job_id: String = get(&row, "ci_job_id")?;
        let Some(exec_id) = failed_ci_to_exec.get(&ci_job_id) else {
            continue;
        };
        let Some(found) = by_exec.get_mut(exec_id) else {
            continue;
        };
        if found.len() >= EXPLAINED_TESTS_MAX {
            continue;
        }
        found.push(ExplainingTestOut {
            test_id: id(get::<i32>(&row, "test_case_id")?),
            project: get(&row, "project")?,
            classname: get(&row, "classname")?,
            name: get(&row, "name")?,
            status: get(&row, "status")?,
        });
    }
    Ok(by_exec)
}

/// One row of the runner statistics before sorting.
struct RunnerStat {
    labels: Vec<String>,
    name: Option<String>,
    executions: i64,
    unexplained: i64,
}

/// Job executions grouped by runner over the Job's newest `window` executions.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn runner_stats(
    db: &impl ConnectionTrait,
    job_id: i32,
    window: usize,
) -> Result<Vec<RunnerStatOut>, DbError> {
    let sql = format!(
        "SELECT job_executions.runner_name, job_executions.runner_labels, \
                job_executions.status, {} AS explained \
           FROM job_executions \
           JOIN jobs ON jobs.id = job_executions.job_id \
           JOIN pipelines ON pipelines.id = jobs.pipeline_id \
          WHERE job_executions.id IN (SELECT w.id FROM ({}) AS w)",
        job_execution_explained_sql(),
        job_window_ids_overall_sql()
    );
    let values = vec![
        int_array(&[job_id]),
        Value::from(i32::try_from(window).unwrap_or(i32::MAX)),
    ];
    let mut label_counts: BTreeMap<Vec<String>, [i64; 2]> = BTreeMap::new();
    let mut name_counts: BTreeMap<String, [i64; 2]> = BTreeMap::new();
    let mut name_labels: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows(db, sql, values).await? {
        let runner_name: String = get(&row, "runner_name")?;
        let labels = runner_labels(&row)?;
        let status: String = get(&row, "status")?;
        let explained: bool = get(&row, "explained")?;
        let unexplained = i64::from(status == "failed" && !explained);
        let counts = label_counts.entry(labels.clone()).or_default();
        counts[0] += 1;
        counts[1] += unexplained;
        if !runner_name.is_empty() {
            let counts = name_counts.entry(runner_name.clone()).or_default();
            counts[0] += 1;
            counts[1] += unexplained;
            name_labels.entry(runner_name).or_default().extend(labels);
        }
    }
    let mut stats: Vec<RunnerStat> = Vec::new();
    for (labels, counts) in label_counts {
        stats.push(RunnerStat {
            labels,
            name: None,
            executions: counts[0],
            unexplained: counts[1],
        });
    }
    for (name, counts) in name_counts {
        if counts[0] < 2 {
            continue;
        }
        stats.push(RunnerStat {
            labels: name_labels
                .get(&name)
                .map(|labels| labels.iter().cloned().collect())
                .unwrap_or_default(),
            name: Some(name),
            executions: counts[0],
            unexplained: counts[1],
        });
    }
    stats.sort_by(|left, right| {
        // Counts of Executions are far below 2^52, so the cast below is exact.
        #[expect(
            clippy::cast_precision_loss,
            reason = "unexplained and executions are Execution counts, far below 2^52"
        )]
        let left_rate = left.unexplained as f64 / left.executions as f64;
        #[expect(
            clippy::cast_precision_loss,
            reason = "unexplained and executions are Execution counts, far below 2^52"
        )]
        let right_rate = right.unexplained as f64 / right.executions as f64;
        left_rate
            .partial_cmp(&right_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.executions.cmp(&right.executions))
            .then_with(|| left.labels.cmp(&right.labels))
            .then_with(|| {
                left.name
                    .as_deref()
                    .unwrap_or_default()
                    .cmp(right.name.as_deref().unwrap_or_default())
            })
            .reverse()
    });
    Ok(stats
        .into_iter()
        .take(RUNNER_STATS_LIMIT)
        .map(|stat| RunnerStatOut {
            labels: stat.labels,
            runner_name: stat.name,
            executions: stat.executions,
            unexplained_failures: stat.unexplained,
        })
        .collect())
}

/// The Jobs dashboard's headline counts.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn job_summary(
    db: &impl ConnectionTrait,
    repo: Option<&str>,
    settings: &ReadSettings,
) -> Result<JobSummaryOut, DbError> {
    let threshold = settings.flake_threshold;
    let (count_conditions, count_values) = repo_filters(repo, "r.name", 2);
    let counts_sql = format!(
        "SELECT count(*) AS total, \
                count(*) FILTER (WHERE j.flakiness_score >= $1) AS flaky, \
                count(*) FILTER (WHERE j.flakiness_score > 0 AND j.flakiness_score < $1) AS suspect, \
                count(*) FILTER (WHERE j.confirmed_flake_count > 0) AS confirmed \
           FROM jobs j \
           JOIN pipelines p ON p.id = j.pipeline_id \
           JOIN repos r ON r.id = p.repo_id{}",
        where_suffix(&count_conditions)
    );
    let mut values = vec![Value::from(threshold)];
    values.extend(count_values);
    let row = one(db, counts_sql, values).await?.ok_or_else(|| {
        DbError::Database(DbErr::RecordNotFound(
            "the job summary query returned no row".to_owned(),
        ))
    })?;
    let total: i64 = get(&row, "total")?;
    let flaky: i64 = get(&row, "flaky")?;
    let suspect: i64 = get(&row, "suspect")?;
    let confirmed: i64 = get(&row, "confirmed")?;

    let today = settings.today();
    let cutoff = today - Duration::days(NEW_FLAKY_DAYS);
    let old_point = score_history::newest_history_before(HistoryKind::Job, "j")
        .replace("$1::DATE", &format!("DATE '{}'", cutoff.format("%Y-%m-%d")));
    let (new_conditions, new_values) = repo_filters(repo, "r.name", 2);
    let mut new_where = new_conditions;
    new_where.push("(past.score IS NULL OR past.score < $1)".to_owned());
    let new_sql = format!(
        "SELECT count(*) FILTER (WHERE j.flakiness_score >= $1) AS new_flaky \
           FROM jobs j \
           JOIN pipelines p ON p.id = j.pipeline_id \
           JOIN repos r ON r.id = p.repo_id \
           LEFT JOIN LATERAL ({old_point}) AS past ON true{}",
        where_suffix(&new_where)
    );
    let mut new_binds = vec![Value::from(threshold)];
    new_binds.extend(new_values);
    let row = one(db, new_sql, new_binds).await?.ok_or_else(|| {
        DbError::Database(DbErr::RecordNotFound(
            "the job new-flaky query returned no row".to_owned(),
        ))
    })?;
    let new_flaky: i64 = get(&row, "new_flaky")?;

    let window_cutoff = today - Duration::days(SUMMARY_WINDOW_DAYS);
    let (sum_conditions, sum_values) = repo_filters(repo, "r.name", 2);
    let mut sum_where = vec!["h.day > $1".to_owned()];
    sum_where.extend(sum_conditions);
    let sums_sql = format!(
        "SELECT coalesce(sum(h.executions), 0) AS executions, \
                coalesce(sum(h.failures), 0) AS failures \
           FROM job_score_history h \
           JOIN jobs j ON j.id = h.job_id \
           JOIN pipelines p ON p.id = j.pipeline_id \
           JOIN repos r ON r.id = p.repo_id{}",
        where_suffix(&sum_where)
    );
    let mut sum_binds = vec![Value::from(window_cutoff)];
    sum_binds.extend(sum_values);
    let row = one(db, sums_sql, sum_binds).await?.ok_or_else(|| {
        DbError::Database(DbErr::RecordNotFound(
            "the job thirty-day sums query returned no row".to_owned(),
        ))
    })?;
    let executions_30d: i64 = get(&row, "executions")?;
    let failures_30d: i64 = get(&row, "failures")?;

    let (pipe_conditions, pipe_values) = repo_filters(repo, "r.name", 1);
    let pipelines_sql = format!(
        "SELECT DISTINCT p.name AS pipeline \
           FROM pipelines p JOIN repos r ON r.id = p.repo_id{} \
          ORDER BY p.name",
        where_suffix(&pipe_conditions)
    );
    let mut pipelines = Vec::new();
    for row in rows(db, pipelines_sql, pipe_values).await? {
        pipelines.push(get::<String>(&row, "pipeline")?);
    }

    Ok(JobSummaryOut {
        total_jobs: total,
        flaky_jobs: flaky,
        suspect_jobs: suspect,
        confirmed_flaky_jobs: confirmed,
        new_flaky_7d: new_flaky,
        job_executions_30d: executions_30d,
        job_failures_30d: failures_30d,
        flake_threshold: threshold,
        pipelines,
    })
}

/// A Repo's conditions for one statement, with placeholders from `start`.
fn repo_filters(repo: Option<&str>, repo_column: &str, start: usize) -> (Vec<String>, Vec<Value>) {
    match repo {
        Some(repo) => (
            vec![format!("{repo_column} = ${start}")],
            vec![Value::from(repo.to_owned())],
        ),
        None => (Vec::new(), Vec::new()),
    }
}

/// Case-insensitive literal substring over a Job's and its Pipeline's name; worst first.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn search_jobs(
    db: &impl ConnectionTrait,
    repo: &str,
    query: &str,
    settings: &ReadSettings,
    limit: i64,
) -> Result<Vec<JobOut>, DbError> {
    let sql = format!(
        "{JOBS_SELECT} {JOBS_FROM} WHERE r.name = $2 AND {} \
          ORDER BY j.flakiness_score DESC, j.id LIMIT $3",
        JOB_SEARCH.replace("$P", "$1")
    );
    let values = vec![
        Value::from(format!("%{}%", escape_like(query))),
        Value::from(repo.to_owned()),
        Value::from(limit),
    ];
    let rows = rows(db, sql, values).await?;
    let ids: Vec<i32> = rows
        .iter()
        .map(|row| get::<i32>(row, "id"))
        .collect::<Result<_, _>>()?;
    let past = score_history::past_scores(db, HistoryKind::Job, &ids, settings.today()).await?;
    let mut found = Vec::with_capacity(rows.len());
    for row in &rows {
        let job_id: i32 = get(row, "id")?;
        let row_score: f64 = get(row, "flakiness_score")?;
        found.push(job_out(
            row,
            settings.flake_threshold,
            trend_for(row_score, past.get(&job_id).copied()),
        )?);
    }
    Ok(found)
}

/// Exact-match lookup of a Job by name, and optionally Pipeline name.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn find_job_ids(
    db: &impl ConnectionTrait,
    repo: &str,
    name: &str,
    pipeline: Option<&str>,
) -> Result<Vec<i64>, DbError> {
    let mut conditions = vec!["r.name = $1".to_owned(), "j.name = $2".to_owned()];
    let mut values = vec![Value::from(repo.to_owned()), Value::from(name.to_owned())];
    if let Some(pipeline) = pipeline {
        conditions.push(format!("p.name = ${}", values.len() + 1));
        values.push(Value::from(pipeline.to_owned()));
    }
    let sql = format!(
        "SELECT j.id FROM jobs j \
           JOIN pipelines p ON p.id = j.pipeline_id \
           JOIN repos r ON r.id = p.repo_id{} \
          ORDER BY j.id",
        where_suffix(&conditions)
    );
    let mut found = Vec::new();
    for row in rows(db, sql, values).await? {
        found.push(id(get::<i32>(&row, "id")?));
    }
    Ok(found)
}

/// One execution of a failing (Test, Run) pair, as the CI run listing reads it.
#[derive(Debug, Clone)]
struct PairExecution {
    status: String,
    failure_category: Option<String>,
    message: String,
}

/// The failing Tests and failed Jobs of one CI run attempt.
///
/// `ci_run_attempt = None` means the highest known attempt.
///
/// # Errors
///
/// Returns [`ReadError::NotFound`] for an unknown Repo, CI run or attempt, and
/// [`ReadError::Database`] when a statement fails.
pub async fn ci_run_failures(
    db: &impl ConnectionTrait,
    repo: &str,
    ci_run_id: &str,
    ci_run_attempt: Option<i64>,
    settings: &ReadSettings,
    limit: i64,
) -> Result<CiRunFailuresOut, ReadError> {
    let Some(repo_row) = one(
        db,
        "SELECT id, name FROM repos WHERE name = $1".to_owned(),
        vec![Value::from(repo.to_owned())],
    )
    .await?
    else {
        return Err(ReadError::NotFound("Repo not found".to_owned()));
    };
    let repo_id: i32 = get(&repo_row, "id")?;
    let repo_name: String = get(&repo_row, "name")?;
    let attempts = ci_run_attempts(db, repo_id, ci_run_id).await?;
    if attempts.is_empty() {
        return Err(ReadError::NotFound("CI run not found".to_owned()));
    }
    let attempt = match ci_run_attempt {
        None => attempts.iter().next_back().copied().unwrap_or(1),
        Some(attempt) => {
            let wanted = i32::try_from(attempt).unwrap_or(i32::MAX);
            if !attempts.contains(&wanted) {
                return Err(ReadError::NotFound("CI run attempt not found".to_owned()));
            }
            wanted
        }
    };
    let (failures, total) =
        ci_run_test_failures(db, repo_id, &repo_name, ci_run_id, attempt, settings, limit).await?;
    let failed_jobs =
        ci_run_failed_jobs(db, repo_id, ci_run_id, attempt, &repo_name, settings).await?;
    Ok(CiRunFailuresOut {
        repo: repo_name,
        ci_run_id: ci_run_id.to_owned(),
        ci_run_attempt: i64::from(attempt),
        flake_threshold: settings.flake_threshold,
        reports: ci_run_report_counts(db, repo_id, ci_run_id, attempt).await?,
        total_failures: total,
        truncated: total > i64::try_from(failures.len()).unwrap_or(i64::MAX),
        failures,
        failed_jobs,
    })
}

/// Every known attempt of one CI run in one Repo (NULL counts as 1).
async fn ci_run_attempts(
    db: &impl ConnectionTrait,
    repo_id: i32,
    ci_run_id: &str,
) -> Result<BTreeSet<i32>, DbError> {
    let mut attempts = BTreeSet::new();
    let statements = [
        "SELECT DISTINCT coalesce(ci_run_attempt, 1) AS attempt FROM reports \
          WHERE repo_id = $1 AND ci_run_id = $2",
        "SELECT DISTINCT coalesce(tr.ci_run_attempt, 1) AS attempt \
           FROM test_runs tr JOIN projects p ON p.id = tr.project_id \
          WHERE p.repo_id = $1 AND tr.ci_run_id = $2",
        "SELECT DISTINCT e.ci_run_attempt AS attempt \
           FROM job_executions e \
           JOIN jobs j ON j.id = e.job_id \
           JOIN pipelines p ON p.id = j.pipeline_id \
          WHERE p.repo_id = $1 AND e.ci_run_id = $2",
    ];
    for sql in statements {
        for row in rows(
            db,
            sql.to_owned(),
            vec![Value::from(repo_id), Value::from(ci_run_id.to_owned())],
        )
        .await?
        {
            let _ = attempts.insert(get::<i32>(&row, "attempt")?);
        }
    }
    Ok(attempts)
}

/// Counts of both Report kinds for one Repo and CI run attempt.
async fn ci_run_report_counts(
    db: &impl ConnectionTrait,
    repo_id: i32,
    ci_run_id: &str,
    attempt: i32,
) -> Result<ReportCountsOut, DbError> {
    let sql = "SELECT status, count(*) AS count FROM reports \
                WHERE repo_id = $1 AND ci_run_id = $2 AND coalesce(ci_run_attempt, 1) = $3 \
                GROUP BY status";
    let mut counts = ReportCountsOut {
        pending: 0,
        processed: 0,
        failed: 0,
    };
    for row in rows(
        db,
        sql.to_owned(),
        vec![
            Value::from(repo_id),
            Value::from(ci_run_id.to_owned()),
            Value::from(attempt),
        ],
    )
    .await?
    {
        let status: String = get(&row, "status")?;
        let count: i64 = get(&row, "count")?;
        match status.as_str() {
            "pending" => counts.pending = count,
            "processed" => counts.processed = count,
            "failed" => counts.failed = count,
            _ => {}
        }
    }
    Ok(counts)
}

/// The failing (Test, Run) pairs of one CI run attempt, capped at `limit`.
async fn ci_run_test_failures(
    db: &impl ConnectionTrait,
    repo_id: i32,
    repo_name: &str,
    ci_run_id: &str,
    attempt: i32,
    settings: &ReadSettings,
    limit: i64,
) -> Result<(Vec<CiRunTestFailureOut>, i64), DbError> {
    let mut values = vec![
        Value::from(repo_id),
        Value::from(ci_run_id.to_owned()),
        Value::from(attempt),
    ];
    let total_sql = "SELECT count(*) AS total FROM ( \
         SELECT e.test_case_id, e.test_run_id \
           FROM test_executions e \
           JOIN test_runs tr ON tr.id = e.test_run_id \
           JOIN projects p ON p.id = tr.project_id \
          WHERE p.repo_id = $1 AND tr.ci_run_id = $2 \
            AND coalesce(tr.ci_run_attempt, 1) = $3 \
            AND e.status IN ('failed', 'error') \
          GROUP BY e.test_case_id, e.test_run_id) AS pairs";
    let count = one(db, total_sql.to_owned(), values.clone())
        .await?
        .ok_or_else(|| {
            DbError::Database(DbErr::RecordNotFound(
                "the CI run failure count returned no row".to_owned(),
            ))
        })?;
    let total: i64 = get(&count, "total")?;
    if total == 0 {
        return Ok((Vec::new(), 0));
    }
    values.push(Value::from(limit));
    let page_sql = "SELECT t.*, t.id AS test_id, p.name AS project_name, \
            tr.id AS run_id, tr.ci_job_id AS ci_job_id \
       FROM ( \
         SELECT e.test_case_id, e.test_run_id \
           FROM test_executions e \
           JOIN test_runs tr ON tr.id = e.test_run_id \
           JOIN projects p ON p.id = tr.project_id \
          WHERE p.repo_id = $1 AND tr.ci_run_id = $2 \
            AND coalesce(tr.ci_run_attempt, 1) = $3 \
            AND e.status IN ('failed', 'error') \
          GROUP BY e.test_case_id, e.test_run_id) AS pairs \
       JOIN test_cases t ON t.id = pairs.test_case_id \
       JOIN test_runs tr ON tr.id = pairs.test_run_id \
       JOIN projects p ON p.id = t.project_id \
      ORDER BY p.name, t.suite, t.classname, t.name, tr.id \
      LIMIT $4";
    let page = rows(db, page_sql.to_owned(), values).await?;
    let run_ids: Vec<i32> = page
        .iter()
        .map(|row| get::<i32>(row, "run_id"))
        .collect::<Result<_, _>>()?;
    let test_ids: Vec<i32> = page
        .iter()
        .map(|row| get::<i32>(row, "test_id"))
        .collect::<Result<_, _>>()?;
    let by_pair = pair_executions(db, vec![int_array(&run_ids), int_array(&test_ids)]).await?;
    let mut failures = Vec::with_capacity(page.len());
    for row in &page {
        if let Some(failure) = ci_run_failure_item(row, repo_name, &by_pair, settings)? {
            failures.push(failure);
        }
    }
    Ok((failures, total))
}

/// The Executions of each failing (Test, Run) pair, ordered by attempt then id.
async fn pair_executions(
    db: &impl ConnectionTrait,
    values: Vec<Value>,
) -> Result<BTreeMap<(i32, i32), Vec<PairExecution>>, DbError> {
    let exec_sql = "SELECT e.test_case_id, e.test_run_id, e.status, \
            e.failure_category, e.message \
       FROM test_executions e \
      WHERE e.test_run_id = ANY($1::INTEGER[]) \
        AND e.test_case_id = ANY($2::INTEGER[]) \
      ORDER BY e.attempt, e.id";
    let mut by_pair: BTreeMap<(i32, i32), Vec<PairExecution>> = BTreeMap::new();
    for row in rows(db, exec_sql.to_owned(), values).await? {
        let key = (
            get::<i32>(&row, "test_case_id")?,
            get::<i32>(&row, "test_run_id")?,
        );
        by_pair.entry(key).or_default().push(PairExecution {
            status: get(&row, "status")?,
            failure_category: get(&row, "failure_category")?,
            message: get(&row, "message")?,
        });
    }
    Ok(by_pair)
}

/// One failing pair as the listing serializes it.
fn ci_run_failure_item(
    row: &QueryResult,
    repo_name: &str,
    by_pair: &BTreeMap<(i32, i32), Vec<PairExecution>>,
    settings: &ReadSettings,
) -> Result<Option<CiRunTestFailureOut>, DbError> {
    let test_id: i32 = get(row, "test_id")?;
    let run_id: i32 = get(row, "run_id")?;
    let executions = by_pair.get(&(test_id, run_id)).cloned().unwrap_or_default();
    let Some(last) = executions.last() else {
        // The pair was failing when it was counted, so it has at least one
        // Execution; a row deleted between the two reads is left out.
        return Ok(None);
    };
    let failing: Vec<&PairExecution> = executions
        .iter()
        .filter(|execution| matches!(execution.status.as_str(), "failed" | "error"))
        .collect();
    let Some(last_failing) = failing.last() else {
        return Ok(None);
    };
    let score: f64 = get(row, "flakiness_score")?;
    let issue_number: Option<i32> = get(row, "github_issue_number")?;
    let issue_number = issue_number.map(id);
    let fail_streak = i64::from(get::<i32>(row, "fail_streak")?);
    Ok(Some(CiRunTestFailureOut {
        test_id: id(test_id),
        project: get(row, "project_name")?,
        suite: get(row, "suite")?,
        classname: get(row, "classname")?,
        name: get(row, "name")?,
        run_id: id(run_id),
        ci_job_id: get(row, "ci_job_id")?,
        status: last.status.clone(),
        failed_attempts: i64::try_from(failing.len()).unwrap_or(i64::MAX),
        failure_category: last_failing.failure_category.clone(),
        message: last_failing
            .message
            .chars()
            .take(CI_RUN_MESSAGE_MAX)
            .collect(),
        tier: tier_for(score, settings.flake_threshold),
        flakiness_score: score,
        confirmed_flake_count: i64::from(get::<i32>(row, "confirmed_flake_count")?),
        clean_streak: i64::from(get::<i32>(row, "clean_streak")?),
        fail_streak,
        broken: is_broken(fail_streak, settings.broken_streak),
        quarantined: get(row, "quarantined")?,
        github_issue_number: issue_number,
        github_issue_url: issue_url(repo_name, issue_number),
        github_issue_closed: get(row, "github_issue_closed")?,
    }))
}

/// The failed Job executions of one CI run attempt, each explained or unexplained.
async fn ci_run_failed_jobs(
    db: &impl ConnectionTrait,
    repo_id: i32,
    ci_run_id: &str,
    attempt: i32,
    repo_name: &str,
    settings: &ReadSettings,
) -> Result<Vec<CiRunFailedJobOut>, DbError> {
    let sql = "SELECT e.ci_job_id AS ci_job_id, e.url AS url, j.id AS job_id, \
            j.name AS job_name, j.summary AS summary, j.flakiness_score AS flakiness_score, \
            j.confirmed_flake_count AS confirmed_flake_count, j.clean_streak AS clean_streak, \
            j.github_issue_number AS github_issue_number, \
            j.github_issue_closed AS github_issue_closed, \
            p.name AS pipeline_name, p.provider AS provider \
       FROM job_executions e \
       JOIN jobs j ON j.id = e.job_id \
       JOIN pipelines p ON p.id = j.pipeline_id \
      WHERE p.repo_id = $1 AND e.ci_run_id = $2 AND e.ci_run_attempt = $3 \
        AND e.status = 'failed' \
      ORDER BY p.name, j.name, e.id";
    let found = rows(
        db,
        sql.to_owned(),
        vec![
            Value::from(repo_id),
            Value::from(ci_run_id.to_owned()),
            Value::from(attempt),
        ],
    )
    .await?;
    let ci_job_ids: Vec<String> = found
        .iter()
        .map(|row| get::<String>(row, "ci_job_id"))
        .collect::<Result<_, _>>()?;
    let explained = explained_ci_job_ids(db, repo_id, &ci_job_ids).await?;
    let mut failed = Vec::with_capacity(found.len());
    for row in &found {
        let ci_job_id: String = get(row, "ci_job_id")?;
        let score: f64 = get(row, "flakiness_score")?;
        let provider: String = get(row, "provider")?;
        let issue_number: Option<i32> = get(row, "github_issue_number")?;
        let issue_number = issue_number.map(id);
        failed.push(CiRunFailedJobOut {
            job_id: id(get(row, "job_id")?),
            pipeline: get(row, "pipeline_name")?,
            name: get(row, "job_name")?,
            outcome: if explained.contains(&ci_job_id) {
                FailedJobOutcome::Explained
            } else {
                FailedJobOutcome::Unexplained
            },
            ci_job_id,
            url: get(row, "url")?,
            summary: get(row, "summary")?,
            tier: tier_for(score, settings.flake_threshold),
            flakiness_score: score,
            confirmed_flake_count: i64::from(get::<i32>(row, "confirmed_flake_count")?),
            clean_streak: i64::from(get::<i32>(row, "clean_streak")?),
            github_issue_number: issue_number,
            github_issue_url: job_issue_url(repo_name, &provider, issue_number),
            github_issue_closed: get(row, "github_issue_closed")?,
        });
    }
    Ok(failed)
}

#[cfg(test)]
mod tests {
    //! The small helpers' own rules, without a database.

    #![expect(
        clippy::expect_used,
        reason = "a helper test fails loudly when a scope or a conversion is wrong"
    )]

    use super::{
        HistoryKind, NEW_FLAKY_DAYS, ReadError, ReadSettings, SortKey, Where, newest_history_join,
        repo_filters, scope_conditions, scope_filters, where_suffix,
    };

    #[test]
    fn sort_keys_parse_their_spellings() {
        assert_eq!(SortKey::parse("score"), Some(SortKey::Score));
        assert_eq!(SortKey::parse("last_seen"), Some(SortKey::LastSeen));
        assert_eq!(SortKey::parse("proven"), Some(SortKey::Proven));
        assert_eq!(SortKey::parse("name"), None);
        assert_eq!(SortKey::default(), SortKey::Score);
    }

    #[test]
    fn the_where_suffix_is_empty_without_conditions() {
        let filter = Where::default();
        assert_eq!(filter.suffix(), "");
        assert_eq!(where_suffix(&[]), "");
    }

    #[test]
    fn the_where_suffix_joins_every_condition() {
        let mut filter = Where::default();
        filter.eq("r.name", sea_orm::Value::from("acme/app"));
        filter.raw("t.flakiness_score > 0");
        assert_eq!(
            filter.suffix(),
            " WHERE r.name = $1 AND t.flakiness_score > 0"
        );
    }

    #[test]
    fn a_scope_and_a_repo_filter_bind_in_order() {
        let scope = super::Scope::new(Some("acme/app".to_owned()), Some("backend".to_owned()))
            .expect("a scope");
        let mut filter = Where::default();
        scope_conditions(&scope, "r.name", "p.name", &mut filter);
        assert_eq!(filter.suffix(), " WHERE r.name = $1 AND p.name = $2");
        assert_eq!(filter.values.len(), 2);

        let (conditions, values) = scope_filters(&scope, "r.name", "p.name", 3);
        assert_eq!(conditions, ["r.name = $3", "p.name = $4"]);
        assert_eq!(values.len(), 2);
        let (conditions, values) = repo_filters(Some("acme/app"), "r.name", 1);
        assert_eq!(conditions, ["r.name = $1"]);
        assert_eq!(values.len(), 1);
        let (conditions, values) = repo_filters(None, "r.name", 1);
        assert_eq!(conditions, Vec::<String>::new());
        assert_eq!(values, Vec::<sea_orm::Value>::new());
    }

    #[test]
    fn a_scope_without_a_repo_refuses_a_project() {
        let error = super::Scope::new(None, Some("backend".to_owned()))
            .expect_err("a Project without a Repo is refused");
        assert_eq!(error, "project filter requires repo");
    }

    #[test]
    fn the_new_flaky_window_reads_a_literal_day() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 1, 20).expect("a day");
        let join = newest_history_join(
            HistoryKind::Test,
            "t",
            today - chrono::Duration::days(NEW_FLAKY_DAYS),
        );
        assert!(join.contains("DATE '2026-01-13'"), "{join}");
        assert!(join.contains("h.test_case_id = t.id"), "{join}");
    }

    #[test]
    fn read_settings_describe_themselves_without_the_clock() {
        let settings = ReadSettings::default();
        let described = format!("{settings:?}");
        assert!(described.contains("flake_threshold"));
        assert!(described.contains("broken_streak"));
        assert!(described.contains("score_window"));
        assert!(!described.contains("clock"));
    }

    #[test]
    fn a_read_error_prints_its_detail() {
        let error = ReadError::from(super::DbError::Database(sea_orm::DbErr::RecordNotFound(
            "a missing row".to_owned(),
        )));
        assert!(error.to_string().contains("a missing row"));
        assert_eq!(
            ReadError::NotFound("Repo not found".to_owned()).to_string(),
            "Repo not found"
        );
    }
}
