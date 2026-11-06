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
    ExecutionStatus, FailureCategory, JobStatus, ParseError, ParsedCase, PipelineReportIn, Tier,
    TierChange, branch_scoped_fail_streak, branch_scoped_score, branch_scoped_streak,
    classify_failure, defaults, dominant_category, is_broken, parse_junit_xml, tier_for,
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
use crate::entities::{jobs, pipelines, reports, repos, test_cases, test_executions, test_runs};
use crate::error::DbError;
use crate::report_intake::{
    REPORT_FAILED, REPORT_KIND_PIPELINE, REPORT_PENDING, REPORT_PROCESSED, complete_report,
    fail_report, requeue_report, stored_junit_body, stored_pipeline_payload,
};
use crate::score_history::{HistoryKind, HistoryUpdate, write_today_history};
use crate::sql::{get, placeholder_list, rows, value_row};

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
    /// `processed`, `failed` or `pending` (requeued for a retry).
    pub status: String,
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
        [Value::from(REPORT_PENDING)],
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

/// One Test's rescore result.
#[derive(Debug, Clone, Copy)]
struct TestUpdate {
    id: i32,
    flakiness_score: f64,
    confirmed_flake_count: i32,
    failure_category: Option<FailureCategory>,
    clean_streak: i32,
    fail_streak: i32,
    clear_fixed_note: bool,
}

/// One Job's rescore result.
#[derive(Debug, Clone, Copy)]
struct JobUpdate {
    id: i32,
    flakiness_score: f64,
    confirmed_flake_count: i32,
    clean_streak: i32,
    clear_fixed_note: bool,
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
                    status: REPORT_PENDING.to_owned(),
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
                status: REPORT_FAILED.to_owned(),
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
    if report.kind == REPORT_KIND_PIPELINE {
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
    } = record_test_executions(&classified, &ids, run.id, now)?;

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
        status: REPORT_PROCESSED.to_owned(),
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
    day_counts: BTreeMap<i32, (i32, i32)>,
}

