//! The untrusted request bodies and the lax readers that validate them.
//!
//! Untrusted CI input never panics (docs/agents/code-standards-rust.md): an
//! upload or an API body is attacker-controlled, so a malformed body produces a
//! `Result`, a stored failure or a 4xx, never a panic and never an arithmetic
//! overflow. Every reader here records an issue rather than unwrapping a value.
//!
//! The root re-exports every public name, so `flakeradar_core::dto::PipelineReportIn`
//! resolves exactly as it did while these bodies lived in the root file. The
//! serde attributes name [`lax_bool`] and [`optional_timestamp`] as they always
//! did, so this module imports the wire readers from the root rather than
//! rewriting an attribute string.

// Rust guideline compliant 2026-07-21

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[expect(
    unused_imports,
    reason = "the two names a serde attribute of a body here resolves, plus the shape `optional_timestamp` reads and writes, imported together as the root's wire readers"
)]
use crate::dto::{lax_bool, optional_timestamp, timestamp};
use crate::identity::{IdentityError, normalize_provider, normalize_repo};
use crate::lax;
use crate::model::JobStatus;
use crate::py::strip_wire_whitespace;
use crate::types::CiRunId;

use super::refusal::{
    AttemptIn, BodyRefusal, FinishIssues, LocSegment, ValidationIssue, body_loc_root,
    read_ci_run_attempt,
};

/// One Job's result inside a Pipeline report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct JobResultIn {
    /// The CI provider's id for the Job.
    pub ci_job_id: String,
    /// The Job's name within its Pipeline.
    pub name: String,
    /// What the Job did.
    pub status: JobStatus,
    /// The Job's CI url, when the reporter sent one.
    #[serde(default)]
    pub url: String,
    /// The runner's name, when the reporter sent one.
    #[serde(default)]
    pub runner_name: String,
    /// The runner's labels, such as `ubuntu-latest`.
    #[serde(default)]
    pub runner_labels: Vec<String>,
    /// When the Job started.
    #[serde(default, with = "optional_timestamp")]
    pub started_at: Option<DateTime<Utc>>,
    /// When the Job finished.
    #[serde(default, with = "optional_timestamp")]
    pub completed_at: Option<DateTime<Utc>>,
    /// Whether the Job is a Summary Job.
    #[serde(default)]
    pub summary: bool,
}

/// The longest name a Report may give a Repo, a Project, a branch, a
/// `ci_job_id`, a `ci_run_id` or a runner label, in code points.
///
/// This is the bound this API publishes: a longer value is refused with the 422
/// `string_too_long` naming the field, which `tests/parity` replays against the
/// recorded answer. Raising it would accept a Report no consumer has ever sent
/// and store a name in a `text` column that the published contract says is too
/// long; lowering it would refuse an `owner/repo` or a matrix Job name a CI
/// provider really reports.
pub const NAME_MAX: usize = 255;

/// The longest name a Pipeline's Job may carry, in code points.
///
/// A matrix Job's name spells its whole matrix in one string, such as
/// `test (ubuntu-24.04, 3.12)`, and several of those run long, so a Job name gets
/// twice the room an identity does. The bound is part of the published 422 and
/// `tests/parity` replays it.
pub const JOB_NAME_MAX: usize = 512;

/// The longest Report `pipeline`, in code points.
///
/// A `pipeline` is a workflow path such as
/// `.github/workflows/integration-tests.yml`, which is why it shares the Job
/// name's bound rather than the shorter identity one.
pub const PIPELINE_MAX: usize = JOB_NAME_MAX;

/// The longest commit SHA a Report may name, in code points.
///
/// Both SHA-1 (40 hex characters) and SHA-256 (64) Git object names fit, and
/// nothing longer is an object name. The column is `text`, so this bound is the
/// contract's rather than storage's.
pub const COMMIT_SHA_MAX: usize = 64;

/// The longest Provider name a Report may name, in code points.
///
/// Providers are a short fixed vocabulary (`github`, `gitlab`, `other`), so
/// anything past a short word is not one. The published 422 names the field when
/// the value is longer.
pub const PROVIDER_MAX: usize = 32;

/// The longest Project name a Report or a query may name, in code points.
///
/// A Project is a test suite's name (`frontend`, `backend`, `e2e`), so a short
/// word is normal and 100 is generous. The bound is published on both the ingest
/// query and the read filters, and `tests/parity` replays the 422 for a longer
/// one. The column is `text`, so nothing in the database enforces it.
pub const PROJECT_MAX: usize = 100;

