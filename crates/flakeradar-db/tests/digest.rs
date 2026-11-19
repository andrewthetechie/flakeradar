//! The weekly digest read: the four lists `weekly_digest` answers per Repo, and
//! the Repos it answers nothing for.
//!
//! A stopped clock at the end of a week, so every window the digest cuts — the
//! week back, the week forward and the impact window behind it — is exact.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a fixture or the digest does not behave"
)]

use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use flakeradar_core::digest::{DigestTest, RepoDigest};
use flakeradar_db::digest::weekly_digest;
use flakeradar_db::entities::test_cases;
use flakeradar_db::reads::ReadSettings;
use flakeradar_test_support::{
    FixedClock, TestDb, make_execution, make_project, make_run, make_test_case, make_test_score,
};
use sea_orm::ConnectionTrait as _;

/// The end of the digested week, which is also where the clock stands.
fn at() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-11-09T09:00:00+00:00").expect("a fixed instant")
}

/// The digest settings: defaults, on a stopped clock.
fn settings() -> ReadSettings {
    ReadSettings {
        clock: Arc::new(FixedClock::new(at())),
        ..ReadSettings::default()
    }
}

/// An instant from its RFC 3339 text, for a fixture that names its own date.
fn instant(text: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(text).expect("a fixed instant")
}

/// A UTC day, for a Score history row that names its own date.
fn day(year: i32, month: u32, value: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, value).expect("a fixed day")
}

/// The names of one digest list, in the order the digest answers them.
fn names(tests: &[DigestTest]) -> Vec<&str> {
    tests
        .iter()
        .map(|test| test.name.as_str())
        .collect::<Vec<_>>()
}

/// The digest of the fixture Repo, which every case here asserts something about.
async fn digest_of(db: &TestDb, repo: &str) -> RepoDigest {
    let digests = weekly_digest(db.connection(), at().with_timezone(&Utc), &settings())
        .await
        .expect("the digest");
    digests
        .into_iter()
        .find(|digest| digest.repo == repo)
        .expect("the fixture Repo is in the digest")
}

/// The fixture Repo: one Test per kind of news, with the Runs, Executions and
/// Score history that makes each one true, plus the neighbouring Repo with nothing
/// to report. Answers the Test the impact figures come from.
async fn seed_week(db: &TestDb) -> test_cases::Model {
    let project = make_project().repo("acme/app").create(db).await;
    let login = make_test_case(&project)
        .name("test_login")
        .flaky_since(Some(instant("2026-11-05T09:00:00+00:00")))
        .create(db)
        .await;
    let ran = instant("2026-11-05T09:00:00+00:00");
    for (ci_run_id, duration) in [("100", 1.5), ("101", 2.5)] {
        let run = make_run(&project)
            .ci_run_id(ci_run_id)
            .created_at(ran)
            .create(db)
            .await;
        make_execution(&login, &run)
            .status("failed")
            .created_at(ran)
            .duration(duration)
            .create(db)
            .await;
    }
    let old = make_test_case(&project)
        .name("test_old")
        .flaky_since(Some(instant("2026-10-20T09:00:00+00:00")))
        .create(db)
        .await;
    // Flaky a month ago and flaky still: its flaky history is no recovery.
    make_test_score(&old, day(2026, 11, 2), 0.5)
        .create(db)
        .await;
    // Flaky since the very instant the digested week began, which is therefore not
    // inside the seven days the digest ends at `now` covers.
    make_test_case(&project)
        .name("test_edge")
        .flaky_since(Some(instant("2026-11-02T09:00:00+00:00")))
        .create(db)
        .await;
    // Not flaky now, and flaky on the day the digested week began: recovered.
    let fixed = make_test_case(&project).name("test_fixed").create(db).await;
    make_test_score(&fixed, day(2026, 11, 2), 0.5)
        .create(db)
        .await;
    // Flaky history only inside the digested week, and no flaky state now: this
    // week's news, not a recovery the week before already showed.
    let recent = make_test_case(&project)
        .name("test_recent")
        .create(db)
        .await;
    make_test_score(&recent, day(2026, 11, 8), 0.5)
        .create(db)
        .await;
    make_test_case(&project)
        .name("test_q1")
        .quarantined()
        .quarantine_expires_at(instant("2026-11-12T09:00:00+00:00"))
        .create(db)
        .await;
    make_test_case(&project)
        .name("test_q2")
        .quarantined()
        .quarantine_expires_at(instant("2026-11-20T09:00:00+00:00"))
        .create(db)
        .await;
    let other = make_project().repo("acme/library").create(db).await;
    make_test_case(&other).name("test_steady").create(db).await;
    login
}

