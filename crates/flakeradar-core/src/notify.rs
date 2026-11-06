//! Webhook Notifications: one message per Tier change, as `json` or `slack`.
//!
//! A Notification is best effort (ADR 0009): the leader renders it and posts it
//! from its own task, so a slow or unreachable endpoint never delays Report
//! processing. Nothing is stored, so one that is not delivered is lost. The
//! payload is built from a [`TierChange`], the snapshot a rescore took when the
//! Test or Job crossed the Flake threshold, so the numbers can never contradict
//! the direction of the event.

// Rust guideline compliant 2026-07-21

use std::fmt::Write as _;

use serde_json::{Map, Value};

use crate::classify::FailureCategory;
use crate::model::{EntityKind, Tier};

/// The format a Webhook URL expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookFormat {
    /// A JSON array with one event object per Tier change.
    Json,
    /// Slack's incoming-webhook body: one `text` field.
    Slack,
}

/// One Test's or Job's crossing of the Flake threshold, snapshotted at detection.
///
/// `tier` is the new tier, so the direction of the event follows from it and the
/// two can never disagree. Test values come from the same rescore that wrote
/// them; a Job has no Project and no Failure category.
#[derive(Debug, Clone, PartialEq)]
pub struct TierChange {
    /// Whether a Test or a Job crossed.
    pub kind: EntityKind,
    /// The Test's or Job's id.
    pub id: i64,
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The Test's Project, or `None` for a Job.
    pub project: Option<String>,
    /// The Test's name, or the Job's name.
    pub name: String,
    /// The score the rescore wrote.
    pub flakiness_score: f64,
    /// The tier the entity is in now.
    pub tier: Tier,
    /// How many commit SHAs recorded both outcomes.
    pub proven_flake_count: usize,
    /// The Test's classname, or `None` for a Job.
    pub classname: Option<String>,
    /// The Job's Pipeline, or `None` for a Test.
    pub pipeline: Option<String>,
    /// The Test's Failure category, or `None` for a Job.
    pub failure_category: Option<FailureCategory>,
}

impl TierChange {
    /// The event name: `test.flaky`, `test.recovered`, `job.flaky` or
    /// `job.recovered`.
    #[must_use]
    pub fn event(&self) -> String {
        let direction = if self.tier == Tier::Flaky {
            "flaky"
        } else {
            "recovered"
        };
        format!("{}.{direction}", self.kind.as_str())
    }
}

