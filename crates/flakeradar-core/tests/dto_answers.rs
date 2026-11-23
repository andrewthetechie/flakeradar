//! The wire shape of the answer shapes.
//!
//! The shape is what the frontend, the CI actions and Fabro parse, so each test
//! names the field it pins.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "a test fails loudly when a DTO or its validation is wrong"
)]

use chrono::{DateTime, NaiveDate, TimeZone as _, Utc};
use flakeradar_core::dto::{ReportOut, ScorePointOut, TestOut};
use flakeradar_core::model::{ExecutionStatus, ReportKind, ReportStatus, Tier, Trend};
use flakeradar_core::types::{CiRunId, Fingerprint, Score};
use serde_json::{Value, json};

/// A Timestamp at a known instant.
fn at(seconds: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 20, 9, 0, seconds)
        .single()
        .expect("the fixture instant exists")
}

/// A Flakiness score a test states as a literal.
fn score(value: f64) -> Score {
    Score::new(value).expect("the test states a score")
}

/// A fingerprint, which is always 40 lowercase hex characters.
fn fingerprint() -> Fingerprint {
    Fingerprint::new("0123456789abcdef0123456789abcdef01234567").expect("a fingerprint")
}

#[test]
fn a_report_out_writes_timestamps_in_the_one_accepted_shape() {
    let report = ReportOut {
        id: 1,
        kind: ReportKind::Junit,
        repo: "acme/app".to_owned(),
        project: Some("default".to_owned()),
        commit_sha: "abc".to_owned(),
        branch: "main".to_owned(),
        ci_run_id: CiRunId::empty(),
        status: ReportStatus::Pending,
        error: None,
        counts: None,
        run_id: None,
        retry_count: 0,
        created_at: at(0),
        processed_at: None,
    };
    let value = serde_json::to_value(&report).expect("the report serializes");
    assert_eq!(value["created_at"], "2026-10-20T09:00:00.000000Z");
    assert_eq!(value["processed_at"], Value::Null);
    assert_eq!(value["error"], Value::Null);
    assert_eq!(value["counts"], Value::Null);
    assert_eq!(value["project"], "default");

    let back: ReportOut = serde_json::from_value(value).expect("the report round-trips");
    assert_eq!(back, report);
}

#[test]
fn timestamps_accept_z_and_an_offset_on_the_way_in() {
    let report = json!({
        "id": 1,
        "kind": "junit",
        "repo": "acme/app",
        "project": null,
        "commit_sha": "abc",
        "branch": "main",
        "ci_run_id": "",
        "status": "processed",
        "error": null,
        "counts": null,
        "run_id": null,
        "retry_count": 0,
        "created_at": "2026-10-20T09:00:00+00:00",
        "processed_at": "2026-10-20T11:00:00.5Z",
    });
    let parsed: ReportOut = serde_json::from_value(report).expect("both spellings parse");
    assert_eq!(parsed.created_at, at(0));
    assert_eq!(
        parsed.processed_at,
        Some(at(0) + chrono::Duration::hours(2) + chrono::Duration::milliseconds(500))
    );
}

#[test]
fn a_bad_timestamp_is_a_deserialization_error() {
    let report = json!({
        "id": 1,
        "kind": "junit",
        "repo": "acme/app",
        "project": null,
        "commit_sha": "abc",
        "branch": "main",
        "ci_run_id": "",
        "status": "pending",
        "error": null,
        "counts": null,
        "run_id": null,
        "retry_count": 0,
        "created_at": "yesterday",
        "processed_at": null,
    });
    serde_json::from_value::<ReportOut>(report).expect_err("yesterday is not a timestamp");
}

#[test]
fn dates_and_tiers_keep_their_wire_spelling() {
    let point = ScorePointOut {
        day: NaiveDate::from_ymd_opt(2026, 10, 20).expect("the date exists"),
        flakiness_score: score(0.5),
        confirmed_flake_count: 1,
        executions: 4,
        failures: 2,
    };
    let value = serde_json::to_value(&point).expect("the point serializes");
    assert_eq!(value["day"], "2026-10-20");
    assert_eq!(value["flakiness_score"], 0.5);
    assert_eq!(serde_json::to_value(Tier::Suspect).unwrap(), "suspect");
    assert_eq!(serde_json::to_value(Trend::Worsening).unwrap(), "worsening");
}

#[test]
fn a_test_out_keeps_every_field_the_consumers_read() {
    let test = TestOut {
        id: 1,
        repo: "acme/app".to_owned(),
        project: "default".to_owned(),
        fingerprint: fingerprint(),
        suite: "unit".to_owned(),
        classname: "c".to_owned(),
        name: "t".to_owned(),
        file: None,
        line: None,
        flakiness_score: Score::ZERO,
        tier: Tier::Stable,
        confirmed_flake_count: 0,
        failure_category: None,
        clean_streak: 3,
        fail_streak: 0,
        broken: false,
        trend: None,
        last_status: ExecutionStatus::Passed,
        last_seen_at: at(0),
        quarantined: false,
        quarantined_at: None,
        quarantine_reason: None,
        quarantine_expires_at: None,
        acknowledged_until: None,
        acknowledge_reason: None,
        github_issue_number: None,
        github_issue_url: None,
        github_issue_closed: false,
        first_seen_at: at(0),
        new: false,
        impacted_runs: 0,
        impacted_branches: 0,
        time_lost_seconds: 0.0,
    };
    let value = serde_json::to_value(&test).expect("the test serializes");
    for field in [
        "id",
        "repo",
        "project",
        "fingerprint",
        "suite",
        "classname",
        "name",
        "file",
        "line",
        "flakiness_score",
        "tier",
        "confirmed_flake_count",
        "failure_category",
        "clean_streak",
        "fail_streak",
        "broken",
        "trend",
        "last_status",
        "last_seen_at",
        "quarantined",
        "quarantined_at",
        "quarantine_reason",
        "quarantine_expires_at",
        "acknowledged_until",
        "acknowledge_reason",
        "github_issue_number",
        "github_issue_url",
        "github_issue_closed",
        "first_seen_at",
        "new",
        "impacted_runs",
        "impacted_branches",
        "time_lost_seconds",
    ] {
        assert!(
            value.get(field).is_some(),
            "{field} is missing from {value}"
        );
    }
    assert_eq!(value["last_seen_at"], "2026-10-20T09:00:00.000000Z");
}
