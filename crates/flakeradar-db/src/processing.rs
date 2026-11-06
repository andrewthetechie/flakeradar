//! The Report processor: one pending Report becomes a Run or Job executions,
//! and Tests and Jobs are rescored.
//!
//! This is the frozen Python server's `processing.py`, statement by statement:
//! scoring depends on Execution order, so one Report is processed at a time and
//! every bulk statement stays in the same shape (and therefore consumes the
//! same sequence values) as the Python server's.

// Rust guideline compliant 2026-07-21

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use flakeradar_core::{
    EntityKind, ExecutionStatus, FailureCategory, JobStatus, ParseError, ParsedCase,
    PipelineReportIn, ReportKind, ReportStatus, ScoredExecution, Tier, TierChange,
    branch_scoped_fail_streak, branch_scoped_score, branch_scoped_streak, classify_failure,
    defaults, dominant_category, is_broken, parse_junit_xml, tier_for,
};
use sea_orm::entity::prelude::DateTimeWithTimeZone;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, DbErr, EntityTrait, FromQueryResult,
    IntoActiveModel, QueryFilter, Set, Statement, TransactionSession, TransactionTrait, Value,
};
use serde_json::{Value as JsonValue, json};

use crate::attribution::{
    int_array, job_execution_explained_sql, job_window_ids_sql, test_window_ids_sql,
};
use crate::batching::chunks;
use crate::clock::{SharedClock, SystemClock};
use crate::entities::{pipelines, reports, repos, test_cases, test_executions, test_runs};
use crate::error::DbError;
use crate::report_intake::{
    complete_report, fail_report, requeue_report, stored_junit_body, stored_pipeline_report,
};
use crate::score_history::{HistoryUpdate, write_today_history};
use crate::sql::{execution_status, get, placeholder_list, rows, value_row};

/// The longest error text a failed Report keeps.
pub const ERROR_MAX: usize = 2000;

/// The score window as a bind parameter: a setting far past `i32` is capped,
/// because the window counts rows, not a database column.
fn window_param(window: usize) -> i32 {
    i32::try_from(window).unwrap_or(i32::MAX)
}

/// The settings the processor reads, with the clock the Score history is keyed on.
///
/// The binary's `Settings` builds one of these; the database layer cannot depend
/// on the binary's config module, and a test states only the values it changes.
#[derive(Clone)]
pub struct ProcessingSettings {
    /// How many recent Executions the Flakiness score weighs.
    pub score_window: usize,
    /// The per-step decay of that weight.
    pub score_decay: f64,
    /// Consecutive Default-branch failures at which a Test counts as broken.
    pub broken_streak: i64,
    /// The Flakiness score at or above which a Test or Job counts as flaky.
    pub flake_threshold: f64,
    /// How many retries a transient failure may spend.
    pub max_report_retries: i32,
    /// Where "now" and "today" come from.
    pub clock: SharedClock,
}

impl Default for ProcessingSettings {
    fn default() -> Self {
        Self {
            score_window: defaults::SCORE_WINDOW,
            score_decay: defaults::SCORE_DECAY,
            broken_streak: defaults::BROKEN_STREAK,
            flake_threshold: defaults::FLAKE_THRESHOLD,
            max_report_retries: defaults::MAX_REPORT_RETRIES,
            clock: std::sync::Arc::new(SystemClock),
        }
    }
}

impl fmt::Debug for ProcessingSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessingSettings")
            .field("score_window", &self.score_window)
            .field("score_decay", &self.score_decay)
            .field("broken_streak", &self.broken_streak)
            .field("flake_threshold", &self.flake_threshold)
            .field("max_report_retries", &self.max_report_retries)
            .finish_non_exhaustive()
    }
}

/// What one `process_next` call did.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessOutcome {
    /// The Report that was claimed.
    pub report_id: i32,
    /// Where the Report stands: `processed`, `failed`, or `pending` after a requeue.
    pub status: ReportStatus,
    /// The Run a JUnit Report became.
    pub run_id: Option<i32>,
    /// The counts stored on a processed Report.
    pub counts: Option<JsonValue>,
    /// The Tests this Report touched, sorted.
    pub touched_test_ids: Vec<i32>,
    /// Why processing failed, when it did.
    pub error: Option<String>,
    /// The Jobs this Report touched, sorted.
    pub touched_job_ids: Vec<i32>,
    /// The Test and Job tier crossings, silent when the Default branch changed.
    pub tier_changes: Vec<TierChange>,
    /// The retries already spent on this Report after a requeue.
    pub retry_count: i32,
}

/// A processing failure: whether a retry could succeed, and the text to store.
///
/// Only `process_next` reads it, so it stays inside this module; the caller sees
/// the failure as a stored Report status and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessError {
    /// True for a failure that may succeed on a later attempt.
    pub transient: bool,
    /// `TypeName: message`, capped at [`ERROR_MAX`].
    pub message: String,
}

impl ProcessError {
    /// A failure that no retry can fix.
    #[must_use]
    fn permanent(message: impl Into<String>) -> Self {
        Self {
            transient: false,
            message: message.into(),
        }
    }
}

impl From<DbError> for ProcessError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::Database(error) => {
                let transient = transient_db_error(&error);
                let name = if transient {
                    "OperationalError"
                } else {
                    "DBAPIError"
                };
                Self {
                    transient,
                    message: format!("{name}: {error}"),
                }
            }
            other => Self::permanent(format!("DataError: {other}")),
        }
    }
}

impl From<DbErr> for ProcessError {
    fn from(error: DbErr) -> Self {
        Self::from(DbError::Database(error))
    }
}

impl From<ParseError> for ProcessError {
    fn from(error: ParseError) -> Self {
        Self::permanent(format!("ParseError: {error}"))
    }
}

/// SQLSTATE classes a retry may survive: connection, insufficient resources and
/// operator intervention, plus serialization failure and deadlock.
fn transient_sqlstate(code: &str) -> bool {
    code.starts_with("08")
        || code.starts_with("53")
        || code.starts_with("57")
        || code == "40001"
        || code == "40P01"
}

/// Whether a driver error belongs to the connection/operational family Python
/// retried (`OperationalError`, `InterfaceError`).
fn transient_db_error(error: &DbErr) -> bool {
    use sea_orm::RuntimeErr;
    use sea_orm::sqlx::error::Error as SqlxError;

    let runtime = match error {
        DbErr::Conn(error) | DbErr::Exec(error) | DbErr::Query(error) => error,
        DbErr::ConnectionAcquire(_) => return true,
        _ => return false,
    };
    let RuntimeErr::SqlxError(error) = runtime else {
        return false;
    };
    match error.as_ref() {
        SqlxError::Io(_) | SqlxError::PoolTimedOut | SqlxError::PoolClosed => true,
        SqlxError::Database(database) => database
            .code()
            .is_some_and(|code| transient_sqlstate(code.as_ref())),
        _ => false,
    }
}

/// Locks the oldest pending Report for this transaction (`FOR UPDATE SKIP LOCKED`).
///
/// Only this query skips a locked row: the rest of the transaction must wait for
/// an admin delete of the Report's Repo or Project, as Python's did.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the claim or a row read fails.
pub async fn claim_next_report(
    db: &impl ConnectionTrait,
) -> Result<Option<reports::Model>, DbError> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT * FROM reports WHERE status = $1 ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED",
        [Value::from(ReportStatus::Pending.as_str())],
    );
    match db.query_one_raw(statement).await? {
        Some(row) => Ok(Some(reports::Model::from_query_result(&row, "")?)),
        None => Ok(None),
    }
}

/// Writes a Report that a transition changed, keeping every other column.
///
/// SeaORM 2 turns a `Model` into an `ActiveModel` of `Unchanged` values, so an
/// `UPDATE` built from a mutated model sends no column at all and silently
/// leaves the row alone. `reset_all` promotes the read values back to `Set`.
pub(crate) async fn write_report(
    report: &reports::Model,
    db: &impl ConnectionTrait,
) -> Result<(), DbError> {
    report
        .clone()
        .into_active_model()
        .reset_all()
        .update(db)
        .await?;
    Ok(())
}

