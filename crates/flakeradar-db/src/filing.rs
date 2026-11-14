//! The rows the issue filer selects: its gate, its candidates and its notes.
//!
//! The GitHub filer decides *what* to file and *how* to say it; which rows
//! qualify is a question about the schema, so those statements live here. Three
//! selections make up the module: the Tests and Jobs that pass [`FilingGate`],
//! the newest Executions an issue body quotes, and the linked rows whose fix
//! appears to hold ([`note_candidates`]).
//!
//! Every selection chunks its ids at [`crate::CHUNK`], so one statement never
//! carries more binds than a PostgreSQL statement accepts.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, Utc};
use flakeradar_core::{
    model::EntityKind, model::ExecutionStatus, types::Fingerprint, types::Score,
};
use sea_orm::sea_query::Value;
use sea_orm::{ConnectionTrait, QueryResult};

use crate::attribution::{int_array, test_window_ids_overall_sql, unexplained_failure_counts};
use crate::batching::chunks;
use crate::error::DbError;
use crate::sql::{execution_status, fingerprint, get, rows, score};

/// The gate every fileable row must pass, as one value.
///
/// Each field is one configured minimum; `min_failures` of `0` skips the failure
/// count, and `fixed_streak` set to `Some` excludes rows whose Clean streak has
/// already reached the fixed-note threshold, because such a row has no live flake
/// to file.
#[derive(Debug, Clone, Copy)]
pub struct FilingGate {
    /// The Flakiness score every row must reach.
    pub min_score: f64,
    /// The Proven flake count every row must reach.
    pub min_proven_flakes: i64,
    /// The recent failure count every row must reach; `0` skips the check.
    pub min_failures: i64,
    /// The Clean streak at which a fix holds rather than files; `None` disables it.
    pub fixed_streak: Option<i64>,
}

impl FilingGate {
    /// The gate these minimums state.
    #[must_use]
    pub fn new(
        min_score: f64,
        min_proven_flakes: i64,
        min_failures: i64,
        fixed_streak: Option<i64>,
    ) -> Self {
        Self {
            min_score,
            min_proven_flakes,
            min_failures,
            fixed_streak,
        }
    }
}

/// One Test as the issue body reads it.
#[derive(Debug, Clone)]
pub struct TestRow {
    /// The Test's id.
    pub id: i32,
    /// The classname the runner reported.
    pub classname: String,
    /// The Test's name.
    pub name: String,
    /// The suite the runner reported.
    pub suite: String,
    /// The file the Test is defined in.
    pub file: Option<String>,
    /// The line the Test is defined on.
    pub line: Option<i32>,
    /// The Test's stable identity hash.
    pub fingerprint: Fingerprint,
    /// The Flakiness score.
    pub flakiness_score: Score,
    /// The Proven flake count.
    pub confirmed_flake_count: i32,
}

/// One Job as the issue body reads it.
#[derive(Debug, Clone)]
pub struct JobRow {
    /// The Job's id.
    pub id: i32,
    /// The Job's name within its Pipeline.
    pub name: String,
    /// The Flakiness score.
    pub flakiness_score: Score,
    /// The Proven flake count.
    pub confirmed_flake_count: i32,
}

/// Reads the columns an issue body needs from one Test row.
fn test_row(row: &QueryResult) -> Result<TestRow, DbError> {
    Ok(TestRow {
        id: get(row, "id")?,
        classname: get(row, "classname")?,
        name: get(row, "name")?,
        suite: get(row, "suite")?,
        file: get(row, "file")?,
        line: get(row, "line")?,
        fingerprint: fingerprint(&get::<String>(row, "fingerprint")?, "fingerprint")?,
        flakiness_score: score(get(row, "flakiness_score")?, "flakiness_score")?,
        confirmed_flake_count: get(row, "confirmed_flake_count")?,
    })
}

