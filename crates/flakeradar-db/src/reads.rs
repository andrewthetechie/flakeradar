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
    CiRunFailedJobOut, CiRunFailuresOut, CiRunTestFailureOut, EntityKind, ExecutionOut,
    ExecutionStatus, ExplainingTestOut, FailedJobOutcome, FailureCategory, HistoryOut,
    JobExecutionOut, JobHistoryOut, JobOut, JobOutcome, JobPage, JobSummaryOut, LocationOut,
    LocationRef, QuarantineItem, RepoOut, ReportCountsOut, ReportStatus, RunnerStatOut,
    ScorePointOut, SummaryOut, TestJobLinkOut, TestOut, TestPage, Trend, defaults, escape_like,
    is_broken, issue_url, permalink, repo_path, tier_for, trend_for,
};
use sea_orm::{ConnectionTrait, DbBackend, QueryResult, Statement, Value};
use serde_json::Value as JsonValue;

use crate::attribution::{
    explained_ci_job_ids, int_array, job_execution_explained_sql, job_window_ids_overall_sql,
    summary_explaining_siblings,
};
use crate::clock::SharedClock;
use crate::error::DbError;
use crate::score_history;
use crate::sql::{Binds, execution_status, get, one, required_one, rows};

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
            flake_threshold: defaults::FLAKE_THRESHOLD,
            broken_streak: defaults::BROKEN_STREAK,
            score_window: defaults::SCORE_WINDOW,
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
    repo: Option<String>,
    project: Option<String>,
}

impl Scope {
    /// A scope for a Repo and optionally a Project.
    ///
    /// # Errors
    ///
    /// Returns [`ScopeError::ProjectWithoutRepo`] when a Project is given
    /// without a Repo, which is the rule the REST and MCP filters share.
    pub fn new(repo: Option<String>, project: Option<String>) -> Result<Self, ScopeError> {
        if project.is_some() && repo.is_none() {
            return Err(ScopeError::ProjectWithoutRepo);
        }
        Ok(Self { repo, project })
    }

    /// The Repo's `owner/name`, when the scope names one.
    #[must_use]
    pub fn repo(&self) -> Option<&str> {
        self.repo.as_deref()
    }

    /// The Project's name, which a scope only holds beside a Repo.
    #[must_use]
    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }
}

/// Why a [`Scope`] cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ScopeError {
    /// A Project name is unique only within its Repo, so a Project needs one.
    #[error("project filter requires repo")]
    ProjectWithoutRepo,
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

/// Adds the Scope's conditions against the given Repo and Project aliases.
fn scope_conditions(scope: &Scope, repo_column: &str, project_column: &str, filter: &mut Binds) {
    if let Some(repo) = scope.repo() {
        filter.eq(repo_column, Value::from(repo.to_owned()));
    }
    if let Some(project) = scope.project() {
        filter.eq(project_column, Value::from(project.to_owned()));
    }
}

/// The SQL a `new_within_days` filter adds: the per-row newest old history point.
fn newest_history_join(kind: EntityKind, alias: &str, cutoff: NaiveDate) -> String {
    let lateral = score_history::newest_history_before(kind, alias)
        .replace("$1::DATE", &format!("DATE '{}'", cutoff.format("%Y-%m-%d")));
    format!(" LEFT JOIN LATERAL ({lateral}) AS past ON true")
}

/// A timestamp column read as UTC.
pub(crate) fn timestamp(row: &QueryResult, column: &str) -> Result<DateTime<Utc>, DbError> {
    let value: DateTime<FixedOffset> = get(row, column)?;
    Ok(value.with_timezone(&Utc))
}

/// An optional timestamp column read as UTC.
pub(crate) fn optional_timestamp(
    row: &QueryResult,
    column: &str,
) -> Result<Option<DateTime<Utc>>, DbError> {
    let value: Option<DateTime<FixedOffset>> = get(row, column)?;
    Ok(value.map(|value| value.with_timezone(&Utc)))
}

/// Converts a stored `INTEGER` id to the DTO's `i64`.
pub(crate) fn id(value: i32) -> i64 {
    i64::from(value)
}

/// What the one leaderboard engine needs to know about an entity to query it.
struct EntityBoard {
    /// The Score-history table the past-score lookups use.
    kind: EntityKind,
    /// The entity's table alias in `select`.
    alias: &'static str,
    /// The `SELECT` list, ending with the entity's own columns.
    select: &'static str,
    /// The `FROM` clause, joining the entity to its parents.
    from: &'static str,
    /// The search condition, given the pattern bind's placeholder.
    search: fn(&str) -> String,
    /// The entity's GitHub issue column.
    issue_column: &'static str,
}