/// Builds one Report's Executions in report order.
///
/// `attempt` counts per Test from 0; the last occurrence of a Test wins its
/// `last_status`, and only an occurrence with a Location sets one. Day counts
/// feed today's Score history row.
fn record_test_executions(
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
    let mut day_counts: BTreeMap<i32, (i32, i32)> = BTreeMap::new();
    for (case, category) in classified {
        let fingerprint = flakeradar_core::fingerprint(&case.suite, &case.classname, &case.name);
        let test_case_id = ids.get(&fingerprint).copied().ok_or_else(|| {
            ProcessError::permanent(format!("DataError: Test {fingerprint} vanished"))
        })?;
        let attempt = attempts.get(&test_case_id).copied().unwrap_or(0);
        attempts.insert(test_case_id, attempt + 1);
        counts.add(case.status);
        let entry = day_counts.entry(test_case_id).or_insert((0, 0));
        entry.0 += 1;
        if case.status.is_failing() {
            entry.1 += 1;
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
        if let std::collections::btree_map::Entry::Vacant(entry) = first.entry(fingerprint.clone())
        {
            order.push(fingerprint);
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
            )),
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
    let payload = stored_pipeline_payload(report)
        .map_err(|error| ProcessError::permanent(format!("ValidationError: {error}")))?;
    let now = settings.clock.now();
    let branch_changed =
        apply_default_branch(db, report.repo_id, payload.default_branch.as_deref())
            .await
            .map_err(ProcessError::from)?;
    let (job_ids, flag_changed) = upsert_pipeline_jobs(db, report.repo_id, &payload, now).await?;
    let (mut new_counts, jobs_with_new_exec, day_counts) =
        insert_pipeline_job_executions(db, &payload, &job_ids, now).await?;
    update_job_last_status(db, &payload, &job_ids, &jobs_with_new_exec, now).await?;

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
    new_counts.duplicate = i64::try_from(payload.jobs.len()).unwrap_or(i64::MAX)
        - (new_counts.passed + new_counts.failed + new_counts.skipped);
    let counts_json = new_counts.pipeline_json();
    complete_report(report, now, None, counts_json.clone());
    write_report(report, db).await.map_err(ProcessError::from)?;
    Ok(ProcessOutcome {
        report_id: report.id,
        status: REPORT_PROCESSED.to_owned(),
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
) -> Result<(Counts, BTreeSet<i32>, BTreeMap<i32, (i32, i32)>), ProcessError> {
    let mut exec_rows: Vec<PipelineExecution> = Vec::with_capacity(payload.jobs.len());
    for job in &payload.jobs {
        let job_id = job_ids.get(&job.name).copied().ok_or_else(|| {
            ProcessError::permanent(format!("DataError: Job {} vanished", job.name))
        })?;
        exec_rows.push(PipelineExecution {
            job_id,
            ci_job_id: job.ci_job_id.clone(),
            ci_run_id: payload.ci_run_id.clone(),
            ci_run_attempt: crate::report_intake::stored_attempt(payload.ci_run_attempt),
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
    let mut day_counts: BTreeMap<i32, (i32, i32)> = BTreeMap::new();
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
            let entry = day_counts.entry(job_id).or_insert((0, 0));
            entry.0 += 1;
            if status == "failed" {
                entry.1 += 1;
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
    day_counts: Option<&BTreeMap<i32, (i32, i32)>>,
    settings: &ProcessingSettings,
) -> Result<Vec<TierChange>, DbError> {
    let mut history: BTreeMap<i32, Vec<(String, String, ExecutionStatus)>> = BTreeMap::new();
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
            history.entry(id).or_default().push((
                get(&row, "commit_sha")?,
                get(&row, "branch")?,
                status_of(&get::<String>(&row, "status")?)?,
            ));
            categories.entry(id).or_default().push(
                get::<Option<String>>(&row, "failure_category")?
                    .as_deref()
                    .and_then(category_from_wire),
            );
            default_branch.insert(id, get(&row, "default_branch")?);
        }
    }

    let mut updates = Vec::with_capacity(test_case_ids.len());
    for id in test_case_ids {
        let empty = Vec::new();
        let executions = history.get(id).unwrap_or(&empty);
        let branch = default_branch.get(id).cloned().flatten();
        let history_refs: Vec<(&str, &str, ExecutionStatus)> = executions
            .iter()
            .map(|(sha, branch, status)| (sha.as_str(), branch.as_str(), *status))
            .collect();
        let (score, confirmed) = branch_scoped_score(
            &history_refs,
            branch.as_deref(),
            settings.score_decay,
            settings.score_window,
        );
        let streak = branch_scoped_streak(&history_refs, branch.as_deref());
        let window_categories: Vec<Option<FailureCategory>> = categories
            .get(id)
            .map(|all| all.iter().take(settings.score_window).copied().collect())
            .unwrap_or_default();
        updates.push(TestUpdate {
            id: *id,
            flakiness_score: score,
            confirmed_flake_count: i32::try_from(confirmed).unwrap_or(i32::MAX),
            failure_category: dominant_category(&window_categories),
            clean_streak: i32::try_from(streak).unwrap_or(i32::MAX),
            fail_streak: i32::try_from(branch_scoped_fail_streak(&history_refs, branch.as_deref()))
                .unwrap_or(i32::MAX),
            clear_fixed_note: streak == 0,
        });
    }
    let old = old_test_scores(db, test_case_ids).await?;
    let changes = test_tier_changes(db, &updates, &old, settings).await?;
    write_test_updates(db, &updates).await?;
    if !updates.is_empty() {
        let history_updates: Vec<HistoryUpdate> = updates
            .iter()
            .map(|update| HistoryUpdate {
                id: update.id,
                flakiness_score: update.flakiness_score,
                confirmed_flake_count: update.confirmed_flake_count,
                executions: day_counts
                    .and_then(|counts| counts.get(&update.id))
                    .map_or(0, |(executions, _)| *executions),
                failures: day_counts
                    .and_then(|counts| counts.get(&update.id))
                    .map_or(0, |(_, failures)| *failures),
            })
            .collect();
        write_today_history(
            db,
            HistoryKind::Test,
            &history_updates,
            settings.clock.today(),
        )
        .await?;
    }
    Ok(changes)
}

/// Parses a stored status, refusing an unknown one.
fn status_of(text: &str) -> Result<ExecutionStatus, DbError> {
    ExecutionStatus::from_wire(text)
        .ok_or_else(|| DbError::Database(DbErr::Custom(format!("unknown Execution status {text}"))))
}

/// Parses a stored Failure category.
fn category_from_wire(text: &str) -> Option<FailureCategory> {
    [
        FailureCategory::Network,
        FailureCategory::Environment,
        FailureCategory::Timing,
        FailureCategory::Assertion,
        FailureCategory::Other,
    ]
    .into_iter()
    .find(|category| category.as_str() == text)
}

/// The scores a Test rescore is about to replace, read before the update.
async fn old_test_scores(
    db: &impl ConnectionTrait,
    ids: &[i32],
) -> Result<BTreeMap<i32, f64>, DbError> {
    let mut scores = BTreeMap::new();
    for chunk in chunks(ids) {
        for row in test_cases::Entity::find()
            .filter(test_cases::Column::Id.is_in(chunk.to_vec()))
            .all(db)
            .await?
        {
            scores.insert(row.id, row.flakiness_score);
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

/// Notifications for the Tests whose tier this rescore moves (ADR 0009).
///
/// A Test that leaves the flaky tier because it is now broken has not recovered,
/// so it sends no event.
async fn test_tier_changes(
    db: &impl ConnectionTrait,
    updates: &[TestUpdate],
    old: &BTreeMap<i32, f64>,
    settings: &ProcessingSettings,
) -> Result<Vec<TierChange>, DbError> {
    let threshold = settings.flake_threshold;
    let scored: Vec<(i32, f64, TestUpdate)> = updates
        .iter()
        .map(|update| (update.id, update.flakiness_score, *update))
        .collect();
    let mut crossed: Vec<TestUpdate> = crossed(&scored, old, threshold)
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
    let mut named: BTreeMap<i32, (String, String, String, String)> = BTreeMap::new();
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
                (
                    get(&row, "repo")?,
                    get(&row, "project")?,
                    get(&row, "classname")?,
                    get(&row, "name")?,
                ),
            );
        }
    }
    let mut changes = Vec::with_capacity(crossed.len());
    for update in crossed {
        let Some((repo, project, classname, name)) = named.get(&update.id) else {
            // A Test deleted before this read names nothing, so it sends nothing.
            continue;
        };
        changes.push(TierChange {
            kind: flakeradar_core::Kind::Test,
            id: i64::from(update.id),
            repo: repo.clone(),
            project: Some(project.clone()),
            name: name.clone(),
            flakiness_score: update.flakiness_score,
            tier: tier_for(update.flakiness_score, threshold),
            confirmed_flake_count: usize::try_from(update.confirmed_flake_count).unwrap_or(0),
            classname: Some(classname.clone()),
            pipeline: None,
            failure_category: update.failure_category,
        });
    }
    Ok(changes)
}

/// Writes one chunk-set of Test rescores.
async fn write_test_updates(
    db: &impl ConnectionTrait,
    updates: &[TestUpdate],
) -> Result<(), DbError> {
    for chunk in chunks(updates) {
        let mut values = Vec::with_capacity(chunk.len() * 7);
        let mut placeholders = Vec::with_capacity(chunk.len());
        for (index, update) in chunk.iter().enumerate() {
            let base = index * 7;
            placeholders.push(value_row(base + 1, 7));
            values.extend([
                Value::from(update.id),
                Value::from(update.flakiness_score),
                Value::from(update.confirmed_flake_count),
                update
                    .failure_category
                    .map_or(Value::String(None), |category| {
                        Value::from(category.as_str())
                    }),
                Value::from(update.clean_streak),
                Value::from(update.fail_streak),
                Value::from(update.clear_fixed_note),
            ]);
        }
        let sql = format!(
            "UPDATE test_cases AS t SET \
               flakiness_score = v.flakiness_score, \
               confirmed_flake_count = v.confirmed_flake_count, \
               failure_category = v.failure_category, \
               clean_streak = v.clean_streak, fail_streak = v.fail_streak, \
               github_issue_fixed_noted_at = CASE WHEN v.reset_note THEN NULL ELSE t.github_issue_fixed_noted_at END \
             FROM (VALUES {}) AS v(id, flakiness_score, confirmed_flake_count, failure_category, clean_streak, fail_streak, reset_note) \
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

/// One Job's scoring input: its newest-first history and its Repo's Default branch.
#[derive(Debug, Clone, Default)]
pub struct JobHistory {
    /// The Default branch of the Job's Repo, or `None` while it is unknown.
    pub default_branch: Option<String>,
    /// Executions as `(commit_sha, branch, status)`, newest first, with explained
    /// failures already turned into `skipped` so they never break a streak.
    pub executions: Vec<(String, String, ExecutionStatus)>,
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
    let mut history: BTreeMap<i32, Vec<(String, String, ExecutionStatus)>> = BTreeMap::new();
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
            let mut status = status_of(&get::<String>(&row, "status")?)?;
            if explained && status == ExecutionStatus::Failed {
                status = ExecutionStatus::Skipped;
            }
            history.entry(id).or_default().push((
                get(&row, "commit_sha")?,
                get(&row, "branch")?,
                status,
            ));
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
    day_counts: Option<&BTreeMap<i32, (i32, i32)>>,
    settings: &ProcessingSettings,
) -> Result<Vec<TierChange>, DbError> {
    let history = job_history(db, job_ids, settings).await?;
    let mut updates = Vec::with_capacity(job_ids.len());
    for id in job_ids {
        let entry = history.get(id).cloned().unwrap_or_default();
        let branch = entry.default_branch;
        let history_refs: Vec<(&str, &str, ExecutionStatus)> = entry
            .executions
            .iter()
            .map(|(sha, branch, status)| (sha.as_str(), branch.as_str(), *status))
            .collect();
        let (score, confirmed) = branch_scoped_score(
            &history_refs,
            branch.as_deref(),
            settings.score_decay,
            settings.score_window,
        );
        let streak = branch_scoped_streak(&history_refs, branch.as_deref());
        updates.push(JobUpdate {
            id: *id,
            flakiness_score: score,
            confirmed_flake_count: i32::try_from(confirmed).unwrap_or(i32::MAX),
            clean_streak: i32::try_from(streak).unwrap_or(i32::MAX),
            clear_fixed_note: streak == 0,
        });
    }
    let old = old_job_scores(db, job_ids).await?;
    let changes = job_tier_changes(db, &updates, &old, settings).await?;
    write_job_updates(db, &updates).await?;
    if !updates.is_empty() {
        let history_updates: Vec<HistoryUpdate> = updates
            .iter()
            .map(|update| HistoryUpdate {
                id: update.id,
                flakiness_score: update.flakiness_score,
                confirmed_flake_count: update.confirmed_flake_count,
                executions: day_counts
                    .and_then(|counts| counts.get(&update.id))
                    .map_or(0, |(executions, _)| *executions),
                failures: day_counts
                    .and_then(|counts| counts.get(&update.id))
                    .map_or(0, |(_, failures)| *failures),
            })
            .collect();
        write_today_history(
            db,
            HistoryKind::Job,
            &history_updates,
            settings.clock.today(),
        )
        .await?;
    }
    Ok(changes)
}

/// The scores a Job rescore is about to replace, read before the update.
async fn old_job_scores(
    db: &impl ConnectionTrait,
    ids: &[i32],
) -> Result<BTreeMap<i32, f64>, DbError> {
    let mut scores = BTreeMap::new();
    for chunk in chunks(ids) {
        for row in jobs::Entity::find()
            .filter(jobs::Column::Id.is_in(chunk.to_vec()))
            .all(db)
            .await?
        {
            scores.insert(row.id, row.flakiness_score);
        }
    }
    Ok(scores)
}

/// Notifications for the Jobs whose tier this rescore moves (ADR 0009).
async fn job_tier_changes(
    db: &impl ConnectionTrait,
    updates: &[JobUpdate],
    old: &BTreeMap<i32, f64>,
    settings: &ProcessingSettings,
) -> Result<Vec<TierChange>, DbError> {
    let threshold = settings.flake_threshold;
    let scored: Vec<(i32, f64, JobUpdate)> = updates
        .iter()
        .map(|update| (update.id, update.flakiness_score, *update))
        .collect();
    let mut crossed: Vec<JobUpdate> = crossed(&scored, old, threshold);
    if crossed.is_empty() {
        return Ok(Vec::new());
    }
    // Python builds these from `_rows_by_id`, a dict sorted by id, so the events go
    // out in id order whatever order the caller listed the ids in.
    crossed.sort_unstable_by_key(|update| update.id);
    let ids: Vec<i32> = crossed.iter().map(|update| update.id).collect();
    let mut named: BTreeMap<i32, (String, String, String)> = BTreeMap::new();
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
                (
                    get(&row, "repo")?,
                    get(&row, "pipeline")?,
                    get(&row, "name")?,
                ),
            );
        }
    }
    let mut changes = Vec::with_capacity(crossed.len());
    for update in crossed {
        let Some((repo, pipeline, name)) = named.get(&update.id) else {
            continue;
        };
        changes.push(TierChange {
            kind: flakeradar_core::Kind::Job,
            id: i64::from(update.id),
            repo: repo.clone(),
            project: None,
            name: name.clone(),
            flakiness_score: update.flakiness_score,
            tier: tier_for(update.flakiness_score, threshold),
            confirmed_flake_count: usize::try_from(update.confirmed_flake_count).unwrap_or(0),
            classname: None,
            pipeline: Some(pipeline.clone()),
            failure_category: None,
        });
    }
    Ok(changes)
}

/// Writes one chunk-set of Job rescores.
async fn write_job_updates(
    db: &impl ConnectionTrait,
    updates: &[JobUpdate],
) -> Result<(), DbError> {
    for chunk in chunks(updates) {
        let mut values = Vec::with_capacity(chunk.len() * 5);
        let mut placeholders = Vec::with_capacity(chunk.len());
        for (index, update) in chunk.iter().enumerate() {
            let base = index * 5;
            placeholders.push(value_row(base + 1, 5));
            values.extend([
                Value::from(update.id),
                Value::from(update.flakiness_score),
                Value::from(update.confirmed_flake_count),
                Value::from(update.clean_streak),
                Value::from(update.clear_fixed_note),
            ]);
        }
        let sql = format!(
            "UPDATE jobs AS j SET \
               flakiness_score = v.flakiness_score, \
               confirmed_flake_count = v.confirmed_flake_count, \
               clean_streak = v.clean_streak, \
               github_issue_fixed_noted_at = CASE WHEN v.reset_note THEN NULL ELSE j.github_issue_fixed_noted_at END \
             FROM (VALUES {}) AS v(id, flakiness_score, confirmed_flake_count, clean_streak, reset_note) \
             WHERE j.id = v.id",
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