/// The longest path a Report or a query may name, in code points.
///
/// A Project root and a Test's `file` are paths inside a repository, and a
/// deeply nested monorepo path runs long; 1024 is the usual filesystem ceiling
/// and far past any path a repository holds. Both the ingest query and the Test
/// list filter publish the same bound, and the column is `text`.
pub const PATH_MAX: usize = 1024;

/// The longest URL a Report or an Issue link may carry, in code points.
///
/// A GitHub issue URL with a long Repo name and a query string fits well inside
/// 2048, which is the bound browsers and GitHub's own API work to. The column is
/// `text`; the bound is the contract's, and `tests/parity` replays the 422.
pub const URL_MAX: usize = 2048;

/// The longest `runner_labels` list a Job may carry.
///
/// A GitHub Actions matrix leg reports one label per dimension plus its own
/// (`ubuntu-24.04`, `3.12`, `gpu` and so on), so a handful is normal and 50 is
/// far past any real matrix. A longer list is refused with the recorded 422
/// rather than stored; the list is stored as one row's text, so no index or
/// column width bounds it.
const RUNNER_LABELS_MAX: usize = 50;

/// The longest one runner label may be.
///
/// Labels are runner names and container image tags, the longest of which fit in
/// a short line. The bound is part of the same published 422 as the list's size.
const RUNNER_LABEL_MAX: usize = NAME_MAX;

impl JobResultIn {
    /// Reads one Job of a Pipeline report with the lax rules this API applies.
    ///
    /// `at` is where the Job sits in the body, so each issue's `loc` names the
    /// Job and the field a client has to fix.
    ///
    /// # Errors
    ///
    /// Returns every issue the Job has, in field order.
    pub fn from_value(
        object: &serde_json::Map<String, Value>,
        at: Vec<LocSegment>,
    ) -> Result<Self, Vec<ValidationIssue>> {
        let fields = lax::Fields::new(object, at);
        let mut issues = Vec::new();

        let ci_job_id =
            read_string(&fields, "ci_job_id", 1, NAME_MAX, &mut issues).unwrap_or_default();
        let name = read_string(&fields, "name", 1, JOB_NAME_MAX, &mut issues).unwrap_or_default();
        let status = read_status(&fields, &mut issues).unwrap_or(JobStatus::Passed);
        let url = read_defaulted_string(&fields, "url", URL_MAX, &mut issues).unwrap_or_default();
        let runner_name = read_defaulted_string(&fields, "runner_name", NAME_MAX, &mut issues)
            .unwrap_or_default();
        let runner_labels = read_runner_labels(&fields, &mut issues);
        let started_at = read_optional_timestamp(&fields, "started_at", &mut issues);
        let completed_at = read_optional_timestamp(&fields, "completed_at", &mut issues);
        let summary = read_defaulted_bool(&fields, "summary", &mut issues).unwrap_or(false);

        issues.finish()?;
        Ok(Self {
            ci_job_id,
            name,
            status,
            url,
            runner_name,
            runner_labels,
            started_at,
            completed_at,
            summary,
        })
    }
}

/// The required string `field` of `fields` holds, stripped and checked.
fn read_string(
    fields: &lax::Fields<'_>,
    field: &str,
    min: usize,
    max: usize,
    issues: &mut Vec<ValidationIssue>,
) -> Option<String> {
    let value = fields.required(field, issues)?;
    let text = lax::recorded(lax::string(value, fields.loc(field)), issues)?;
    Some(check_string(&text, min, max, fields.loc(field), issues))
}

/// The string `field` of `fields` holds, stripped and checked, when it is there.
///
/// The field has a default, so an absent value leaves it; a `null` is a value of
/// the wrong type, which is refused exactly as a number of the wrong type is.
fn read_defaulted_string(
    fields: &lax::Fields<'_>,
    field: &str,
    max: usize,
    issues: &mut Vec<ValidationIssue>,
) -> Option<String> {
    let value = fields.field(field)?;
    let text = lax::recorded(lax::string(value, fields.loc(field)), issues)?;
    Some(check_string(&text, 0, max, fields.loc(field), issues))
}

