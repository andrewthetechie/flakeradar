//! The transition a rescore decides, with no database and no clock to wait on.
//!
//! Every case hands [`decide`] a window, a stored state and the settings it
//! decides against, and asserts the values it answers to write and the events it
//! answers firing: the Proven flake that starts a flaky state, the hysteresis
//! band that holds one (ADR 0012), the Fail streak that crosses the broken
//! threshold once, and the acknowledgement that suppresses an event without
//! suppressing the write.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the decision, or an instant it states, is wrong"
)]

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset};
use flakeradar_core::decision::{Answer, Settings, Stored, Window, decide};
use flakeradar_core::defaults::{SCORE_DECAY, SCORE_WINDOW};
use flakeradar_core::helpers::tier_for;
use flakeradar_core::model::ExecutionStatus::{self, Failed, Passed};
use flakeradar_core::model::{EntityKind, Tier};
use flakeradar_core::scoring::ScoredExecution;
use flakeradar_core::types::Score;

/// The row every single-row case decides about.
const ID: i32 = 1;

/// The instant every case decides at.
fn now() -> DateTime<FixedOffset> {
    instant("2026-07-21T12:00:00+00:00")
}

/// The instant a stored flaky state was set, before `now`.
fn earlier() -> DateTime<FixedOffset> {
    instant("2026-07-01T00:00:00+00:00")
}

/// The instant a day after `now`: an acknowledgement still in force.
fn unexpired() -> DateTime<FixedOffset> {
    instant("2026-07-22T12:00:00+00:00")
}

/// The instant a day before `now`: an acknowledgement that has expired.
fn expired() -> DateTime<FixedOffset> {
    instant("2026-07-20T12:00:00+00:00")
}

/// An instant, as the row's column spells it.
fn instant(text: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(text).expect("a case states an RFC 3339 instant")
}

/// The score a test states as a literal.
fn score(value: f64) -> Score {
    Score::new(value).expect("the test states a score")
}

/// The settings a case decides against: the shipped window and decay, and the
/// three thresholds the case states.
fn thresholds(flake_threshold: f64, recover_threshold: f64, broken_streak: i64) -> Settings {
    Settings {
        score_window: SCORE_WINDOW,
        score_decay: SCORE_DECAY,
        broken_streak,
        flake_threshold,
        recover_threshold,
    }
}

/// A window from `(commit_sha, status)` rows, newest first, all on `main` and all
/// uncategorised.
fn window(rows: &[(&str, ExecutionStatus)]) -> Window {
    Window {
        executions: rows
            .iter()
            .map(|(sha, status)| ScoredExecution {
                commit_sha: (*sha).to_owned(),
                branch: "main".to_owned(),
                status: *status,
            })
            .collect(),
        categories: Vec::new(),
        default_branch: Some("main".to_owned()),
    }
}

/// The one answer `ID` gets, from a window and a stored state the caller may not
/// have read at all.
fn one(
    kind: EntityKind,
    window: Option<Window>,
    state: Option<Stored>,
    settings: &Settings,
) -> Answer {
    let windows = match window {
        Some(window) => BTreeMap::from([(ID, window)]),
        None => BTreeMap::new(),
    };
    let stored = match state {
        Some(state) => BTreeMap::from([(ID, state)]),
        None => BTreeMap::new(),
    };
    let answers = decide(kind, &[ID], &windows, &stored, settings, now());
    assert_eq!(answers.len(), 1, "one id in, one answer out");
    *answers.first().expect("the answer for the id it was given")
}

#[test]
fn a_proven_flake_crosses_a_first_report_into_the_flaky_tier() {
    // One commit SHA that holds both outcomes is a Proven flake, and the newest
    // Execution of it failed.
    let answer = one(
        EntityKind::Test,
        Some(window(&[("g", Failed), ("g", Passed)])),
        None,
        &thresholds(0.4, 0.3, 2),
    );

    assert_eq!(
        answer.values.flakiness_score,
        score(0.6),
        "a Proven flake puts its floor under the flip rate"
    );
    assert_eq!(answer.values.confirmed_flake_count, 1);
    assert_eq!(answer.values.clean_streak, 0, "the newest row failed");
    assert!(
        answer.values.clear_fixed_note,
        "ADR 0007: a Clean streak of 0 clears the fixed note"
    );
    assert_eq!(answer.values.fail_streak, 1);
    assert_eq!(
        answer.values.flaky_since,
        Some(now()),
        "0.6 is at or over the Flake threshold"
    );
    assert_eq!(
        tier_for(
            answer.values.flakiness_score,
            answer.values.flaky_since.is_some()
        ),
        Tier::Flaky
    );
    assert!(answer.crossing, "no stored state, a flaky state now");
    assert!(
        !answer.broken_change,
        "one failure is under the broken threshold"
    );
}

#[test]
fn a_pass_and_a_fail_on_two_commit_shas_prove_no_flake() {
    // The same two outcomes on two SHAs prove nothing: no floor under the damped
    // flip rate of 0.1667, so it stays under the Flake threshold and no state
    // starts.
    let answer = one(
        EntityKind::Test,
        Some(window(&[("h", Failed), ("g", Passed)])),
        None,
        &thresholds(0.4, 0.3, 2),
    );

    assert_eq!(answer.values.confirmed_flake_count, 0);
    assert_eq!(answer.values.flakiness_score, score(0.1667));
    assert_eq!(answer.values.flaky_since, None);
    assert!(!answer.crossing);
}