#[tokio::test]
async fn weekly_digest_lists_each_kind_of_news_for_a_repo() {
    let db = TestDb::new().await.expect("a test database");
    let login = seed_week(&db).await;

    let digest = digest_of(&db, "acme/app").await;
    assert_eq!(names(&digest.newly_flaky), ["test_login"]);
    assert_eq!(names(&digest.recovered), ["test_fixed"]);
    assert_eq!(names(&digest.top_impact), ["test_login"]);
    assert_eq!(
        digest.top_impact[0].impacted_runs,
        Some(2),
        "the two CI runs the flakes cost"
    );
    assert_eq!(
        digest.top_impact[0].time_lost_seconds,
        Some(4.0),
        "1.5 and 2.5 seconds of Executions"
    );
    assert_eq!(names(&digest.quarantine_expiring), ["test_q1"]);
    assert_eq!(
        digest.quarantine_expiring[0].quarantine_expires_at,
        Some(instant("2026-11-12T09:00:00+00:00").with_timezone(&Utc)),
        "the list carries the day the quarantine lapses"
    );
    assert_eq!(
        digest.newly_flaky[0].project, "default",
        "a Test names its Project"
    );
    assert_eq!(
        digest.newly_flaky[0].id,
        i64::from(login.id),
        "a Test carries its own id"
    );

    let repos = weekly_digest(db.connection(), at().with_timezone(&Utc), &settings())
        .await
        .expect("the digest");
    assert_eq!(
        repos
            .iter()
            .map(|entry| entry.repo.as_str())
            .collect::<Vec<_>>(),
        ["acme/app"],
        "a Repo with only a stable Test has nothing to report"
    );
}

#[tokio::test]
async fn weekly_digest_answers_repos_in_name_order() {
    let db = TestDb::new().await.expect("a test database");
    // Created worst-first, so name order is not the order the rows arrive in.
    for (number, repo) in [("three", "acme/c"), ("one", "acme/a"), ("two", "acme/b")] {
        let project = make_project().repo(repo).create(&db).await;
        make_test_case(&project)
            .name(number)
            .flaky_since(Some(instant("2026-11-05T09:00:00+00:00")))
            .create(&db)
            .await;
    }
    let silent = make_project().repo("acme/d").create(&db).await;
    make_test_case(&silent)
        .name("test_steady")
        .create(&db)
        .await;

    let repos = weekly_digest(db.connection(), at().with_timezone(&Utc), &settings())
        .await
        .expect("the digest");
    assert_eq!(
        repos
            .iter()
            .map(|entry| entry.repo.as_str())
            .collect::<Vec<_>>(),
        ["acme/a", "acme/b", "acme/c"],
        "ordered by Repo name, and the all-stable Repo is left out"
    );
}

#[tokio::test]
async fn top_impact_leaves_out_a_test_that_cost_nothing() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    let ran = instant("2026-11-05T09:00:00+00:00");
    let run = make_run(&project)
        .ci_run_id("100")
        .created_at(ran)
        .create(&db)
        .await;
    let costly = make_test_case(&project)
        .name("test_costly")
        .create(&db)
        .await;
    make_execution(&costly, &run)
        .status("failed")
        .created_at(ran)
        .duration(1.0)
        .create(&db)
        .await;
    // A passing Execution costs nothing, and a failure in a Run flagged as suspect
    // is not the Test's cost: neither Test is in the list.
    let steady = make_test_case(&project)
        .name("test_steady")
        .create(&db)
        .await;
    make_execution(&steady, &run)
        .status("passed")
        .created_at(ran)
        .duration(9.0)
        .create(&db)
        .await;
    let suspect = make_run(&project)
        .ci_run_id("101")
        .infra_suspect(true)
        .created_at(ran)
        .create(&db)
        .await;
    let flaked = make_test_case(&project)
        .name("test_infra")
        .create(&db)
        .await;
    make_execution(&flaked, &suspect)
        .status("failed")
        .created_at(ran)
        .duration(9.0)
        .create(&db)
        .await;

    let digest = digest_of(&db, "acme/app").await;
    assert_eq!(
        names(&digest.top_impact),
        ["test_costly"],
        "only the Test with an impacted CI run is listed"
    );
    assert_eq!(
        digest.top_impact[0].time_lost_seconds,
        Some(1.0),
        "the suspect Run's seconds are not the Test's cost"
    );
}