/// The `status` of a Job, read against the literals a Job status may be.
fn read_status(fields: &lax::Fields<'_>, issues: &mut Vec<ValidationIssue>) -> Option<JobStatus> {
    let value = fields.required("status", issues)?;
    let options: Vec<&str> = JobStatus::ALL
        .iter()
        .map(|status| status.as_str())
        .collect();
    lax::recorded(lax::literal(value, &options, fields.loc("status")), issues)
        .and_then(JobStatus::parse)
}

/// The `runner_labels` of a Job: a list of short strings, at most 50 of them.
fn read_runner_labels(fields: &lax::Fields<'_>, issues: &mut Vec<ValidationIssue>) -> Vec<String> {
    let Some(value) = fields.field("runner_labels") else {
        return Vec::new();
    };
    let Some(labels) = lax::recorded(lax::list_of(value, fields.loc("runner_labels")), issues)
    else {
        return Vec::new();
    };
    // A list past the bound is refused whole, before any item is read: one issue
    // names the list's length rather than one issue per label.
    if labels.len() > RUNNER_LABELS_MAX {
        issues.push(ValidationIssue::list_too_long(
            fields.loc("runner_labels"),
            Value::from(labels.len()),
            RUNNER_LABELS_MAX,
            labels.len(),
        ));
        return Vec::new();
    }
    let mut out = Vec::with_capacity(labels.len());
    for (index, label) in labels.iter().enumerate() {
        let loc = fields.item_loc("runner_labels", index);
        let Some(label) = lax::recorded(lax::string(label, loc.clone()), issues) else {
            continue;
        };
        let label = stripped(&label);
        lax::refuse_nul(&label, loc.clone(), issues);
        if label.chars().count() > RUNNER_LABEL_MAX {
            issues.push(ValidationIssue::string_too_long(
                loc,
                Value::from(label.clone()),
                RUNNER_LABEL_MAX,
            ));
        }
        out.push(label);
    }
    out
}

/// The optional timestamp `field` of `fields` holds.
fn read_optional_timestamp(
    fields: &lax::Fields<'_>,
    field: &str,
    issues: &mut Vec<ValidationIssue>,
) -> Option<DateTime<Utc>> {
    let value = fields.field(field).filter(|value| !value.is_null())?;
    lax::recorded(lax::timestamp(value, fields.loc(field)), issues)
}

/// The boolean `field` of `fields` holds, when it is there.
///
/// The field has a default, so an absent value leaves it; a `null` is a value of
/// the wrong type, which is refused the same way.
fn read_defaulted_bool(
    fields: &lax::Fields<'_>,
    field: &str,
    issues: &mut Vec<ValidationIssue>,
) -> Option<bool> {
    let value = fields.field(field)?;
    lax::recorded(lax::boolean(value, fields.loc(field)), issues)
}

/// Trims text on both ends past every whitespace character.
fn stripped(text: &str) -> String {
    strip_wire_whitespace(text).to_owned()
}

/// Checks a string's NUL and length bounds, returning the stripped value.
fn check_string(
    text: &str,
    min: usize,
    max: usize,
    loc: Vec<LocSegment>,
    issues: &mut Vec<ValidationIssue>,
) -> String {
    let text = stripped(text);
    lax::refuse_nul(&text, loc.clone(), issues);
    let len = text.chars().count();
    if len < min {
        issues.push(ValidationIssue::string_too_short(
            loc.clone(),
            Value::from(text.clone()),
            min,
        ));
    }
    if len > max {
        issues.push(ValidationIssue::string_too_long(
            loc,
            Value::from(text.clone()),
            max,
        ));
    }
    text
}

/// The most Jobs one Pipeline report may carry.
///
/// One Pipeline run reports its whole Job matrix in one upload, and a large
/// repository's is dozens, not hundreds. 1000 is far past any real matrix and is
/// what the published 422 states; a longer list is refused before anything is
/// stored, so the upload cannot fill `job_executions` on its own.
const JOBS_MAX: usize = 1000;

/// The largest `ci_run_attempt` the `int4` column holds.
pub const ATTEMPT_MAX: i64 = i32::MAX as i64;

