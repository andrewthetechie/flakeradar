//! Job attribution: which failed Job executions are explained, and by what.
//!
//! A failed Job execution is *explained* when a Run with the same `ci_job_id`
//! in the same Repo has a failing Test execution, or when the Job is a Summary
//! Job and a non-summary Job failed in the same CI run attempt. Scoring, the
//! read API and issue filing share these rules, so they live here once. The
//! rules are the frozen Python server's `attribution.py`.

// Rust guideline compliant 2026-07-21

use std::collections::{BTreeMap, BTreeSet};

use flakeradar_core::dto::ExplainingJobOut;
use sea_orm::{ConnectionTrait, Value};

use crate::batching::chunks;
use crate::error::DbError;
use crate::sql::{get, rows, value_row};

/// The most explaining sibling Jobs listed per `(ci_run_id, ci_run_attempt)`.
///
/// An explained failure names the Jobs of the same CI run attempt that failed
/// too, which is how a person sees whether a summary Job is the only failure. A
/// wide matrix can fail twenty-plus Jobs at once and the list is read as a
/// sentence, so 20 is where the answer stops growing. Raising it lengthens the
/// answer for every failed Job execution; lowering it hides the sibling that
/// explains the failure.
pub const SIBLING_MAX: usize = 20;

/// Joins a `Vec<i32>` as one PostgreSQL `integer[]` bind parameter.
///
/// The window queries take the whole chunk as one array, exactly as the frozen
/// server's `unnest(bindparam(...))` did, so the statement keeps the measured
/// lateral shape in `docs/perf/20-test-rescore-explain.md`.
#[must_use]
pub(crate) fn int_array(ids: &[i32]) -> Value {
    Value::Array(
        sea_orm::sea_query::ArrayType::Int,
        Some(Box::new(ids.iter().copied().map(Value::from).collect())),
    )
}

/// The newest `window` Test-execution ids per Test: `unnest` plus per-Test
/// `LATERAL ... LIMIT`, unioned with the newest `window` on the Default branch.
///
/// The statement is the one measured in `docs/perf/20-test-rescore-explain.md`:
/// each arm stops at the window on `ix_exec_case_id`, so its cost follows the
/// window rather than the Test's retained history (a Repo whose Default branch
/// rarely runs CI is the accepted worst case). Binds: `$1` ids, `$2` window,
/// `$3` ids, `$4` window.
#[must_use]
pub(crate) fn test_window_ids_sql() -> &'static str {
    "SELECT w.test_case_id, w.id FROM (\
       SELECT u.test_case_id, l.id \
         FROM unnest($1::INTEGER[]) AS u(test_case_id) \
         JOIN LATERAL ( \
           SELECT e.id FROM test_executions e \
            WHERE e.test_case_id = u.test_case_id \
            ORDER BY e.id DESC LIMIT $2::INTEGER \
         ) AS l ON true \
       UNION \
       SELECT u.test_case_id, l.id \
         FROM unnest($3::INTEGER[]) AS u(test_case_id) \
         JOIN LATERAL ( \
           SELECT e.id FROM test_executions e \
             JOIN test_runs r ON r.id = e.test_run_id \
             JOIN projects p ON p.id = r.project_id \
             JOIN repos ro ON ro.id = p.repo_id \
            WHERE e.test_case_id = u.test_case_id \
              AND (ro.default_branch IS NULL OR r.branch = ro.default_branch) \
            ORDER BY e.id DESC LIMIT $4::INTEGER \
         ) AS l ON true \
     ) AS w"
}

/// The newest `window` Test-execution ids per Test, any branch: the first arm of
/// [`test_window_ids_sql`] alone. Binds `$1` ids and `$2` window.
#[must_use]
pub(crate) fn test_window_ids_overall_sql() -> &'static str {
    "SELECT u.test_case_id, l.id \
       FROM unnest($1::INTEGER[]) AS u(test_case_id) \
       JOIN LATERAL ( \
         SELECT e.id FROM test_executions e \
          WHERE e.test_case_id = u.test_case_id \
          ORDER BY e.id DESC LIMIT $2::INTEGER \
       ) AS l ON true"
}

