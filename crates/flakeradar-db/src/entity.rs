//! The Test/Job axis stated once: the scored table's name, its alias, its
//! `SELECT` list, its joins, the Score-history table and owner column, and the
//! per-entity words a read statement splices into its own SQL.
//!
//! [`EntityKind`] has two variants, so [`entity`] answers each with one
//! descriptor. A third entity means one new `entity()` arm plus its descriptor:
//! the statements that read it need no new arm, because they ask the descriptor
//! for every per-entity word they name.
//!
//! Every string is the text the statements serve, copied as it stood, so no
//! query changes shape by being built here. Two fields hold a join because one
//! string cannot be both spellings: `from` carries the `FROM` keyword, which the
//! leaderboard splices whole, while `tally_from` omits it, because the count
//! statements write `FROM` before interpolating it.
//!
//! The module also holds what the descriptor makes possible on its own: the
//! impact fragment every leaderboard statement joins, the one-row and locked
//! one-row statements, and the row mappers they answer. A read module imports
//! those instead of spelling the statement again.

// Rust guideline compliant 2026-07-21

use chrono::{DateTime, Duration, Utc};
use flakeradar_core::{
    classify::FailureCategory, dto::JobOut, dto::TestOut, helpers::is_broken, helpers::issue_url,
    helpers::job_issue_url, helpers::tier_for, model::EntityKind, model::Trend,
};
use sea_orm::{ConnectionTrait, QueryResult, Value};

use crate::attribution::job_execution_explained_sql;
use crate::error::DbError;
use crate::reads::{IMPACT_DAYS, ReadSettings};
use crate::sql::{
    execution_status, fingerprint, get, id, one, optional_timestamp, score, timestamp,
};

/// What the read statements need to know about one scored entity.
pub(crate) struct Entity {
    /// The entity the descriptor is about.
    pub(crate) kind: EntityKind,
    /// The scored table's name, as the migration creates it.
    pub(crate) table: &'static str,
    /// The scored table's alias in the statements that read it.
    pub(crate) alias: &'static str,
    /// The row shape: the entity's columns, then the names of its parents.
    pub(crate) select: &'static str,
    /// The scored table joined to its parents, `FROM` keyword included.
    pub(crate) from: &'static str,
    /// The same join without the `FROM` keyword, for the headline counts.
    pub(crate) tally_from: &'static str,
    /// The Issue-link columns `locked_entity_row` adds, with their leading
    /// comma and the space that precedes the `FROM` that follows them.
    pub(crate) locked_select: &'static str,
    /// The Score history joined to the scored table and its parents.
    pub(crate) history_from: &'static str,
    /// The Score-history table that holds the entity's own history.
    pub(crate) history_table: &'static str,
    /// The column naming the owner: the Test's or the Job's id.
    pub(crate) owner_column: &'static str,
    /// The entity's GitHub issue column.
    pub(crate) issue_column: &'static str,
    /// The search condition, given the pattern bind's placeholder.
    pub(crate) search: fn(&str) -> String,
    /// The column an exact-name lookup narrows by: a Test's classname, a Job's
    /// Pipeline name.
    pub(crate) lookup_column: &'static str,
    /// How the entity's queries are named in the error a statement that
    /// answered nothing gives: nothing for a Test, `job ` for a Job.
    pub(crate) prefix: &'static str,
    /// Whether only a GitHub Pipeline's entity can hold an Issue link (ADR 0008).
    pub(crate) provider_only: bool,
}

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

/// The Tests leaderboard's and headline counts' descriptor.
const TEST: Entity = Entity {
    kind: EntityKind::Test,
    table: "test_cases",
    alias: "t",
    select: "SELECT t.*, p.name AS project_name, r.name AS repo_name",
    from: "FROM test_cases t \
     JOIN projects p ON p.id = t.project_id \
     JOIN repos r ON r.id = p.repo_id",
    tally_from: "test_cases t \
        JOIN projects p ON p.id = t.project_id \
        JOIN repos r ON r.id = p.repo_id",
    locked_select: ", t.github_issue_checked_at AS checked_at, \
        t.github_issue_closed AS closed, \
        t.github_issue_fixed_noted_at AS fixed_noted_at ",
    history_from: "test_score_history h \
        JOIN test_cases t ON t.id = h.test_case_id \
        JOIN projects p ON p.id = t.project_id \
        JOIN repos r ON r.id = p.repo_id",
    history_table: "test_score_history",
    owner_column: "test_case_id",
    issue_column: "t.github_issue_number",
    search: test_search,
    lookup_column: "t.classname",
    prefix: "",
    provider_only: false,
};

