//! The Report processor: one pending Report becomes a Run or Job executions,
//! and Tests and Jobs are rescored.
//!
//! Scoring depends on the order Executions ran in, so one Report is processed at
//! a time, and every bulk statement keeps a shape whose row order is the report's
//! own: a rewritten statement that reordered rows would move served scores.
//! `tests/parity` replays the recorded answers.

// Rust guideline compliant 2026-07-21

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use flakeradar_core::{
    classify::FailureCategory, classify::classify_failure, classify::dominant_category, defaults,
    dto::PipelineReportIn, helpers::is_broken, helpers::tier_for, model::EntityKind,
    model::ExecutionStatus, model::JobStatus, model::ReportKind, model::ReportStatus, model::Tier,
    notify::TierChange, parsing::ParseError, parsing::ParsedCase, parsing::parse_junit_xml,
    scoring::ScoredExecution, scoring::branch_scoped_fail_streak, scoring::branch_scoped_score,
    scoring::branch_scoped_streak, types::Fingerprint, types::Score,
};
use sea_orm::entity::prelude::DateTimeWithTimeZone;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, DbErr, EntityTrait, FromQueryResult,
    IntoActiveModel, QueryFilter, QueryResult, Set, Statement, TransactionSession,
    TransactionTrait, Value,
};
use serde_json::{Value as JsonValue, json};

use crate::attribution::{
    int_array, job_execution_explained_sql, job_window_ids_sql, test_window_ids_sql,
};
use crate::batching::{chunks, owned_chunks};
use crate::clock::{SharedClock, SystemClock};
use crate::entities::{pipelines, reports, repos, test_cases, test_executions, test_runs};
use crate::error::DbError;
use crate::report_intake::{
    complete_report, fail_report, requeue_report, stored_junit_body, stored_pipeline_report,
    write_report,
};
use crate::score_history::{HistoryUpdate, write_today_history};
use crate::sql::{execution_status, get, placeholder_list, rows, score, value_row};

/// The longest error text a failed Report keeps, in code points.
///
/// The text is what `/api/reports/{id}` shows the person who uploaded the file,
/// and it is one line plus the database's own message, so 2000 holds it with
/// room. It is stored in the Report's `error` column, which is `text`, so this
/// bound is the contract's: a longer message is cut with the truncation marker
/// rather than refused, because a failure must always be recorded.
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
    /// The Flakiness score below which a flaky Test or Job recovers.
    pub recover_threshold: f64,
    /// How many retries a transient failure may spend.
    pub max_report_retries: i32,
    /// The share of a Run's Tests that must fail for the Run to be suspected
    /// infrastructure.
    pub infra_fail_share: f64,
    /// The fewest distinct Tests a Run must hold to be judged for suspected
    /// infrastructure at all.
    pub infra_min_tests: i64,
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
            recover_threshold: defaults::RECOVER_THRESHOLD,
            max_report_retries: defaults::MAX_REPORT_RETRIES,
            infra_fail_share: defaults::INFRA_FAIL_SHARE,
            infra_min_tests: defaults::INFRA_MIN_TESTS,
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
            .field("recover_threshold", &self.recover_threshold)
            .field("max_report_retries", &self.max_report_retries)
            .field("infra_fail_share", &self.infra_fail_share)
            .field("infra_min_tests", &self.infra_min_tests)
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
///
/// The message is `TypeName: message`, and `TypeName` is the failure class an
/// operator reads: a failed Report's `error` is served by `GET /api/reports` and
/// shown in the dashboard's failed-Report panel, so its shape is part of the
/// wire and a 0.1.x instance's stored text reads the same way. The five names are `OperationalError` (a retryable driver failure),
/// `DBAPIError` (a driver failure no retry fixes), `DataError` (a value the
/// stored row could not hold), `ValidationError` (a stored Pipeline body that is
/// not one this server wrote) and `ParseError` (a stored JUnit body that is not
/// JUnit); `RuntimeError` covers a blocking task that did not finish. Nothing
/// branches on the name: whether a failure is retried is [`ProcessError::transient`].
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

/// Whether a driver error belongs to the family a retry can survive: a dropped
/// connection, a pool timeout or a transient SQLSTATE.
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
/// an admin delete of the Report's Repo or Project.
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

/// Parses a JUnit body and classifies each failing case: the CPU half, run off
/// the async thread.
///
/// The fingerprint of every case is hashed here, once, so neither the upsert nor
/// the Execution build hashes the same identity again.
fn parse_and_classify(body: &[u8], line_base: i32) -> Result<ParsedReport, ParseError> {
    let parsed = parse_junit_xml(body, i64::from(line_base))?;
    let cases: Vec<ReportedCase> = parsed
        .into_iter()
        .map(|case| ReportedCase {
            fingerprint: flakeradar_core::parsing::fingerprint(
                &case.suite,
                &case.classname,
                &case.name,
            ),
            category: case
                .status
                .is_failing()
                .then(|| classify_failure(&case.message, &case.details)),
            case,
        })
        .collect();
    let first = first_occurrences(&cases);
    Ok(ParsedReport { cases, first })
}