/// Parses a JUnit body and classifies each failing case: the CPU half, run off
/// the async thread.
fn parse_and_classify(
    body: &[u8],
    line_base: i32,
) -> Result<Vec<(ParsedCase, Option<FailureCategory>)>, ParseError> {
    let parsed = parse_junit_xml(body, i64::from(line_base))?;
    Ok(parsed
        .into_iter()
        .map(|case| {
            let category = case
                .status
                .is_failing()
                .then(|| classify_failure(&case.message, &case.details));
            (case, category)
        })
        .collect())
}

/// The counts a processed Report stores.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counts {
    passed: i64,
    failed: i64,
    error: i64,
    skipped: i64,
    retried: i64,
    duplicate: i64,
}

impl Counts {
    /// Counts one Execution's status.
    fn add(&mut self, status: ExecutionStatus) {
        match status {
            ExecutionStatus::Passed => self.passed += 1,
            ExecutionStatus::Failed => self.failed += 1,
            ExecutionStatus::Error => self.error += 1,
            ExecutionStatus::Skipped => self.skipped += 1,
        }
    }

    /// The JSON a JUnit Report stores.
    fn junit_json(self) -> JsonValue {
        json!({
            "passed": self.passed,
            "failed": self.failed,
            "error": self.error,
            "skipped": self.skipped,
            "retried": self.retried,
        })
    }

    /// The JSON a Pipeline Report stores.
    fn pipeline_json(self) -> JsonValue {
        json!({
            "passed": self.passed,
            "failed": self.failed,
            "skipped": self.skipped,
            "duplicate": self.duplicate,
        })
    }
}

/// One entity's rescore result: the row a bulk update writes.
///
/// A Test and a Job write the same Flakiness score, Proven flake count, Clean
/// streak and fixed-note reset. A Test also writes its Fail streak and its
/// dominant Failure category, which a Job has no column for, so [`TEST_TABLE`]
/// writes those and [`JOB_TABLE`] leaves them out.
#[derive(Debug, Clone, Copy)]
struct Update {
    id: i32,
    flakiness_score: f64,
    confirmed_flake_count: i32,
    clean_streak: i32,
    clear_fixed_note: bool,
    /// The Fail streak. Only a Test scores one.
    fail_streak: i32,
    /// The dominant Failure category. Only a Test is categorised.
    failure_category: Option<FailureCategory>,
}

/// Claim and process the oldest pending Report; `None` when the queue is empty.
///
/// The work runs in a SAVEPOINT: on any error it is rolled back and, in the same
/// outer transaction, the Report is either requeued for a retry or permanently
/// marked failed, so one bad Report can never block the queue.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the claim, the commit or the terminal
/// Report write fails. A processing error is reported in [`ProcessOutcome`].
pub async fn process_next(
    db: &(impl ConnectionTrait + TransactionTrait),
    settings: &ProcessingSettings,
) -> Result<Option<ProcessOutcome>, DbError> {
    let transaction = db.begin().await?;
    let mut report = claim_next_report(&transaction).await?;
    let Some(report) = report.as_mut() else {
        transaction.rollback().await?;
        return Ok(None);
    };
    let savepoint = transaction.begin().await?;
    match process_report(&savepoint, report, settings).await {
        Ok(outcome) => {
            savepoint.commit().await?;
            transaction.commit().await?;
            Ok(Some(outcome))
        }
        Err(error) => {
            savepoint.rollback().await?;
            let message: String = error.message.chars().take(ERROR_MAX).collect();
            if error.transient
                && i64::from(report.retry_count) < i64::from(settings.max_report_retries)
            {
                requeue_report(report, Some(&message));
                let retry_count = report.retry_count;
                write_report(report, &transaction).await?;
                transaction.commit().await?;
                return Ok(Some(ProcessOutcome {
                    report_id: report.id,
                    status: ReportStatus::Pending,
                    run_id: None,
                    counts: None,
                    touched_test_ids: Vec::new(),
                    error: Some(message),
                    touched_job_ids: Vec::new(),
                    tier_changes: Vec::new(),
                    retry_count,
                }));
            }
            let now = settings.clock.now();
            fail_report(report, now, &message);
            write_report(report, &transaction).await?;
            transaction.commit().await?;
            Ok(Some(ProcessOutcome {
                report_id: report.id,
                status: ReportStatus::Failed,
                run_id: None,
                counts: None,
                touched_test_ids: Vec::new(),
                error: Some(message),
                touched_job_ids: Vec::new(),
                tier_changes: Vec::new(),
                retry_count: report.retry_count,
            }))
        }
    }
}

/// Persists one claimed Report. The caller owns the transaction.
async fn process_report(
    db: &impl ConnectionTrait,
    report: &mut reports::Model,
    settings: &ProcessingSettings,
) -> Result<ProcessOutcome, ProcessError> {
    if report.kind == ReportKind::Pipeline.as_str() {
        return process_pipeline_report(db, report, settings).await;
    }
    let (body, line_base) = stored_junit_body(report);
    let body = body.to_vec();
    let classified = tokio::task::spawn_blocking(move || parse_and_classify(&body, line_base))
        .await
        .map_err(|error| ProcessError::permanent(format!("RuntimeError: {error}")))??;
    let parsed: Vec<ParsedCase> = classified.iter().map(|(case, _)| case.clone()).collect();
    let now = settings.clock.now();

    // A reported Default branch changes the Repo, so apply it before inserting
    // anything: every Test in the Repo is rescored under the new value.
    let branch_changed = apply_default_branch(db, report.repo_id, report.default_branch.as_deref())
        .await
        .map_err(ProcessError::from)?;

    if let Some(root) = &report.root {
        // Write only when the value actually changes, so a run of Reports with
        // the same root never rewrites (and briefly locks) the Project row.
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE projects SET root = $1 WHERE id = $2 AND root <> $1",
            [Value::from(root.clone()), Value::from(report.project_id)],
        ))
        .await?;
    }

    let project_id = report
        .project_id
        .ok_or_else(|| ProcessError::permanent("DataError: a JUnit Report has no Project"))?;
    let run = test_runs::Entity::insert(test_runs::ActiveModel {
        project_id: Set(project_id),
        commit_sha: Set(report.commit_sha.clone()),
        branch: Set(report.branch.clone()),
        ci_run_id: Set(report.ci_run_id.clone()),
        created_at: Set(now),
        ci_job_id: Set(report.ci_job_id.clone()),
        ci_run_attempt: Set(report.ci_run_attempt),
        pipeline: Set(report.pipeline.clone()),
        ..Default::default()
    })
    .exec_with_returning(db)
    .await
    .map_err(ProcessError::from)?;

    let ids = upsert_test_cases(db, project_id, &parsed, now)
        .await
        .map_err(ProcessError::from)?;

    let RecordedTests {
        counts,
        executions,
        latest,
        day_counts,
    } = build_test_executions(&classified, &ids, run.id, now)?;

    for chunk in chunks(&executions) {
        test_executions::Entity::insert_many(chunk.to_vec())
            .exec_without_returning(db)
            .await
            .map_err(ProcessError::from)?;
    }
    update_latest_tests(db, &latest, now)
        .await
        .map_err(ProcessError::from)?;

    // Python rescored `sorted(latest)`: the ids are ordered by id, not by where a
    // Test first appeared in the report, because they set `touched_test_ids` and
    // the order the Tier changes are emitted in.
    let mut touched: Vec<i32> = latest.iter().map(|row| row.id).collect();
    touched.sort_unstable();
    let mut tier_changes = rescore(db, &touched, Some(&day_counts), settings)
        .await
        .map_err(ProcessError::from)?;
    if branch_changed {
        rescore_repo(db, report.repo_id, &tier_changes, settings)
            .await
            .map_err(ProcessError::from)?;
    }

    // A linked JUnit report explains (or un-explains) Job failures: rescore the
    // Jobs that carry this ci_job_id, so arrival order does not matter.
    let mut touched_jobs = Vec::new();
    if let Some(ci_job_id) = report.ci_job_id.clone() {
        let (job_ids, job_changes) =
            rescore_jobs_for_ci_job_id(db, report.repo_id, &ci_job_id, settings)
                .await
                .map_err(ProcessError::from)?;
        touched_jobs = job_ids;
        tier_changes.extend(job_changes);
    }

    let counts_json = counts.junit_json();
    complete_report(report, now, Some(run.id), counts_json.clone());
    write_report(report, db).await.map_err(ProcessError::from)?;
    Ok(ProcessOutcome {
        report_id: report.id,
        status: ReportStatus::Processed,
        run_id: Some(run.id),
        counts: Some(counts_json),
        touched_test_ids: touched,
        error: None,
        touched_job_ids: touched_jobs,
        // A whole-Repo rescore must stay silent (ADR 0009).
        tier_changes: if branch_changed {
            Vec::new()
        } else {
            tier_changes
        },
        retry_count: report.retry_count,
    })
}