/// The Jobs leaderboard's and headline counts' descriptor.
const JOB: Entity = Entity {
    kind: EntityKind::Job,
    table: "jobs",
    alias: "j",
    select: "SELECT j.*, p.name AS pipeline_name, p.provider AS provider, \
     p.repo_id AS repo_id, r.name AS repo_name",
    from: "FROM jobs j \
     JOIN pipelines p ON p.id = j.pipeline_id \
     JOIN repos r ON r.id = p.repo_id",
    tally_from: "jobs j \
        JOIN pipelines p ON p.id = j.pipeline_id \
        JOIN repos r ON r.id = p.repo_id",
    locked_select: ", j.github_issue_checked_at AS checked_at, \
        j.github_issue_closed AS closed, \
        j.github_issue_fixed_noted_at AS fixed_noted_at ",
    history_from: "job_score_history h \
        JOIN jobs j ON j.id = h.job_id \
        JOIN pipelines p ON p.id = j.pipeline_id \
        JOIN repos r ON r.id = p.repo_id",
    history_table: "job_score_history",
    owner_column: "job_id",
    issue_column: "j.github_issue_number",
    search: job_search,
    lookup_column: "p.name",
    prefix: "job ",
    provider_only: true,
};

/// The descriptor `kind` is scored by.
#[must_use]
pub(crate) const fn entity(kind: EntityKind) -> &'static Entity {
    match kind {
        EntityKind::Test => &TEST,
        EntityKind::Job => &JOB,
    }
}

/// The instant the impact window starts: [`IMPACT_DAYS`] before the settings' clock.
pub(crate) fn impact_since(settings: &ReadSettings) -> DateTime<Utc> {
    settings.clock.now().with_timezone(&Utc) - Duration::days(IMPACT_DAYS)
}

/// The three impact columns, to append to the `SELECT` list of a query that joins
/// [`impact_lateral_sql`].
pub(crate) const IMPACT_COLUMNS: &str =
    ", impact.impacted_runs, impact.impacted_branches, impact.time_lost_seconds";

/// The SQL the impact fields add: one aggregate over the entity's failures in the
/// window, joined per row so a page reads it in the statement that fetches it.
///
/// The Test arm excludes the Runs infrastructure marking flagged as suspect: their
/// failures are not the Test's cost. The Job arm reuses the attribution rule the
/// Job score uses, so an explained failure costs the Job nothing.
///
/// The cutoff is inlined as a literal for the reason `newest_history_join`
/// inlines its own: the board's binds are positional and shared by the count and
/// the page statement, so an extra bind would shift the filters' placeholders.
pub(crate) fn impact_lateral_sql(kind: EntityKind, cutoff: DateTime<Utc>) -> String {
    let since = format!("timestamptz '{}'", cutoff.to_rfc3339());
    let descriptor = entity(kind);
    let (alias, table) = (descriptor.alias, descriptor.table);
    let aggregate = match kind {
        EntityKind::Test => format!(
            "SELECT count(DISTINCT r2.ci_run_id) FILTER (WHERE r2.ci_run_id <> '') \
                  AS impacted_runs, \
                  count(DISTINCT r2.branch) FILTER (WHERE r.default_branch IS NOT NULL \
                      AND r2.branch <> r.default_branch) AS impacted_branches, \
                  coalesce(sum(e2.duration), 0)::double precision AS time_lost_seconds \
             FROM test_executions e2 JOIN test_runs r2 ON r2.id = e2.test_run_id \
            WHERE e2.test_case_id = {alias}.id \
              AND e2.status IN ('failed', 'error') \
              AND NOT r2.infra_suspect \
              AND e2.created_at > {since}"
        ),
        // The Job tables appear under their bare names: the outer query aliases
        // them, and [`job_execution_explained_sql`] states its rule for those.
        EntityKind::Job => format!(
            "SELECT count(DISTINCT job_executions.ci_run_id) \
                  FILTER (WHERE job_executions.ci_run_id <> '') AS impacted_runs, \
                  count(DISTINCT job_executions.branch) \
                      FILTER (WHERE r.default_branch IS NOT NULL \
                          AND job_executions.branch <> r.default_branch) \
                      AS impacted_branches, \
                  coalesce(sum(EXTRACT(EPOCH FROM \
                      (job_executions.completed_at - job_executions.started_at))), 0) \
                      ::double precision AS time_lost_seconds \
             FROM job_executions \
             JOIN {table} ON {table}.id = job_executions.job_id \
             JOIN pipelines ON pipelines.id = {table}.pipeline_id \
            WHERE job_executions.job_id = {alias}.id \
              AND job_executions.status = 'failed' \
              AND NOT ({}) \
              AND job_executions.created_at > {since}",
            job_execution_explained_sql()
        ),
    };
    format!(" LEFT JOIN LATERAL ({aggregate}) AS impact ON true")
}