/// The index of each distinct Test's first case, in the order they first appear.
fn first_occurrences(cases: &[ReportedCase]) -> Vec<usize> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut first = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        if seen.insert(case.fingerprint.as_str()) {
            first.push(index);
        }
    }
    first
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
/// dominant Failure category, which a Job has no column for, so [`TEST_UPDATES`]
/// writes those and [`JOB_UPDATES`] leaves them out.
#[derive(Debug, Clone, Copy)]
struct Update {
    id: i32,
    flakiness_score: Score,
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

/// What one processed Report contributed: the Run, the counts and what it touched.
struct Scored {
    run_id: Option<i32>,
    counts: JsonValue,
    touched_test_ids: Vec<i32>,
    touched_job_ids: Vec<i32>,
    tier_changes: Vec<TierChange>,
}

/// Persists one claimed Report. The caller owns the transaction.
///
/// Both kinds take the same shape of steps — read the stored payload, store what
/// the Report carries, rescore what it touched — and share [`finish_report`],
/// which is its completion.
async fn process_report(
    db: &impl ConnectionTrait,
    report: &mut reports::Model,
    settings: &ProcessingSettings,
) -> Result<ProcessOutcome, ProcessError> {
    if report.kind == ReportKind::Pipeline.as_str() {
        return process_pipeline_report(db, report, settings).await;
    }
    process_junit_report(db, report, settings).await
}

/// Persists one JUnit Report: parse, store the Run and its Executions, rescore.
async fn process_junit_report(
    db: &impl ConnectionTrait,
    report: &mut reports::Model,
    settings: &ProcessingSettings,
) -> Result<ProcessOutcome, ProcessError> {
    let parsed = parse_report(report).await?;
    let now = settings.clock.now();
    // A reported Default branch changes the Repo, so apply it before inserting
    // anything: every Test in the Repo is rescored under the new value.
    let branch_changed =
        apply_default_branch(db, report.repo_id, report.default_branch.as_deref()).await?;
    write_project_root(db, report).await?;
    let project_id = report
        .project_id
        .ok_or_else(|| ProcessError::permanent("DataError: a JUnit Report has no Project"))?;
    let run_id = store_run(db, report, project_id, now).await?;
    let built = build_executions(db, parsed, project_id, run_id, now).await?;
    store_executions(db, built.executions, &built.latest, now).await?;
    // Most of a large Run failing together is the runner's problem, not a hundred
    // flaky Tests, so the Run is flagged before the rescore that reads the flag.
    mark_infra_suspect(db, run_id, &built.latest, settings).await?;

    // The ids are ordered by id, not by where a Test first appeared in the
    // report, because they set `touched_test_ids` and the order the Tier changes
    // are emitted in.
    let mut touched: Vec<i32> = built.latest.iter().map(|row| row.id).collect();
    touched.sort_unstable();
    let mut tier_changes = rescore(
        db,
        EntityKind::Test,
        &touched,
        Some(&built.day_counts),
        settings,
    )
    .await?;
    if branch_changed {
        rescore_repo(db, report.repo_id, settings).await?;
    }
    // A linked JUnit report explains (or un-explains) Job failures: rescore the
    // Jobs that carry this ci_job_id, so arrival order does not matter.
    let mut touched_job_ids = Vec::new();
    if let Some(ci_job_id) = report.ci_job_id.clone() {
        let (job_ids, job_changes) =
            rescore_jobs_for_ci_job_id(db, report.repo_id, &ci_job_id, settings).await?;
        touched_job_ids = job_ids;
        tier_changes.extend(job_changes);
    }

    let scored = Scored {
        run_id: Some(run_id),
        counts: built.counts.junit_json(),
        touched_test_ids: touched,
        touched_job_ids,
        tier_changes,
    };
    finish_report(db, report, now, scored, branch_changed).await
}

/// Completes one processed Report and answers what it did.
///
/// A Default-branch change rescored the whole Repo, so its Tier changes stay
/// silent wherever they came from (ADR 0009).
async fn finish_report(
    db: &impl ConnectionTrait,
    report: &mut reports::Model,
    now: DateTimeWithTimeZone,
    scored: Scored,
    branch_changed: bool,
) -> Result<ProcessOutcome, ProcessError> {
    let Scored {
        run_id,
        counts,
        touched_test_ids,
        touched_job_ids,
        tier_changes,
    } = scored;
    complete_report(report, now, run_id, counts.clone());
    write_report(report, db).await?;
    Ok(ProcessOutcome {
        report_id: report.id,
        status: ReportStatus::Processed,
        run_id,
        counts: Some(counts),
        touched_test_ids,
        error: None,
        touched_job_ids,
        tier_changes: if branch_changed {
            Vec::new()
        } else {
            tier_changes
        },
        retry_count: report.retry_count,
    })
}

/// Parses and classifies the stored JUnit body off the async thread.
///
/// The body moves to the blocking thread, because the parser owns the bytes it
/// reads and the row keeps its own copy for the status write.
async fn parse_report(report: &reports::Model) -> Result<ParsedReport, ProcessError> {
    let (body, line_base) = stored_junit_body(report);
    let body = body.to_vec();
    let parsed = tokio::task::spawn_blocking(move || parse_and_classify(&body, line_base))
        .await
        .map_err(|error| ProcessError::permanent(format!("RuntimeError: {error}")))?;
    Ok(parsed?)
}

/// Writes the Project root the Report named, when it differs from the stored one.
async fn write_project_root(
    db: &impl ConnectionTrait,
    report: &reports::Model,
) -> Result<(), ProcessError> {
    let Some(root) = &report.root else {
        return Ok(());
    };
    // Write only when the value actually changes, so a run of Reports with the
    // same root never rewrites (and briefly locks) the Project row.
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE projects SET root = $1 WHERE id = $2 AND root <> $1",
        [Value::from(root.clone()), Value::from(report.project_id)],
    ))
    .await?;
    Ok(())
}

