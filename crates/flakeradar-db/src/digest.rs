//! The weekly flaky-test digest, read from what is already stored.
//!
//! [`weekly_digest`] answers the four lists a Repo digest carries per Repo: what
//! became flaky this week, what recovered, which flakes cost the most CI runs,
//! and which quarantines lapse soon. Every one of them is a read over state the
//! processor already maintains — `flaky_since`, Score history, the impact rule
//! and the quarantine columns — so a digest of a week is never a second account
//! of it. The impact numbers come from the very fragment the board serves, so a
//! digest and the leaderboard cannot disagree about what a flake cost.

// Rust guideline compliant 2026-07-21

use chrono::{DateTime, Duration, Utc};
use flakeradar_core::digest::{DigestTest, RepoDigest};
use flakeradar_core::model::EntityKind;
use sea_orm::{ConnectionTrait, QueryResult, Value};

use crate::entity::entity;
use crate::error::DbError;
use crate::reads::{
    IMPACT_COLUMNS, ReadSettings, id, impact_lateral_sql, impact_since, optional_timestamp,
};
use crate::score_history;
use crate::sql::{Binds, get, rows};

/// How many days a digest spans: one week, backwards and forwards.
///
/// The message is weekly, so each of its three time-boxed lists — the flakes
/// that started, the Score history that says a Test was flaky, and the
/// quarantines about to lapse — spans the same week its header names. Changing
/// it reports Tests from outside the week the digest says it covers.
const WEEK_DAYS: i64 = 7;

/// The most Tests one Repo's `top_impact` list holds.
///
/// The list is a shortlist a reviewer acts on, not a table: four lists plus the
/// header is what one webhook message should hold, and the tail of a bad Repo is
/// on the leaderboard, which ranks by the same impact rule. Raising it buries the
/// other three lists in one Repo's block; lowering it hides a flake someone would
/// fix.
const TOP_IMPACT_MAX: i64 = 5;

/// The columns every digest list reports: a Test's own identity and its score.
const DIGEST_COLUMNS: &str =
    "SELECT t.id, t.classname, t.name, t.flakiness_score, p.name AS project";

/// The weekly flaky-test digest: one entry per Repo with something to report,
/// ordered by Repo name.
///
/// `now` ends the week every list is cut at. A Repo whose four lists are all
/// empty is absent, because a week in which nothing happened has nothing to send.
///
/// # Errors
///
/// Returns [`DbError::Database`] when a statement fails.
pub async fn weekly_digest(
    db: &impl ConnectionTrait,
    now: DateTime<Utc>,
    settings: &ReadSettings,
) -> Result<Vec<RepoDigest>, DbError> {
    let mut digests = Vec::new();
    for repo in repo_names(db).await? {
        let newly_flaky = list_newly_flaky(db, &repo, now).await?;
        let recovered = list_recovered(db, &repo, now, settings).await?;
        let top_impact = list_top_impact(db, &repo, settings).await?;
        let quarantine_expiring = list_quarantine_expiring(db, &repo, now).await?;
        if newly_flaky.is_empty()
            && recovered.is_empty()
            && top_impact.is_empty()
            && quarantine_expiring.is_empty()
        {
            continue;
        }
        digests.push(RepoDigest {
            repo,
            newly_flaky,
            recovered,
            top_impact,
            quarantine_expiring,
        });
    }
    Ok(digests)
}

/// Every Repo's `owner/name`, in the order the digest answers them.
async fn repo_names(db: &impl ConnectionTrait) -> Result<Vec<String>, DbError> {
    let stored = rows(db, "SELECT name FROM repos ORDER BY name", Vec::new()).await?;
    let mut names = Vec::with_capacity(stored.len());
    for row in &stored {
        names.push(get(row, "name")?);
    }
    Ok(names)
}

/// The Tests of `repo` whose flaky state began inside the week ending `now`.
async fn list_newly_flaky(
    db: &impl ConnectionTrait,
    repo: &str,
    now: DateTime<Utc>,
) -> Result<Vec<DigestTest>, DbError> {
    let mut binds = Binds::new();
    binds.eq("r.name", Value::from(repo.to_owned()));
    let started = binds.bind(Value::from(now - Duration::days(WEEK_DAYS)));
    binds.raw(format!("t.flaky_since > {started}"));
    let tests_from = entity(EntityKind::Test).from;
    let sql = format!(
        "{DIGEST_COLUMNS} {tests_from}{} ORDER BY t.flaky_since DESC, t.id",
        binds.suffix()
    );
    digest_rows(db, sql, binds.into_values()).await
}

