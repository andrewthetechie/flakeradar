//! The weekly flaky-test digest: one message per Repo covering a whole week.
//!
//! A Notification says that one Test crossed a threshold at one instant; a
//! digest instead answers what a Repo did over the past week, so it carries four
//! lists of Tests rather than one event. This module owns the data shape, the
//! schedule that says when the message goes out, and the webhook body, so the
//! database query that fills the lists, the setting that names the slot and the
//! command that posts them cannot disagree about either. Posting is not this
//! crate's job, just as posting a Notification is not.

// Rust guideline compliant 2026-07-21

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, TimeDelta, Utc, Weekday};
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

/// When a week's digest goes out: a weekday and an hour, as `mon@09`.
///
/// The operator writes the spelling in `FLAKERADAR_DIGEST` and the scheduler
/// reads it back each week, so the two directions are the same text. An hour is
/// UTC, because the digest says what a week held and a week is held in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DigestSchedule {
    /// The day of the week the digest goes out.
    pub weekday: Weekday,
    /// The UTC hour of the day it goes out, `0` to `23`.
    pub hour: u32,
}

/// Why a string is not a [`DigestSchedule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DigestScheduleError {
    /// The text is not a known day, an `@` and exactly two hour digits.
    #[error("a digest schedule is <mon|tue|wed|thu|fri|sat|sun>@<00-23>, like mon@09")]
    Format,
}

impl FromStr for DigestSchedule {
    type Err = DigestScheduleError;

    /// The schedule the spelling `text` names, as `mon@09`.
    ///
    /// The day is one of the three-letter lowercase names and the hour is
    /// exactly two digits at most `23`; anything else, including an uppercase
    /// name or a one-digit hour, is refused, so a typo says so at start-up
    /// instead of quietly moving the digest.
    ///
    /// # Errors
    ///
    /// Returns [`DigestScheduleError::Format`] when `text` is not of that form.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (day, hour_text) = text.split_once('@').ok_or(DigestScheduleError::Format)?;
        let weekday = match day {
            "mon" => Weekday::Mon,
            "tue" => Weekday::Tue,
            "wed" => Weekday::Wed,
            "thu" => Weekday::Thu,
            "fri" => Weekday::Fri,
            "sat" => Weekday::Sat,
            "sun" => Weekday::Sun,
            _ => return Err(DigestScheduleError::Format),
        };
        // The hour is read as a number first and held to the documented spelling
        // afterwards, because the two refusals are different: text that is no
        // number at all, and a number spelled wrong.
        let Ok(hour) = hour_text.parse::<u32>() else {
            return Err(DigestScheduleError::Format);
        };
        // Exactly two ASCII digits at most `23`, so a one- or three-digit hour and
        // a signed one are refused even though they name an hour, and an hour off
        // a clock is refused too.
        let digits = hour_text.as_bytes();
        if digits.len() != 2 || !digits.iter().all(u8::is_ascii_digit) || hour > 23 {
            return Err(DigestScheduleError::Format);
        }
        Ok(Self { weekday, hour })
    }
}

impl fmt::Display for DigestSchedule {
    /// The spelling [`DigestSchedule::from_str`] accepts, so `parse` after
    /// `to_string` is the identity.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let day = match self.weekday {
            Weekday::Mon => "mon",
            Weekday::Tue => "tue",
            Weekday::Wed => "wed",
            Weekday::Thu => "thu",
            Weekday::Fri => "fri",
            Weekday::Sat => "sat",
            Weekday::Sun => "sun",
        };
        write!(f, "{day}@{:02}", self.hour)
    }
}

impl DigestSchedule {
    /// When this schedule rings in the ISO week (Monday first) that holds `now`.
    ///
    /// The week is `now`'s, so the answer can lie in the past: that is how the
    /// caller learns whether this week's digest is still owed. An hour outside
    /// `0` to `23` is only reachable by building the struct directly, and it
    /// adds past midnight like any other duration; the arithmetic saturates at
    /// the last instant chrono can name rather than panicking.
    ///
    /// # Examples
    ///
    /// ```
    /// # use chrono::{TimeZone, Utc};
    /// # use flakeradar_core::digest::DigestSchedule;
    /// let now = Utc.with_ymd_and_hms(2026, 11, 11, 12, 0, 0).unwrap();
    /// let schedule: DigestSchedule = "mon@09".parse().unwrap();
    /// assert_eq!(schedule.slot(now), Utc.with_ymd_and_hms(2026, 11, 9, 9, 0, 0).unwrap());
    /// ```
    #[must_use]
    pub fn slot(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        let monday = now
            .date_naive()
            .checked_sub_signed(TimeDelta::days(i64::from(
                now.weekday().num_days_from_monday(),
            )))
            .unwrap_or(NaiveDate::MIN);
        let day = monday
            .checked_add_signed(TimeDelta::days(i64::from(
                self.weekday.num_days_from_monday(),
            )))
            .unwrap_or(NaiveDate::MAX);
        let hour = TimeDelta::try_hours(i64::from(self.hour)).unwrap_or(TimeDelta::MAX);
        day.and_time(NaiveTime::MIN)
            .and_utc()
            .checked_add_signed(hour)
            .unwrap_or(DateTime::<Utc>::MAX_UTC)
    }
}
