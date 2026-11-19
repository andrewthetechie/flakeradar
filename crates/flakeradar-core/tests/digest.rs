//! The weekly digest's webhook body, in each format a Webhook URL chooses.
//!
//! The rendered text and the JSON keys are what a Slack channel and a JSON
//! consumer read, so each test names the spelling it pins rather than reusing a
//! helper that builds the same string.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the digest render is wrong"
)]

use std::slice;

use chrono::{NaiveDate, TimeZone as _, Utc};
use flakeradar_core::digest::{DigestTest, RepoDigest, render_digest};
use flakeradar_core::notify::WebhookFormat;
use serde_json::{Value, json};

/// The week ending the digest is rendered for.
fn week_ending() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 9).expect("a real day")
}

/// One Test for the digest, with only the score and the optional impact fields naming
/// what differs, because those are what the render reads.
fn digest_test(
    name: &str,
    flakiness_score: f64,
    impacted_runs: Option<i64>,
    time_lost_seconds: Option<f64>,
) -> DigestTest {
    DigestTest {
        id: 1,
        project: "pytest".to_owned(),
        classname: "tests::test_login".to_owned(),
        name: name.to_owned(),
        flakiness_score,
        impacted_runs,
        time_lost_seconds,
        quarantine_expires_at: None,
    }
}

/// The digest the acceptance criteria describe: one Repo, one Test that became
/// flaky this week, and the same Test first in the impact ranking.
fn fixture() -> RepoDigest {
    RepoDigest {
        repo: "acme/app".to_owned(),
        newly_flaky: vec![digest_test("test_login", 0.42, None, None)],
        recovered: Vec::new(),
        top_impact: vec![digest_test("test_login", 0.42, Some(2), Some(4.0))],
        quarantine_expiring: Vec::new(),
    }
}

/// The `Json` body for `digests`, which must be an array, as its first event.
fn first_event(digests: &[RepoDigest]) -> Value {
    let body = render_digest(digests, WebhookFormat::Json, week_ending());
    let events = body.as_array().expect("Json answers a list");
    events
        .first()
        .cloned()
        .expect("the digest has one event per Repo")
}

#[test]
fn a_slack_digest_is_one_text_of_the_pinned_lines() {
    assert_eq!(
        render_digest(&[fixture()], WebhookFormat::Slack, week_ending()),
        json!({"text": "FlakeRadar weekly digest — acme/app, week ending 2026-11-09\nNewly flaky: test_login\nTop impact: test_login (2 CI runs, 4.0s lost)"})
    );
}

#[test]
fn a_json_digest_is_one_event_per_repo() {
    let event = first_event(&[fixture()]);
    assert_eq!(event["event"], "digest.weekly");
    assert_eq!(event["repo"], "acme/app");
    assert_eq!(event["week_ending"], "2026-11-09");
    for key in [
        "newly_flaky",
        "recovered",
        "top_impact",
        "quarantine_expiring",
    ] {
        assert!(
            event[key].is_array(),
            "every list is in the event even when empty: {key}"
        );
    }
    assert_eq!(event["recovered"], json!([]));
}

#[test]
fn a_json_test_carries_the_identity_of_the_test() {
    let event = first_event(&[fixture()]);
    let item = &event["top_impact"][0];
    assert_eq!(item["id"], 1);
    assert_eq!(item["project"], "pytest");
    assert_eq!(item["classname"], "tests::test_login");
    assert_eq!(item["name"], "test_login");
    assert_eq!(item["flakiness_score"], 0.42);
    assert_eq!(item["impacted_runs"], 2);
    assert_eq!(item["time_lost_seconds"], 4.0);
}