/// The newest `window` Job-execution ids per Job, the Job twin of
/// [`test_window_ids_sql`] and the statement measured in
/// `docs/perf/21-job-rescore-explain.md`. Binds are the same four.
#[must_use]
pub(crate) fn job_window_ids_sql() -> &'static str {
    "SELECT w.job_id, w.id FROM (\
       SELECT u.job_id, l.id \
         FROM unnest($1::INTEGER[]) AS u(job_id) \
         JOIN LATERAL ( \
           SELECT e.id FROM job_executions e \
            WHERE e.job_id = u.job_id \
            ORDER BY e.id DESC LIMIT $2::INTEGER \
         ) AS l ON true \
       UNION \
       SELECT u.job_id, l.id \
         FROM unnest($3::INTEGER[]) AS u(job_id) \
         JOIN LATERAL ( \
           SELECT e.id FROM job_executions e \
             JOIN jobs j ON j.id = e.job_id \
             JOIN pipelines p ON p.id = j.pipeline_id \
             JOIN repos ro ON ro.id = p.repo_id \
            WHERE e.job_id = u.job_id \
              AND (ro.default_branch IS NULL OR e.branch = ro.default_branch) \
            ORDER BY e.id DESC LIMIT $4::INTEGER \
         ) AS l ON true \
     ) AS w"
}

/// The newest `window` Job-execution ids per Job, any branch: the first arm of
/// [`job_window_ids_sql`] alone. Binds `$1` ids and `$2` window.
#[must_use]
pub(crate) fn job_window_ids_overall_sql() -> &'static str {
    "SELECT u.job_id, l.id \
       FROM unnest($1::INTEGER[]) AS u(job_id) \
       JOIN LATERAL ( \
         SELECT e.id FROM job_executions e \
          WHERE e.job_id = u.job_id \
          ORDER BY e.id DESC LIMIT $2::INTEGER \
       ) AS l ON true"
}

/// A failing Test run with this `ci_job_id` in the Repo explains the failure.
#[must_use]
pub(crate) fn test_explained_sql(exec: &str, pipeline: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM test_runs tr \
           JOIN projects tp ON tp.id = tr.project_id \
           JOIN test_executions te ON te.test_run_id = tr.id \
          WHERE tr.ci_job_id = {exec}.ci_job_id \
            AND tp.repo_id = {pipeline}.repo_id \
            AND te.status IN ('failed', 'error'))"
    )
}

/// A failed non-summary Job in the same attempt explains a Summary Job's failure.
///
/// `ci_run_id` must be non-empty: a reporter that omits it would collapse every
/// run onto one attempt, and an unrelated older failure would explain the
/// Summary Job forever.
#[must_use]
pub(crate) fn summary_explained_sql(exec: &str, job: &str, pipeline: &str) -> String {
    format!(
        "{exec}.ci_run_id <> '' AND {job}.summary AND EXISTS ( \
           SELECT 1 FROM job_executions sje \
             JOIN jobs sj ON sj.id = sje.job_id \
             JOIN pipelines sp ON sp.id = sj.pipeline_id \
            WHERE sje.ci_run_id = {exec}.ci_run_id \
              AND sje.ci_run_attempt = {exec}.ci_run_attempt \
              AND sje.status = 'failed' \
              AND NOT sj.summary \
              AND sp.repo_id = {pipeline}.repo_id)"
    )
}

/// The whole attribution rule for a query that joins `job_executions`,
/// `jobs` and `pipelines` under those names.
#[must_use]
pub(crate) fn job_execution_explained_sql() -> String {
    format!(
        "({} OR {})",
        test_explained_sql("job_executions", "pipelines"),
        summary_explained_sql("job_executions", "jobs", "pipelines")
    )
}