/// One Report's Executions, its newest occurrence per Test and its day counts.
struct RecordedTests {
    counts: Counts,
    executions: Vec<test_executions::ActiveModel>,
    latest: Vec<LatestTest>,
    day_counts: BTreeMap<i32, DayCounts>,
}

/// Executions and failures one day added to a Test's or Job's Score history.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DayCounts {
    /// Executions the day recorded.
    pub executions: i32,
    /// How many of them failed.
    pub failures: i32,
}

/// Builds one Report's Executions in report order.
///
/// `attempt` counts per Test from 0; the last occurrence of a Test wins its
/// `last_status`, and only an occurrence with a Location sets one. Day counts
/// feed today's Score history row.
fn build_test_executions(
    classified: &[(ParsedCase, Option<FailureCategory>)],
    ids: &BTreeMap<String, i32>,
    run_id: i32,
    now: DateTimeWithTimeZone,
) -> Result<RecordedTests, ProcessError> {
    let mut counts = Counts::default();
    let mut attempts: BTreeMap<i32, i32> = BTreeMap::new();
    let mut executions: Vec<test_executions::ActiveModel> = Vec::with_capacity(classified.len());
    let mut latest: Vec<LatestTest> = Vec::new();
    let mut latest_by_id: BTreeMap<i32, usize> = BTreeMap::new();
    let mut day_counts: BTreeMap<i32, DayCounts> = BTreeMap::new();
    for (case, category) in classified {
        let fingerprint = flakeradar_core::fingerprint(&case.suite, &case.classname, &case.name);
        let test_case_id = ids.get(fingerprint.as_str()).copied().ok_or_else(|| {
            ProcessError::permanent(format!("DataError: Test {fingerprint} vanished"))
        })?;
        let attempt = attempts.get(&test_case_id).copied().unwrap_or(0);
        attempts.insert(test_case_id, attempt + 1);
        counts.add(case.status);
        let entry = day_counts.entry(test_case_id).or_default();
        entry.executions += 1;
        if case.status.is_failing() {
            entry.failures += 1;
        }
        // The reference writes a Location only when the report gave a file, so a
        // line without one is never stored and never has to fit the column.
        let line = if case.file.is_some() {
            case.line
                .map(|line| {
                    i32::try_from(line).map_err(|_out_of_range| {
                        ProcessError::permanent(format!("DataError: line {line} is out of range"))
                    })
                })
                .transpose()?
        } else {
            None
        };
        executions.push(test_executions::ActiveModel {
            test_case_id: Set(test_case_id),
            test_run_id: Set(run_id),
            status: Set(case.status.as_str().to_owned()),
            duration: Set(case.duration),
            message: Set(case.message.clone()),
            details: Set(case.details.clone()),
            attempt: Set(attempt),
            failure_category: Set(category.map(|category| category.as_str().to_owned())),
            created_at: Set(now),
            ..Default::default()
        });
        if let Some(index) = latest_by_id.get(&test_case_id) {
            let row = latest.get_mut(*index).ok_or_else(|| {
                ProcessError::permanent("DataError: Test bookkeeping is inconsistent")
            })?;
            case.status.as_str().clone_into(&mut row.last_status);
            // A report without a Location never erases one.
            if case.file.is_some() {
                row.file.clone_from(&case.file);
                row.line = line;
            }
        } else {
            latest_by_id.insert(test_case_id, latest.len());
            latest.push(LatestTest {
                id: test_case_id,
                last_status: case.status.as_str().to_owned(),
                file: case.file.clone(),
                line,
            });
        }
    }
    counts.retried =
        i64::try_from(attempts.values().filter(|seen| **seen > 1).count()).unwrap_or(i64::MAX);
    Ok(RecordedTests {
        counts,
        executions,
        latest,
        day_counts,
    })
}

/// A Test's newest Report occurrence, the row the bulk update writes.
#[derive(Debug, Clone)]
struct LatestTest {
    id: i32,
    last_status: String,
    file: Option<String>,
    line: Option<i32>,
}

/// Inserts unseen Tests; returns `{fingerprint: test_case_id}` for all of them.
async fn upsert_test_cases(
    db: &impl ConnectionTrait,
    project_id: i32,
    parsed: &[ParsedCase],
    now: DateTimeWithTimeZone,
) -> Result<BTreeMap<String, i32>, DbError> {
    let mut order: Vec<String> = Vec::new();
    let mut first: BTreeMap<String, &ParsedCase> = BTreeMap::new();
    for case in parsed {
        let fingerprint = flakeradar_core::fingerprint(&case.suite, &case.classname, &case.name);
        if let std::collections::btree_map::Entry::Vacant(entry) =
            first.entry(fingerprint.to_string())
        {
            order.push(fingerprint.to_string());
            entry.insert(case);
        }
    }
    let inserted: Vec<test_cases::ActiveModel> = order
        .iter()
        .filter_map(|fingerprint| first.get(fingerprint).copied())
        .map(|case| test_cases::ActiveModel {
            project_id: Set(project_id),
            fingerprint: Set(flakeradar_core::fingerprint(
                &case.suite,
                &case.classname,
                &case.name,
            )
            .to_string()),
            suite: Set(case.suite.clone()),
            classname: Set(case.classname.clone()),
            name: Set(case.name.clone()),
            last_seen_at: Set(now),
            ..Default::default()
        })
        .collect();
    for chunk in chunks(&inserted) {
        test_cases::Entity::insert_many(chunk.to_vec())
            // Python's bare `ON CONFLICT DO NOTHING` skips a conflict on *any*
            // constraint; SeaORM's target-less `OnConflict` is that statement,
            // while `on_conflict_do_nothing` would name the primary key only.
            .on_conflict(
                sea_orm::sea_query::OnConflict::new()
                    .do_nothing()
                    .to_owned(),
            )
            .exec_without_returning(db)
            .await?;
    }
    let mut ids = BTreeMap::new();
    for chunk in chunks(&order) {
        let placeholders = placeholder_list(2, chunk.len());
        let sql = format!(
            "SELECT fingerprint, id FROM test_cases WHERE project_id = $1 AND fingerprint IN ({placeholders})"
        );
        let mut values = Vec::with_capacity(chunk.len() + 1);
        values.push(Value::from(project_id));
        values.extend(
            chunk
                .iter()
                .map(|fingerprint| Value::from(fingerprint.clone())),
        );
        for row in rows(db, sql, values).await? {
            ids.insert(get::<String>(&row, "fingerprint")?, get::<i32>(&row, "id")?);
        }
    }
    Ok(ids)
}