/// One attempt of one Pipeline run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PipelineReportIn {
    /// The Repo's `owner/name`, normalized on the way in.
    pub repo: String,
    /// The CI provider's label, such as `github`.
    pub provider: String,
    /// The Pipeline's name, such as `.github/workflows/ci.yml`.
    pub pipeline: String,
    /// The commit the attempt ran.
    pub commit_sha: String,
    /// The branch the commit is on.
    pub branch: String,
    /// The Repo's Default branch, when the reporter knew it.
    #[serde(default)]
    pub default_branch: Option<String>,
    /// The CI provider's run id.
    #[serde(default)]
    pub ci_run_id: CiRunId,
    /// Which attempt of the CI run this is, numbered from 1.
    #[serde(default = "default_attempt")]
    pub ci_run_attempt: i64,
    /// The Jobs this attempt ran.
    pub jobs: Vec<JobResultIn>,
}

/// The attempt number a Report that names none counts as.
fn default_attempt() -> i64 {
    1
}

impl PipelineReportIn {
    /// Reads a Pipeline report body with the lax rules this API applies.
    ///
    /// # Errors
    ///
    /// Returns every issue the body has, in field order, or
    /// [`BodyRefusal::AttemptPastColumn`] for an attempt the `int4` column cannot
    /// hold.
    pub fn from_value(object: &serde_json::Map<String, Value>) -> Result<Self, BodyRefusal> {
        let fields = lax::Fields::new(object, body_loc_root());
        let mut issues = Vec::new();

        let repo = read_name(&fields, "repo", NAME_MAX, &mut issues, normalize_repo);
        let provider = read_name(
            &fields,
            "provider",
            PROVIDER_MAX,
            &mut issues,
            normalize_provider,
        );
        let pipeline =
            read_string(&fields, "pipeline", 1, PIPELINE_MAX, &mut issues).unwrap_or_default();
        let commit_sha =
            read_string(&fields, "commit_sha", 1, COMMIT_SHA_MAX, &mut issues).unwrap_or_default();
        let branch = read_string(&fields, "branch", 1, NAME_MAX, &mut issues).unwrap_or_default();
        let default_branch = read_default_branch(&fields, &mut issues);
        // A refused id is the reason the whole body is refused, so the fallback
        // value never reaches a row.
        let ci_run_id = read_defaulted_string(&fields, "ci_run_id", NAME_MAX, &mut issues)
            .and_then(|text| CiRunId::new(&text).ok())
            .unwrap_or_else(CiRunId::empty);
        let mut ci_run_attempt = default_attempt();
        let mut attempt_past_column = false;
        if let Some(value) = fields.field("ci_run_attempt") {
            let reading = read_ci_run_attempt(value);
            if let Some(issue) = reading.issue(fields.loc("ci_run_attempt")) {
                issues.push(issue);
            }
            match reading {
                AttemptIn::Number(number) => ci_run_attempt = i64::from(number),
                AttemptIn::PastColumn => attempt_past_column = true,
                _ => {}
            }
        }
        let jobs = read_jobs(&fields, &mut issues);

        if issues.is_empty() {
            // A model-level rule runs only once every field validated, so a body
            // with two faults reports the field's issue and not the model's.
            let mut seen = std::collections::BTreeSet::new();
            if !jobs.iter().all(|job| seen.insert(job.ci_job_id.clone())) {
                issues.push(ValidationIssue::model_error("duplicate ci_job_id in jobs"));
            }
        }
        issues.finish().map_err(BodyRefusal::Issues)?;
        if attempt_past_column {
            return Err(BodyRefusal::AttemptPastColumn);
        }
        Ok(Self {
            repo,
            provider,
            pipeline,
            commit_sha,
            branch,
            default_branch,
            ci_run_id,
            ci_run_attempt,
            jobs,
        })
    }
}

/// The required name `field` of `fields` holds, normalized by `normalize`.
fn read_name(
    fields: &lax::Fields<'_>,
    field: &str,
    max: usize,
    issues: &mut Vec<ValidationIssue>,
    normalize: fn(&str) -> Result<String, IdentityError>,
) -> String {
    let Some(value) = fields.required(field, issues) else {
        return String::new();
    };
    let Some(text) = lax::recorded(lax::string(value, fields.loc(field)), issues) else {
        return String::new();
    };
    normalized_name(&text, max, field, issues, normalize)
}