/// Splits the comma-separated Webhook setting, dropping blanks.
///
/// # Examples
///
/// ```
/// assert_eq!(
///     flakeradar_core::notify::parse_urls(" https://a , ,https://b "),
///     ["https://a", "https://b"]
/// );
/// ```
#[must_use]
pub fn parse_urls(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// The dashboard deep link for one Test or Job, or `None` without a public URL.
///
/// Mirrors `frontend/src/urlstate.ts`, so the link opens the drawer.
fn dashboard_url(change: &TierChange, public_url: &str) -> Option<String> {
    let base = public_url.trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    let repo = percent_encode(&change.repo);
    match change.kind {
        EntityKind::Job => Some(format!("{base}/?view=jobs&repo={repo}&job={}", change.id)),
        EntityKind::Test => {
            let project = percent_encode(change.project.as_deref().unwrap_or_default());
            Some(format!(
                "{base}/?repo={repo}&project={project}&test={}",
                change.id
            ))
        }
    }
}

/// Percent-encodes text for a URL path segment.
///
/// The ASCII letters, digits and `_.-~` are kept, and every other byte of the
/// UTF-8 encoding is written as uppercase `%XX`, which is the one form webhook
/// clients agree on.
fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'~') {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// One Tier change as the JSON webhook sees it.
fn json_payload(change: &TierChange, public_url: &str) -> Value {
    let mut entity = Map::new();
    let _ = entity.insert("id".to_owned(), Value::from(change.id));
    match change.kind {
        EntityKind::Job => {
            let _ = entity.insert(
                "pipeline".to_owned(),
                change.pipeline.clone().map_or(Value::Null, Value::from),
            );
        }
        EntityKind::Test => {
            let _ = entity.insert(
                "classname".to_owned(),
                change.classname.clone().map_or(Value::Null, Value::from),
            );
        }
    }
    let _ = entity.insert("name".to_owned(), Value::from(change.name.clone()));
    let _ = entity.insert(
        "flakiness_score".to_owned(),
        Value::from(change.flakiness_score),
    );
    let _ = entity.insert("tier".to_owned(), Value::from(change.tier.as_str()));
    let _ = entity.insert(
        "confirmed_flake_count".to_owned(),
        Value::from(change.proven_flake_count),
    );
    if change.kind == EntityKind::Test {
        let _ = entity.insert(
            "failure_category".to_owned(),
            change
                .failure_category
                .map_or(Value::Null, |category| Value::from(category.as_str())),
        );
    }

    let mut payload = Map::new();
    let _ = payload.insert("event".to_owned(), Value::from(change.event()));
    let _ = payload.insert("repo".to_owned(), Value::from(change.repo.clone()));
    let _ = payload.insert(
        "project".to_owned(),
        change.project.clone().map_or(Value::Null, Value::from),
    );
    let _ = payload.insert(change.kind.as_str().to_owned(), Value::Object(entity));
    if let Some(url) = dashboard_url(change, public_url) {
        let _ = payload.insert("url".to_owned(), Value::from(url));
    }
    Value::Object(payload)
}

/// A list of event objects: one shape whether the Report crossed one tier or many.
#[must_use]
pub fn render_json(changes: &[TierChange], public_url: &str) -> Vec<Value> {
    changes
        .iter()
        .map(|change| json_payload(change, public_url))
        .collect()
}

/// One line per event; Slack renders the newlines as separate lines.
fn slack_line(change: &TierChange, public_url: &str) -> String {
    let subject = match change.kind {
        EntityKind::Test => format!(
            "{} / {}",
            change.repo,
            change.project.as_deref().unwrap_or_default()
        ),
        EntityKind::Job => change.repo.clone(),
    };
    let noun = change.kind.as_str();
    let where_ = match change.kind {
        EntityKind::Test => String::new(),
        EntityKind::Job => change
            .pipeline
            .as_deref()
            .map_or_else(String::new, |pipeline| format!(" in {pipeline}")),
    };
    let verb = if change.tier == Tier::Flaky {
        "became flaky"
    } else {
        "recovered"
    };
    let line = format!(
        "FlakeRadar: {subject} — {noun} \"{}\"{where_} {verb} (score {:.2})",
        change.name, change.flakiness_score
    );
    match dashboard_url(change, public_url) {
        Some(url) => format!("{line} {url}"),
        None => line,
    }
}

/// One `text` with a line per event.
#[must_use]
pub fn render_slack(changes: &[TierChange], public_url: &str) -> Value {
    let text = changes
        .iter()
        .map(|change| slack_line(change, public_url))
        .collect::<Vec<_>>()
        .join("\n");
    let mut payload = Map::new();
    let _ = payload.insert("text".to_owned(), Value::from(text));
    Value::Object(payload)
}

/// The request body for one Report's batch of Tier changes, in `format`.
#[must_use]
pub fn render(changes: &[TierChange], format: WebhookFormat, public_url: &str) -> Value {
    match format {
        WebhookFormat::Slack => render_slack(changes, public_url),
        WebhookFormat::Json => Value::Array(render_json(changes, public_url)),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        TierChange, WebhookFormat, dashboard_url, parse_urls, render, render_json, render_slack,
    };
    use crate::classify::FailureCategory;
    use crate::model::{EntityKind, Tier};

    fn test_change() -> TierChange {
        TierChange {
            kind: EntityKind::Test,
            id: 7,
            repo: "acme/app".to_owned(),
            project: Some("frontend".to_owned()),
            name: "test_login".to_owned(),
            flakiness_score: 0.5,
            tier: Tier::Flaky,
            proven_flake_count: 2,
            classname: Some("tests.login".to_owned()),
            pipeline: None,
            failure_category: Some(FailureCategory::Assertion),
        }
    }

    fn job_change() -> TierChange {
        TierChange {
            kind: EntityKind::Job,
            id: 3,
            repo: "acme/app".to_owned(),
            project: None,
            name: "test (ubuntu)".to_owned(),
            flakiness_score: 0.125,
            tier: Tier::Stable,
            proven_flake_count: 0,
            classname: None,
            pipeline: Some("ci.yml".to_owned()),
            failure_category: None,
        }
    }

    #[test]
    fn events_follow_the_new_tier() {
        assert_eq!(test_change().event(), "test.flaky");
        assert_eq!(job_change().event(), "job.recovered");
        let mut change = test_change();
        change.tier = Tier::Stable;
        assert_eq!(change.event(), "test.recovered");
        let mut change = job_change();
        change.tier = Tier::Flaky;
        assert_eq!(change.event(), "job.flaky");
    }

    #[test]
    fn parse_urls_trims_and_drops_blanks() {
        assert_eq!(
            parse_urls(" https://a , ,https://b ,"),
            ["https://a", "https://b"]
        );
        assert_eq!(parse_urls(""), Vec::<String>::new());
    }

    #[test]
    fn dashboard_links_encode_every_reserved_character() {
        let mut change = test_change();
        change.repo = "acme/my app".to_owned();
        change.project = Some("front end".to_owned());
        assert_eq!(
            dashboard_url(&change, "https://flakes.example/"),
            Some(
                "https://flakes.example/?repo=acme%2Fmy%20app&project=front%20end&test=7"
                    .to_owned()
            )
        );
        assert_eq!(
            dashboard_url(&job_change(), "https://flakes.example"),
            Some("https://flakes.example/?view=jobs&repo=acme%2Fapp&job=3".to_owned())
        );
    }

    #[test]
    fn dashboard_links_are_omitted_without_a_public_url() {
        assert_eq!(dashboard_url(&test_change(), ""), None);
        assert_eq!(dashboard_url(&test_change(), "/"), None);
    }

    #[test]
    fn json_payloads_carry_the_rescore_snapshot() {
        assert_eq!(
            render_json(&[test_change()], "https://flakes.example"),
            [json!({
                "event": "test.flaky",
                "repo": "acme/app",
                "project": "frontend",
                "test": {
                    "id": 7,
                    "classname": "tests.login",
                    "name": "test_login",
                    "flakiness_score": 0.5,
                    "tier": "flaky",
                    "confirmed_flake_count": 2,
                    "failure_category": "assertion"
                },
                "url": "https://flakes.example/?repo=acme%2Fapp&project=frontend&test=7"
            })]
        );
        assert_eq!(
            render_json(&[job_change()], ""),
            [json!({
                "event": "job.recovered",
                "repo": "acme/app",
                "project": null,
                "job": {
                    "id": 3,
                    "pipeline": "ci.yml",
                    "name": "test (ubuntu)",
                    "flakiness_score": 0.125,
                    "tier": "stable",
                    "confirmed_flake_count": 0
                }
            })]
        );
    }

    #[test]
    fn slack_renders_one_line_per_event() {
        let body = render(
            &[test_change(), job_change()],
            WebhookFormat::Slack,
            "https://flakes.example",
        );
        assert_eq!(
            body,
            json!({"text": "FlakeRadar: acme/app / frontend — test \"test_login\" became flaky (score 0.50) https://flakes.example/?repo=acme%2Fapp&project=frontend&test=7\nFlakeRadar: acme/app — job \"test (ubuntu)\" in ci.yml recovered (score 0.12) https://flakes.example/?view=jobs&repo=acme%2Fapp&job=3"})
        );
    }

    /// The recorded Tier change of a Test, with the values the payloads below
    /// state.
    fn recorded_test_change() -> TierChange {
        TierChange {
            kind: EntityKind::Test,
            id: 42,
            repo: "acme/app".to_owned(),
            project: Some("backend".to_owned()),
            name: "test_login".to_owned(),
            flakiness_score: 0.42,
            tier: Tier::Flaky,
            proven_flake_count: 2,
            classname: Some("tests.test_auth".to_owned()),
            pipeline: None,
            failure_category: Some(FailureCategory::Timing),
        }
    }

    /// The recorded Tier change of a Job, with the values the payloads below
    /// state.
    fn recorded_job_change() -> TierChange {
        TierChange {
            kind: EntityKind::Job,
            id: 3,
            repo: "acme/app".to_owned(),
            project: None,
            name: "test (ubuntu, 3.12)".to_owned(),
            flakiness_score: 0.51,
            tier: Tier::Flaky,
            proven_flake_count: 1,
            classname: None,
            pipeline: Some(".github/workflows/ci.yml".to_owned()),
            failure_category: None,
        }
    }

    /// The public URL every link in these cases is relative to.
    const PUBLIC_URL: &str = "https://flakeradar.example.com";

    #[test]
    fn parse_urls_of_only_blanks_and_commas_is_empty() {
        for raw in ["", "   ", ",", " , , "] {
            assert_eq!(parse_urls(raw), Vec::<String>::new(), "{raw:?}");
        }
    }

    #[test]
    fn a_json_test_event_carries_every_field() {
        assert_eq!(
            render_json(&[recorded_test_change()], PUBLIC_URL),
            [json!({
                "event": "test.flaky",
                "repo": "acme/app",
                "project": "backend",
                "test": {
                    "id": 42,
                    "classname": "tests.test_auth",
                    "name": "test_login",
                    "flakiness_score": 0.42,
                    "tier": "flaky",
                    "confirmed_flake_count": 2,
                    "failure_category": "timing"
                },
                "url": "https://flakeradar.example.com/?repo=acme%2Fapp&project=backend&test=42"
            })]
        );
    }

    #[test]
    fn a_json_job_event_has_no_project_and_names_its_pipeline() {
        let mut change = recorded_job_change();
        change.tier = Tier::Suspect;
        change.flakiness_score = 0.1;
        assert_eq!(
            render_json(&[change], PUBLIC_URL),
            [json!({
                "event": "job.recovered",
                "repo": "acme/app",
                "project": null,
                "job": {
                    "id": 3,
                    "pipeline": ".github/workflows/ci.yml",
                    "name": "test (ubuntu, 3.12)",
                    "flakiness_score": 0.1,
                    "tier": "suspect",
                    "confirmed_flake_count": 1
                },
                "url": "https://flakeradar.example.com/?view=jobs&repo=acme%2Fapp&job=3"
            })]
        );
    }

    #[test]
    fn a_json_event_omits_the_url_without_a_public_url() {
        let payloads = render_json(&[recorded_test_change()], "");
        assert_eq!(payloads.len(), 1, "one event renders one payload");
        let Some(payload) = payloads.first() else {
            return;
        };
        assert!(!payload.as_object().expect("an object").contains_key("url"));
    }

    #[test]
    fn a_json_batch_is_a_list_for_one_event() {
        assert_eq!(render_json(&[recorded_test_change()], PUBLIC_URL).len(), 1);
        let body = render(&[recorded_test_change()], WebhookFormat::Json, PUBLIC_URL);
        assert_eq!(body.as_array().expect("a list").len(), 1);
    }

    #[test]
    fn a_slack_payload_has_one_line_per_event() {
        assert_eq!(
            render_slack(&[recorded_test_change(), recorded_job_change()], PUBLIC_URL),
            json!({
                "text": concat!(
                    "FlakeRadar: acme/app / backend — test \"test_login\" became flaky \
                     (score 0.42) https://flakeradar.example.com/?repo=acme%2Fapp&project=backend&test=42\n",
                    "FlakeRadar: acme/app — job \"test (ubuntu, 3.12)\" in \
                     .github/workflows/ci.yml became flaky (score 0.51) \
                     https://flakeradar.example.com/?view=jobs&repo=acme%2Fapp&job=3"
                )
            })
        );
    }

    #[test]
    fn a_slack_line_says_recovered_for_the_other_tier() {
        let mut change = recorded_test_change();
        change.tier = Tier::Suspect;
        change.flakiness_score = 0.1;
        assert_eq!(
            render_slack(&[change], PUBLIC_URL)["text"],
            json!(
                "FlakeRadar: acme/app / backend — test \"test_login\" recovered (score 0.10) \
                 https://flakeradar.example.com/?repo=acme%2Fapp&project=backend&test=42"
            )
        );
    }

    #[test]
    fn a_slack_line_without_a_public_url_ends_cleanly() {
        assert_eq!(
            render_slack(&[recorded_test_change()], "")["text"],
            json!("FlakeRadar: acme/app / backend — test \"test_login\" became flaky (score 0.42)")
        );
    }

    #[test]
    fn render_selects_the_format() {
        let json_body = render(&[recorded_job_change()], WebhookFormat::Json, "");
        assert!(json_body.is_array());
        let slack_body = render(&[recorded_job_change()], WebhookFormat::Slack, "");
        assert!(slack_body.is_object());
        let text = slack_body["text"].as_str().expect("slack carries text");
        assert!(text.starts_with("FlakeRadar: "), "{text}");
        assert!(!text.contains("https://"), "{text}");
    }
}