/// The bulk update of Tests' last status and Location.
async fn update_latest_tests(
    db: &impl ConnectionTrait,
    latest: &[LatestTest],
    now: DateTimeWithTimeZone,
) -> Result<(), DbError> {
    for chunk in chunks(latest) {
        let mut values = Vec::with_capacity(chunk.len() * 6);
        let mut placeholders = Vec::with_capacity(chunk.len());
        for (index, row) in chunk.iter().enumerate() {
            let base = index * 6;
            placeholders.push(value_row(base + 1, 6));
            values.extend([
                Value::from(row.id),
                Value::from(row.last_status.clone()),
                Value::from(now),
                Value::from(row.file.is_some()),
                row.file.clone().map_or(Value::String(None), Value::from),
                row.line.map_or(Value::Int(None), Value::from),
            ]);
        }
        let sql = format!(
            "UPDATE test_cases AS t SET \
               last_status = v.last_status, last_seen_at = v.last_seen_at, \
               file = CASE WHEN v.has_file THEN v.file ELSE t.file END, \
               line = CASE WHEN v.has_file THEN v.line ELSE t.line END \
             FROM (VALUES {}) AS v(id, last_status, last_seen_at, has_file, file, line) \
             WHERE t.id = v.id",
            placeholders.join(", ")
        );
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await?;
    }
    Ok(())
}

/// Stores a reported Default branch on the Repo; `true` when it changed.
///
/// A Report that names none never clears a known one.
async fn apply_default_branch(
    db: &impl ConnectionTrait,
    repo_id: i32,
    value: Option<&str>,
) -> Result<bool, DbError> {
    let Some(value) = value else {
        return Ok(false);
    };
    let Some(repo) = repos::Entity::find_by_id(repo_id).one(db).await? else {
        return Ok(false);
    };
    if repo.default_branch.as_deref() == Some(value) {
        return Ok(false);
    }
    let mut active = repo.into_active_model();
    active.default_branch = Set(Some(value.to_owned()));
    active.update(db).await?;
    Ok(true)
}

/// Persists one Pipeline report as Job executions. The caller owns the transaction.
async fn process_pipeline_report(
    db: &impl ConnectionTrait,
    report: &mut reports::Model,
    settings: &ProcessingSettings,
) -> Result<ProcessOutcome, ProcessError> {
    let pipeline = stored_pipeline_report(report)
        .map_err(|error| ProcessError::permanent(format!("ValidationError: {error}")))?;
    let now = settings.clock.now();
    let branch_changed =
        apply_default_branch(db, report.repo_id, pipeline.default_branch.as_deref())
            .await
            .map_err(ProcessError::from)?;
    let (job_ids, flag_changed) = upsert_pipeline_jobs(db, report.repo_id, &pipeline, now).await?;
    let (mut new_counts, jobs_with_new_exec, day_counts) =
        insert_pipeline_job_executions(db, &pipeline, &job_ids, now).await?;
    update_job_last_status(db, &pipeline, &job_ids, &jobs_with_new_exec, now).await?;

    let touched: Vec<i32> = jobs_with_new_exec.union(&flag_changed).copied().collect();
    let mut tier_changes = rescore_jobs(db, &touched, Some(&day_counts), settings)
        .await
        .map_err(ProcessError::from)?;
    if branch_changed {
        rescore_repo(db, report.repo_id, &tier_changes, settings)
            .await
            .map_err(ProcessError::from)?;
        tier_changes = Vec::new();
    }
    new_counts.duplicate = i64::try_from(pipeline.jobs.len()).unwrap_or(i64::MAX)
        - (new_counts.passed + new_counts.failed + new_counts.skipped);
    let counts_json = new_counts.pipeline_json();
    complete_report(report, now, None, counts_json.clone());
    write_report(report, db).await.map_err(ProcessError::from)?;
    Ok(ProcessOutcome {
        report_id: report.id,
        status: ReportStatus::Processed,
        run_id: None,
        counts: Some(counts_json),
        touched_test_ids: Vec::new(),
        error: None,
        touched_job_ids: touched,
        tier_changes,
        retry_count: report.retry_count,
    })
}

/// Upserts the report's Pipeline and Jobs, returning every Job id by name and
/// the Jobs whose `summary` flag this report changed.
async fn upsert_pipeline_jobs(
    db: &impl ConnectionTrait,
    repo_id: i32,
    payload: &PipelineReportIn,
    now: DateTimeWithTimeZone,
) -> Result<(BTreeMap<String, i32>, BTreeSet<i32>), ProcessError> {
    pipelines::Entity::insert(pipelines::ActiveModel {
        repo_id: Set(repo_id),
        provider: Set(payload.provider.clone()),
        name: Set(payload.pipeline.clone()),
        created_at: Set(now),
        ..Default::default()
    })
    .on_conflict(
        sea_orm::sea_query::OnConflict::new()
            .do_nothing()
            .to_owned(),
    )
    .exec_without_returning(db)
    .await
    .map_err(ProcessError::from)?;
    let pipeline = pipelines::Entity::find()
        .filter(pipelines::Column::RepoId.eq(repo_id))
        .filter(pipelines::Column::Provider.eq(payload.provider.clone()))
        .filter(pipelines::Column::Name.eq(payload.pipeline.clone()))
        .one(db)
        .await
        .map_err(ProcessError::from)?
        .ok_or_else(|| ProcessError::permanent("DataError: the Pipeline vanished"))?;

    let mut job_names: Vec<String> = payload.jobs.iter().map(|job| job.name.clone()).collect();
    job_names.sort();
    job_names.dedup();
    // A single report names each Job once, so the last `summary` flag wins if a
    // caller ever repeats a name.
    let mut summary_by_name: BTreeMap<String, bool> = BTreeMap::new();
    for job in &payload.jobs {
        summary_by_name.insert(job.name.clone(), job.summary);
    }
    let mut flag_changed: BTreeSet<i32> = BTreeSet::new();
    for chunk in chunks(&job_names) {
        let mut values = Vec::with_capacity(chunk.len() * 4);
        let mut placeholders = Vec::with_capacity(chunk.len());
        for (index, name) in chunk.iter().enumerate() {
            let base = index * 4;
            placeholders.push(value_row(base + 1, 4));
            values.extend([
                Value::from(pipeline.id),
                Value::from(name.clone()),
                Value::from(summary_by_name.get(name).copied().unwrap_or(false)),
                Value::from(now),
            ]);
        }
        // Last-write-wins on `summary`, so a workflow that starts marking a
        // Summary Job is picked up on the next report. RETURNING yields the rows
        // inserted or whose flag actually changed, so a flip rescores even when
        // the report added no new executions.
        let now_placeholder = chunk.len() * 4 + 1;
        let sql = format!(
            "INSERT INTO jobs (pipeline_id, name, summary, last_seen_at, created_at) \
             SELECT v.pipeline_id, v.name, v.summary, v.last_seen_at, ${now_placeholder}::timestamptz \
               FROM (VALUES {}) AS v(pipeline_id, name, summary, last_seen_at) \
             ON CONFLICT ON CONSTRAINT uq_jobs_pipeline_name \
             DO UPDATE SET summary = EXCLUDED.summary \
              WHERE jobs.summary IS DISTINCT FROM EXCLUDED.summary \
             RETURNING id",
            placeholders.join(", ")
        );
        values.push(Value::from(now));
        for row in rows(db, sql, values).await.map_err(ProcessError::from)? {
            flag_changed.insert(get(&row, "id").map_err(ProcessError::from)?);
        }
    }

    let mut job_ids: BTreeMap<String, i32> = BTreeMap::new();
    for chunk in chunks(&job_names) {
        let placeholders = placeholder_list(2, chunk.len());
        let sql = format!(
            "SELECT name, id FROM jobs WHERE pipeline_id = $1 AND name IN ({placeholders})"
        );
        let mut values = Vec::with_capacity(chunk.len() + 1);
        values.push(Value::from(pipeline.id));
        values.extend(chunk.iter().map(|name| Value::from(name.clone())));
        for row in rows(db, sql, values).await.map_err(ProcessError::from)? {
            job_ids.insert(
                get(&row, "name").map_err(ProcessError::from)?,
                get(&row, "id").map_err(ProcessError::from)?,
            );
        }
    }
    Ok((job_ids, flag_changed))
}