/// Stores the Run this Report becomes, and answers its id.
async fn store_run(
    db: &impl ConnectionTrait,
    report: &reports::Model,
    project_id: i32,
    now: DateTimeWithTimeZone,
) -> Result<i32, ProcessError> {
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
    .await?;
    Ok(run.id)
}

/// Upserts the Report's Tests, then builds its Executions off the async thread.
async fn build_executions(
    db: &impl ConnectionTrait,
    parsed: ParsedReport,
    project_id: i32,
    run_id: i32,
    now: DateTimeWithTimeZone,
) -> Result<BuiltExecutions, ProcessError> {
    let ids = upsert_test_cases(db, project_id, &parsed, now).await?;
    tokio::task::spawn_blocking(move || build_test_executions(parsed, &ids, run_id, now))
        .await
        .map_err(|error| ProcessError::permanent(format!("RuntimeError: {error}")))?
}

/// Stores this Report's Executions and the Tests' newest status.
async fn store_executions(
    db: &impl ConnectionTrait,
    executions: Vec<test_executions::ActiveModel>,
    latest: &[LatestTest],
    now: DateTimeWithTimeZone,
) -> Result<(), ProcessError> {
    for chunk in owned_chunks(executions) {
        test_executions::Entity::insert_many(chunk)
            .exec_without_returning(db)
            .await?;
    }
    update_latest_tests(db, latest, now).await?;
    Ok(())
}

/// Flags a Run whose Tests mostly failed as suspected infrastructure.
///
/// Both limits come from [`ProcessingSettings`]: the Run must hold at least
/// `infra_min_tests` distinct Tests, and at least `infra_fail_share` of them must
/// have failed or errored on their last attempt, so a Test that recovered on retry
/// does not count against it. The flag alone changes nothing the caller can see:
/// `test_window_ids_sql` leaves the Run's Executions out of the Test scoring
/// window, which is what keeps the flip score, the Proven flake count and the
/// streaks off a bad runner. The Executions stay stored, counted and
/// listed. A Run under the floor is never judged, because a small suite that fails
/// entirely most likely has a broken Test.
async fn mark_infra_suspect(
    db: &impl ConnectionTrait,
    run_id: i32,
    latest: &[LatestTest],
    settings: &ProcessingSettings,
) -> Result<(), ProcessError> {
    // The floor is checked first, so a Run under it never reaches the division.
    let distinct = i64::try_from(latest.len()).unwrap_or(i64::MAX);
    if distinct < settings.infra_min_tests {
        return Ok(());
    }
    let mut failing = 0_i64;
    for row in latest {
        if execution_status(&row.last_status)?.is_failing() {
            failing += 1;
        }
    }
    // A Run holds at most one Test per Report row, far below 2^52, so both casts
    // are exact.
    #[expect(
        clippy::cast_precision_loss,
        reason = "Execution counts, far below 2^52"
    )]
    let share = failing as f64 / distinct as f64;
    if share < settings.infra_fail_share {
        return Ok(());
    }
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE test_runs SET infra_suspect = true WHERE id = $1",
        [Value::from(run_id)],
    ))
    .await?;
    Ok(())
}

/// One Report's cases, and the first case of each distinct Test.
struct ParsedReport {
    /// Every case the parser produced, in Report order.
    cases: Vec<ReportedCase>,
    /// The index in `cases` of each Test's first occurrence, in that order.
    first: Vec<usize>,
}

/// One Test a Report named, with the two facts the processor derives once.
struct ReportedCase {
    /// The Test's identity, hashed once for the whole Report.
    fingerprint: Fingerprint,
    /// The case the parser read.
    case: ParsedCase,
    /// The likely cause of a failure; only a failing case has one.
    category: Option<FailureCategory>,
}