/// Whether a Test first seen at `first_seen_at` is within the new-test window.
pub(crate) fn is_new(first_seen_at: DateTime<Utc>, settings: &ReadSettings) -> bool {
    first_seen_at
        > settings.clock.now().with_timezone(&Utc) - Duration::days(settings.new_test_days)
}

/// The Failure category one stored spelling names.
///
/// `failure_category` is free text in the baseline schema, so a row can hold a
/// word the five categories do not name. It reads as no category, which is the
/// same rule the summary's per-category counts apply.
pub(crate) fn stored_category(text: Option<&str>) -> Option<FailureCategory> {
    text.and_then(FailureCategory::from_wire)
}

/// Reads one Test row into its response shape.
pub(crate) fn test_out(
    row: &QueryResult,
    settings: &ReadSettings,
    trend: Option<Trend>,
) -> Result<TestOut, DbError> {
    let score = score(get(row, "flakiness_score")?, "flakiness_score")?;
    // The tier reads the row's stored flaky state, so a score that drifted back
    // under the Flake threshold still lists as flaky until the state clears.
    let flaky = optional_timestamp(row, "flaky_since")?.is_some();
    let repo: String = get(row, "repo_name")?;
    let issue_number: Option<i32> = get(row, "github_issue_number")?;
    let issue_number = issue_number.map(id);
    let fail_streak = i64::from(get::<i32>(row, "fail_streak")?);
    let source_url = issue_url(&repo, issue_number);
    let first_seen_at = timestamp(row, "first_seen_at")?;
    Ok(TestOut {
        id: id(get(row, "id")?),
        repo,
        project: get(row, "project_name")?,
        fingerprint: fingerprint(&get::<String>(row, "fingerprint")?, "fingerprint")?,
        suite: get(row, "suite")?,
        classname: get(row, "classname")?,
        name: get(row, "name")?,
        file: get(row, "file")?,
        line: get::<Option<i32>>(row, "line")?.map(id),
        flakiness_score: score,
        tier: tier_for(score, flaky),
        confirmed_flake_count: i64::from(get::<i32>(row, "confirmed_flake_count")?),
        failure_category: stored_category(
            get::<Option<String>>(row, "failure_category")?.as_deref(),
        ),
        clean_streak: i64::from(get::<i32>(row, "clean_streak")?),
        fail_streak,
        broken: is_broken(fail_streak, settings.broken_streak),
        trend,
        last_status: execution_status(&get::<String>(row, "last_status")?)?,
        last_seen_at: timestamp(row, "last_seen_at")?,
        quarantined: get(row, "quarantined")?,
        quarantined_at: optional_timestamp(row, "quarantined_at")?,
        quarantine_reason: get(row, "quarantine_reason")?,
        quarantine_expires_at: optional_timestamp(row, "quarantine_expires_at")?,
        acknowledged_until: optional_timestamp(row, "acknowledged_until")?,
        acknowledge_reason: get(row, "acknowledge_reason")?,
        github_issue_number: issue_number,
        github_issue_url: source_url,
        github_issue_closed: get(row, "github_issue_closed")?,
        first_seen_at,
        new: is_new(first_seen_at, settings),
        impacted_runs: get(row, "impacted_runs")?,
        impacted_branches: get(row, "impacted_branches")?,
        time_lost_seconds: get(row, "time_lost_seconds")?,
    })
}