/// The Tests leaderboard's descriptor.
const TESTS_BOARD: EntityBoard = EntityBoard {
    kind: EntityKind::Test,
    alias: "t",
    select: TESTS_SELECT,
    from: TESTS_FROM,
    search: test_search,
    issue_column: "t.github_issue_number",
};

/// The Jobs leaderboard's descriptor.
const JOBS_BOARD: EntityBoard = EntityBoard {
    kind: EntityKind::Job,
    alias: "j",
    select: JOBS_SELECT,
    from: JOBS_FROM,
    search: job_search,
    issue_column: "j.github_issue_number",
};

/// Which tiers a leaderboard listing may return.
///
/// The three cases are the engine's whole tier rule, decided once by
/// [`TierFilter::of`], so `flaky_only` and `include_stable` cannot be combined
/// into a state the query has to guess about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TierFilter {
    /// At or above the Flake threshold: the MCP flaky-only listing and the
    /// "became flaky recently" window.
    FlakyOnly,
    /// Above a zero score: the stable-hiding default.
    AboveZero,
    /// Every row.
    All,
}

impl TierFilter {
    /// The tier a listing returns, from the options and the entity's own filters.
    fn of(options: &ListOptions, keeps_stable_rows: bool) -> Self {
        let searching = options.q.as_deref().is_some_and(|text| !text.is_empty());
        if options.flaky_only || options.new_within_days.is_some() {
            Self::FlakyOnly
        } else if !options.include_stable && !searching && !keeps_stable_rows {
            Self::AboveZero
        } else {
            Self::All
        }
    }
}

/// One leaderboard request: the entity, the caller's options and the extra filters.
struct Board<'a> {
    /// The entity being listed.
    entity: &'a EntityBoard,
    /// The sort, paging, search and filter options.
    options: &'a ListOptions,
    /// The conditions the wrapper scoped and filtered with.
    filter: Binds,
    /// The tier the listing returns, decided by the caller.
    tier: TierFilter,
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
        match self.tier {
            TierFilter::FlakyOnly => {
                let placeholder = conditions.bind(Value::from(self.threshold));
                conditions.raw(format!(
                    "{}.flakiness_score >= {placeholder}",
                    self.entity.alias
                ));
            }
            TierFilter::AboveZero => {
                conditions.raw(format!("{}.flakiness_score > 0", self.entity.alias));
            }
            TierFilter::All => {}
        }

        if let Some(text) = self.options.q.as_deref().filter(|text| !text.is_empty()) {
            let placeholder = conditions.bind(Value::from(format!("%{}%", escape_like(text))));
            conditions.raw((self.entity.search)(&placeholder));
        }

        if let Some(wanted) = self.options.has_issue {
            conditions.raw(if wanted {
                format!("{} IS NOT NULL", self.entity.issue_column)
            } else {
                format!("{} IS NULL", self.entity.issue_column)
            });
        }

        if let Some(days) = self.options.new_within_days {
            joins = newest_history_join(
                self.entity.kind,
                self.entity.alias,
                self.today - Duration::days(days),
            );
            let placeholder = conditions.bind(Value::from(self.threshold));
            conditions.raw(format!(
                "(past.score IS NULL OR past.score < {placeholder})"
            ));
        }

        let base = format!(
            "{} {}{}{}",
            self.entity.select,
            self.entity.from,
            joins,
            conditions.suffix()
        );
        let count_sql = format!("SELECT count(*) AS total FROM ({base}) AS matches");
        let count = one(db, count_sql, conditions.values().to_vec()).await?;
        let total: i64 = match count {
            Some(row) => get(&row, "total")?,
            None => 0,
        };

        let order = match self.options.sort {
            SortKey::Score => "flakiness_score DESC, last_seen_at DESC, id",
            SortKey::LastSeen => "last_seen_at DESC, id DESC",
            SortKey::Proven => "confirmed_flake_count DESC, flakiness_score DESC, id",
        };
        let page = self.options.page;
        let page_size = self.options.page_size;
        let limit = conditions.bind(Value::from(page_size));
        // A page whose offset does not fit is an error rather than a wrapped
        // offset: the route refuses such a page, and a caller building the
        // options itself is told instead of being answered another page's rows.
        let offset_rows = page
            .saturating_sub(1)
            .checked_mul(page_size)
            .ok_or(DbError::PageOffsetTooLarge { page, page_size })?;
        let offset = conditions.bind(Value::from(offset_rows));
        let values = conditions.into_values();
        let page_sql = format!("{base} ORDER BY {order} LIMIT {limit} OFFSET {offset}");
        let page = rows(db, page_sql, values).await?;
        Ok(BoardPage { rows: page, total })
    }
}