/// Reads the columns a Job issue body needs.
fn job_row(row: &QueryResult) -> Result<JobRow, DbError> {
    Ok(JobRow {
        id: get(row, "id")?,
        name: get(row, "name")?,
        flakiness_score: score(get(row, "flakiness_score")?, "flakiness_score")?,
        confirmed_flake_count: get(row, "confirmed_flake_count")?,
    })
}

/// One recent Execution of a Test, with the Run it is in.
#[derive(Debug, Clone)]
pub struct RecentExecution {
    /// The Execution's outcome.
    pub status: ExecutionStatus,
    /// The Failure message.
    pub message: String,
    /// The Failure details.
    pub details: Option<String>,
    /// The Run's commit.
    pub commit_sha: String,
    /// The Run's branch.
    pub branch: String,
    /// When the Execution ran.
    pub created_at: DateTime<Utc>,
}

/// The newest Executions of one Test, newest first, at most `limit` of them.
///
/// "Newest" is by Execution id, which is the order the Runs were processed in,
/// so the read is stable however the rows are stored.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn recent_executions(
    db: &impl ConnectionTrait,
    test_id: i32,
    limit: i64,
) -> Result<Vec<RecentExecution>, DbError> {
    let sql = format!(
        "SELECT e.status, e.message, e.details, e.created_at, r.commit_sha, r.branch \
         FROM test_executions e JOIN test_runs r ON r.id = e.test_run_id \
         WHERE e.test_case_id = $1 ORDER BY e.id DESC LIMIT {limit}"
    );
    let mut recent = Vec::new();
    for row in rows(db, sql, vec![Value::from(test_id)]).await? {
        let created: DateTime<FixedOffset> = get(&row, "created_at")?;
        recent.push(RecentExecution {
            status: execution_status(&get::<String>(&row, "status")?)?,
            message: get(&row, "message")?,
            details: get(&row, "details")?,
            commit_sha: get(&row, "commit_sha")?,
            branch: get(&row, "branch")?,
            created_at: created.with_timezone(&Utc),
        });
    }
    Ok(recent)
}

/// The failed/error Executions per Test over its newest `window` Executions.
///
/// The window is the overall one, with no Default-branch arm, so older
/// Default-branch failures that the newest `window` Executions no longer reach do
/// not count.
async fn window_failures(
    db: &impl ConnectionTrait,
    test_ids: &[i32],
    window: usize,
) -> Result<BTreeMap<i32, i64>, DbError> {
    let mut counts = BTreeMap::new();
    for chunk in chunks(test_ids) {
        let sql = format!(
            "SELECT e.test_case_id, e.status FROM test_executions e \
             WHERE e.id IN (SELECT w.id FROM ({}) AS w)",
            test_window_ids_overall_sql()
        );
        let window = i64::try_from(window).unwrap_or(i64::MAX);
        let values = vec![int_array(chunk), Value::from(window)];
        for row in rows(db, sql, values).await? {
            let status: String = get(&row, "status")?;
            if ExecutionStatus::from_wire(&status).is_some_and(ExecutionStatus::is_failing) {
                *counts.entry(get::<i32>(&row, "test_case_id")?).or_insert(0) += 1;
            }
        }
    }
    Ok(counts)
}

/// The gate's binds, in the order both filing statements place them, and whether the
/// fourth of them exists.
///
/// `$1` is the chunk of ids, `$2` and `$3` the score and Proven-flake minimums, and
/// `$4` the fixed-note streak — bound only when the gate sets one, which is the third
/// part of the answer because the statement has to leave its clause out at the same
/// time. Each statement keeps its own clause text, so the SQL stays greppable.
fn gate_binds(chunk: &[i32], gate: &FilingGate) -> (bool, Vec<Value>) {
    let mut values = vec![
        int_array(chunk),
        Value::from(gate.min_score),
        Value::from(i32::try_from(gate.min_proven_flakes).unwrap_or(i32::MAX)),
    ];
    let streak = match gate.fixed_streak {
        Some(streak) => {
            values.push(Value::from(i32::try_from(streak).unwrap_or(i32::MAX)));
            true
        }
        None => false,
    };
    (streak, values)
}

