//! The weekly flaky-test digest: one message per Repo covering a whole week.
//!
//! A Notification says that one Test crossed a threshold at one instant; a
//! digest instead answers what a Repo did over the past week, so it carries four
//! lists of Tests rather than one event. This module owns the data shape and the
//! webhook body, so the database query that fills the lists and the command that
//! posts them cannot disagree about either. Posting is not this crate's job, just
//! as posting a Notification is not.

// Rust guideline compliant 2026-07-21

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Value};

use crate::notify::WebhookFormat;

/// One Test in a weekly digest.
///
/// The four lists of a [`RepoDigest`] name four different reasons a Test is worth
/// reporting, and each reason has its own evidence: a `top_impact` entry carries
/// `impacted_runs` and `time_lost_seconds`, a `quarantine_expiring` entry carries
/// `quarantine_expires_at`, and a Test in neither list carries neither. Each
/// `None` is left out of the JSON rather than written as `null`, so a consumer
/// reads an absent key as "this list says nothing about that".
#[derive(Debug, Clone, PartialEq)]
pub struct DigestTest {
    /// The Test's id.
    pub id: i64,
    /// The Test's Project.
    pub project: String,
    /// The Test's classname.
    pub classname: String,
    /// The Test's name.
    pub name: String,
    /// The score the digest was built from.
    pub flakiness_score: f64,
    /// The CI runs the Test's flakes cost in the impact window, for the lists that rank by impact.
    pub impacted_runs: Option<i64>,
    /// The seconds those runs cost, for the lists that rank by impact.
    pub time_lost_seconds: Option<f64>,
    /// When the Test's quarantine lapses, for the list that warns about it.
    pub quarantine_expires_at: Option<DateTime<Utc>>,
}

/// One Repo's week: the four lists a team lead reviews, in the order the digest
/// shows them.
#[derive(Debug, Clone, PartialEq)]
pub struct RepoDigest {
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The Tests that became flaky during the week.
    pub newly_flaky: Vec<DigestTest>,
    /// The Tests that were flaky and are no longer.
    pub recovered: Vec<DigestTest>,
    /// The Tests whose flakes cost the most CI runs, worst first.
    pub top_impact: Vec<DigestTest>,
    /// The Tests whose quarantine lapses within the coming week.
    pub quarantine_expiring: Vec<DigestTest>,
}

/// One Test as the JSON webhook sees it, with each `None` field absent.
fn test_json(test: &DigestTest) -> Value {
    let mut item = Map::new();
    let _ = item.insert("id".to_owned(), Value::from(test.id));
    let _ = item.insert("project".to_owned(), Value::from(test.project.clone()));
    let _ = item.insert("classname".to_owned(), Value::from(test.classname.clone()));
    let _ = item.insert("name".to_owned(), Value::from(test.name.clone()));
    let _ = item.insert(
        "flakiness_score".to_owned(),
        Value::from(test.flakiness_score),
    );
    if let Some(runs) = test.impacted_runs {
        let _ = item.insert("impacted_runs".to_owned(), Value::from(runs));
    }
    if let Some(seconds) = test.time_lost_seconds {
        let _ = item.insert("time_lost_seconds".to_owned(), Value::from(seconds));
    }
    if let Some(expires) = test.quarantine_expires_at {
        let _ = item.insert(
            "quarantine_expires_at".to_owned(),
            Value::from(expires.to_rfc3339()),
        );
    }
    Value::Object(item)
}

/// One event object per Repo, each with all four lists even when they are empty.
fn digest_json(digests: &[RepoDigest], week_ending: NaiveDate) -> Value {
    Value::Array(
        digests
            .iter()
            .map(|digest| {
                let mut event = Map::new();
                let _ = event.insert("event".to_owned(), Value::from("digest.weekly"));
                let _ = event.insert("repo".to_owned(), Value::from(digest.repo.clone()));
                let _ = event.insert(
                    "week_ending".to_owned(),
                    Value::from(week_ending.to_string()),
                );
                for (key, tests) in [
                    ("newly_flaky", &digest.newly_flaky),
                    ("recovered", &digest.recovered),
                    ("top_impact", &digest.top_impact),
                    ("quarantine_expiring", &digest.quarantine_expiring),
                ] {
                    let _ = event.insert(
                        key.to_owned(),
                        Value::Array(tests.iter().map(test_json).collect()),
                    );
                }
                Value::Object(event)
            })
            .collect(),
    )
}

/// One Test as a `Top impact` item, with the runs and the time it cost.
fn impact_item(test: &DigestTest) -> String {
    match (test.impacted_runs, test.time_lost_seconds) {
        // A digest built without the impact fields still names the Test, which is
        // what a reader needs to act on.
        (Some(runs), Some(seconds)) => {
            let name = test.name.as_str();
            let plural = if runs == 1 { "" } else { "s" };
            format!("{name} ({runs} CI run{plural}, {seconds:.1}s lost)")
        }
        _ => test.name.clone(),
    }
}

/// One Test as a `Quarantine expiring` item, with the day it lapses.
fn expiry_item(test: &DigestTest) -> String {
    test.quarantine_expires_at.map_or_else(
        || test.name.clone(),
        |expires| format!("{} ({})", test.name, expires.format("%Y-%m-%d")),
    )
}

/// One Test's name, which is all the two lists without evidence need.
fn name_item(test: &DigestTest) -> String {
    test.name.clone()
}

/// Appends one `label: a, b` line for a list that has something in it, so an
/// empty list costs the reader nothing.
fn push_list(
    lines: &mut Vec<String>,
    label: &str,
    tests: &[DigestTest],
    item: fn(&DigestTest) -> String,
) {
    if tests.is_empty() {
        return;
    }
    let joined = tests.iter().map(item).collect::<Vec<_>>().join(", ");
    lines.push(format!("{label}: {joined}"));
}

/// One Repo's block: the header, then a line per list with something in it.
fn slack_block(digest: &RepoDigest, week_ending: NaiveDate) -> String {
    let repo = digest.repo.as_str();
    let mut lines = vec![format!(
        "FlakeRadar weekly digest — {repo}, week ending {week_ending}"
    )];
    push_list(&mut lines, "Newly flaky", &digest.newly_flaky, name_item);
    push_list(&mut lines, "Recovered", &digest.recovered, name_item);
    push_list(&mut lines, "Top impact", &digest.top_impact, impact_item);
    push_list(
        &mut lines,
        "Quarantine expiring",
        &digest.quarantine_expiring,
        expiry_item,
    );
    lines.join("\n")
}

/// One `text` with a block per Repo, separated by a blank line.
fn digest_slack(digests: &[RepoDigest], week_ending: NaiveDate) -> Value {
    let text = digests
        .iter()
        .map(|digest| slack_block(digest, week_ending))
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut payload = Map::new();
    let _ = payload.insert("text".to_owned(), Value::from(text));
    Value::Object(payload)
}

/// The weekly digest body, in the `format` the Webhook URL chose, ending the week
/// at `week_ending`.
///
/// `Json` answers a list with one `digest.weekly` event per Repo, because a list
/// of event objects is the shape every other `json` webhook body has: the same
/// endpoint reads a digest and a Tier change the same way. `Slack` answers one
/// `text`, as every Slack incoming-webhook body does.
#[must_use]
pub fn render_digest(
    digests: &[RepoDigest],
    format: WebhookFormat,
    week_ending: NaiveDate,
) -> Value {
    match format {
        WebhookFormat::Slack => digest_slack(digests, week_ending),
        WebhookFormat::Json => digest_json(digests, week_ending),
    }
}