/// One Report's Executions as they are built, before they are stored.
struct BuiltExecutions {
    /// The rows to insert, in Report order.
    executions: Vec<test_executions::ActiveModel>,
    /// The counts a processed Report stores.
    counts: Counts,
    /// Each Test's newest occurrence in the Report.
    latest: Vec<LatestTest>,
    /// What the day added to each Test's Score history.
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
/// feed today's Score history row. The cases are consumed, so a Failure's
/// details move into the row they belong to instead of being copied.
fn build_test_executions(
    report: ParsedReport,
    ids: &BTreeMap<String, i32>,
    run_id: i32,
    now: DateTimeWithTimeZone,
) -> Result<BuiltExecutions, ProcessError> {
    let mut counts = Counts::default();
    let mut attempts: BTreeMap<i32, i32> = BTreeMap::new();
    let mut executions: Vec<test_executions::ActiveModel> = Vec::with_capacity(report.cases.len());
    let mut latest: Vec<LatestTest> = Vec::new();
    let mut latest_by_id: BTreeMap<i32, usize> = BTreeMap::new();
    let mut day_counts: BTreeMap<i32, DayCounts> = BTreeMap::new();
    for reported in report.cases {
        let ReportedCase {
            fingerprint,
            case,
            category,
        } = reported;
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
        // A Location is stored only when the report gave a file, so a line
        // without one is never stored and never has to fit the column.
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
            message: Set(case.message),
            details: Set(case.details),
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
    Ok(BuiltExecutions {
        executions,
        counts,
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
    report: &ParsedReport,
    now: DateTimeWithTimeZone,
) -> Result<BTreeMap<String, i32>, DbError> {
    let first: Vec<&ReportedCase> = report
        .first
        .iter()
        .filter_map(|index| report.cases.get(*index))
        .collect();
    let inserted: Vec<test_cases::ActiveModel> = first
        .iter()
        .map(|reported| test_cases::ActiveModel {
            project_id: Set(project_id),
            fingerprint: Set(reported.fingerprint.as_str().to_owned()),
            suite: Set(reported.case.suite.clone()),
            classname: Set(reported.case.classname.clone()),
            name: Set(reported.case.name.clone()),
            last_seen_at: Set(now),
            ..Default::default()
        })
        .collect();
    for chunk in owned_chunks(inserted) {
        test_cases::Entity::insert_many(chunk)
            // A conflict on *any* constraint must skip the row, because the
            // unique index and the primary key can both collide. SeaORM's
            // target-less `OnConflict` is that statement, while
            // `on_conflict_do_nothing` would name the primary key only.
            .on_conflict(
                sea_orm::sea_query::OnConflict::new()
                    .do_nothing()
                    .to_owned(),
            )
            .exec_without_returning(db)
            .await?;
    }
    let mut ids = BTreeMap::new();
    for chunk in chunks(&first) {
        let placeholders = placeholder_list(2, chunk.len());
        let sql = format!(
            "SELECT fingerprint, id FROM test_cases WHERE project_id = $1 AND fingerprint IN ({placeholders})"
        );
        let mut values = Vec::with_capacity(chunk.len() + 1);
        values.push(Value::from(project_id));
        values.extend(
            chunk
                .iter()
                .map(|reported| Value::from(reported.fingerprint.as_str().to_owned())),
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
        apply_default_branch(db, report.repo_id, pipeline.default_branch.as_deref()).await?;
    let (job_ids, flag_changed) = upsert_pipeline_jobs(db, report.repo_id, &pipeline, now).await?;
    let (mut new_counts, jobs_with_new_exec, day_counts) =
        insert_pipeline_job_executions(db, &pipeline, &job_ids, now).await?;
    update_job_last_status(db, &pipeline, &job_ids, &jobs_with_new_exec, now).await?;

    let touched_job_ids: Vec<i32> = jobs_with_new_exec.union(&flag_changed).copied().collect();
    let tier_changes = rescore(
        db,
        EntityKind::Job,
        &touched_job_ids,
        Some(&day_counts),
        settings,
    )
    .await?;
    if branch_changed {
        rescore_repo(db, report.repo_id, settings).await?;
    }
    new_counts.duplicate = i64::try_from(pipeline.jobs.len()).unwrap_or(i64::MAX)
        - (new_counts.passed + new_counts.failed + new_counts.skipped);
    let scored = Scored {
        run_id: None,
        counts: new_counts.pipeline_json(),
        touched_test_ids: Vec::new(),
        touched_job_ids,
        tier_changes,
    };
    finish_report(db, report, now, scored, branch_changed).await
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
    .await?;
    let pipeline = pipelines::Entity::find()
        .filter(pipelines::Column::RepoId.eq(repo_id))
        .filter(pipelines::Column::Provider.eq(payload.provider.clone()))
        .filter(pipelines::Column::Name.eq(payload.pipeline.clone()))
        .one(db)
        .await?
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
        for row in rows(db, sql, values).await? {
            flag_changed.insert(get(&row, "id")?);
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
        for row in rows(db, sql, values).await? {
            job_ids.insert(get(&row, "name")?, get(&row, "id")?);
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
        for row in rows(db, sql, values).await? {
            let job_id: i32 = get(&row, "job_id")?;
            let status: String = get(&row, "status")?;
            jobs_with_new_exec.insert(job_id);
            let Some(status) = JobStatus::parse(&status) else {
                return Err(ProcessError::permanent(format!(
                    "DataError: unknown Job status {status}"
                )));
            };
            match status {
                JobStatus::Passed => new_counts.passed += 1,
                JobStatus::Failed => new_counts.failed += 1,
                JobStatus::Skipped => new_counts.skipped += 1,
            }
            let entry = day_counts.entry(job_id).or_default();
            entry.executions += 1;
            if status == JobStatus::Failed {
                entry.failures += 1;
            }
        }
    }
    Ok((new_counts, jobs_with_new_exec, day_counts))
}

/// The columns one `job_executions` row of the statement below binds.
///
/// It is the width of a `VALUES` row, so it is also the stride of the placeholder
/// numbering: a bind out of step with it lands in the wrong column rather than
/// failing loudly. The row is therefore a fixed-size array typed with this count,
/// which makes a mismatch a compile error. It equals the column list of the
/// `INSERT` in [`insert_job_executions_sql`], and a new column changes both.
const JOB_EXECUTION_COLUMNS: usize = 13;

/// The `INSERT ... ON CONFLICT DO NOTHING RETURNING job_id, status` for one
/// chunk, so only the rows that were actually inserted come back.
fn insert_job_executions_sql(
    chunk: &[PipelineExecution],
    now: DateTimeWithTimeZone,
) -> (String, Vec<Value>) {
    let mut values = Vec::with_capacity(chunk.len() * JOB_EXECUTION_COLUMNS);
    let mut placeholders = Vec::with_capacity(chunk.len());
    for (index, row) in chunk.iter().enumerate() {
        let base = index * JOB_EXECUTION_COLUMNS;
        let row_values: [Value; JOB_EXECUTION_COLUMNS] = [
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
        ];
        placeholders.push(value_row(base + 1, row_values.len()));
        values.extend(row_values);
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
        .await?;
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
/// One entity's scoring window: its newest Executions and its Repo's branch.
#[derive(Debug, Clone, Default)]
struct Window {
    /// Executions, newest first.
    executions: Vec<ScoredExecution>,
    /// The Failure categories of the same Executions, newest first. Only a Test
    /// is categorised, so a Job's window leaves this empty.
    categories: Vec<Option<FailureCategory>>,
    /// The Default branch of the entity's Repo, or `None` while it is unknown.
    default_branch: Option<String>,
}

impl Window {
    /// Adds one newest-first row: its Execution, and the Repo's Default branch.
    fn push(&mut self, row: &QueryResult, status: ExecutionStatus) -> Result<(), DbError> {
        self.executions.push(ScoredExecution {
            commit_sha: get(row, "commit_sha")?,
            branch: get(row, "branch")?,
            status,
        });
        self.default_branch = get(row, "default_branch")?;
        Ok(())
    }
}

/// The binds both window statements take: the ids and the window, twice.
fn window_binds(chunk: &[i32], settings: &ProcessingSettings) -> Vec<Value> {
    vec![
        int_array(chunk),
        Value::from(window_param(settings.score_window)),
        int_array(chunk),
        Value::from(window_param(settings.score_window)),
    ]
}

impl Update {
    /// The row `id`'s window scores to.
    ///
    /// A row with no window scores as an empty one: no flip, no Proven flake and
    /// a Clean streak of 0.
    fn of(id: i32, window: Option<&Window>, settings: &ProcessingSettings) -> Self {
        let empty = Window::default();
        let window = window.unwrap_or(&empty);
        let branch = window.default_branch.as_deref();
        let (score, proven) = branch_scoped_score(
            &window.executions,
            branch,
            settings.score_decay,
            settings.score_window,
        );
        let streak = branch_scoped_streak(&window.executions, branch);
        let categories: Vec<Option<FailureCategory>> = window
            .categories
            .iter()
            .take(settings.score_window)
            .copied()
            .collect();
        Self {
            id,
            flakiness_score: score,
            confirmed_flake_count: i32::try_from(proven).unwrap_or(i32::MAX),
            clean_streak: i32::try_from(streak).unwrap_or(i32::MAX),
            clear_fixed_note: streak == 0,
            fail_streak: i32::try_from(branch_scoped_fail_streak(&window.executions, branch))
                .unwrap_or(i32::MAX),
            failure_category: dominant_category(&categories),
        }
    }
}

/// The windows of every id, newest first.
async fn windows(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    ids: &[i32],
    settings: &ProcessingSettings,
) -> Result<BTreeMap<i32, Window>, DbError> {
    match kind {
        EntityKind::Test => test_windows(db, ids, settings).await,
        EntityKind::Job => job_windows(db, ids, settings).await,
    }
}

/// The Tests' scoring windows, with the Failure category of each Execution.
async fn test_windows(
    db: &impl ConnectionTrait,
    ids: &[i32],
    settings: &ProcessingSettings,
) -> Result<BTreeMap<i32, Window>, DbError> {
    let mut windows: BTreeMap<i32, Window> = BTreeMap::new();
    for chunk in chunks(ids) {
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
        for row in rows(db, sql, window_binds(chunk, settings)).await? {
            let id: i32 = get(&row, "test_case_id")?;
            let status = execution_status(&get::<String>(&row, "status")?)?;
            let window = windows.entry(id).or_default();
            window.push(&row, status)?;
            window.categories.push(
                get::<Option<String>>(&row, "failure_category")?
                    .as_deref()
                    .and_then(FailureCategory::from_wire),
            );
        }
    }
    Ok(windows)
}

/// The Jobs' scoring windows, newest first.
///
/// Explained failed Executions become `skipped`, so they never break a streak.
async fn job_windows(
    db: &impl ConnectionTrait,
    ids: &[i32],
    settings: &ProcessingSettings,
) -> Result<BTreeMap<i32, Window>, DbError> {
    let mut windows: BTreeMap<i32, Window> = BTreeMap::new();
    for chunk in chunks(ids) {
        // The table names are written out: `job_execution_explained_sql` reads
        // `job_executions`, `jobs` and `pipelines` by name, and a query that named
        // them by alias could not be spliced into this one.
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
        for row in rows(db, sql, window_binds(chunk, settings)).await? {
            let id: i32 = get(&row, "job_id")?;
            let explained: bool = get(&row, "explained")?;
            let mut status = execution_status(&get::<String>(&row, "status")?)?;
            if explained && status == ExecutionStatus::Failed {
                status = ExecutionStatus::Skipped;
            }
            windows.entry(id).or_default().push(&row, status)?;
        }
    }
    Ok(windows)
}

/// The `Notices` that report `kind`'s Tier changes.
fn notices_of(kind: EntityKind) -> &'static Notices {
    match kind {
        EntityKind::Test => &TEST_NOTICES,
        EntityKind::Job => &JOB_NOTICES,
    }
}

/// Recomputes flakiness for `kind`'s rows, and writes the Tier changes it makes.
///
/// Every rule a rescore applies is here, once: the window scores the row, the
/// old score says whether the tier crossed, the bulk write stores the row and
/// today's Score history gains the day's counts. What differs between a Test and
/// a Job is a window statement, a write statement and a set of Notices.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a read or write fails.
pub async fn rescore(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    ids: &[i32],
    day_counts: Option<&BTreeMap<i32, DayCounts>>,
    settings: &ProcessingSettings,
) -> Result<Vec<TierChange>, DbError> {
    let windows = windows(db, kind, ids, settings).await?;
    let updates: Vec<Update> = ids
        .iter()
        .map(|id| Update::of(*id, windows.get(id), settings))
        .collect();
    let old = old_scores(db, kind, ids).await?;
    let changes = tier_changes(db, notices_of(kind), &updates, &old, settings).await?;
    write_updates(db, updates_of(kind), &updates).await?;
    write_day_counts(db, kind, &updates, day_counts, settings).await?;
    Ok(changes)
}

/// Adds this rescore to today's Score history row of every id.
async fn write_day_counts(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    updates: &[Update],
    day_counts: Option<&BTreeMap<i32, DayCounts>>,
    settings: &ProcessingSettings,
) -> Result<(), DbError> {
    if updates.is_empty() {
        return Ok(());
    }
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
    write_today_history(db, kind, &history_updates, settings.clock.today()).await
}

/// The scores a rescore is about to replace, read before the update.
async fn old_scores(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    ids: &[i32],
) -> Result<BTreeMap<i32, Score>, DbError> {
    let from = match kind {
        EntityKind::Test => "SELECT id, flakiness_score FROM test_cases WHERE id IN ",
        EntityKind::Job => "SELECT id, flakiness_score FROM jobs WHERE id IN ",
    };
    let mut scores = BTreeMap::new();
    for chunk in chunks(ids) {
        let sql = format!("{}({})", from, placeholder_list(1, chunk.len()));
        let values = chunk.iter().map(|id| Value::from(*id)).collect();
        for row in rows(db, sql, values).await? {
            scores.insert(
                get(&row, "id")?,
                score(get(&row, "flakiness_score")?, "flakiness_score")?,
            );
        }
    }
    Ok(scores)
}

/// Whether one update crossed the Flake threshold, in either direction.
///
/// Only the flaky boundary counts: a move between Stable and Suspect changes the
/// tier but crosses nothing, so it sends no Notification. A row with no stored
/// score is new, and new is 0.0.
fn crossed(update: &Update, old: &BTreeMap<i32, Score>, threshold: f64) -> bool {
    let before = old.get(&update.id).copied().unwrap_or(Score::ZERO);
    (update.flakiness_score.as_f64() >= threshold) != (before.as_f64() >= threshold)
}

/// What differs between a Test's and a Job's Tier change.
struct Notices {
    /// The statement naming the rows an event reports, by id.
    names_sql: fn(&str) -> String,
    /// Whether a row that left the flaky tier has recovered.
    ///
    /// A Job always has. A Test that is now broken has not fixed itself, so it
    /// sends no event.
    recovered: fn(&Update, &ProcessingSettings) -> bool,
    /// Builds the event from one row's name columns.
    change: fn(&Update, &QueryResult, f64) -> Result<TierChange, DbError>,
}

/// The Tests' Tier changes.
const TEST_NOTICES: Notices = Notices {
    names_sql: test_names_sql,
    recovered: test_recovered,
    change: test_change,
};

/// The Jobs' Tier changes.
const JOB_NOTICES: Notices = Notices {
    names_sql: job_names_sql,
    recovered: job_recovered,
    change: job_change,
};

/// The statement naming the Tests an event reports, by id.
fn test_names_sql(placeholders: &str) -> String {
    format!(
        "SELECT t.id, t.classname, t.name, p.name AS project, r.name AS repo \
           FROM test_cases t \
           JOIN projects p ON p.id = t.project_id \
           JOIN repos r ON r.id = p.repo_id \
          WHERE t.id IN ({placeholders}) ORDER BY t.id"
    )
}

/// The statement naming the Jobs an event reports, by id.
fn job_names_sql(placeholders: &str) -> String {
    format!(
        "SELECT j.id, j.name, p.name AS pipeline, r.name AS repo \
           FROM jobs j \
           JOIN pipelines p ON p.id = j.pipeline_id \
           JOIN repos r ON r.id = p.repo_id \
          WHERE j.id IN ({placeholders}) ORDER BY j.id"
    )
}

/// Whether a Test that left the flaky tier has recovered.
fn test_recovered(update: &Update, settings: &ProcessingSettings) -> bool {
    !is_broken(i64::from(update.fail_streak), settings.broken_streak)
}

/// Whether a Job that left the flaky tier has recovered.
fn job_recovered(_update: &Update, _settings: &ProcessingSettings) -> bool {
    true
}

/// The Tier change a Test's row names.
fn test_change(update: &Update, row: &QueryResult, threshold: f64) -> Result<TierChange, DbError> {
    Ok(TierChange {
        kind: EntityKind::Test,
        id: i64::from(update.id),
        repo: get(row, "repo")?,
        project: Some(get(row, "project")?),
        name: get(row, "name")?,
        flakiness_score: update.flakiness_score,
        tier: tier_for(update.flakiness_score, threshold),
        proven_flake_count: usize::try_from(update.confirmed_flake_count).unwrap_or(0),
        classname: Some(get(row, "classname")?),
        pipeline: None,
        failure_category: update.failure_category,
    })
}

/// The Tier change a Job's row names.
fn job_change(update: &Update, row: &QueryResult, threshold: f64) -> Result<TierChange, DbError> {
    Ok(TierChange {
        kind: EntityKind::Job,
        id: i64::from(update.id),
        repo: get(row, "repo")?,
        project: None,
        name: get(row, "name")?,
        flakiness_score: update.flakiness_score,
        tier: tier_for(update.flakiness_score, threshold),
        proven_flake_count: usize::try_from(update.confirmed_flake_count).unwrap_or(0),
        classname: None,
        pipeline: Some(get(row, "pipeline")?),
        failure_category: None,
    })
}

/// Notifications for the rows whose tier this rescore moves (ADR 0009).
///
/// The crossing rule, the id order and the read of the names are the same for
/// both entities; the descriptor carries what differs.
async fn tier_changes(
    db: &impl ConnectionTrait,
    notices: &Notices,
    updates: &[Update],
    old: &BTreeMap<i32, Score>,
    settings: &ProcessingSettings,
) -> Result<Vec<TierChange>, DbError> {
    let threshold = settings.flake_threshold;
    let mut crossing: Vec<Update> = updates
        .iter()
        .filter(|update| {
            crossed(update, old, threshold)
                && (tier_for(update.flakiness_score, threshold) == Tier::Flaky
                    || (notices.recovered)(update, settings))
        })
        .copied()
        .collect();
    if crossing.is_empty() {
        return Ok(Vec::new());
    }
    // The events go out in id order whatever order the caller listed the ids in:
    // the named rows are read back into a map keyed by id.
    crossing.sort_unstable_by_key(|update| update.id);
    let by_id: BTreeMap<i32, Update> = crossing.iter().map(|update| (update.id, *update)).collect();
    let ids: Vec<i32> = crossing.iter().map(|update| update.id).collect();
    let mut named: BTreeMap<i32, TierChange> = BTreeMap::new();
    for chunk in chunks(&ids) {
        let placeholders = placeholder_list(1, chunk.len());
        let sql = (notices.names_sql)(&placeholders);
        let values = chunk.iter().map(|id| Value::from(*id)).collect();
        for row in rows(db, sql, values).await? {
            let id: i32 = get(&row, "id")?;
            let Some(update) = by_id.get(&id) else {
                continue;
            };
            named.insert(id, (notices.change)(update, &row, threshold)?);
        }
    }
    // A row deleted before this read names nothing, so it sends nothing.
    Ok(crossing
        .iter()
        .filter_map(|update| named.get(&update.id).cloned())
        .collect())
}

/// One rescore write: the statement, and the values one row binds into it.
///
/// The two halves are the SQL as PostgreSQL receives it, so a statement can be
/// grepped for what it sets. The rows go between them, and one row's own length
/// numbers the next row's placeholders, so the two statements cannot drift from
/// their binds.
struct BulkUpdate {
    /// The `UPDATE ... FROM (VALUES` the rows follow.
    head: &'static str,
    /// The `) AS v(...) WHERE ...` the rows precede.
    tail: &'static str,
    /// The values one update binds, in the order `tail` names its columns.
    row: fn(&Update) -> Vec<Value>,
}

/// The Tests' rescore write, which also stores the Failure category and Fail streak.
const TEST_UPDATES: BulkUpdate = BulkUpdate {
    head: "UPDATE test_cases AS t SET flakiness_score = v.flakiness_score, \
             confirmed_flake_count = v.confirmed_flake_count, \
             failure_category = v.failure_category, clean_streak = v.clean_streak, \
             fail_streak = v.fail_streak, \
             github_issue_fixed_noted_at = CASE WHEN v.reset_note THEN NULL ELSE t.github_issue_fixed_noted_at END \
           FROM (VALUES ",
    tail: ") AS v(id, flakiness_score, confirmed_flake_count, failure_category, clean_streak, \
             fail_streak, reset_note) WHERE t.id = v.id",
    row: |update| {
        vec![
            Value::from(update.id),
            Value::from(update.flakiness_score.as_f64()),
            Value::from(update.confirmed_flake_count),
            update
                .failure_category
                .map_or(Value::String(None), |category| {
                    Value::from(category.as_str())
                }),
            Value::from(update.clean_streak),
            Value::from(update.fail_streak),
            Value::from(update.clear_fixed_note),
        ]
    },
};

/// The Jobs' rescore write: no Failure category is scored and no Fail streak kept.
const JOB_UPDATES: BulkUpdate = BulkUpdate {
    head: "UPDATE jobs AS j SET flakiness_score = v.flakiness_score, \
             confirmed_flake_count = v.confirmed_flake_count, clean_streak = v.clean_streak, \
             github_issue_fixed_noted_at = CASE WHEN v.reset_note THEN NULL ELSE j.github_issue_fixed_noted_at END \
           FROM (VALUES ",
    tail: ") AS v(id, flakiness_score, confirmed_flake_count, clean_streak, reset_note) \
           WHERE j.id = v.id",
    row: |update| {
        vec![
            Value::from(update.id),
            Value::from(update.flakiness_score.as_f64()),
            Value::from(update.confirmed_flake_count),
            Value::from(update.clean_streak),
            Value::from(update.clear_fixed_note),
        ]
    },
};

/// The rescore write for `kind`'s rows.
const fn updates_of(kind: EntityKind) -> &'static BulkUpdate {
    match kind {
        EntityKind::Test => &TEST_UPDATES,
        EntityKind::Job => &JOB_UPDATES,
    }
}

/// Writes one chunk-set of rescores, one statement per chunk.
///
/// The fixed-note reset is the same `CASE` in both statements: a Clean streak of 0
/// clears the note and any other streak leaves it alone.
async fn write_updates(
    db: &impl ConnectionTrait,
    updates: &BulkUpdate,
    rows: &[Update],
) -> Result<(), DbError> {
    for chunk in chunks(rows) {
        let mut values = Vec::new();
        let mut placeholders = Vec::with_capacity(chunk.len());
        for update in chunk {
            let row = (updates.row)(update);
            placeholders.push(value_row(values.len() + 1, row.len()));
            values.extend(row);
        }
        let sql = format!(
            "{}{}{}",
            updates.head,
            placeholders.join(", "),
            updates.tail
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
    let mut windows = job_windows(db, job_ids, settings).await?;
    Ok(job_ids
        .iter()
        .map(|id| {
            // The window is consumed: a Job's whole history is large, so it is
            // taken out of the map rather than copied out of it.
            let window = windows.remove(id).unwrap_or_default();
            (
                *id,
                JobHistory {
                    default_branch: window.default_branch,
                    executions: window.executions,
                },
            )
        })
        .collect())
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
    for row in rows(db, sql, vec![Value::from(ci_job_id), Value::from(repo_id)]).await? {
        job_ids.insert(get(&row, "job_id")?);
    }
    let job_ids: Vec<i32> = job_ids.into_iter().collect();
    if job_ids.is_empty() {
        return Ok((job_ids, Vec::new()));
    }
    let changes = rescore(db, EntityKind::Job, &job_ids, None, settings).await?;
    Ok((job_ids, changes))
}

/// Rescores every Test and every Job in the Repo (chunked).
///
/// The Tier changes are dropped on purpose: a Default branch change rescores the
/// whole Repo at once, so notifying about it would send hundreds of events for
/// one Report (ADR 0009). Only their count is reported.
async fn rescore_repo(
    db: &impl ConnectionTrait,
    repo_id: i32,
    settings: &ProcessingSettings,
) -> Result<(), DbError> {
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
    let mut dropped = rescore(db, EntityKind::Test, &test_ids, None, settings)
        .await?
        .len();
    dropped += rescore(db, EntityKind::Job, &job_ids, None, settings)
        .await?
        .len();
    if dropped > 0 {
        // One event per changed Test would flood the channel, so only the count
        // is reported (ADR 0009).
        tracing::info!(
            name: "db.processing.notifications_dropped",
            repo_id,
            dropped,
            "Rescore of a Repo dropped Notifications",
        );
    }
    Ok(())
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
    use crate::error::DbError;

    /// A statement failure, wrapped the way the driver returns one.
    fn driver(error: SqlxError) -> DbErr {
        DbErr::Exec(RuntimeErr::SqlxError(Arc::new(error)))
    }

    #[test]
    fn a_transient_failure_is_the_connection_and_the_transient_sqlstates() {
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
