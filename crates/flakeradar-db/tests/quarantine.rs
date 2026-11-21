//! Quarantine: the decision a request body describes, and what the module's
//! write and lapse rule store for it against a real database.
//!
//! The clock is injected, so the expiry the write computes is a value the test
//! states rather than one it waits for. The last test pins what the HTTP suite
//! and the prune tests can only agree on: a quarantine whose expiry has passed
//! leaves exactly the state a [`QuarantineDecision::Release`] leaves.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a fixture or a read does not behave"
)]

use std::sync::Arc;

use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use flakeradar_core::dto::QuarantineIn;
use flakeradar_db::{
    entities::test_cases,
    quarantine::{QuarantineDecision, clear_expired_quarantines, decision, set_quarantine},
    reads::ReadSettings,
};
use flakeradar_test_support::{FixedClock, TestDb, make_project, make_test_case};
use sea_orm::EntityTrait as _;

/// The body of a quarantine request, with the reason and expiry the person gave.
fn body(reason: Option<&str>, expires_in_days: Option<i64>) -> QuarantineIn {
    QuarantineIn {
        quarantined: true,
        reason: reason.map(str::to_owned),
        expires_in_days,
    }
}

/// A fixed instant, so the expiry the write computes from the clock is a value
/// the test states.
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

#[test]
fn a_quarantined_body_becomes_its_quarantine() {
    assert_eq!(
        decision(body(Some("flaky on arm runners"), Some(3))),
        QuarantineDecision::Quarantine {
            reason: Some("flaky on arm runners".to_owned()),
            expires_in_days: Some(3),
        }
    );
}

#[test]
fn a_quarantined_body_without_a_reason_or_expiry_quarantines_with_none() {
    assert_eq!(
        decision(body(None, None)),
        QuarantineDecision::Quarantine {
            reason: None,
            expires_in_days: None,
        }
    );
}

#[test]
fn an_unquarantined_body_releases_even_with_a_reason_and_expiry() {
    let released = QuarantineIn {
        quarantined: false,
        ..body(Some("stale"), Some(9))
    };
    assert_eq!(decision(released), QuarantineDecision::Release);
}

#[tokio::test]
async fn the_write_computes_the_expiry_from_the_injected_clock() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().create(&db).await;
    let test = make_test_case(&project)
        .name("test_flaky")
        .create(&db)
        .await;

    let stored = set_quarantine(
        db.connection(),
        i64::from(test.id),
        decision(body(Some("flaky on arm runners"), Some(3))),
        &settings(),
    )
    .await
    .expect("the write")
    .expect("the Test exists");

    assert!(stored.quarantined);
    assert_eq!(
        stored.quarantine_reason.as_deref(),
        Some("flaky on arm runners")
    );
    assert_eq!(stored.quarantined_at, Some(at().with_timezone(&Utc)));
    assert_eq!(
        stored.quarantine_expires_at,
        Some((at() + TimeDelta::days(3)).with_timezone(&Utc))
    );

    let stored = test_cases::Entity::find_by_id(test.id)
        .one(db.connection())
        .await
        .expect("the Test reads")
        .expect("the Test exists");
    assert!(stored.quarantined);
    assert_eq!(stored.quarantined_at, Some(at()));
    assert_eq!(
        stored.quarantine_reason.as_deref(),
        Some("flaky on arm runners")
    );
    assert_eq!(
        stored.quarantine_expires_at,
        Some(at() + TimeDelta::days(3))
    );
}

#[tokio::test]
async fn an_expired_quarantine_leaves_exactly_what_a_release_leaves() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project().create(&db).await;
    let released = make_test_case(&project)
        .name("test_released")
        .create(&db)
        .await;
    let lapsed = make_test_case(&project)
        .name("test_lapsed")
        .create(&db)
        .await;
    let quarantine = decision(body(Some("flaky on arm runners"), Some(3)));

    set_quarantine(
        db.connection(),
        i64::from(released.id),
        quarantine.clone(),
        &settings(),
    )
    .await
    .expect("the quarantine")
    .expect("the Test exists");
    set_quarantine(
        db.connection(),
        i64::from(released.id),
        QuarantineDecision::Release,
        &settings(),
    )
    .await
    .expect("the release")
    .expect("the Test exists");
    set_quarantine(
        db.connection(),
        i64::from(lapsed.id),
        quarantine,
        &settings(),
    )
    .await
    .expect("the quarantine")
    .expect("the Test exists");

    let cleared = clear_expired_quarantines(db.connection(), at() + TimeDelta::days(10))
        .await
        .expect("the lapse rule runs");

    assert_eq!(cleared, vec![(lapsed.id, "test_lapsed".to_owned())]);
    let after_release = test_cases::Entity::find_by_id(released.id)
        .one(db.connection())
        .await
        .expect("the Test reads")
        .expect("the Test exists");
    let after_lapse = test_cases::Entity::find_by_id(lapsed.id)
        .one(db.connection())
        .await
        .expect("the Test reads")
        .expect("the Test exists");
    assert!(!after_release.quarantined);
    assert!(!after_lapse.quarantined);
    assert_eq!(after_lapse.quarantined_at, after_release.quarantined_at);
    assert_eq!(
        after_lapse.quarantine_reason,
        after_release.quarantine_reason
    );
    assert_eq!(
        after_lapse.quarantine_expires_at,
        after_release.quarantine_expires_at
    );
    assert_eq!(after_release.quarantined_at, None);
    assert_eq!(after_release.quarantine_reason, None);
    assert_eq!(after_release.quarantine_expires_at, None);
}