/// The row shape both leaderboards select: the entity's columns, then the names.
pub(crate) const TESTS_SELECT: &str = "SELECT t.*, p.name AS project_name, r.name AS repo_name";
pub(crate) const TESTS_FROM: &str = "FROM test_cases t \
     JOIN projects p ON p.id = t.project_id \
     JOIN repos r ON r.id = p.repo_id";

pub(crate) const JOBS_SELECT: &str = "SELECT j.*, p.name AS pipeline_name, p.provider AS provider, \
     p.repo_id AS repo_id, r.name AS repo_name";
pub(crate) const JOBS_FROM: &str = "FROM jobs j \
     JOIN pipelines p ON p.id = j.pipeline_id \
     JOIN repos r ON r.id = p.repo_id";

/// The case-insensitive literal substring search over a Test's identity.
///
/// One pattern bind covers every column, so `pattern` is repeated.
fn test_search(pattern: &str) -> String {
    format!(
        "(t.name ILIKE {pattern} ESCAPE '\\' \
         OR t.classname ILIKE {pattern} ESCAPE '\\' \
         OR t.file ILIKE {pattern} ESCAPE '\\')"
    )
}

/// The case-insensitive literal substring search over a Job's name and Pipeline.
fn job_search(pattern: &str) -> String {
    format!("(j.name ILIKE {pattern} ESCAPE '\\' OR p.name ILIKE {pattern} ESCAPE '\\')")
}

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
pub(crate) fn test_out(
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
        failure_category: stored_category(
            get::<Option<String>>(row, "failure_category")?.as_deref(),
        ),
        clean_streak: i64::from(get::<i32>(row, "clean_streak")?),
        fail_streak,
        broken: is_broken(fail_streak, broken_streak),
        trend,
        last_status: execution_status(&get::<String>(row, "last_status")?)?,
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
pub(crate) fn job_out(
    row: &QueryResult,
    threshold: f64,
    trend: Option<Trend>,
) -> Result<JobOut, DbError> {
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

impl ListOptions {
    /// The MCP "top flaky" listing: score order, one page, and flaky-only unless
    /// the caller asked for suspect rows too.
    #[must_use]
    pub fn top_flaky(page_size: i64, include_suspect: bool, new_within_days: Option<i64>) -> Self {
        Self {
            include_stable: false,
            flaky_only: !include_suspect,
            sort: SortKey::Score,
            page: 1,
            page_size,
            q: None,
            has_issue: None,
            new_within_days,
        }
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

/// Reads each row into its response shape, with the score it had a week ago.
///
/// The past scores are one query for the whole page, whatever its size.
async fn read_items<T>(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    rows: &[QueryResult],
    settings: &ReadSettings,
    out: impl Fn(&QueryResult, Option<Trend>) -> Result<T, DbError>,
) -> Result<Vec<T>, DbError> {
    let ids: Vec<i32> = rows
        .iter()
        .map(|row| get::<i32>(row, "id"))
        .collect::<Result<_, _>>()?;
    let past = score_history::past_scores(db, kind, &ids, settings.today()).await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let id: i32 = get(row, "id")?;
        let score: f64 = get(row, "flakiness_score")?;
        items.push(out(row, trend_for(score, past.get(&id).copied()))?);
    }
    Ok(items)
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
    let mut filter = Binds::default();
    scope_conditions(scope, "r.name", "p.name", &mut filter);
    if let Some(file) = filters.file.as_deref().filter(|file| !file.is_empty()) {
        let placeholder = filter.bind(Value::from(format!("%{}%", escape_like(file))));
        filter.raw(format!("t.file ILIKE {placeholder} ESCAPE '\\'"));
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
            let placeholder = filter.bind(Value::from(settings.broken_streak));
            filter.raw(format!("t.fail_streak >= {placeholder}"));
        } else {
            // Detection disabled: `is_broken` is false for every row, so nothing is broken.
            filter.raw("FALSE");
        }
    }
    // A quarantine or broken filter keeps the stable rows it selected by.
    let keeps_stable_rows = filters.quarantined == Some(true) || filters.broken;
    let board = Board {
        entity: &TESTS_BOARD,
        options,
        filter,
        tier: TierFilter::of(options, keeps_stable_rows),
        threshold: settings.flake_threshold,
        today: settings.today(),
    };
    let page = board.run(db).await?;
    let items = read_items(db, EntityKind::Test, &page.rows, settings, |row, trend| {
        test_out(row, settings.flake_threshold, settings.broken_streak, trend)
    })
    .await?;
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
    let mut filter = Binds::default();
    if let Some(repo) = repo {
        filter.eq("r.name", Value::from(repo.to_owned()));
    }
    if let Some(pipeline) = &filters.pipeline {
        filter.eq("p.name", Value::from(pipeline.clone()));
    }
    // ADR 0004: Jobs have no Quarantine, so no Job listing keeps stable rows.
    let board = Board {
        entity: &JOBS_BOARD,
        options,
        filter,
        tier: TierFilter::of(options, false),
        threshold: settings.flake_threshold,
        today: settings.today(),
    };
    let page = board.run(db).await?;
    let items = read_items(db, EntityKind::Job, &page.rows, settings, |row, trend| {
        job_out(row, settings.flake_threshold, trend)
    })
    .await?;
    Ok(JobPage {
        items,
        total: page.total,
        page: options.page,
        page_size: options.page_size,
    })
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
    let new_flaky = new_flaky_count(db, &TEST_TALLY, threshold, today, |binds| {
        scope_conditions(scope, "r.name", "p.name", binds);
    })
    .await?;
    let (executions_30d, failures_30d) = window_sums(db, &TEST_TALLY, today, |binds| {
        scope_conditions(scope, "r.name", "p.name", binds);
    })
    .await?;
    Ok(SummaryOut {
        total_tests: counts.total,
        flaky_tests: counts.flaky,
        suspect_tests: counts.suspect,
        confirmed_flaky_tests: counts.proven,
        new_flaky_7d: new_flaky,
        total_runs: counts.total_runs,
        executions_30d,
        failures_30d,
        flake_threshold: threshold,
        category_counts: category_counts(db, scope).await?,
    })
}

/// The dashboard's tile counts and the Run count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TestCounts {
    /// Every Test in scope.
    total: i64,
    /// Tests at or above the Flake threshold.
    flaky: i64,
    /// Tests above zero but below the threshold.
    suspect: i64,
    /// Tests with at least one Proven flake.
    proven: i64,
    /// Runs in scope.
    total_runs: i64,
}

/// The five dashboard tile counts and the Run count, in one statement.
async fn test_counts(
    db: &impl ConnectionTrait,
    scope: &Scope,
    threshold: f64,
) -> Result<TestCounts, DbError> {
    // `$1` is the threshold everywhere; the Runs subquery and the scope follow.
    let mut runs = Binds::after(2);
    scope_conditions(scope, "trr.name", "tp.name", &mut runs);
    let runs_sql = format!(
        "SELECT count(*) FROM test_runs tr \
           JOIN projects tp ON tp.id = tr.project_id \
           JOIN repos trr ON trr.id = tp.repo_id{}",
        runs.suffix()
    );
    let mut counts = Binds::after(2 + runs.values().len());
    scope_conditions(scope, "r.name", "p.name", &mut counts);
    let mut values = vec![Value::from(threshold)];
    values.extend(runs.into_values());
    values.extend(counts.clone().into_values());
    let counts_sql = format!(
        "SELECT count(*) AS total, \
                count(*) FILTER (WHERE t.flakiness_score >= $1) AS flaky, \
                count(*) FILTER (WHERE t.flakiness_score > 0 AND t.flakiness_score < $1) AS suspect, \
                count(*) FILTER (WHERE t.confirmed_flake_count > 0) AS confirmed, \
                ({runs_sql}) AS total_runs \
           FROM test_cases t \
           JOIN projects p ON p.id = t.project_id \
           JOIN repos r ON r.id = p.repo_id{}",
        counts.suffix()
    );
    let row = required_one(db, counts_sql, values, "summary").await?;
    Ok(TestCounts {
        total: get(&row, "total")?,
        flaky: get(&row, "flaky")?,
        suspect: get(&row, "suspect")?,
        proven: get(&row, "confirmed")?,
        total_runs: get(&row, "total_runs")?,
    })
}

/// The scored table and its Score history, as a headline count reads them.
struct Tally {
    /// The entity the counts are about.
    kind: EntityKind,
    /// The table alias the conditions name.
    alias: &'static str,
    /// The scored table joined to its Repo.
    from: &'static str,
    /// The Score history joined to the scored table and its Repo.
    history_from: &'static str,
    /// How the entity's queries are named in the error a statement that
    /// answered nothing gives: nothing for a Test, `job ` for a Job.
    prefix: &'static str,
}

/// The Tests' headline counts.
const TEST_TALLY: Tally = Tally {
    kind: EntityKind::Test,
    alias: "t",
    from: "test_cases t \
        JOIN projects p ON p.id = t.project_id \
        JOIN repos r ON r.id = p.repo_id",
    history_from: "test_score_history h \
        JOIN test_cases t ON t.id = h.test_case_id \
        JOIN projects p ON p.id = t.project_id \
        JOIN repos r ON r.id = p.repo_id",
    prefix: "",
};

/// The Jobs' headline counts.
const JOB_TALLY: Tally = Tally {
    kind: EntityKind::Job,
    alias: "j",
    from: "jobs j \
        JOIN pipelines p ON p.id = j.pipeline_id \
        JOIN repos r ON r.id = p.repo_id",
    history_from: "job_score_history h \
        JOIN jobs j ON j.id = h.job_id \
        JOIN pipelines p ON p.id = j.pipeline_id \
        JOIN repos r ON r.id = p.repo_id",
    prefix: "job ",
};

/// How many rows became flaky inside [`NEW_FLAKY_DAYS`].
///
/// `filter` adds the caller's own conditions: the Test side scopes by Repo and
/// Project, and the Job side by Repo alone.
async fn new_flaky_count(
    db: &impl ConnectionTrait,
    tally: &Tally,
    threshold: f64,
    today: NaiveDate,
    filter: impl FnOnce(&mut Binds),
) -> Result<i64, DbError> {
    let cutoff = today - Duration::days(NEW_FLAKY_DAYS);
    // The lateral subquery's cut-off is a literal day, so the binds stay simple.
    let old_point = score_history::newest_history_before(tally.kind, tally.alias)
        .replace("$1::DATE", &format!("DATE '{}'", cutoff.format("%Y-%m-%d")));
    let mut new_flaky = Binds::after(2);
    new_flaky.raw("(past.score IS NULL OR past.score < $1)");
    filter(&mut new_flaky);
    let new_sql = format!(
        "SELECT count(*) FILTER (WHERE {}.flakiness_score >= $1) AS new_flaky \
           FROM {} \
           LEFT JOIN LATERAL ({old_point}) AS past ON true{}",
        tally.alias,
        tally.from,
        new_flaky.suffix()
    );
    let mut new_binds = vec![Value::from(threshold)];
    new_binds.extend(new_flaky.into_values());
    let what = format!("{}new-flaky", tally.prefix);
    let row = required_one(db, new_sql, new_binds, &what).await?;
    get(&row, "new_flaky")
}

/// One runner's Executions and the failures nothing explained.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RunnerCounts {
    /// Executions the runner reported.
    executions: i64,
    /// Failed Executions no failing Test explained.
    unexplained: i64,
}