/// The Tests of `repo` that are no longer flaky, but were as of the week's start:
/// their newest Score history at or before then already flaky.
async fn list_recovered(
    db: &impl ConnectionTrait,
    repo: &str,
    now: DateTime<Utc>,
    settings: &ReadSettings,
) -> Result<Vec<DigestTest>, DbError> {
    // The lateral's cut-off day is a literal, the same way the board inlines its
    // own, so the positional binds below it keep counting from `$1`.
    let week_ago = now.date_naive() - Duration::days(WEEK_DAYS);
    let history = score_history::newest_history_before(EntityKind::Test, "t").replace(
        "$1::DATE",
        &format!("DATE '{}'", week_ago.format("%Y-%m-%d")),
    );
    let mut binds = Binds::new();
    binds.eq("r.name", Value::from(repo.to_owned()));
    binds.raw("t.flaky_since IS NULL");
    let threshold = binds.bind(Value::from(settings.flake_threshold));
    binds.raw(format!("past.score >= {threshold}"));
    let tests_from = entity(EntityKind::Test).from;
    let sql = format!(
        "{DIGEST_COLUMNS} {tests_from} LEFT JOIN LATERAL ({history}) AS past ON true{} \
          ORDER BY t.id",
        binds.suffix()
    );
    digest_rows(db, sql, binds.into_values()).await
}

/// The Tests of `repo` whose flakes cost the most CI runs, worst first, capped at
/// [`TOP_IMPACT_MAX`].
///
/// The cost is the impact rule itself: the same lateral, the same columns and the
/// same window the board's `impact` sort uses.
async fn list_top_impact(
    db: &impl ConnectionTrait,
    repo: &str,
    settings: &ReadSettings,
) -> Result<Vec<DigestTest>, DbError> {
    let impact = impact_lateral_sql(EntityKind::Test, impact_since(settings));
    let mut binds = Binds::new();
    binds.eq("r.name", Value::from(repo.to_owned()));
    binds.raw("impact.impacted_runs > 0");
    let tests_from = entity(EntityKind::Test).from;
    let sql = format!(
        "{DIGEST_COLUMNS}{IMPACT_COLUMNS} {tests_from}{impact}{} ORDER BY \
           impact.impacted_runs DESC, t.flakiness_score DESC, t.id LIMIT {TOP_IMPACT_MAX}",
        binds.suffix()
    );
    let stored = rows(db, sql, binds.into_values()).await?;
    let mut tests = Vec::with_capacity(stored.len());
    for row in &stored {
        let mut test = digest_test(row)?;
        test.impacted_runs = Some(get(row, "impacted_runs")?);
        test.time_lost_seconds = Some(get(row, "time_lost_seconds")?);
        tests.push(test);
    }
    Ok(tests)
}

/// The Tests of `repo` under a quarantine that lapses inside the week after `now`.
async fn list_quarantine_expiring(
    db: &impl ConnectionTrait,
    repo: &str,
    now: DateTime<Utc>,
) -> Result<Vec<DigestTest>, DbError> {
    let mut binds = Binds::new();
    binds.eq("r.name", Value::from(repo.to_owned()));
    binds.raw("t.quarantined");
    let starts = binds.bind(Value::from(now));
    let ends = binds.bind(Value::from(now + Duration::days(WEEK_DAYS)));
    binds.raw(format!(
        "t.quarantine_expires_at > {starts} AND t.quarantine_expires_at <= {ends}"
    ));
    let tests_from = entity(EntityKind::Test).from;
    let sql = format!(
        "{DIGEST_COLUMNS}, t.quarantine_expires_at {tests_from}{} ORDER BY \
           t.quarantine_expires_at, t.id",
        binds.suffix()
    );
    let stored = rows(db, sql, binds.into_values()).await?;
    let mut tests = Vec::with_capacity(stored.len());
    for row in &stored {
        let mut test = digest_test(row)?;
        test.quarantine_expires_at = optional_timestamp(row, "quarantine_expires_at")?;
        tests.push(test);
    }
    Ok(tests)
}

/// Runs one digest statement and reads every row of it.
async fn digest_rows(
    db: &impl ConnectionTrait,
    sql: String,
    values: Vec<Value>,
) -> Result<Vec<DigestTest>, DbError> {
    let stored = rows(db, sql, values).await?;
    let mut tests = Vec::with_capacity(stored.len());
    for row in &stored {
        tests.push(digest_test(row)?);
    }
    Ok(tests)
}

/// One row's Test, as the digest reports it: the columns every list shares, with
/// the evidence a list selects of its own filled in by that list.
fn digest_test(row: &QueryResult) -> Result<DigestTest, DbError> {
    Ok(DigestTest {
        id: id(get(row, "id")?),
        project: get(row, "project")?,
        classname: get(row, "classname")?,
        name: get(row, "name")?,
        flakiness_score: get(row, "flakiness_score")?,
        impacted_runs: None,
        time_lost_seconds: None,
        quarantine_expires_at: None,
    })
}