/// Reads one Job row into its response shape.
pub(crate) fn job_out(row: &QueryResult, trend: Option<Trend>) -> Result<JobOut, DbError> {
    let score = score(get(row, "flakiness_score")?, "flakiness_score")?;
    // The tier reads the row's stored flaky state, so a score that drifted back
    // under the Flake threshold still lists as flaky until the state clears.
    let flaky = optional_timestamp(row, "flaky_since")?.is_some();
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
        tier: tier_for(score, flaky),
        confirmed_flake_count: i64::from(get::<i32>(row, "confirmed_flake_count")?),
        clean_streak: i64::from(get::<i32>(row, "clean_streak")?),
        trend,
        last_status: get(row, "last_status")?,
        last_seen_at: timestamp(row, "last_seen_at")?,
        summary: get(row, "summary")?,
        acknowledged_until: optional_timestamp(row, "acknowledged_until")?,
        acknowledge_reason: get(row, "acknowledge_reason")?,
        github_issue_number: issue_number,
        github_issue_url,
        github_issue_closed: get(row, "github_issue_closed")?,
        impacted_runs: get(row, "impacted_runs")?,
        impacted_branches: get(row, "impacted_branches")?,
        time_lost_seconds: get(row, "time_lost_seconds")?,
    })
}

/// One entity's row in its own response shape.
pub(crate) enum EntityOut {
    /// A Test, in the Tests leaderboard shape.
    Test(TestOut),
    /// A Job, in the Jobs leaderboard shape.
    Job(JobOut),
}

/// Reads one Test or Job row in the leaderboard shape, impact columns included.
///
/// This is the one home of the one-row statement: the row shape, the impact
/// pieces and the id filter live here, so a caller states only the kind and the
/// id. [`locked_entity_row`] is its twin for a write that locks the row first.
/// It answers the raw row — [`entity_out`] maps it — because a write patches
/// the columns it stored onto the mapped shape before it answers.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub(crate) async fn entity_row(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    entity_id: i64,
    settings: &ReadSettings,
) -> Result<Option<QueryResult>, DbError> {
    let since = impact_since(settings);
    let descriptor = entity(kind);
    let (select, from, alias) = (descriptor.select, descriptor.from, descriptor.alias);
    let sql = format!(
        "{select}{IMPACT_COLUMNS} {from}{lateral} WHERE {alias}.id = $1",
        lateral = impact_lateral_sql(kind, since)
    );
    one(db, sql, vec![Value::from(entity_id)]).await
}

/// The locked twin of [`entity_row`]: the same row, `FOR UPDATE`, with the three
/// Issue-link columns the link engines read.
///
/// Those columns are aliased `checked_at`, `closed` and `fixed_noted_at`, and the
/// alias text is the descriptor's `locked_select`: the link engines read that
/// state by name from the row this answer maps, so it is part of this row and
/// never a second read.
///
/// It carries the impact columns because the row feeds the leaderboard's own
/// mapper: without the lateral the answer names unknown columns. Locking stays
/// legal because the lateral is on the target's non-null side.
///
/// # Errors
///
/// Returns [`DbError::Database`] when the statement fails.
pub(crate) async fn locked_entity_row(
    db: &impl ConnectionTrait,
    kind: EntityKind,
    entity_id: i64,
    settings: &ReadSettings,
) -> Result<Option<QueryResult>, DbError> {
    let since = impact_since(settings);
    let descriptor = entity(kind);
    let (select, from, alias, locked_select) = (
        descriptor.select,
        descriptor.from,
        descriptor.alias,
        descriptor.locked_select,
    );
    let sql = format!(
        "{select}{IMPACT_COLUMNS}{locked_select}{from}{lateral} WHERE {alias}.id = $1 \
         FOR UPDATE OF {alias}",
        lateral = impact_lateral_sql(kind, since)
    );
    one(db, sql, vec![Value::from(entity_id)]).await
}