/// The Executions and failures of the last [`SUMMARY_WINDOW_DAYS`].
async fn window_sums(
    db: &impl ConnectionTrait,
    tally: &Tally,
    today: NaiveDate,
    filter: impl FnOnce(&mut Binds),
) -> Result<(i64, i64), DbError> {
    let cutoff = today - Duration::days(SUMMARY_WINDOW_DAYS);
    let mut sums = Binds::after(2);
    sums.raw("h.day > $1");
    filter(&mut sums);
    let sums_sql = format!(
        "SELECT coalesce(sum(h.executions), 0) AS executions, \
                coalesce(sum(h.failures), 0) AS failures \
           FROM {}{}",
        tally.history_from,
        sums.suffix()
    );
    let mut sum_binds = vec![Value::from(cutoff)];
    sum_binds.extend(sums.into_values());
    let what = format!("{}thirty-day sums", tally.prefix);
    let row = required_one(db, sums_sql, sum_binds, &what).await?;
    Ok((get(&row, "executions")?, get(&row, "failures")?))
}

/// Failing Executions per Failure category, for the flaky Tests in scope.
async fn category_counts(
    db: &impl ConnectionTrait,
    scope: &Scope,
) -> Result<BTreeMap<FailureCategory, i64>, DbError> {
    let mut categories = Binds::after(1);
    categories.raw("t.flakiness_score > 0");
    categories.raw("t.failure_category IS NOT NULL");
    scope_conditions(scope, "r.name", "p.name", &mut categories);
    let categories_sql = format!(
        "SELECT t.failure_category AS category, count(*) AS count \
           FROM test_cases t \
           JOIN projects p ON p.id = t.project_id \
           JOIN repos r ON r.id = p.repo_id{} \
          GROUP BY t.failure_category",
        categories.suffix()
    );
    let mut category_counts: BTreeMap<FailureCategory, i64> = BTreeMap::new();
    for category in FailureCategory::ALL {
        let _ = category_counts.insert(category, 0);
    }
    for row in rows(db, categories_sql, categories.into_values()).await? {
        let category = stored_category(get::<Option<String>>(&row, "category")?.as_deref());
        if let Some(category) = category {
            let _ = category_counts.insert(category, get(&row, "count")?);
        }
    }
    Ok(category_counts)
}