/// Inserts the report's Job executions, deduplicated by `(job_id, ci_job_id)`.
///
/// Only brand-new executions count toward scoring and today's history.
async fn insert_pipeline_job_executions(
    db: &impl ConnectionTrait,
    payload: &PipelineReportIn,
    job_ids: &BTreeMap<String, i32>,
    now: DateTimeWithTimeZone,
) -> Result<(Counts, BTreeSet<i32>, BTreeMap<i32, DayCounts>), ProcessError> {
    let mut exec_rows: Vec<PipelineExecution> = Vec::with_capacity(payload.jobs.len());
    for job in &payload.jobs {
        let job_id = job_ids.get(&job.name).copied().ok_or_else(|| {
            ProcessError::permanent(format!("DataError: Job {} vanished", job.name))
        })?;
        exec_rows.push(PipelineExecution {
            job_id,
            ci_job_id: job.ci_job_id.clone(),
            ci_run_id: payload.ci_run_id.as_str().to_owned(),
            ci_run_attempt: crate::report_intake::stored_ci_run_attempt(payload.ci_run_attempt),
            commit_sha: payload.commit_sha.clone(),
            branch: payload.branch.clone(),
            status: job.status,
            url: job.url.clone(),
            runner_name: job.runner_name.clone(),
            runner_labels: job.runner_labels.clone(),
            started_at: job.started_at.map(|at| at.fixed_offset()),
            completed_at: job.completed_at.map(|at| at.fixed_offset()),
        });
    }
    let mut new_counts = Counts::default();
    let mut jobs_with_new_exec: BTreeSet<i32> = BTreeSet::new();
    let mut day_counts: BTreeMap<i32, DayCounts> = BTreeMap::new();
    for chunk in chunks(&exec_rows) {
        let (sql, values) = insert_job_executions_sql(chunk, now);
        for row in rows(db, sql, values).await.map_err(ProcessError::from)? {
            let job_id: i32 = get(&row, "job_id").map_err(ProcessError::from)?;
            let status: String = get(&row, "status").map_err(ProcessError::from)?;
            jobs_with_new_exec.insert(job_id);
            match status.as_str() {
                "passed" => new_counts.passed += 1,
                "failed" => new_counts.failed += 1,
                "skipped" => new_counts.skipped += 1,
                other => {
                    return Err(ProcessError::permanent(format!(
                        "DataError: unknown Job status {other}"
                    )));
                }
            }
            let entry = day_counts.entry(job_id).or_default();
            entry.executions += 1;
            if status == "failed" {
                entry.failures += 1;
            }
        }
    }
    Ok((new_counts, jobs_with_new_exec, day_counts))
}

/// The `INSERT ... ON CONFLICT DO NOTHING RETURNING job_id, status` for one
/// chunk, so only the rows that were actually inserted come back.
fn insert_job_executions_sql(
    chunk: &[PipelineExecution],
    now: DateTimeWithTimeZone,
) -> (String, Vec<Value>) {
    let mut values = Vec::with_capacity(chunk.len() * 13);
    let mut placeholders = Vec::with_capacity(chunk.len());
    for (index, row) in chunk.iter().enumerate() {
        let base = index * 13;
        placeholders.push(value_row(base + 1, 13));
        values.extend([
            Value::from(row.job_id),
            Value::from(row.ci_job_id.clone()),
            Value::from(row.ci_run_id.clone()),
            Value::from(row.ci_run_attempt),
            Value::from(row.commit_sha.clone()),
            Value::from(row.branch.clone()),
            Value::from(row.status.as_str()),
            Value::from(row.url.clone()),
            Value::from(row.runner_name.clone()),
            Value::Json(Some(Box::new(JsonValue::Array(
                row.runner_labels
                    .iter()
                    .map(|label| JsonValue::String(label.clone()))
                    .collect(),
            )))),
            row.started_at
                .map_or(Value::ChronoDateTimeWithTimeZone(None), Value::from),
            row.completed_at
                .map_or(Value::ChronoDateTimeWithTimeZone(None), Value::from),
            Value::from(now),
        ]);
    }
    let sql = format!(
        "INSERT INTO job_executions (job_id, ci_job_id, ci_run_id, ci_run_attempt, commit_sha, \
            branch, status, url, runner_name, runner_labels, started_at, completed_at, created_at) \
         VALUES {} \
         ON CONFLICT ON CONSTRAINT uq_job_executions_job_ci_job_id DO NOTHING \
         RETURNING job_id, status",
        placeholders.join(", ")
    );
    (sql, values)
}

/// Writes every Job's last status from the executions this report inserted.
async fn update_job_last_status(
    db: &impl ConnectionTrait,
    payload: &PipelineReportIn,
    job_ids: &BTreeMap<String, i32>,
    jobs_with_new_exec: &BTreeSet<i32>,
    now: DateTimeWithTimeZone,
) -> Result<(), ProcessError> {
    let mut status_by_job: BTreeMap<i32, JobStatus> = BTreeMap::new();
    for job in &payload.jobs {
        if let Some(id) = job_ids.get(&job.name)
            && jobs_with_new_exec.contains(id)
        {
            status_by_job.insert(*id, job.status);
        }
    }
    let rows: Vec<(i32, JobStatus)> = status_by_job.into_iter().collect();
    for chunk in chunks(&rows) {
        let mut values = Vec::with_capacity(chunk.len() * 3);
        let mut placeholders = Vec::with_capacity(chunk.len());
        for (index, (job_id, status)) in chunk.iter().enumerate() {
            let base = index * 3;
            placeholders.push(value_row(base + 1, 3));
            values.extend([
                Value::from(*job_id),
                Value::from(status.as_str()),
                Value::from(now),
            ]);
        }
        let sql = format!(
            "UPDATE jobs AS j SET last_status = v.last_status, last_seen_at = v.last_seen_at \
             FROM (VALUES {}) AS v(id, last_status, last_seen_at) WHERE j.id = v.id",
            placeholders.join(", ")
        );
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await
        .map_err(ProcessError::from)?;
    }
    Ok(())
}

/// One Job execution a Pipeline report is about to insert.
#[derive(Debug, Clone)]
struct PipelineExecution {
    job_id: i32,
    ci_job_id: String,
    ci_run_id: String,
    ci_run_attempt: i32,
    commit_sha: String,
    branch: String,
    status: JobStatus,
    url: String,
    runner_name: String,
    runner_labels: Vec<String>,
    started_at: Option<DateTimeWithTimeZone>,
    completed_at: Option<DateTimeWithTimeZone>,
}

