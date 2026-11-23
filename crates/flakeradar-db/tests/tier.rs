//! The SQL shapes of the tier classification and the broken filter, read
//! through the readers that splice them, against the Rust rules they mirror.
//!
//! The tier gauges answer the tier of every stored row, so they are where a SQL
//! restatement of `tier_for` or `is_broken` would first disagree with the
//! in-process rule.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a fixture or a read does not behave"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset};
use flakeradar_core::{
    helpers::{is_broken, tier_for},
    model::Tier,
    types::Score,
};
use flakeradar_db::{
    job_tier_counts, reads::ListOptions, reads::ReadSettings, reads::Scope, reads::TestListFilters,
    reads::list_tests, test_tier_counts,
};
use flakeradar_test_support::{
    FIXTURE_INSTANT, FixedClock, TestDb, make_job, make_pipeline, make_project, make_test_case,
};

/// A fixed instant, so "today" in the reads is deterministic.
fn at() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-10-20T09:00:00+00:00").expect("a fixed instant")
}

/// The read settings a test uses: defaults, on a stopped clock.
fn settings() -> ReadSettings {
    ReadSettings {
        clock: Arc::new(FixedClock::new(at())),
        ..ReadSettings::default()
    }
}

/// A page request with every default and the given options.
fn options(change: impl FnOnce(&mut ListOptions)) -> ListOptions {
    let mut options = ListOptions {
        page: 1,
        page_size: 50,
        ..ListOptions::default()
    };
    change(&mut options);
    options
}

/// The stored flaky state, at the fixture instant the builders default to.
fn stored_flaky_state() -> DateTime<FixedOffset> {
    FIXTURE_INSTANT.fixed_offset()
}

/// The counts the four rows answer for: one stable, one suspect, two flaky.
fn expected_counts() -> BTreeMap<String, i64> {
    BTreeMap::from([
        ("flaky".to_owned(), 2),
        ("stable".to_owned(), 1),
        ("suspect".to_owned(), 1),
    ])
}

/// The four rows that separate the tiers: `(0.0, None)` is stable, `(0.25,
/// None)` suspect, `(0.30, Some)` flaky with the state the fixture derives from
/// the score, and `(0.0, Some)` flaky with the state set on its own. The `0.25`
/// row is given `None` explicitly, which is also what its derivation answers at
/// the Flake threshold of `0.30`.
#[tokio::test]
async fn tier_sql_shapes_agree_with_tier_for() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().create(&db).await;
    make_test_case(&project)
        .name("stable")
        .flakiness_score(0.0)
        .create(&db)
        .await;
    make_test_case(&project)
        .name("suspect")
        .flakiness_score(0.25)
        .flaky_since(None)
        .create(&db)
        .await;
    make_test_case(&project)
        .name("flaky")
        .flakiness_score(0.30)
        .create(&db)
        .await;
    make_test_case(&project)
        .name("flaky-at-zero")
        .flakiness_score(0.0)
        .flaky_since(Some(stored_flaky_state()))
        .create(&db)
        .await;

    assert_eq!(
        test_tier_counts(db.connection()).await.expect("the counts"),
        expected_counts(),
        "the gauge groups the four rows as `tier_for` classifies them"
    );

    // The same four combinations as Jobs of one Pipeline, read through the
    // Job gauge: requirement 6 names both readers.
    let pipeline = make_pipeline().repo("acme/app").create(&db).await;
    make_job(&pipeline)
        .name("stable")
        .flakiness_score(0.0)
        .create(&db)
        .await;
    make_job(&pipeline)
        .name("suspect")
        .flakiness_score(0.25)
        .flaky_since(None)
        .create(&db)
        .await;
    make_job(&pipeline)
        .name("flaky")
        .flakiness_score(0.30)
        .create(&db)
        .await;
    make_job(&pipeline)
        .name("flaky-at-zero")
        .flakiness_score(0.0)
        .flaky_since(Some(stored_flaky_state()))
        .create(&db)
        .await;

    assert_eq!(
        job_tier_counts(db.connection()).await.expect("the counts"),
        expected_counts(),
        "a Job is tiered by the same SQL shape a Test is"
    );

    // The Rust rule those shapes mirror, at the same two boundaries: a score
    // above 0 with no stored state is Suspect, and a stored state at a score
    // of 0 is Flaky.
    assert_eq!(
        tier_for(Score::new(0.30).expect("a score"), false),
        Tier::Suspect
    );
    assert_eq!(
        tier_for(Score::new(0.0).expect("a score"), true),
        Tier::Flaky
    );
}

/// A `broken_streak` of 0 disables broken detection, exactly as `is_broken`
/// answers false for it, so the broken filter lists no row.
#[tokio::test]
async fn a_zero_broken_streak_lists_no_broken_row() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().create(&db).await;
    make_test_case(&project)
        .name("broken")
        .flakiness_score(0.5)
        .fail_streak(9)
        .create(&db)
        .await;
    let filters = TestListFilters {
        broken: true,
        ..TestListFilters::default()
    };
    let disabled = ReadSettings {
        broken_streak: 0,
        ..settings()
    };

    let page = list_tests(
        db.connection(),
        &Scope::default(),
        &filters,
        &options(|_| {}),
        &disabled,
    )
    .await
    .expect("the page");
    assert_eq!(page.total, 0, "detection disabled breaks no row");

    // The same call at the default `broken_streak` lists the row, so the empty
    // answer above is the disabled filter's and not the board hiding it.
    let listed = list_tests(
        db.connection(),
        &Scope::default(),
        &filters,
        &options(|_| {}),
        &settings(),
    )
    .await
    .expect("the page");
    assert_eq!(listed.total, 1);
    assert_eq!(listed.items[0].name, "broken");

    assert!(!is_broken(9, 0));
}
