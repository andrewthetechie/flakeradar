//! The weekly digest's webhook body, in each format a Webhook URL chooses, and
//! the schedule that says when it goes out.
//!
//! The rendered text, the JSON keys and the schedule's spelling are what a Slack
//! channel, a JSON consumer and an operator read, so each test names the
//! spelling it pins rather than reusing a helper that builds the same string.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the digest render is wrong"
)]

use std::slice;

use chrono::{DateTime, NaiveDate, TimeZone as _, Utc, Weekday};
use flakeradar_core::digest::{DigestSchedule, DigestTest, RepoDigest, render_digest};
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

/// The instant the acceptance criteria name: Wednesday 2026-11-11, midday UTC,
/// in the week that runs from Monday the 9th to Sunday the 15th.
fn wed_noon() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 11, 12, 0, 0)
        .single()
        .expect("a real instant")
}

/// The UTC instant the given fields name.
fn utc(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, 0, 0)
        .single()
        .expect("a real instant")
}

/// The schedule the spelling names, which the parse tests assert against.
fn schedule(text: &str) -> DigestSchedule {
    text.parse().expect("a spelled schedule")
}

#[test]
fn a_schedule_is_three_lowercase_letters_two_digits_and_an_at_sign() {
    let parsed = schedule("mon@09");
    assert_eq!(parsed.weekday, Weekday::Mon);
    assert_eq!(parsed.hour, 9);
    assert_eq!(parsed.to_string(), "mon@09");
    assert_eq!(
        "sun@23"
            .parse::<DigestSchedule>()
            .expect("a schedule")
            .to_string(),
        "sun@23"
    );
}

#[test]
fn every_documented_day_name_is_a_schedule() {
    // All seven names, because each is one arm of the parse and one arm of the
    // display: reading two of them would leave five of each untested. The hours
    // differ so the display is seen padding as well as printing two digits.
    let texts = [
        "mon@07", "tue@11", "wed@02", "thu@23", "fri@00", "sat@19", "sun@06",
    ];
    let (weekdays, displayed): (Vec<Weekday>, Vec<String>) = texts
        .into_iter()
        .map(|text| {
            let parsed = schedule(text);
            (parsed.weekday, parsed.to_string())
        })
        .unzip();
    assert_eq!(
        weekdays,
        [
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
            Weekday::Sat,
            Weekday::Sun,
        ],
        "each name reads as the day it spells"
    );
    assert_eq!(
        displayed, texts,
        "each schedule displays as the text it was read from"
    );
}

#[test]
fn a_slot_is_the_day_and_hour_in_the_week_holding_now() {
    let now = wed_noon();
    assert_eq!(schedule("mon@09").slot(now), utc(2026, 11, 9, 9));
    assert_eq!(schedule("sun@23").slot(now), utc(2026, 11, 15, 23));
    // The same week answers whatever day `now` is, including the slot's own day.
    let monday_evening = utc(2026, 11, 9, 18);
    assert_eq!(schedule("mon@09").slot(monday_evening), utc(2026, 11, 9, 9));
}

#[test]
fn a_refused_spelling_says_so_rather_than_guessing() {
    for text in [
        "fri@24",    // an hour that is not on a clock
        "monday@09", // a day name that is not three letters
        "mon@9",     // one hour digit
        "mon09",     // no separator at all
        "",          // nothing to read
        "Mon@09",    // a name compared exactly
        "mon@009",   // three hour digits
        "mon@ab",    // an hour that is no number
        "mon@+9",    // a signed hour, which is not two digits
    ] {
        assert!(
            text.parse::<DigestSchedule>().is_err(),
            "`{text}` is not a schedule"
        );
    }
}

#[test]
fn an_hour_beyond_the_clock_moves_the_slot_instead_of_panicking() {
    // An hour outside 0 to 23 cannot be parsed, so the only way to hold one is
    // to build the struct directly; the slot then just carries on past midnight.
    let rolled = DigestSchedule {
        weekday: Weekday::Mon,
        hour: 24,
    };
    assert_eq!(rolled.slot(wed_noon()), utc(2026, 11, 10, 0));
    // The largest hour there is cannot name an instant, so the answer saturates
    // at the last instant chrono can hold rather than panicking.
    let extreme = DigestSchedule {
        weekday: Weekday::Sun,
        hour: u32::MAX,
    };
    assert_eq!(extreme.slot(wed_noon()), DateTime::<Utc>::MAX_UTC);
}