/// The `default_branch` of a Pipeline report: a short string, blank for none.
fn read_default_branch(
    fields: &lax::Fields<'_>,
    issues: &mut Vec<ValidationIssue>,
) -> Option<String> {
    let value = fields
        .field("default_branch")
        .filter(|value| !value.is_null())?;
    let text = lax::recorded(lax::string(value, fields.loc("default_branch")), issues)?;
    let text = stripped(&text);
    lax::refuse_nul(&text, fields.loc("default_branch"), issues);
    if text.is_empty() {
        return None;
    }
    if text.chars().count() > NAME_MAX {
        issues.push(ValidationIssue::string_too_long(
            fields.loc("default_branch"),
            Value::from(text),
            NAME_MAX,
        ));
        return None;
    }
    Some(text)
}

/// The `jobs` of a Pipeline report: one or more Jobs, each read field by field.
fn read_jobs(fields: &lax::Fields<'_>, issues: &mut Vec<ValidationIssue>) -> Vec<JobResultIn> {
    let Some(value) = fields.required("jobs", issues) else {
        return Vec::new();
    };
    let Some(items) = lax::recorded(lax::list_of(value, fields.loc("jobs")), issues) else {
        return Vec::new();
    };
    if items.is_empty() {
        issues.push(ValidationIssue::list_too_short(
            fields.loc("jobs"),
            Value::from(0),
            1,
        ));
        return Vec::new();
    }
    if items.len() > JOBS_MAX {
        issues.push(ValidationIssue::list_too_long(
            fields.loc("jobs"),
            Value::from(items.len()),
            JOBS_MAX,
            items.len(),
        ));
        return Vec::new();
    }
    let mut jobs = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let at = fields.item_loc("jobs", index);
        let Some(object) = item.as_object() else {
            issues.push(ValidationIssue::model_attributes_type(at, item.clone()));
            continue;
        };
        match JobResultIn::from_value(object, at) {
            Ok(job) => jobs.push(job),
            Err(job_issues) => issues.extend(job_issues),
        }
    }
    jobs
}

/// The stored body of a Pipeline Report: the validated Report, canonically
/// serialized.
///
/// A Pipeline Report keeps its body so the processor reads back exactly what was
/// validated, so this and `PipelineReportIn`'s deserializer are inverses.
///
/// # Panics
///
/// Panics only if the body cannot be serialized, which a Report that
/// deserialized from JSON cannot do: every field is a string, a bool, an
/// integer, a list or an optional value.
#[must_use]
#[expect(
    clippy::expect_used,
    reason = "a Report that deserialized from JSON always serializes back: every field is a \
              string, a bool, an integer, a list or an optional value, none of which can fail"
)]
pub fn pipeline_body(report: &PipelineReportIn) -> Vec<u8> {
    serde_json::to_vec(report).expect("a validated Pipeline Report always serializes")
}

/// Where one field of the Pipeline report's body sits.
fn body_loc(field: &str) -> Vec<LocSegment> {
    vec![
        LocSegment::Field("body".to_owned()),
        LocSegment::Field(field.to_owned()),
    ]
}

/// Normalizes a required name, recording the length or NUL issue it finds.
fn normalized_name(
    raw: &str,
    max: usize,
    field: &str,
    issues: &mut Vec<ValidationIssue>,
    normalize: fn(&str) -> Result<String, IdentityError>,
) -> String {
    let raw = stripped(raw);
    let loc = body_loc(field);
    lax::refuse_nul(&raw, loc.clone(), issues);
    if raw.chars().count() > max {
        issues.push(ValidationIssue::string_too_long(
            loc,
            Value::from(raw.clone()),
            max,
        ));
        return raw;
    }
    match normalize(&raw) {
        Ok(normalized) => normalized,
        Err(error) => {
            issues.push(ValidationIssue::value_error(
                &error,
                loc,
                Value::from(raw.clone()),
            ));
            raw
        }
    }
}

/// A human's quarantine decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct QuarantineIn {
    /// Whether the Test is quarantined, in any spelling [`lax_bool`] accepts.
    #[serde(deserialize_with = "lax_bool::deserialize")]
    pub quarantined: bool,
    /// Why the human quarantined it.
    #[serde(default)]
    pub reason: Option<String>,
    /// How many days the quarantine lasts.
    #[serde(default)]
    pub expires_in_days: Option<i64>,
}