/// The Failure category one stored spelling names.
///
/// `failure_category` is free text in the baseline schema, so a row can hold a
/// word the five categories do not name. It reads as no category, which is the
/// same rule the summary's per-category counts apply.
fn stored_category(text: Option<&str>) -> Option<FailureCategory> {
    text.and_then(FailureCategory::from_wire)
}

/// One Execution in a Test's history, with the Run facts it carries.
fn execution_out(row: &QueryResult) -> Result<ExecutionOut, DbError> {
    Ok(ExecutionOut {
        id: get(row, "id")?,
        status: execution_status(&get::<String>(row, "status")?)?,
        duration: get(row, "duration")?,
        message: get(row, "message")?,
        details: get(row, "details")?,
        created_at: timestamp(row, "created_at")?,
        commit_sha: get(row, "commit_sha")?,
        branch: get(row, "branch")?,
        ci_run_id: get(row, "ci_run_id")?,
        attempt: i64::from(get::<i32>(row, "attempt")?),
        failure_category: stored_category(
            get::<Option<String>>(row, "failure_category")?.as_deref(),
        ),
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
    let past = score_history::past_scores(db, EntityKind::Test, &[stored_id], today).await?;
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
            url: last_failing_sha.as_deref().map(|sha| {
                permalink(&LocationRef {
                    repo: &test.repo,
                    commit_sha: sha,
                    project_root: &root,
                    file,
                    line: test.line,
                })
            }),
        }),
        None => None,
    };

    let history = score_history::window_rows(db, EntityKind::Test, stored_id, today).await?;
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
    let mut binds = Binds::after(2);
    binds.raw(test_search("$1"));
    scope_conditions(scope, "r.name", "p.name", &mut binds);
    let limit_placeholder = binds.bind(Value::from(limit));
    let sql = format!(
        "{TESTS_SELECT} {TESTS_FROM}{} ORDER BY t.flakiness_score DESC, t.id LIMIT {limit_placeholder}",
        binds.suffix()
    );
    let mut values = vec![Value::from(format!("%{}%", escape_like(query)))];
    values.extend(binds.into_values());
    let rows = rows(db, sql, values).await?;
    read_items(db, EntityKind::Test, &rows, settings, |row, trend| {
        test_out(row, settings.flake_threshold, settings.broken_streak, trend)
    })
    .await
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
    let mut binds = Binds::new();
    binds.eq("r.name", Value::from(repo.to_owned()));
    binds.eq("p.name", Value::from(project.to_owned()));
    binds.eq("t.name", Value::from(name.to_owned()));
    if let Some(classname) = classname {
        binds.eq("t.classname", Value::from(classname.to_owned()));
    }
    let sql = format!(
        "SELECT t.id FROM test_cases t \
           JOIN projects p ON p.id = t.project_id \
           JOIN repos r ON r.id = p.repo_id{} \
          ORDER BY t.id",
        binds.suffix()
    );
    let mut found = Vec::new();
    for row in rows(db, sql, binds.into_values()).await? {
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
    let past = score_history::past_scores(db, EntityKind::Job, &[stored_id], today).await?;
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

    let history = score_history::window_rows(db, EntityKind::Job, stored_id, today).await?;
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
            status: execution_status(&get::<String>(&row, "status")?)?,
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
    let mut label_counts: BTreeMap<Vec<String>, RunnerCounts> = BTreeMap::new();
    let mut name_counts: BTreeMap<String, RunnerCounts> = BTreeMap::new();
    let mut name_labels: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows(db, sql, values).await? {
        let runner_name: String = get(&row, "runner_name")?;
        let labels = runner_labels(&row)?;
        let status: String = get(&row, "status")?;
        let explained: bool = get(&row, "explained")?;
        let unexplained = i64::from(status == "failed" && !explained);
        let counts = label_counts.entry(labels.clone()).or_default();
        counts.executions += 1;
        counts.unexplained += unexplained;
        if !runner_name.is_empty() {
            let counts = name_counts.entry(runner_name.clone()).or_default();
            counts.executions += 1;
            counts.unexplained += unexplained;
            name_labels.entry(runner_name).or_default().extend(labels);
        }
    }
    let mut stats: Vec<RunnerStat> = Vec::new();
    for (labels, counts) in label_counts {
        stats.push(RunnerStat {
            labels,
            name: None,
            executions: counts.executions,
            unexplained: counts.unexplained,
        });
    }
    for (name, counts) in name_counts {
        if counts.executions < 2 {
            continue;
        }
        stats.push(RunnerStat {
            labels: name_labels
                .get(&name)
                .map(|labels| labels.iter().cloned().collect())
                .unwrap_or_default(),
            name: Some(name),
            executions: counts.executions,
            unexplained: counts.unexplained,
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
    let mut counts = Binds::after(2);
    if let Some(repo) = repo {
        counts.eq("r.name", Value::from(repo.to_owned()));
    }
    let counts_sql = format!(
        "SELECT count(*) AS total, \
                count(*) FILTER (WHERE j.flakiness_score >= $1) AS flaky, \
                count(*) FILTER (WHERE j.flakiness_score > 0 AND j.flakiness_score < $1) AS suspect, \
                count(*) FILTER (WHERE j.confirmed_flake_count > 0) AS confirmed \
           FROM jobs j \
           JOIN pipelines p ON p.id = j.pipeline_id \
           JOIN repos r ON r.id = p.repo_id{}",
        counts.suffix()
    );
    let mut values = vec![Value::from(threshold)];
    values.extend(counts.into_values());
    let row = required_one(db, counts_sql, values, "job summary").await?;
    let total: i64 = get(&row, "total")?;
    let flaky: i64 = get(&row, "flaky")?;
    let suspect: i64 = get(&row, "suspect")?;
    let confirmed: i64 = get(&row, "confirmed")?;

    let today = settings.today();
    let new_flaky = new_flaky_count(db, &JOB_TALLY, threshold, today, |binds| {
        if let Some(repo) = repo {
            binds.eq("r.name", Value::from(repo.to_owned()));
        }
    })
    .await?;

    let (executions_30d, failures_30d) = window_sums(db, &JOB_TALLY, today, |binds| {
        if let Some(repo) = repo {
            binds.eq("r.name", Value::from(repo.to_owned()));
        }
    })
    .await?;

    let mut pipelines_in = Binds::after(1);
    if let Some(repo) = repo {
        pipelines_in.eq("r.name", Value::from(repo.to_owned()));
    }
    let pipelines_sql = format!(
        "SELECT DISTINCT p.name AS pipeline \
           FROM pipelines p JOIN repos r ON r.id = p.repo_id{} \
          ORDER BY p.name",
        pipelines_in.suffix()
    );
    let mut pipelines = Vec::new();
    for row in rows(db, pipelines_sql, pipelines_in.into_values()).await? {
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
        job_search("$1")
    );
    let values = vec![
        Value::from(format!("%{}%", escape_like(query))),
        Value::from(repo.to_owned()),
        Value::from(limit),
    ];
    let rows = rows(db, sql, values).await?;
    read_items(db, EntityKind::Job, &rows, settings, |row, trend| {
        job_out(row, settings.flake_threshold, trend)
    })
    .await
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
    let mut binds = Binds::new();
    binds.eq("r.name", Value::from(repo.to_owned()));
    binds.eq("j.name", Value::from(name.to_owned()));
    if let Some(pipeline) = pipeline {
        binds.eq("p.name", Value::from(pipeline.to_owned()));
    }
    let sql = format!(
        "SELECT j.id FROM jobs j \
           JOIN pipelines p ON p.id = j.pipeline_id \
           JOIN repos r ON r.id = p.repo_id{} \
          ORDER BY j.id",
        binds.suffix()
    );
    let mut found = Vec::new();
    for row in rows(db, sql, binds.into_values()).await? {
        found.push(id(get::<i32>(&row, "id")?));
    }
    Ok(found)
}

/// One execution of a failing (Test, Run) pair, as the CI run listing reads it.
#[derive(Debug, Clone)]
struct PairExecution {
    status: ExecutionStatus,
    failure_category: Option<FailureCategory>,
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
        ci_run_failed_jobs(db, repo_id, &repo_name, ci_run_id, attempt, settings).await?;
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
        match ReportStatus::from_wire(&status) {
            Some(ReportStatus::Pending) => counts.pending = count,
            Some(ReportStatus::Processed) => counts.processed = count,
            Some(ReportStatus::Failed) => counts.failed = count,
            None => {}
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
    let count = required_one(
        db,
        total_sql.to_owned(),
        values.clone(),
        "CI run failure count",
    )
    .await?;
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
            status: execution_status(&get::<String>(&row, "status")?)?,
            failure_category: stored_category(
                get::<Option<String>>(&row, "failure_category")?.as_deref(),
            ),
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
        .filter(|execution| execution.status.is_failing())
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
        status: last.status,
        failed_attempts: i64::try_from(failing.len()).unwrap_or(i64::MAX),
        failure_category: last_failing.failure_category,
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
    repo_name: &str,
    ci_run_id: &str,
    attempt: i32,
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
        Binds, EntityKind, ListOptions, NEW_FLAKY_DAYS, ReadError, ReadSettings, SortKey,
        TierFilter, newest_history_join, scope_conditions,
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
    fn a_scope_binds_from_the_number_the_statement_reserved() {
        let scope = super::Scope::new(Some("acme/app".to_owned()), Some("backend".to_owned()))
            .expect("a scope");
        let mut binds = Binds::after(3);
        scope_conditions(&scope, "r.name", "p.name", &mut binds);
        assert_eq!(binds.suffix(), " WHERE r.name = $3 AND p.name = $4");
        assert_eq!(binds.values().len(), 2);
    }

    #[test]
    fn the_tier_filter_table_names_every_state_the_options_can_be_in() {
        let options = |change: fn(&mut ListOptions)| {
            let mut options = ListOptions::default();
            change(&mut options);
            options
        };
        // The stable-hiding default.
        assert_eq!(
            TierFilter::of(&options(|_| {}), false),
            TierFilter::AboveZero
        );
        // `include_stable` shows everything.
        assert_eq!(
            TierFilter::of(&options(|options| options.include_stable = true), false),
            TierFilter::All
        );
        // A search shows everything, because the caller narrowed it already.
        assert_eq!(
            TierFilter::of(
                &options(|options| options.q = Some("checkout".to_owned())),
                false
            ),
            TierFilter::All
        );
        // An empty search is no search: the default still hides stable rows.
        assert_eq!(
            TierFilter::of(&options(|options| options.q = Some(String::new())), false),
            TierFilter::AboveZero
        );
        // Quarantine and broken filters keep the stable rows they selected.
        assert_eq!(TierFilter::of(&options(|_| {}), true), TierFilter::All);
        // The flaky-only listing needs no stable rows and outranks every other rule.
        assert_eq!(
            TierFilter::of(
                &options(|options| {
                    options.flaky_only = true;
                    options.include_stable = true;
                }),
                true
            ),
            TierFilter::FlakyOnly
        );
        // The "became flaky recently" window is flaky-only too.
        assert_eq!(
            TierFilter::of(
                &options(|options| {
                    options.new_within_days = Some(NEW_FLAKY_DAYS);
                    options.include_stable = true;
                }),
                false
            ),
            TierFilter::FlakyOnly
        );
    }

    #[test]
    fn a_scope_reads_back_only_what_its_constructor_accepted() {
        let scope = super::Scope::new(Some("acme/app".to_owned()), Some("backend".to_owned()))
            .expect("a scope");
        assert_eq!(scope.repo(), Some("acme/app"));
        assert_eq!(scope.project(), Some("backend"));
        let empty = super::Scope::default();
        assert_eq!(empty.repo(), None);
        assert_eq!(empty.project(), None);
        // Test and Job flows build a scope from a Repo and a Project name, and the
        // fields are private, so a Project cannot be set without a Repo from outside
        // this module.
        let repo_only = super::Scope::new(Some("acme/app".to_owned()), None).expect("a scope");
        assert_eq!(repo_only.repo(), Some("acme/app"));
        assert_eq!(repo_only.project(), None);
    }

    #[test]
    fn a_scope_without_a_repo_refuses_a_project() {
        let error = super::Scope::new(None, Some("backend".to_owned()))
            .expect_err("a Project without a Repo is refused");
        assert_eq!(error, super::ScopeError::ProjectWithoutRepo);
        assert_eq!(error.to_string(), "project filter requires repo");
        let boxed: Box<dyn std::error::Error> = Box::new(error);
        assert_eq!(boxed.to_string(), "project filter requires repo");
    }

    #[test]
    fn the_new_flaky_window_reads_a_literal_day() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 1, 20).expect("a day");
        let join = newest_history_join(
            EntityKind::Test,
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