/// Maps one row of [`entity_row`] or [`locked_entity_row`] into the response
/// shape of `kind`.
///
/// # Errors
///
/// Returns [`DbError::UnusableStoredValue`] or [`DbError::UnknownExecutionStatus`]
/// for a stored value its mapper rejects.
pub(crate) fn entity_out(
    row: &QueryResult,
    kind: EntityKind,
    settings: &ReadSettings,
) -> Result<EntityOut, DbError> {
    match kind {
        EntityKind::Test => Ok(EntityOut::Test(test_out(row, settings, None)?)),
        EntityKind::Job => Ok(EntityOut::Job(job_out(row, None)?)),
    }
}

#[cfg(test)]
mod tests {
    //! The descriptor's own spellings, without a database.

    use super::{EntityKind, entity};

    /// The two `FROM` fields are one join spelled two ways, and the join text
    /// is the single-spaced sentence the statements serve.
    #[test]
    fn the_join_fields_are_one_spelling_with_the_keyword_and_one_without() {
        let tests = entity(EntityKind::Test);
        assert_eq!(
            tests.from,
            "FROM test_cases t JOIN projects p ON p.id = t.project_id \
             JOIN repos r ON r.id = p.repo_id"
        );
        assert!(tests.from.starts_with("FROM test_cases t "));
        assert_eq!(
            tests.tally_from,
            "test_cases t JOIN projects p ON p.id = t.project_id \
             JOIN repos r ON r.id = p.repo_id"
        );
        assert!(tests.tally_from.starts_with("test_cases t"));
        assert!(!tests.tally_from.contains("FROM"));
    }

    /// The Job twin names the Job table, its Pipeline and its Repo.
    #[test]
    fn the_job_arm_names_the_job_table_and_its_parents() {
        let jobs = entity(EntityKind::Job);
        assert_eq!(jobs.table, "jobs");
        assert_eq!(
            jobs.from,
            "FROM jobs j JOIN pipelines p ON p.id = j.pipeline_id \
             JOIN repos r ON r.id = p.repo_id"
        );
        assert_eq!(
            jobs.tally_from,
            "jobs j JOIN pipelines p ON p.id = j.pipeline_id \
             JOIN repos r ON r.id = p.repo_id"
        );
    }

    /// The two descriptors name the scored tables exactly as the migration
    /// creates them, which is what the impact lateral's Job arm interpolates.
    #[test]
    fn each_descriptor_names_its_own_scored_table() {
        assert_eq!(entity(EntityKind::Test).table, "test_cases");
        assert_eq!(entity(EntityKind::Job).table, "jobs");
    }

    /// The names the acknowledge, link and Score-history modules read: the
    /// history tables and owner columns exactly as the migrations create them,
    /// the narrowing column the exact-match lookups bind, and the one per-target
    /// rule the link engine answers from the descriptor.
    #[test]
    fn each_descriptor_names_its_history_table_owner_and_provider_rule() {
        assert_eq!(entity(EntityKind::Test).history_table, "test_score_history");
        assert_eq!(entity(EntityKind::Job).history_table, "job_score_history");
        assert_eq!(entity(EntityKind::Test).owner_column, "test_case_id");
        assert_eq!(entity(EntityKind::Job).owner_column, "job_id");
        assert_eq!(entity(EntityKind::Test).lookup_column, "t.classname");
        assert_eq!(entity(EntityKind::Job).lookup_column, "p.name");
        assert!(entity(EntityKind::Job).provider_only);
        assert!(!entity(EntityKind::Test).provider_only);
    }
}