impl QuarantineIn {
    /// Reads a quarantine body with the lax rules this API applies.
    ///
    /// # Errors
    ///
    /// Returns every issue the body has, in field order, which the 422's `detail`
    /// list carries.
    pub fn from_value(
        object: &serde_json::Map<String, Value>,
    ) -> Result<Self, Vec<ValidationIssue>> {
        let fields = lax::Fields::new(object, body_loc_root());
        let mut issues = Vec::new();

        let quarantined = fields
            .required("quarantined", &mut issues)
            .and_then(|value| {
                lax::recorded(lax::boolean(value, fields.loc("quarantined")), &mut issues)
            })
            .unwrap_or_default();
        let reason = if let Some(text) = fields
            .field("reason")
            .filter(|value| !value.is_null())
            .and_then(|value| lax::recorded(lax::string(value, fields.loc("reason")), &mut issues))
        {
            lax::refuse_nul(&text, fields.loc("reason"), &mut issues);
            if text.chars().count() > QUARANTINE_REASON_MAX {
                issues.push(ValidationIssue::string_too_long(
                    fields.loc("reason"),
                    Value::from(text.clone()),
                    QUARANTINE_REASON_MAX,
                ));
            }
            Some(text)
        } else {
            None
        };
        let mut expires_in_days = None;
        if let Some(value) = fields
            .field("expires_in_days")
            .filter(|value| !value.is_null())
        {
            match lax::recorded(
                lax::integer(value, fields.loc("expires_in_days")),
                &mut issues,
            ) {
                Some(days) if days < 1 => issues.push(ValidationIssue::greater_than_equal(
                    fields.loc("expires_in_days"),
                    value.clone(),
                    1,
                )),
                Some(days) if days > QUARANTINE_DAYS_MAX => {
                    issues.push(ValidationIssue::less_than_equal(
                        fields.loc("expires_in_days"),
                        value.clone(),
                        QUARANTINE_DAYS_MAX,
                    ));
                }
                Some(days) => expires_in_days = Some(days),
                // A value that is no integer has recorded its own issue.
                None => {}
            }
        }

        issues.finish().map(|()| Self {
            quarantined,
            reason,
            expires_in_days,
        })
    }
}

/// The longest quarantine reason a human may record, in code points.
///
/// A reason is a sentence or two, such as "fails on CI's Windows runner, see
/// issue 42"; 500 characters holds that with room to spare. The value travels to
/// the frontend's quarantine form, which publishes the same bound, and a longer
/// one is refused with the recorded 422.
const QUARANTINE_REASON_MAX: usize = 500;

/// The most days a quarantine may last.
///
/// A quarantine expires by itself when its day count runs out, so this bound is
/// how far ahead one person can switch a Test off without coming back to it. A
/// year is the longest a fix could plausibly take; a longer one would be a way to
/// forget a Test forever, so it is refused with the recorded 422.
const QUARANTINE_DAYS_MAX: i64 = 365;

/// A human's acknowledgement decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AcknowledgeIn {
    /// Whether the Test is acknowledged, in any spelling [`lax_bool`] accepts.
    #[serde(deserialize_with = "lax_bool::deserialize")]
    pub acknowledged: bool,
    /// Why the human acknowledged it.
    #[serde(default)]
    pub reason: Option<String>,
    /// How many days the acknowledgement lasts.
    #[serde(default)]
    pub days: Option<i64>,
}

impl AcknowledgeIn {
    /// Reads an acknowledgement body with the lax rules this API applies.
    ///
    /// # Errors
    ///
    /// Returns every issue the body has, in field order, which the 422's `detail`
    /// list carries.
    pub fn from_value(
        object: &serde_json::Map<String, Value>,
    ) -> Result<Self, Vec<ValidationIssue>> {
        let fields = lax::Fields::new(object, body_loc_root());
        let mut issues = Vec::new();

        let acknowledged = fields
            .required("acknowledged", &mut issues)
            .and_then(|value| {
                lax::recorded(lax::boolean(value, fields.loc("acknowledged")), &mut issues)
            })
            .unwrap_or_default();
        let mut reason = None;
        let mut days = None;
        if acknowledged {
            // An un-acknowledge stores nothing, so the two values that only an
            // acknowledgement carries are skipped, not validated.
            if let Some(text) = fields
                .field("reason")
                .filter(|value| !value.is_null())
                .and_then(|value| {
                    lax::recorded(lax::string(value, fields.loc("reason")), &mut issues)
                })
            {
                lax::refuse_nul(&text, fields.loc("reason"), &mut issues);
                if text.chars().count() > ACKNOWLEDGE_REASON_MAX {
                    issues.push(ValidationIssue::string_too_long(
                        fields.loc("reason"),
                        Value::from(text.clone()),
                        ACKNOWLEDGE_REASON_MAX,
                    ));
                }
                reason = Some(text);
            }
            if let Some(value) = fields.required("days", &mut issues) {
                match lax::recorded(lax::integer(value, fields.loc("days")), &mut issues) {
                    Some(day) if day < 1 => issues.push(ValidationIssue::greater_than_equal(
                        fields.loc("days"),
                        value.clone(),
                        1,
                    )),
                    Some(day) if day > ACKNOWLEDGE_DAYS_MAX => {
                        issues.push(ValidationIssue::less_than_equal(
                            fields.loc("days"),
                            value.clone(),
                            ACKNOWLEDGE_DAYS_MAX,
                        ));
                    }
                    Some(day) => days = Some(day),
                    // A value that is no integer has recorded its own issue.
                    None => {}
                }
            }
        }

        issues.finish().map(|()| Self {
            acknowledged,
            reason,
            days,
        })
    }
}