/// Recomputes flakiness for the given Tests from their score window.
///
/// Flip scoring looks only at Executions on the Repo's Default branch, while
/// Proven flakes still count on every branch. `day_counts` adds each Test's
/// executions and failures to today's Score history row. A Clean streak of 0
/// also clears `github_issue_fixed_noted_at`; any other streak leaves it alone,
/// which is what the bulk update's per-row reset flag does.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a read or write fails.
pub async fn rescore(
    db: &impl ConnectionTrait,
    test_case_ids: &[i32],
    day_counts: Option<&BTreeMap<i32, DayCounts>>,
    settings: &ProcessingSettings,
) -> Result<Vec<TierChange>, DbError> {
    let mut history: BTreeMap<i32, Vec<ScoredExecution>> = BTreeMap::new();
    let mut categories: BTreeMap<i32, Vec<Option<FailureCategory>>> = BTreeMap::new();
    let mut default_branch: BTreeMap<i32, Option<String>> = BTreeMap::new();
    for chunk in chunks(test_case_ids) {
        let sql = format!(
            "SELECT e.test_case_id, r.commit_sha, r.branch, e.status, e.failure_category, ro.default_branch \
               FROM test_executions e \
               JOIN test_runs r ON r.id = e.test_run_id \
               JOIN projects p ON p.id = r.project_id \
               JOIN repos ro ON ro.id = p.repo_id \
              WHERE e.id IN (SELECT w.id FROM ({}) AS w) \
              ORDER BY e.test_case_id, e.id DESC",
            test_window_ids_sql()
        );
        let values = vec![
            int_array(chunk),
            Value::from(window_param(settings.score_window)),
            int_array(chunk),
            Value::from(window_param(settings.score_window)),
        ];
        for row in rows(db, sql, values).await? {
            let id: i32 = get(&row, "test_case_id")?;
            history.entry(id).or_default().push(ScoredExecution {
                commit_sha: get::<String>(&row, "commit_sha")?,
                branch: get::<String>(&row, "branch")?,
                status: execution_status(&get::<String>(&row, "status")?)?,
            });
            categories.entry(id).or_default().push(
                get::<Option<String>>(&row, "failure_category")?
                    .as_deref()
                    .and_then(FailureCategory::from_wire),
            );
            default_branch.insert(id, get(&row, "default_branch")?);
        }
    }

    let mut updates = Vec::with_capacity(test_case_ids.len());
    for id in test_case_ids {
        let empty = Vec::new();
        let executions = history.get(id).unwrap_or(&empty);
        let branch = default_branch.get(id).cloned().flatten();
        let (score, proven) = branch_scoped_score(
            executions,
            branch.as_deref(),
            settings.score_decay,
            settings.score_window,
        );
        let streak = branch_scoped_streak(executions, branch.as_deref());
        let window_categories: Vec<Option<FailureCategory>> = categories
            .get(id)
            .map(|all| all.iter().take(settings.score_window).copied().collect())
            .unwrap_or_default();
        updates.push(Update {
            id: *id,
            flakiness_score: score.as_f64(),
            confirmed_flake_count: i32::try_from(proven).unwrap_or(i32::MAX),
            failure_category: dominant_category(&window_categories),
            clean_streak: i32::try_from(streak).unwrap_or(i32::MAX),
            fail_streak: i32::try_from(branch_scoped_fail_streak(executions, branch.as_deref()))
                .unwrap_or(i32::MAX),
            clear_fixed_note: streak == 0,
        });
    }
    let old = old_scores(db, EntityKind::Test, test_case_ids).await?;
    let changes = test_tier_changes(db, &updates, &old, settings).await?;
    write_updates(db, &TEST_TABLE, &updates).await?;
    if !updates.is_empty() {
        let history_updates: Vec<HistoryUpdate> = updates
            .iter()
            .map(|update| HistoryUpdate {
                id: update.id,
                flakiness_score: update.flakiness_score,
                confirmed_flake_count: update.confirmed_flake_count,
                executions: day_counts
                    .and_then(|counts| counts.get(&update.id))
                    .map_or(0, |counts| counts.executions),
                failures: day_counts
                    .and_then(|counts| counts.get(&update.id))
                    .map_or(0, |counts| counts.failures),
            })
            .collect();
        write_today_history(
            db,
            EntityKind::Test,
            &history_updates,
            settings.clock.today(),
        )
        .await?;
    }
    Ok(changes)
}

/// The table that holds `kind`'s scored rows.
fn scored_table(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Test => "test_cases",
        EntityKind::Job => "jobs",
    }
}

/// The scores a rescore is about to replace, read before the update.
async fn old_scores(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    ids: &[i32],
) -> Result<BTreeMap<i32, f64>, DbError> {
    let table = scored_table(kind);
    let mut scores = BTreeMap::new();
    for chunk in chunks(ids) {
        let sql = format!(
            "SELECT id, flakiness_score FROM {table} WHERE id IN ({})",
            placeholder_list(1, chunk.len())
        );
        let values = chunk.iter().map(|id| Value::from(*id)).collect();
        for row in rows(db, sql, values).await? {
            scores.insert(get(&row, "id")?, get(&row, "flakiness_score")?);
        }
    }
    Ok(scores)
}

/// The updates that crossed the Flake threshold, in either direction.
///
/// Only the flaky boundary counts: a move between Stable and Suspect changes the
/// tier but crosses nothing, so it sends no Notification.
fn crossed<T: Copy>(updates: &[(i32, f64, T)], old: &BTreeMap<i32, f64>, threshold: f64) -> Vec<T> {
    updates
        .iter()
        .filter(|(id, score, _)| {
            (*score >= threshold) != (old.get(id).copied().unwrap_or(0.0) >= threshold)
        })
        .map(|(_, _, value)| *value)
        .collect()
}

/// The names a tier-change event gives a Test.
#[derive(Debug, Clone)]
struct NamedTest {
    repo: String,
    project: String,
    classname: String,
    name: String,
}

/// Notifications for the Tests whose tier this rescore moves (ADR 0009).
///
/// A Test that leaves the flaky tier because it is now broken has not recovered,
/// so it sends no event.
async fn test_tier_changes(
    db: &impl ConnectionTrait,
    updates: &[Update],
    old: &BTreeMap<i32, f64>,
    settings: &ProcessingSettings,
) -> Result<Vec<TierChange>, DbError> {
    let threshold = settings.flake_threshold;
    let scored: Vec<(i32, f64, Update)> = updates
        .iter()
        .map(|update| (update.id, update.flakiness_score, *update))
        .collect();
    let mut crossed: Vec<Update> = crossed(&scored, old, threshold)
        .into_iter()
        .filter(|update| {
            tier_for(update.flakiness_score, threshold) == Tier::Flaky
                || !is_broken(i64::from(update.fail_streak), settings.broken_streak)
        })
        .collect();
    if crossed.is_empty() {
        return Ok(Vec::new());
    }
    // Python builds these from `_rows_by_id`, a dict sorted by id, so the events go
    // out in id order whatever order the caller listed the ids in.
    crossed.sort_unstable_by_key(|update| update.id);
    let ids: Vec<i32> = crossed.iter().map(|update| update.id).collect();
    let mut named: BTreeMap<i32, NamedTest> = BTreeMap::new();
    for chunk in chunks(&ids) {
        let placeholders = placeholder_list(1, chunk.len());
        let sql = format!(
            "SELECT t.id, t.classname, t.name, p.name AS project, r.name AS repo \
               FROM test_cases t \
               JOIN projects p ON p.id = t.project_id \
               JOIN repos r ON r.id = p.repo_id \
              WHERE t.id IN ({placeholders}) ORDER BY t.id"
        );
        let values = chunk.iter().map(|id| Value::from(*id)).collect();
        for row in rows(db, sql, values).await? {
            named.insert(
                get(&row, "id")?,
                NamedTest {
                    repo: get(&row, "repo")?,
                    project: get(&row, "project")?,
                    classname: get(&row, "classname")?,
                    name: get(&row, "name")?,
                },
            );
        }
    }
    let mut changes = Vec::with_capacity(crossed.len());
    for update in crossed {
        let Some(named) = named.get(&update.id) else {
            // A Test deleted before this read names nothing, so it sends nothing.
            continue;
        };
        changes.push(TierChange {
            kind: EntityKind::Test,
            id: i64::from(update.id),
            repo: named.repo.clone(),
            project: Some(named.project.clone()),
            name: named.name.clone(),
            flakiness_score: update.flakiness_score,
            tier: tier_for(update.flakiness_score, threshold),
            proven_flake_count: usize::try_from(update.confirmed_flake_count).unwrap_or(0),
            classname: Some(named.classname.clone()),
            pipeline: None,
            failure_category: update.failure_category,
        });
    }
    Ok(changes)
}

/// One column of a bulk rescore write, and the value it binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    /// The row the update is for.
    Id,
    /// The Flakiness score.
    Score,
    /// The Proven flake count.
    Proven,
    /// The dominant Failure category.
    Category,
    /// The Clean streak.
    CleanStreak,
    /// The Fail streak.
    FailStreak,
    /// Whether the rescore clears the fixed note.
    ResetNote,
}

impl Column {
    /// The stored name of the column this binds.
    fn name(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Score => "flakiness_score",
            Self::Proven => "confirmed_flake_count",
            Self::Category => "failure_category",
            Self::CleanStreak => "clean_streak",
            Self::FailStreak => "fail_streak",
            Self::ResetNote => "reset_note",
        }
    }

    /// The value this column takes in `update`.
    fn value(self, update: &Update) -> Value {
        match self {
            Self::Id => Value::from(update.id),
            Self::Score => Value::from(update.flakiness_score),
            Self::Proven => Value::from(update.confirmed_flake_count),
            Self::Category => update
                .failure_category
                .map_or(Value::String(None), |category| {
                    Value::from(category.as_str())
                }),
            Self::CleanStreak => Value::from(update.clean_streak),
            Self::FailStreak => Value::from(update.fail_streak),
            Self::ResetNote => Value::from(update.clear_fixed_note),
        }
    }
}