/// The `IN ($1, $2, ...)` placeholder list for a text bind, starting at `start`.
///
/// Text binds are spelled one by one because `= ANY(array)` would plan
/// differently from the `IN` list the read paths use.
fn text_placeholders(start: usize, count: usize) -> String {
    (start..start + count)
        .map(|index| format!("${index}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The subset of `ci_job_ids` explained by a failing Test or a failed sibling Job.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn explained_ci_job_ids(
    db: &impl ConnectionTrait,
    repo_id: i32,
    ci_job_ids: &[String],
) -> Result<BTreeSet<String>, DbError> {
    let wanted: Vec<String> = ci_job_ids
        .iter()
        .filter(|id| !id.is_empty())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut found = BTreeSet::new();
    for chunk in chunks(&wanted) {
        let placeholders = text_placeholders(1, chunk.len());
        let sql = format!(
            "SELECT DISTINCT tr.ci_job_id FROM test_runs tr \
               JOIN projects p ON p.id = tr.project_id \
               JOIN test_executions te ON te.test_run_id = tr.id \
              WHERE tr.ci_job_id IN ({placeholders}) \
                AND p.repo_id = ${} \
                AND te.status IN ('failed', 'error')",
            chunk.len() + 1
        );
        let mut values: Vec<Value> = chunk.iter().map(|id| Value::from(id.clone())).collect();
        values.push(Value::from(repo_id));
        for row in rows(db, sql, values).await? {
            found.insert(get::<String>(&row, "ci_job_id")?);
        }
    }
    for chunk in chunks(&wanted) {
        let placeholders = text_placeholders(1, chunk.len());
        let summary = summary_explained_sql("job_executions", "jobs", "pipelines");
        let sql = format!(
            "SELECT DISTINCT job_executions.ci_job_id FROM job_executions \
               JOIN jobs ON jobs.id = job_executions.job_id \
               JOIN pipelines ON pipelines.id = jobs.pipeline_id \
              WHERE job_executions.ci_job_id IN ({placeholders}) \
                AND job_executions.status = 'failed' \
                AND pipelines.repo_id = ${} \
                AND {summary}",
            chunk.len() + 1
        );
        let mut values: Vec<Value> = chunk.iter().map(|id| Value::from(id.clone())).collect();
        values.push(Value::from(repo_id));
        for row in rows(db, sql, values).await? {
            found.insert(get::<String>(&row, "ci_job_id")?);
        }
    }
    Ok(found)
}

/// The failed non-summary Jobs that explain each `(ci_run_id, ci_run_attempt)` pair.
///
/// The sibling half of the attribution rule, stated once here: the read API
/// labels a Summary Job's execution explained with the same clause and lists
/// these Jobs beside it. Pairs with an empty `ci_run_id` are dropped. Each pair
/// gets at most [`SIBLING_MAX`] Jobs, oldest first.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn summary_explaining_siblings(
    db: &impl ConnectionTrait,
    repo_id: i32,
    pairs: &[(String, i32)],
) -> Result<BTreeMap<(String, i32), Vec<ExplainingJobOut>>, DbError> {
    let wanted: Vec<(String, i32)> = pairs
        .iter()
        .filter(|(run_id, _)| !run_id.is_empty())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if wanted.is_empty() {
        return Ok(BTreeMap::new());
    }
    let placeholders = (0..wanted.len())
        .map(|index| value_row(index * 2 + 1, 2))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT job_executions.ci_run_id, job_executions.ci_run_attempt, \
                jobs.id, pipelines.name AS pipeline, jobs.name, job_executions.url \
           FROM job_executions \
           JOIN jobs ON jobs.id = job_executions.job_id \
           JOIN pipelines ON pipelines.id = jobs.pipeline_id \
          WHERE (job_executions.ci_run_id, job_executions.ci_run_attempt) IN ({placeholders}) \
            AND job_executions.status = 'failed' \
            AND pipelines.repo_id = ${} \
            AND NOT jobs.summary \
          ORDER BY job_executions.id",
        wanted.len() * 2 + 1
    );
    let mut values = Vec::with_capacity(wanted.len() * 2 + 1);
    for (run_id, attempt) in &wanted {
        values.push(Value::from(run_id.clone()));
        values.push(Value::from(*attempt));
    }
    values.push(Value::from(repo_id));
    let mut by_pair: BTreeMap<(String, i32), Vec<ExplainingJobOut>> = BTreeMap::new();
    for row in rows(db, sql, values).await? {
        let key = (
            get::<String>(&row, "ci_run_id")?,
            get::<i32>(&row, "ci_run_attempt")?,
        );
        let siblings = by_pair.entry(key).or_default();
        if siblings.len() < SIBLING_MAX {
            siblings.push(ExplainingJobOut {
                job_id: i64::from(get::<i32>(&row, "id")?),
                pipeline: get(&row, "pipeline")?,
                name: get(&row, "name")?,
                url: get(&row, "url")?,
            });
        }
    }
    Ok(by_pair)
}

/// Unexplained failures per Job over its newest `window` Job executions, any branch.
///
/// Jobs with none are left out of the result.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub async fn unexplained_failure_counts(
    db: &impl ConnectionTrait,
    job_ids: &[i32],
    window: usize,
) -> Result<BTreeMap<i32, i32>, DbError> {
    let mut counts: BTreeMap<i32, i32> = BTreeMap::new();
    for chunk in chunks(job_ids) {
        let sql = format!(
            "SELECT job_executions.job_id FROM job_executions \
               JOIN jobs ON jobs.id = job_executions.job_id \
               JOIN pipelines ON pipelines.id = jobs.pipeline_id \
              WHERE job_executions.id IN (SELECT w.id FROM ({}) AS w) \
                AND job_executions.status = 'failed' \
                AND NOT ({})",
            job_window_ids_overall_sql(),
            job_execution_explained_sql()
        );
        let values = vec![
            int_array(chunk),
            Value::from(i32::try_from(window).unwrap_or(i32::MAX)),
        ];
        for row in rows(db, sql, values).await? {
            *counts.entry(get::<i32>(&row, "job_id")?).or_default() += 1;
        }
    }
    Ok(counts)
}