/// The longest acknowledgement reason a human may record, in code points.
///
/// A reason is a sentence or two, such as "fails on CI's Windows runner, see
/// issue 42"; 500 characters holds that with room to spare. The bound is the
/// quarantine reason's, so the two decisions a human files read the same, and a
/// longer one is refused with the recorded 422.
const ACKNOWLEDGE_REASON_MAX: usize = 500;

/// The most days an acknowledgement may last.
///
/// An acknowledgement is one person saying they own a Test for now, unlike a
/// quarantine, which parks it for as long as a fix may take. It expires unless
/// its owner renews it, so 30 days is the longest claim the API takes: anything
/// longer would be a way to stop looking at a Test, so it is refused with the
/// recorded 422.
const ACKNOWLEDGE_DAYS_MAX: i64 = 30;

/// The largest GitHub issue number a link can hold.
///
/// `github_issue_number` is a PostgreSQL `INTEGER`.
pub const MAX_ISSUE_NUMBER: i64 = 2_147_483_647;

/// A human's Issue link decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IssueLinkIn {
    /// The GitHub issue number.
    pub number: i64,
    /// The issue's url, when the caller has one.
    #[serde(default)]
    pub url: Option<String>,
}

impl IssueLinkIn {
    /// Reads an Issue link body with the lax rules this API applies.
    ///
    /// # Errors
    ///
    /// Returns every issue the body has, in field order, which the 422's `detail`
    /// list carries.
    pub fn from_value(
        object: &serde_json::Map<String, Value>,
    ) -> Result<Self, Vec<ValidationIssue>> {
        let fields = lax::Fields::new(object, body_loc_root());
        let mut issues = Vec::new();

        let mut number = 0;
        if let Some(value) = fields.required("number", &mut issues) {
            match lax::recorded(lax::integer(value, fields.loc("number")), &mut issues) {
                Some(value_number) if value_number < 1 => issues.push(
                    ValidationIssue::greater_than_equal(fields.loc("number"), value.clone(), 1),
                ),
                Some(value_number) if value_number > MAX_ISSUE_NUMBER => {
                    issues.push(ValidationIssue::less_than_equal(
                        fields.loc("number"),
                        value.clone(),
                        MAX_ISSUE_NUMBER,
                    ));
                }
                Some(value_number) => number = value_number,
                // A value that is no integer has recorded its own issue.
                None => {}
            }
        }
        let url = if let Some(text) = fields
            .field("url")
            .filter(|value| !value.is_null())
            .and_then(|value| lax::recorded(lax::string(value, fields.loc("url")), &mut issues))
        {
            lax::refuse_nul(&text, fields.loc("url"), &mut issues);
            if text.chars().count() > ISSUE_URL_MAX {
                issues.push(ValidationIssue::string_too_long(
                    fields.loc("url"),
                    Value::from(text.clone()),
                    ISSUE_URL_MAX,
                ));
            }
            Some(text)
        } else {
            None
        };

        issues.finish().map(|()| Self { number, url })
    }
}

/// The longest issue url an Issue link body may carry, in code points.
///
/// It is [`URL_MAX`]: a GitHub issue URL with a long Repo name, a query string
/// and a fragment fits inside it, and the published 422 names the field when one
/// does not. The URL is stored as `text`, so the bound is the contract's.
const ISSUE_URL_MAX: usize = URL_MAX;