#[test]
fn an_impact_the_test_does_not_have_is_left_out_of_the_json() {
    let event = first_event(&[fixture()]);
    let newly_flaky = event["newly_flaky"].as_array().expect("a list");
    let item = newly_flaky.first().expect("the one Test");
    for key in [
        "impacted_runs",
        "time_lost_seconds",
        "quarantine_expires_at",
    ] {
        assert!(
            item.get(key).is_none(),
            "a None field is absent rather than null: {key}"
        );
    }
    assert_eq!(event["top_impact"][0]["impacted_runs"], 2);
}

#[test]
fn an_expiring_quarantine_shows_its_day_in_both_formats() {
    let expires = Utc
        .with_ymd_and_hms(2026, 11, 12, 6, 30, 0)
        .single()
        .expect("a real instant");
    let digest = RepoDigest {
        quarantine_expiring: vec![DigestTest {
            quarantine_expires_at: Some(expires),
            ..digest_test("test_q1", 0.61, None, None)
        }],
        ..fixture()
    };
    assert_eq!(
        render_digest(
            slice::from_ref(&digest),
            WebhookFormat::Slack,
            week_ending()
        )["text"],
        "FlakeRadar weekly digest — acme/app, week ending 2026-11-09\nNewly flaky: test_login\nTop impact: test_login (2 CI runs, 4.0s lost)\nQuarantine expiring: test_q1 (2026-11-12)"
    );
    assert_eq!(
        first_event(&[digest])["quarantine_expiring"][0]["quarantine_expires_at"],
        "2026-11-12T06:30:00+00:00"
    );
}

#[test]
fn one_run_loses_its_plural_in_the_impact_line() {
    let digest = RepoDigest {
        top_impact: vec![digest_test("test_login", 0.42, Some(1), Some(1.0))],
        ..fixture()
    };
    assert_eq!(
        render_digest(&[digest], WebhookFormat::Slack, week_ending())["text"],
        "FlakeRadar weekly digest — acme/app, week ending 2026-11-09\nNewly flaky: test_login\nTop impact: test_login (1 CI run, 1.0s lost)"
    );
}

#[test]
fn a_list_item_missing_its_evidence_is_still_named() {
    // A digest built without the evidence a list usually carries — here a
    // `top_impact` Test with no time lost and a `quarantine_expiring` Test with no
    // day — still names the Test, because the name is what a reader acts on.
    let digest = RepoDigest {
        repo: "acme/app".to_owned(),
        newly_flaky: Vec::new(),
        recovered: Vec::new(),
        top_impact: vec![digest_test("test_login", 0.42, Some(2), None)],
        quarantine_expiring: vec![digest_test("test_q1", 0.61, None, None)],
    };
    assert_eq!(
        render_digest(
            slice::from_ref(&digest),
            WebhookFormat::Slack,
            week_ending()
        )["text"],
        "FlakeRadar weekly digest — acme/app, week ending 2026-11-09\nTop impact: test_login\nQuarantine expiring: test_q1"
    );
    let event = first_event(slice::from_ref(&digest));
    assert_eq!(event["top_impact"][0]["name"], "test_login");
    assert!(
        event["top_impact"][0].get("time_lost_seconds").is_none(),
        "an absent field stays out of the JSON"
    );
    assert_eq!(event["quarantine_expiring"][0]["name"], "test_q1");
}

#[test]
fn each_repo_is_a_block_and_an_empty_list_emits_no_line() {
    let quiet = RepoDigest {
        repo: "acme/other".to_owned(),
        newly_flaky: Vec::new(),
        recovered: vec![digest_test("test_fixed", 0.05, None, None)],
        top_impact: Vec::new(),
        quarantine_expiring: Vec::new(),
    };
    assert_eq!(
        render_digest(&[fixture(), quiet], WebhookFormat::Slack, week_ending())["text"],
        "FlakeRadar weekly digest — acme/app, week ending 2026-11-09\nNewly flaky: test_login\nTop impact: test_login (2 CI runs, 4.0s lost)\n\nFlakeRadar weekly digest — acme/other, week ending 2026-11-09\nRecovered: test_fixed"
    );
}