#[tokio::test]
async fn quarantine_expiring_covers_the_coming_week_exactly() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    // The first quarantine lapsed at the instant the digest is built, the third is
    // one second past the week it warns about: neither is coming news.
    for (name, expires) in [
        ("test_at_now", "2026-11-09T09:00:00+00:00"),
        ("test_midweek", "2026-11-13T09:00:00+00:00"),
        ("test_week_end", "2026-11-16T09:00:00+00:00"),
        ("test_past_week", "2026-11-16T09:00:01+00:00"),
    ] {
        make_test_case(&project)
            .name(name)
            .quarantined()
            .quarantine_expires_at(instant(expires))
            .create(&db)
            .await;
    }
    // A Test whose quarantine flag is off but whose expiry is still stored: the
    // flag is what says a Test is quarantined, not the leftover date.
    make_test_case(&project)
        .name("test_lifted")
        .quarantined()
        .quarantine_expires_at(instant("2026-11-13T12:00:00+00:00"))
        .create(&db)
        .await;
    db.connection()
        .execute_unprepared("UPDATE test_cases SET quarantined = FALSE WHERE name = 'test_lifted'")
        .await
        .expect("the quarantine flag is cleared");

    let digest = digest_of(&db, "acme/app").await;
    assert_eq!(
        names(&digest.quarantine_expiring),
        ["test_midweek", "test_week_end"],
        "the week ahead, its last instant included, ordered by when each lapses"
    );
}

#[tokio::test]
async fn top_impact_keeps_the_five_worst_costliest_tests() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().repo("acme/app").create(&db).await;
    let ran = instant("2026-11-05T09:00:00+00:00");
    let mut runs = Vec::new();
    for number in 1..=7 {
        let run = make_run(&project)
            .ci_run_id(&format!("{number}"))
            .created_at(ran)
            .create(&db)
            .await;
        runs.push(run);
    }
    // Test `n` failed in runs `1` to `n`, so its impacted run count is `n`, while
    // its score falls as its cost rises: only the run count can order the list.
    let scores = [0.07, 0.06, 0.05, 0.04, 0.03, 0.02, 0.01];
    for (number, score) in (1..=7_usize).zip(scores) {
        let test = make_test_case(&project)
            .name(&format!("test_{number}"))
            .flakiness_score(score)
            .create(&db)
            .await;
        for run in runs.iter().take(number) {
            make_execution(&test, run)
                .status("failed")
                .created_at(ran)
                .duration(1.0)
                .create(&db)
                .await;
        }
    }
    let quiet = make_test_case(&project)
        .name("test_untouched")
        .create(&db)
        .await;
    let run = make_run(&project)
        .ci_run_id("8")
        .created_at(ran)
        .create(&db)
        .await;
    make_execution(&quiet, &run)
        .status("passed")
        .created_at(ran)
        .create(&db)
        .await;

    let digest = digest_of(&db, "acme/app").await;
    assert_eq!(
        names(&digest.top_impact),
        ["test_7", "test_6", "test_5", "test_4", "test_3"],
        "five Tests at most, the most impacted runs first"
    );
    assert_eq!(
        digest
            .top_impact
            .iter()
            .map(|test| test.impacted_runs)
            .collect::<Vec<_>>(),
        [Some(7), Some(6), Some(5), Some(4), Some(3)],
        "each item carries the count it was ranked by"
    );
}