/// A table a rescore writes, and the columns one row of it binds.
struct UpdateTable {
    /// The table's name.
    table: &'static str,
    /// The alias the statement gives it.
    alias: &'static str,
    /// The columns of one `VALUES` row, in order.
    columns: &'static [Column],
}

/// The Tests table, which also holds a Fail streak and a Failure category.
const TEST_TABLE: UpdateTable = UpdateTable {
    table: "test_cases",
    alias: "t",
    columns: &[
        Column::Id,
        Column::Score,
        Column::Proven,
        Column::Category,
        Column::CleanStreak,
        Column::FailStreak,
        Column::ResetNote,
    ],
};

/// The Jobs table, which scores no Failure category and keeps no Fail streak.
const JOB_TABLE: UpdateTable = UpdateTable {
    table: "jobs",
    alias: "j",
    columns: &[
        Column::Id,
        Column::Score,
        Column::Proven,
        Column::CleanStreak,
        Column::ResetNote,
    ],
};

/// Writes one chunk-set of rescores to `table`.
///
/// One statement per chunk assigns every column the table holds, and the
/// fixed-note reset is the same `CASE` for both entities: a Clean streak of 0
/// clears the note and any other streak leaves it alone.
async fn write_updates(
    db: &impl ConnectionTrait,
    table: &UpdateTable,
    updates: &[Update],
) -> Result<(), DbError> {
    let width = table.columns.len();
    let names: Vec<&str> = table.columns.iter().map(|column| column.name()).collect();
    let mut assignments: Vec<String> = table
        .columns
        .iter()
        .filter(|column| !matches!(column, Column::Id | Column::ResetNote))
        .map(|column| format!("{} = v.{}", column.name(), column.name()))
        .collect();
    assignments.push(format!(
        "github_issue_fixed_noted_at = CASE WHEN v.{} THEN NULL ELSE {}.github_issue_fixed_noted_at END",
        Column::ResetNote.name(),
        table.alias
    ));
    let statement = |rows: &str| {
        format!(
            "UPDATE {} AS {} SET {} FROM (VALUES {rows}) AS v({}) WHERE {}.id = v.id",
            table.table,
            table.alias,
            assignments.join(", "),
            names.join(", "),
            table.alias
        )
    };
    for chunk in chunks(updates) {
        let mut values = Vec::with_capacity(chunk.len() * width);
        let mut placeholders = Vec::with_capacity(chunk.len());
        for (index, update) in chunk.iter().enumerate() {
            placeholders.push(value_row(index * width + 1, width));
            values.extend(table.columns.iter().map(|column| column.value(update)));
        }
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            statement(&placeholders.join(", ")),
            values,
        ))
        .await?;
    }
    Ok(())
}

/// One Job's scoring input: its newest-first history and its Repo's Default branch.
#[derive(Debug, Clone, Default)]
pub struct JobHistory {
    /// The Default branch of the Job's Repo, or `None` while it is unknown.
    pub default_branch: Option<String>,
    /// Executions, newest first, with explained failures already turned into
    /// `skipped` so they never break a streak.
    pub executions: Vec<ScoredExecution>,
}

/// Newest-first Job-execution history for each Job, plus its Repo's Default branch.
///
/// Explained failed executions become `skipped`, so they never break a streak.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a read fails.
pub async fn job_history(
    db: &impl ConnectionTrait,
    job_ids: &[i32],
    settings: &ProcessingSettings,
) -> Result<BTreeMap<i32, JobHistory>, DbError> {
    let mut history: BTreeMap<i32, Vec<ScoredExecution>> = BTreeMap::new();
    let mut branches: BTreeMap<i32, Option<String>> = BTreeMap::new();
    for id in job_ids {
        history.entry(*id).or_default();
        branches.entry(*id).or_insert(None);
    }
    for chunk in chunks(job_ids) {
        // The table names are written out: `job_execution_explained_sql` reads
        // `job_executions`, `jobs` and `pipelines` by name, as Python's query did.
        let sql = format!(
            "SELECT job_executions.job_id, job_executions.commit_sha, job_executions.branch, \
                    job_executions.status, ({}) AS explained, repos.default_branch \
               FROM job_executions \
               JOIN jobs ON jobs.id = job_executions.job_id \
               JOIN pipelines ON pipelines.id = jobs.pipeline_id \
               JOIN repos ON repos.id = pipelines.repo_id \
              WHERE job_executions.id IN (SELECT w.id FROM ({}) AS w) \
              ORDER BY job_executions.job_id, job_executions.id DESC",
            job_execution_explained_sql(),
            job_window_ids_sql()
        );
        let values = vec![
            int_array(chunk),
            Value::from(window_param(settings.score_window)),
            int_array(chunk),
            Value::from(window_param(settings.score_window)),
        ];
        for row in rows(db, sql, values).await? {
            let id: i32 = get(&row, "job_id")?;
            let explained: bool = get(&row, "explained")?;
            let mut status = execution_status(&get::<String>(&row, "status")?)?;
            if explained && status == ExecutionStatus::Failed {
                status = ExecutionStatus::Skipped;
            }
            history.entry(id).or_default().push(ScoredExecution {
                commit_sha: get::<String>(&row, "commit_sha")?,
                branch: get::<String>(&row, "branch")?,
                status,
            });
            branches.insert(id, get(&row, "default_branch")?);
        }
    }
    Ok(history
        .into_iter()
        .map(|(id, executions)| {
            (
                id,
                JobHistory {
                    default_branch: branches.get(&id).cloned().flatten(),
                    executions,
                },
            )
        })
        .collect())
}

/// Recomputes flakiness for Jobs with the Default-branch rule.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a read or write fails.
pub async fn rescore_jobs(
    db: &impl ConnectionTrait,
    job_ids: &[i32],
    day_counts: Option<&BTreeMap<i32, DayCounts>>,
    settings: &ProcessingSettings,
) -> Result<Vec<TierChange>, DbError> {
    let history = job_history(db, job_ids, settings).await?;
    let mut updates = Vec::with_capacity(job_ids.len());
    for id in job_ids {
        let entry = history.get(id).cloned().unwrap_or_default();
        let branch = entry.default_branch;
        let (score, proven) = branch_scoped_score(
            &entry.executions,
            branch.as_deref(),
            settings.score_decay,
            settings.score_window,
        );
        let streak = branch_scoped_streak(&entry.executions, branch.as_deref());
        updates.push(Update {
            id: *id,
            flakiness_score: score.as_f64(),
            confirmed_flake_count: i32::try_from(proven).unwrap_or(i32::MAX),
            clean_streak: i32::try_from(streak).unwrap_or(i32::MAX),
            clear_fixed_note: streak == 0,
            // A Job scores neither: the two columns are a Test's.
            fail_streak: 0,
            failure_category: None,
        });
    }
    let old = old_scores(db, EntityKind::Job, job_ids).await?;
    let changes = job_tier_changes(db, &updates, &old, settings).await?;
    write_updates(db, &JOB_TABLE, &updates).await?;
    if !updates.is_empty() {
        let history_updates: Vec<HistoryUpdate> = updates
            .iter()
            .map(|update| HistoryUpdate {
                id: update.id,
                flakiness_score: update.flakiness_score,
                confirmed_flake_count: update.confirmed_flake_count,
                executions: day_counts
                    .and_then(|counts| counts.get(&update.id))
                    .map_or(0, |counts| counts.executions),
                failures: day_counts
                    .and_then(|counts| counts.get(&update.id))
                    .map_or(0, |counts| counts.failures),
            })
            .collect();
        write_today_history(
            db,
            EntityKind::Job,
            &history_updates,
            settings.clock.today(),
        )
        .await?;
    }
    Ok(changes)
}