/// One Test that meets the filing gate and still has no open issue.
#[derive(Debug, Clone)]
pub struct TestCandidate {
    /// The Test itself.
    pub test: TestRow,
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The Project's name.
    pub project: String,
    /// The Project root.
    pub root: String,
}

/// One Job that meets the filing gate and still has no open issue.
#[derive(Debug, Clone)]
pub struct JobCandidate {
    /// The Job itself.
    pub job: JobRow,
    /// The Pipeline's name.
    pub pipeline: String,
    /// The Repo's `owner/name`.
    pub repo: String,
}

/// The Tests that meet `gate` and still have no open issue.
///
/// `window` is the number of newest Executions the failure minimum counts over,
/// read only when the gate sets one.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn test_candidates(
    db: &impl ConnectionTrait,
    test_ids: &[i32],
    gate: &FilingGate,
    window: usize,
) -> Result<Vec<TestCandidate>, DbError> {
    let mut candidates = Vec::new();
    for chunk in chunks(test_ids) {
        let (streak, values) = gate_binds(chunk, gate);
        let fixed = if streak {
            " AND t.clean_streak < $4"
        } else {
            ""
        };
        // This comparison is the GitHub filing gate (`GITHUB_ISSUE_MIN_SCORE`):
        // a score, not a tier. A row whose tier is merely suspect files nothing,
        // so `flaky_since` is not what this query reads.
        let sql = format!(
            "SELECT t.*, p.name AS project_name, p.root AS root, r.name AS repo_name \
             FROM test_cases t JOIN projects p ON p.id = t.project_id \
             JOIN repos r ON r.id = p.repo_id \
             WHERE t.id = ANY($1::INTEGER[]) AND t.flakiness_score >= $2 \
               AND t.confirmed_flake_count >= $3 AND t.github_issue_number IS NULL{fixed} \
             ORDER BY t.id"
        );
        for row in rows(db, sql, values).await? {
            candidates.push(TestCandidate {
                test: test_row(&row)?,
                repo: get(&row, "repo_name")?,
                project: get(&row, "project_name")?,
                root: get(&row, "root")?,
            });
        }
    }
    if gate.min_failures > 0 {
        let ids: Vec<i32> = candidates
            .iter()
            .map(|candidate| candidate.test.id)
            .collect();
        let failures = window_failures(db, &ids, window).await?;
        candidates.retain(|candidate| {
            failures
                .get(&candidate.test.id)
                .is_some_and(|count| *count >= gate.min_failures)
        });
    }
    Ok(candidates)
}

/// The github-provider Jobs that meet `gate` and still have no open issue.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn job_candidates(
    db: &impl ConnectionTrait,
    job_ids: &[i32],
    gate: &FilingGate,
    window: usize,
) -> Result<Vec<JobCandidate>, DbError> {
    let mut candidates = Vec::new();
    for chunk in chunks(job_ids) {
        let (streak, values) = gate_binds(chunk, gate);
        let fixed = if streak {
            " AND j.clean_streak < $4"
        } else {
            ""
        };
        // This comparison is the GitHub filing gate (`GITHUB_ISSUE_MIN_SCORE`):
        // a score, not a tier. A Job whose tier is merely suspect files nothing,
        // so `flaky_since` is not what this query reads.
        let sql = format!(
            "SELECT j.*, p.name AS pipeline_name, r.name AS repo_name \
             FROM jobs j JOIN pipelines p ON p.id = j.pipeline_id \
             JOIN repos r ON r.id = p.repo_id \
             WHERE j.id = ANY($1::INTEGER[]) AND p.provider = 'github' \
               AND j.flakiness_score >= $2 AND j.confirmed_flake_count >= $3 \
               AND j.github_issue_number IS NULL{fixed} \
             ORDER BY j.id"
        );
        for row in rows(db, sql, values).await? {
            candidates.push(JobCandidate {
                job: job_row(&row)?,
                pipeline: get(&row, "pipeline_name")?,
                repo: get(&row, "repo_name")?,
            });
        }
    }
    if gate.min_failures > 0 {
        let ids: Vec<i32> = candidates
            .iter()
            .map(|candidate| candidate.job.id)
            .collect();
        let failures = unexplained_failure_counts(db, &ids, window).await?;
        candidates.retain(|candidate| {
            failures
                .get(&candidate.job.id)
                .is_some_and(|count| i64::from(*count) >= gate.min_failures)
        });
    }
    Ok(candidates)
}