#[test]
fn an_in_band_score_keeps_the_stored_flaky_state_and_sends_nothing() {
    // Two flips across three Executions is a 0.3333 score: under the Flake
    // threshold, so it starts nothing, and over the Recover threshold, so it
    // clears nothing (ADR 0012's keep case).
    let answer = one(
        EntityKind::Test,
        Some(window(&[("c", Passed), ("b", Failed), ("a", Passed)])),
        Some(Stored {
            flaky_since: Some(earlier()),
            ..Stored::default()
        }),
        &thresholds(0.4, 0.3, 2),
    );

    assert_eq!(
        answer.values.flakiness_score,
        score(0.3333),
        "the computed score sits between 0.4 and 0.3"
    );
    assert_eq!(
        answer.values.flaky_since,
        Some(earlier()),
        "the state holds, and keeps the instant it started at"
    );
    assert!(
        !answer.crossing,
        "a score that drifts inside the band sends no Notification"
    );
    assert!(!answer.broken_change);
}

#[test]
fn a_score_below_the_recover_threshold_clears_the_state_and_crosses() {
    // A row with no window at all scores nothing, which is below the Recover
    // threshold: the state clears, and the row is not broken, so it recovered.
    let answer = one(
        EntityKind::Test,
        None,
        Some(Stored {
            flaky_since: Some(earlier()),
            ..Stored::default()
        }),
        &thresholds(0.4, 0.3, 2),
    );

    assert_eq!(answer.values.flakiness_score, Score::ZERO);
    assert_eq!(answer.values.flaky_since, None);
    assert_eq!(
        tier_for(
            answer.values.flakiness_score,
            answer.values.flaky_since.is_some()
        ),
        Tier::Stable
    );
    assert!(answer.crossing, "a flaky state cleared is a Tier change");
    assert!(!answer.broken_change);
}

#[test]
fn a_live_acknowledgement_answers_the_values_and_no_event() {
    // Two identical rows, one acknowledged until a day from now: the same values,
    // and the event only for the unacknowledged one.
    let settings = thresholds(0.4, 0.3, 2);
    let proven_flake = window(&[("g", Failed), ("g", Passed)]);
    let answers = decide(
        EntityKind::Test,
        &[ID, 2],
        &BTreeMap::from([(ID, proven_flake.clone()), (2, proven_flake)]),
        &BTreeMap::from([(
            2,
            Stored {
                acknowledged_until: Some(unexpired()),
                ..Stored::default()
            },
        )]),
        &settings,
        now(),
    );
    let plain = answers.first().expect("one answer per id");
    let acknowledged = answers.get(1).expect("one answer per id");

    assert!(plain.crossing, "the unacknowledged row sends");
    assert_eq!(
        acknowledged.values, plain.values,
        "the acknowledged row still gets its values written"
    );
    assert!(
        !acknowledged.crossing,
        "an acknowledged row is dropped like a deleted one"
    );
    assert!(!acknowledged.broken_change);
}

#[test]
fn an_acknowledgement_that_expired_answers_its_event() {
    let answer = one(
        EntityKind::Test,
        Some(window(&[("g", Failed), ("g", Passed)])),
        Some(Stored {
            acknowledged_until: Some(expired()),
            ..Stored::default()
        }),
        &thresholds(0.4, 0.3, 2),
    );

    assert_eq!(answer.values.flaky_since, Some(now()));
    assert!(answer.crossing, "a past expiry suppresses nothing");
}

#[test]
fn a_broken_test_sends_no_recovered_event_while_a_job_sends_one() {
    // A window of nothing but failures scores no flip, so it clears a stored
    // flaky state, while its Fail streak sits at the broken threshold. A Test
    // that is broken has not fixed itself; a Job has no streak to be broken by.
    let settings = thresholds(0.4, 0.3, 2);
    let failing = window(&[("b", Failed), ("a", Failed)]);
    let state = Stored {
        flaky_since: Some(earlier()),
        fail_streak: Some(2),
        acknowledged_until: None,
    };

    let test = one(
        EntityKind::Test,
        Some(failing.clone()),
        Some(state),
        &settings,
    );
    assert_eq!(test.values.flaky_since, None, "the tier does clear");
    assert_eq!(test.values.fail_streak, 2);
    assert!(
        !test.crossing,
        "a broken Test that left the flaky tier sends no recovered event"
    );
    assert!(
        !test.broken_change,
        "a streak that was already at the threshold crossed nothing"
    );

    let job = one(EntityKind::Job, Some(failing), Some(state), &settings);
    assert_eq!(job.values.flaky_since, None);
    assert!(
        job.crossing,
        "the same streak is no obstacle to a Job's recovery"
    );
    assert!(
        !job.broken_change,
        "a Job stores no Fail streak, so no Job becomes broken"
    );
}

#[test]
fn a_fail_streak_crossing_the_broken_threshold_fires_once() {
    let settings = thresholds(0.4, 0.3, 2);
    let failing = window(&[("b", Failed), ("a", Failed)]);
    let first = one(
        EntityKind::Test,
        Some(failing.clone()),
        Some(Stored {
            fail_streak: Some(1),
            ..Stored::default()
        }),
        &settings,
    );

    assert_eq!(first.values.fail_streak, 2);
    assert!(
        first.broken_change,
        "the streak reached the broken threshold this rescore"
    );
    assert!(!first.crossing, "a row that never was flaky moves no Tier");

    let second = one(
        EntityKind::Test,
        Some(failing),
        Some(Stored {
            fail_streak: Some(2),
            ..Stored::default()
        }),
        &settings,
    );
    assert!(
        !second.broken_change,
        "the streak held there, so the event does not repeat"
    );
}