/// The names a tier-change event gives a Job.
#[derive(Debug, Clone)]
struct NamedJob {
    repo: String,
    pipeline: String,
    name: String,
}

/// Notifications for the Jobs whose tier this rescore moves (ADR 0009).
async fn job_tier_changes(
    db: &impl ConnectionTrait,
    updates: &[Update],
    old: &BTreeMap<i32, f64>,
    settings: &ProcessingSettings,
) -> Result<Vec<TierChange>, DbError> {
    let threshold = settings.flake_threshold;
    let scored: Vec<(i32, f64, Update)> = updates
        .iter()
        .map(|update| (update.id, update.flakiness_score, *update))
        .collect();
    let mut crossed: Vec<Update> = crossed(&scored, old, threshold);
    if crossed.is_empty() {
        return Ok(Vec::new());
    }
    // Python builds these from `_rows_by_id`, a dict sorted by id, so the events go
    // out in id order whatever order the caller listed the ids in.
    crossed.sort_unstable_by_key(|update| update.id);
    let ids: Vec<i32> = crossed.iter().map(|update| update.id).collect();
    let mut named: BTreeMap<i32, NamedJob> = BTreeMap::new();
    for chunk in chunks(&ids) {
        let placeholders = placeholder_list(1, chunk.len());
        let sql = format!(
            "SELECT j.id, j.name, p.name AS pipeline, r.name AS repo \
               FROM jobs j \
               JOIN pipelines p ON p.id = j.pipeline_id \
               JOIN repos r ON r.id = p.repo_id \
              WHERE j.id IN ({placeholders}) ORDER BY j.id"
        );
        let values = chunk.iter().map(|id| Value::from(*id)).collect();
        for row in rows(db, sql, values).await? {
            named.insert(
                get(&row, "id")?,
                NamedJob {
                    repo: get(&row, "repo")?,
                    pipeline: get(&row, "pipeline")?,
                    name: get(&row, "name")?,
                },
            );
        }
    }
    let mut changes = Vec::with_capacity(crossed.len());
    for update in crossed {
        let Some(named) = named.get(&update.id) else {
            continue;
        };
        changes.push(TierChange {
            kind: EntityKind::Job,
            id: i64::from(update.id),
            repo: named.repo.clone(),
            project: None,
            name: named.name.clone(),
            flakiness_score: update.flakiness_score,
            tier: tier_for(update.flakiness_score, threshold),
            proven_flake_count: usize::try_from(update.confirmed_flake_count).unwrap_or(0),
            classname: None,
            pipeline: Some(named.pipeline.clone()),
            failure_category: None,
        });
    }
    Ok(changes)
}

/// Rescales every Job that carries this `ci_job_id` in the Repo.
async fn rescore_jobs_for_ci_job_id(
    db: &impl ConnectionTrait,
    repo_id: i32,
    ci_job_id: &str,
    settings: &ProcessingSettings,
) -> Result<(Vec<i32>, Vec<TierChange>), DbError> {
    let mut job_ids: BTreeSet<i32> = BTreeSet::new();
    let sql = "SELECT job_executions.job_id FROM job_executions \
               JOIN jobs ON jobs.id = job_executions.job_id \
               JOIN pipelines ON pipelines.id = jobs.pipeline_id \
              WHERE job_executions.ci_job_id = $1 AND pipelines.repo_id = $2";
    for row in rows(
        db,
        sql.to_owned(),
        vec![Value::from(ci_job_id), Value::from(repo_id)],
    )
    .await?
    {
        job_ids.insert(get(&row, "job_id")?);
    }
    let job_ids: Vec<i32> = job_ids.into_iter().collect();
    if job_ids.is_empty() {
        return Ok((job_ids, Vec::new()));
    }
    let changes = rescore_jobs(db, &job_ids, None, settings).await?;
    Ok((job_ids, changes))
}

/// Rescores every Test and every Job in the Repo (chunked).
///
/// Returns `(test_ids, job_ids)`, each sorted. The tier changes are dropped on
/// purpose: a Default branch change rescores the whole Repo at once, so
/// notifying about it would send hundreds of events for one Report (ADR 0009).
async fn rescore_repo(
    db: &impl ConnectionTrait,
    repo_id: i32,
    report_tier_changes: &[TierChange],
    settings: &ProcessingSettings,
) -> Result<(Vec<i32>, Vec<i32>), DbError> {
    let mut test_ids = Vec::new();
    for row in rows(
        db,
        "SELECT t.id FROM test_cases t JOIN projects p ON p.id = t.project_id WHERE p.repo_id = $1"
            .to_owned(),
        vec![Value::from(repo_id)],
    )
    .await?
    {
        test_ids.push(get(&row, "id")?);
    }
    let mut job_ids = Vec::new();
    for row in rows(
        db,
        "SELECT j.id FROM jobs j JOIN pipelines p ON p.id = j.pipeline_id WHERE p.repo_id = $1"
            .to_owned(),
        vec![Value::from(repo_id)],
    )
    .await?
    {
        job_ids.push(get(&row, "id")?);
    }
    test_ids.sort_unstable();
    job_ids.sort_unstable();
    let mut dropped = report_tier_changes.len();
    dropped += rescore(db, &test_ids, None, settings).await?.len();
    dropped += rescore_jobs(db, &job_ids, None, settings).await?.len();
    if dropped > 0 {
        // One event per changed Test would flood the channel, so only the count
        // is reported (ADR 0009).
        tracing::info!("Rescore of Repo {repo_id} dropped {dropped} Notification(s)");
    }
    Ok((test_ids, job_ids))
}

#[cfg(test)]
mod tests {
    //! Error classification needs driver failures no statement of ours raises, so
    //! the arms are checked against the errors a driver reports.

    use std::io;
    use std::sync::Arc;

    use sea_orm::DbErr;
    use sea_orm::error::{ConnAcquireErr, RuntimeErr};
    use sea_orm::sqlx::error::Error as SqlxError;

    use super::{ProcessError, transient_db_error, transient_sqlstate};
    use crate::DbError;

    /// A statement failure, wrapped the way the driver returns one.
    fn driver(error: SqlxError) -> DbErr {
        DbErr::Exec(RuntimeErr::SqlxError(Arc::new(error)))
    }

    #[test]
    fn a_transient_failure_is_the_connection_and_the_sqlstates_python_retried() {
        assert!(transient_db_error(&DbErr::ConnectionAcquire(
            ConnAcquireErr::Timeout
        )));
        assert!(transient_db_error(&DbErr::ConnectionAcquire(
            ConnAcquireErr::ConnectionClosed
        )));
        assert!(transient_db_error(&driver(SqlxError::PoolTimedOut)));
        assert!(transient_db_error(&driver(SqlxError::PoolClosed)));
        assert!(transient_db_error(&driver(SqlxError::Io(
            io::Error::other("connection reset")
        ))));
    }

    #[test]
    fn every_other_failure_is_permanent() {
        assert!(!transient_db_error(&DbErr::RecordNotFound(
            "Report".to_owned()
        )));
        assert!(!transient_db_error(&DbErr::Exec(RuntimeErr::Internal(
            "boom".to_owned()
        ))));
        assert!(!transient_db_error(&driver(SqlxError::Configuration(
            "no such DSN".to_owned().into()
        ))));
        assert!(!transient_db_error(&driver(SqlxError::Protocol(
            "57014: statement timeout".to_owned()
        ))));
    }

    #[test]
    fn a_transient_sqlstate_is_the_three_classes_and_the_two_codes() {
        for code in ["08006", "53300", "57014", "40001", "40P01"] {
            assert!(transient_sqlstate(code), "{code}");
        }
        for code in ["23514", "42601", "4000", "40P0"] {
            assert!(!transient_sqlstate(code), "{code}");
        }
    }

    #[test]
    fn a_failure_outside_the_database_is_a_permanent_data_error() {
        let error = ProcessError::from(DbError::NotPostgres);
        assert!(!error.transient);
        assert!(
            error.message.starts_with("DataError: "),
            "{}",
            error.message
        );
    }
}