/// One linked row the fixed-note step selected.
#[derive(Debug, Clone)]
pub struct NoteCandidate {
    /// Whether the row is a Test or a Job.
    pub target: EntityKind,
    /// The Test's or Job's id.
    pub entity_id: i32,
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The Repo's Default branch, when it is known.
    pub default_branch: Option<String>,
    /// The Clean streak the comment names.
    pub clean_streak: i32,
    /// The Flakiness score the comment names.
    pub score: Score,
    /// The stored issue number.
    pub number: i32,
}

/// The fixed-note selection, per entity: both statements name their columns the
/// same way (`id`, `number`, `clean_streak`, `flakiness_score`, `repo_name`,
/// `default_branch`), so [`note_candidates`] reads either with one mapper.
///
/// A Job is selected only from a github-provider Pipeline, because a Job of
/// another provider has no issue to note.
const NOTE_CANDIDATES: [(EntityKind, &str); 2] = [
    (
        EntityKind::Test,
        "SELECT t.id AS id, t.github_issue_number AS number, t.clean_streak, \
                t.flakiness_score, r.name AS repo_name, r.default_branch \
           FROM test_cases t JOIN projects p ON p.id = t.project_id \
           JOIN repos r ON r.id = p.repo_id \
          WHERE t.id = ANY($1::INTEGER[]) AND t.github_issue_number IS NOT NULL \
            AND t.github_issue_fixed_noted_at IS NULL AND t.github_issue_closed IS FALSE \
            AND t.clean_streak >= $2 ORDER BY t.id",
    ),
    (
        EntityKind::Job,
        "SELECT j.id AS id, j.github_issue_number AS number, j.clean_streak, \
                j.flakiness_score, r.name AS repo_name, r.default_branch \
           FROM jobs j JOIN pipelines p ON p.id = j.pipeline_id \
           JOIN repos r ON r.id = p.repo_id \
          WHERE j.id = ANY($1::INTEGER[]) AND p.provider = 'github' \
            AND j.github_issue_number IS NOT NULL \
            AND j.github_issue_fixed_noted_at IS NULL AND j.github_issue_closed IS FALSE \
            AND j.clean_streak >= $2 ORDER BY j.id",
    ),
];

/// The linked rows of `kind` at the note threshold whose fix has not been noted yet.
///
/// The Tests and the Jobs are two statements — they read different tables and the
/// Job one insists on a github Pipeline — and one mapper, because every column the
/// note needs is named the same in both.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn note_candidates(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    ids: &[i32],
    fixed_streak: i64,
) -> Result<Vec<NoteCandidate>, DbError> {
    let (_, sql) = NOTE_CANDIDATES
        .into_iter()
        .find(|(entity, _)| *entity == kind)
        .unwrap_or(NOTE_CANDIDATES[0]);
    let threshold = i32::try_from(fixed_streak).unwrap_or(i32::MAX);
    let mut candidates = Vec::new();
    for chunk in chunks(ids) {
        for row in rows(db, sql, vec![int_array(chunk), Value::from(threshold)]).await? {
            candidates.push(NoteCandidate {
                target: kind,
                entity_id: get(&row, "id")?,
                repo: get(&row, "repo_name")?,
                default_branch: get(&row, "default_branch")?,
                clean_streak: get(&row, "clean_streak")?,
                score: score(get(&row, "flakiness_score")?, "flakiness_score")?,
                number: get(&row, "number")?,
            });
        }
    }
    Ok(candidates)
}
