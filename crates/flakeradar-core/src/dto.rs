//! The request and response DTOs, one per model in the frozen server's
//! `schemas.py`.
//!
//! Each struct keeps that model's field names and optionality, so the JSON the
//! Rust server serves is the JSON its consumers already parse. A `None` Python
//! serialized as `null` stays `null`: no field is skipped unless Python omitted
//! it. Timestamps go through [`timestamp`] so every one follows the timestamp
//! rule.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::identity::{IdentityError, normalize_provider, normalize_repo};
use crate::model::{FailedJobOutcome, JobOutcome, JobStatus, Tier, Trend};

/// The timestamp shape every response uses.
///
/// A timestamp is `YYYY-MM-DDTHH:MM:SS`, then `.` and six fractional digits,
/// then `Z`. Fabro rejects anything else, so the fraction is always written,
/// even when it is zero.
pub mod timestamp {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    /// The `strftime` pattern every timestamp is written with.
    pub const FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.6fZ";

    /// Writes a timestamp in the one accepted shape.
    ///
    /// # Errors
    ///
    /// Returns the serializer's error when the sink rejects the string.
    pub fn serialize<S: Serializer>(
        value: &DateTime<Utc>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.format(FORMAT).to_string())
    }

    /// Reads a timestamp written as RFC 3339.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not an RFC 3339 timestamp.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<DateTime<Utc>, D::Error> {
        let text = String::deserialize(deserializer)?;
        DateTime::parse_from_rfc3339(&text)
            .map(|parsed| parsed.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)
    }
}

/// The timestamp helpers for an optional field.
pub mod optional_timestamp {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    /// Writes an optional timestamp in the one accepted shape.
    ///
    /// # Errors
    ///
    /// Returns the serializer's error when the sink rejects the string.
    pub fn serialize<S: Serializer>(
        value: &Option<DateTime<Utc>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => super::timestamp::serialize(value, serializer),
            None => serializer.serialize_none(),
        }
    }

    /// Reads an optional RFC 3339 timestamp.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not an RFC 3339 timestamp.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<DateTime<Utc>>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| {
                DateTime::parse_from_rfc3339(&text)
                    .map(|parsed| parsed.with_timezone(&Utc))
                    .map_err(serde::de::Error::custom)
            })
            .transpose()
    }
}

/// One segment of a validation error's location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum LocSegment {
    /// A field name, or the request part (`body`, `query`, `path`).
    Field(String),
    /// A position in a list.
    Index(usize),
}

/// One entry of the `detail` list a 422 response carries.
#[expect(
    clippy::derive_partial_eq_without_eq,
    reason = "serde_json::Value holds floats, so the issue cannot be Eq"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ValidationIssue {
    /// Pydantic's error kind, such as `string_too_long` or `value_error`.
    pub r#type: String,
    /// Where the bad value was.
    pub loc: Vec<LocSegment>,
    /// The message, in Pydantic's wording.
    pub msg: String,
    /// The rejected input, when Pydantic would report it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    /// The constraint that failed, when Pydantic would report it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctx: Option<Value>,
}

impl ValidationIssue {
    /// A string shorter than its minimum length.
    #[must_use]
    pub fn string_too_short(loc: Vec<LocSegment>, input: Value, limit: usize) -> Self {
        Self::length(
            "string_too_short",
            format!(
                "String should have at least {limit} character{}",
                plural(limit)
            ),
            "min_length",
            loc,
            input,
            limit,
        )
    }

    /// A string longer than its maximum length.
    #[must_use]
    pub fn string_too_long(loc: Vec<LocSegment>, input: Value, limit: usize) -> Self {
        Self::length(
            "string_too_long",
            format!(
                "String should have at most {limit} character{}",
                plural(limit)
            ),
            "max_length",
            loc,
            input,
            limit,
        )
    }

    /// A list shorter than its minimum length.
    #[must_use]
    pub fn list_too_short(loc: Vec<LocSegment>, input: Value, limit: usize) -> Self {
        let actual = input.as_u64().unwrap_or_default();
        Self::length(
            "too_short",
            format!(
                "List should have at least {limit} item{} after validation, not {actual}",
                plural(limit)
            ),
            "min_length",
            loc,
            input,
            limit,
        )
    }

    /// A list longer than its maximum length.
    #[must_use]
    pub fn list_too_long(loc: Vec<LocSegment>, input: Value, limit: usize, actual: usize) -> Self {
        Self::length(
            "too_long",
            format!(
                "List should have at most {limit} item{} after validation, not {actual}",
                plural(limit)
            ),
            "max_length",
            loc,
            input,
            limit,
        )
    }

    /// A length constraint that failed.
    fn length(
        error_type: &str,
        msg: String,
        key: &str,
        loc: Vec<LocSegment>,
        input: Value,
        limit: usize,
    ) -> Self {
        let mut ctx = serde_json::Map::new();
        let _ = ctx.insert(key.to_owned(), Value::from(limit));
        Self {
            r#type: error_type.to_owned(),
            loc,
            msg,
            input: Some(input),
            ctx: Some(Value::Object(ctx)),
        }
    }

    /// A value below its minimum.
    #[must_use]
    pub fn greater_than_equal(loc: Vec<LocSegment>, input: Value, limit: i64) -> Self {
        let mut ctx = serde_json::Map::new();
        let _ = ctx.insert("ge".to_owned(), Value::from(limit));
        Self {
            r#type: "greater_than_equal".to_owned(),
            loc,
            msg: format!("Input should be greater than or equal to {limit}"),
            input: Some(input),
            ctx: Some(Value::Object(ctx)),
        }
    }

    /// A value above its maximum.
    #[must_use]
    pub fn less_than_equal(loc: Vec<LocSegment>, input: Value, limit: i64) -> Self {
        let mut ctx = serde_json::Map::new();
        let _ = ctx.insert("le".to_owned(), Value::from(limit));
        Self {
            r#type: "less_than_equal".to_owned(),
            loc,
            msg: format!("Input should be less than or equal to {limit}"),
            input: Some(input),
            ctx: Some(Value::Object(ctx)),
        }
    }

    /// A value that is not an integer.
    #[must_use]
    pub fn int_parsing(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing(
            "int_parsing",
            "Input should be a valid integer, unable to parse string as an integer",
            loc,
            input,
        )
    }

    /// A value that is not a number.
    #[must_use]
    pub fn float_parsing(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing(
            "float_parsing",
            "Input should be a valid number, unable to parse string as a number",
            loc,
            input,
        )
    }

    /// A value that is not a boolean.
    #[must_use]
    pub fn bool_parsing(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing(
            "bool_parsing",
            "Input should be a valid boolean, unable to interpret input",
            loc,
            input,
        )
    }

    /// A value that is not a dictionary or object, where a model was expected.
    #[must_use]
    pub fn model_attributes_type(loc: Vec<LocSegment>, input: Value) -> Self {
        Self::parsing(
            "model_attributes_type",
            "Input should be a valid dictionary or object to extract fields from",
            loc,
            input,
        )
    }

    /// A required field that was not there.
    #[must_use]
    pub fn missing(loc: Vec<LocSegment>) -> Self {
        Self {
            r#type: "missing".to_owned(),
            loc,
            msg: "Field required".to_owned(),
            input: Some(Value::Null),
            ctx: None,
        }
    }

    /// A value that is not one of a fixed set.
    #[must_use]
    pub fn literal_error(loc: Vec<LocSegment>, input: Value, expected: &str) -> Self {
        let mut ctx = serde_json::Map::new();
        let _ = ctx.insert("expected".to_owned(), Value::from(expected));
        Self {
            r#type: "literal_error".to_owned(),
            loc,
            msg: format!("Input should be {expected}"),
            input: Some(input),
            ctx: Some(Value::Object(ctx)),
        }
    }

    /// A request body that is not valid JSON.
    #[must_use]
    pub fn json_invalid(loc: Vec<LocSegment>, error: &str) -> Self {
        let mut ctx = serde_json::Map::new();
        let _ = ctx.insert("error".to_owned(), Value::from(error));
        Self {
            r#type: "json_invalid".to_owned(),
            loc,
            msg: "JSON decode error".to_owned(),
            input: Some(Value::Object(serde_json::Map::new())),
            ctx: Some(Value::Object(ctx)),
        }
    }

    /// A value a field type could not parse, with Pydantic's message for it.
    fn parsing(error_type: &str, msg: &str, loc: Vec<LocSegment>, input: Value) -> Self {
        Self {
            r#type: error_type.to_owned(),
            loc,
            msg: msg.to_owned(),
            input: Some(input),
            ctx: None,
        }
    }

    /// A value a field validator rejected.
    #[must_use]
    pub fn value_error(error: &IdentityError, loc: Vec<LocSegment>, input: Value) -> Self {
        Self {
            r#type: "value_error".to_owned(),
            loc,
            msg: format!("Value error, {error}"),
            input: Some(input),
            ctx: None,
        }
    }

    /// A model-level rule that failed, such as duplicate job ids.
    #[must_use]
    pub fn model_error(message: &str) -> Self {
        Self {
            r#type: "value_error".to_owned(),
            loc: body_loc_root(),
            msg: format!("Value error, {message}"),
            input: None,
            ctx: None,
        }
    }
}

/// Where a model-level error points: the body the model was built from.
fn body_loc_root() -> Vec<LocSegment> {
    vec![LocSegment::Field("body".to_owned())]
}

/// Pydantic's singular or plural noun for a limit of `limit`.
fn plural(limit: usize) -> &'static str {
    if limit == 1 { "" } else { "s" }
}

/// The response to an accepted upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IngestAccepted {
    /// The new Report's id, or the stored duplicate's.
    pub report_id: i64,
    /// The Report's status: pending for a new one, else the stored status.
    pub status: String,
    /// Whether the upload repeated a stored Report.
    pub duplicate: bool,
}

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

impl JobResultIn {
    /// Strips the strings and checks the lengths Pydantic checked.
    ///
    /// `index` is the Job's position in the Pipeline report's `jobs` list, so
    /// each issue's `loc` points at the field a client has to fix.
    ///
    /// # Errors
    ///
    /// Returns one [`ValidationIssue`] per failed rule, in field order, which
    /// the route serves as the 422 `detail` list.
    pub fn validate(self, index: usize) -> Result<Self, Vec<ValidationIssue>> {
        let loc = |field: &str| job_loc(index, field);
        let mut issues = Vec::new();
        let ci_job_id = check_string(&self.ci_job_id, 1, 255, loc("ci_job_id"), &mut issues);
        let name = check_string(&self.name, 1, 512, loc("name"), &mut issues);
        let url = check_string(&self.url, 0, 2048, loc("url"), &mut issues);
        let runner_name = check_string(&self.runner_name, 0, 255, loc("runner_name"), &mut issues);
        let mut runner_labels = Vec::with_capacity(self.runner_labels.len());
        for (label_index, label) in self.runner_labels.into_iter().enumerate() {
            let label = stripped(&label);
            if label.chars().count() > 255 {
                issues.push(ValidationIssue::string_too_long(
                    job_loc_with(label_index, "runner_labels"),
                    Value::from(label.clone()),
                    255,
                ));
            }
            runner_labels.push(label);
        }
        if runner_labels.len() > 50 {
            issues.push(ValidationIssue::list_too_long(
                loc("runner_labels"),
                Value::from(runner_labels.len()),
                50,
                runner_labels.len(),
            ));
        }
        if !issues.is_empty() {
            return Err(issues);
        }
        Ok(Self {
            ci_job_id,
            name,
            status: self.status,
            url,
            runner_name,
            runner_labels,
            started_at: self.started_at,
            completed_at: self.completed_at,
            summary: self.summary,
        })
    }
}

/// Where one field of one Job in a Pipeline report sits.
fn job_loc(index: usize, field: &str) -> Vec<LocSegment> {
    job_loc_with(index, field)
}

/// Where one field, or one list item's field, of a Job sits.
fn job_loc_with(index: usize, field: &str) -> Vec<LocSegment> {
    vec![
        LocSegment::Field("body".to_owned()),
        LocSegment::Field("jobs".to_owned()),
        LocSegment::Index(index),
        LocSegment::Field(field.to_owned()),
    ]
}

/// Trims text the way Pydantic's `str_strip_whitespace` does.
fn stripped(text: &str) -> String {
    crate::py::py_strip(text).to_owned()
}

/// Checks a string's length bounds, returning the stripped value.
fn check_string(
    text: &str,
    min: usize,
    max: usize,
    loc: Vec<LocSegment>,
    issues: &mut Vec<ValidationIssue>,
) -> String {
    let text = stripped(text);
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
    pub ci_run_id: String,
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
    /// Strips the strings, checks Pydantic's rules and normalizes the names.
    ///
    /// # Errors
    ///
    /// Returns one [`ValidationIssue`] per failed rule, in field order, which
    /// the route serves as the 422 `detail` list.
    pub fn validate(self) -> Result<Self, Vec<ValidationIssue>> {
        let mut issues = Vec::new();

        let repo = normalized_name(&self.repo, 255, "repo", &mut issues, normalize_repo);
        let provider = normalized_name(
            &self.provider,
            32,
            "provider",
            &mut issues,
            normalize_provider,
        );

        let pipeline = check_string(&self.pipeline, 1, 512, body_loc("pipeline"), &mut issues);
        let commit_sha = check_string(&self.commit_sha, 1, 64, body_loc("commit_sha"), &mut issues);
        let branch = check_string(&self.branch, 1, 255, body_loc("branch"), &mut issues);

        let default_branch = self
            .default_branch
            .map(|branch| stripped(&branch))
            .and_then(|branch| {
                if branch.is_empty() {
                    None
                } else if branch.chars().count() > 255 {
                    issues.push(ValidationIssue::string_too_long(
                        body_loc("default_branch"),
                        Value::from(branch),
                        255,
                    ));
                    None
                } else {
                    Some(branch)
                }
            });

        let ci_run_id = stripped(&self.ci_run_id);
        if ci_run_id.chars().count() > 255 {
            issues.push(ValidationIssue::string_too_long(
                body_loc("ci_run_id"),
                Value::from(ci_run_id.clone()),
                255,
            ));
        }

        if self.ci_run_attempt < 1 {
            issues.push(ValidationIssue::greater_than_equal(
                body_loc("ci_run_attempt"),
                Value::from(self.ci_run_attempt),
                1,
            ));
        }

        if self.jobs.is_empty() {
            issues.push(ValidationIssue::list_too_short(
                body_loc("jobs"),
                Value::from(0),
                1,
            ));
        } else if self.jobs.len() > 1000 {
            issues.push(ValidationIssue::list_too_long(
                body_loc("jobs"),
                Value::from(self.jobs.len()),
                1000,
                self.jobs.len(),
            ));
        }

        let mut jobs = Vec::with_capacity(self.jobs.len());
        for (index, job) in self.jobs.into_iter().enumerate() {
            match job.validate(index) {
                Ok(job) => jobs.push(job),
                Err(job_issues) => issues.extend(job_issues),
            }
        }

        if issues.is_empty() {
            let mut seen = std::collections::BTreeSet::new();
            if !jobs.iter().all(|job| seen.insert(job.ci_job_id.clone())) {
                issues.push(ValidationIssue::model_error("duplicate ci_job_id in jobs"));
            }
        }

        if !issues.is_empty() {
            return Err(issues);
        }
        Ok(Self {
            repo,
            provider,
            pipeline,
            commit_sha,
            branch,
            default_branch,
            ci_run_id,
            ci_run_attempt: self.ci_run_attempt,
            jobs,
        })
    }
}

/// Where one field of the Pipeline report's body sits.
fn body_loc(field: &str) -> Vec<LocSegment> {
    vec![
        LocSegment::Field("body".to_owned()),
        LocSegment::Field(field.to_owned()),
    ]
}

/// Records a maximum-length failure, returning whether the value fits.
fn normalized_name(
    raw: &str,
    max: usize,
    field: &str,
    issues: &mut Vec<ValidationIssue>,
    normalize: fn(&str) -> Result<String, IdentityError>,
) -> String {
    let raw = stripped(raw);
    if raw.chars().count() > max {
        issues.push(ValidationIssue::string_too_long(
            body_loc(field),
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
                body_loc(field),
                Value::from(raw.clone()),
            ));
            raw
        }
    }
}

/// The response describing one Report.
#[expect(
    clippy::derive_partial_eq_without_eq,
    reason = "the counts payload is JSON, which holds floats, so the report cannot be Eq"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ReportOut {
    /// The Report's id.
    pub id: i64,
    /// `junit` or `pipeline`.
    pub kind: String,
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The Project's name, or `None` for a Pipeline report.
    pub project: Option<String>,
    /// The commit the Report covers.
    pub commit_sha: String,
    /// The branch the commit is on.
    pub branch: String,
    /// The CI provider's run id.
    pub ci_run_id: String,
    /// `pending`, `processed` or `failed`.
    pub status: String,
    /// Why processing failed, when it did.
    pub error: Option<String>,
    /// The Executions the Report produced, when it was processed.
    pub counts: Option<Value>,
    /// The Run the Report became, when it was processed.
    pub run_id: Option<i64>,
    /// Automatic retries spent, capped by the maximum report retries.
    pub retry_count: i64,
    /// When the Report arrived.
    #[serde(with = "timestamp")]
    pub created_at: DateTime<Utc>,
    /// When processing finished, or failed.
    #[serde(with = "optional_timestamp")]
    pub processed_at: Option<DateTime<Utc>>,
}

/// Pending and failed Report counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReportSummaryOut {
    /// Reports waiting to be processed.
    pub pending: i64,
    /// Reports that failed permanently.
    pub failed: i64,
}

/// The processor's readiness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ReadinessOut {
    /// Whether the database answers.
    pub database: bool,
    /// Whether this process holds the processor lock.
    pub processor_leader: bool,
    /// Reports waiting to be processed.
    pub pending_reports: i64,
    /// How long the oldest pending Report has waited, in seconds.
    pub oldest_pending_age_seconds: f64,
}

/// One Project inside a Repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProjectOut {
    /// The Project's name.
    pub name: String,
    /// The Project root, repo-relative; `""` at the Repo root.
    pub root: String,
}

/// One Repo and its Projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RepoOut {
    /// The Repo's `owner/name`.
    pub name: String,
    /// Every Project in the Repo.
    pub projects: Vec<ProjectOut>,
}

/// How many rows a delete removed, counted before the cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeletedCountsOut {
    /// Tests removed.
    pub tests: i64,
    /// Executions removed.
    pub executions: i64,
    /// Jobs removed.
    pub jobs: i64,
    /// Reports removed.
    pub reports: i64,
}

/// The result of deleting a Repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RepoDeleteOut {
    /// The Repo that was deleted.
    pub repo: String,
    /// How much went with it.
    pub deleted: DeletedCountsOut,
    /// Every GitHub issue number the removed rows stored.
    pub github_issue_numbers: Vec<i64>,
}

/// The result of deleting a Project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProjectDeleteOut {
    /// The Project's Repo.
    pub repo: String,
    /// The Project that was deleted.
    pub project: String,
    /// How much went with it.
    pub deleted: DeletedCountsOut,
    /// Every GitHub issue number the removed Tests stored.
    pub github_issue_numbers: Vec<i64>,
}

/// One Test as the leaderboard serves it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TestOut {
    /// The Test's id.
    pub id: i64,
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The Project's name.
    pub project: String,
    /// The Test's stable identity hash.
    pub fingerprint: String,
    /// The suite the runner reported.
    pub suite: String,
    /// The classname the runner reported.
    pub classname: String,
    /// The Test's name within its classname.
    pub name: String,
    /// The file the Test is defined in, when reported.
    pub file: Option<String>,
    /// The line the Test is defined on, when reported.
    pub line: Option<i64>,
    /// The Flakiness score.
    pub flakiness_score: f64,
    /// The tier the score falls in.
    pub tier: Tier,
    /// How many commit SHAs recorded both outcomes.
    pub confirmed_flake_count: i64,
    /// The most common Failure category among recent failures.
    pub failure_category: Option<String>,
    /// Non-skipped Executions since the newest failure.
    pub clean_streak: i64,
    /// Consecutive failures, newest first.
    pub fail_streak: i64,
    /// Whether the Fail streak reached the broken threshold.
    pub broken: bool,
    /// The score's direction over the trend window.
    #[serde(default)]
    pub trend: Option<Trend>,
    /// The newest Execution's status.
    pub last_status: String,
    /// When the Test last ran.
    #[serde(with = "timestamp")]
    pub last_seen_at: DateTime<Utc>,
    /// Whether the Test is quarantined.
    pub quarantined: bool,
    /// When the quarantine started.
    #[serde(with = "optional_timestamp")]
    pub quarantined_at: Option<DateTime<Utc>>,
    /// Why the Test was quarantined.
    pub quarantine_reason: Option<String>,
    /// When the quarantine lapses.
    #[serde(with = "optional_timestamp")]
    pub quarantine_expires_at: Option<DateTime<Utc>>,
    /// The GitHub issue the Test is linked to.
    pub github_issue_number: Option<i64>,
    /// The linked issue's url.
    pub github_issue_url: Option<String>,
    /// Whether the linked issue was closed as not planned.
    pub github_issue_closed: bool,
}

/// One page of Tests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TestPage {
    /// The Tests on this page.
    pub items: Vec<TestOut>,
    /// How many Tests the filter matched.
    pub total: i64,
    /// The page number, numbered from 1.
    pub page: i64,
    /// How many items a page holds.
    pub page_size: i64,
}

/// The dashboard's headline counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SummaryOut {
    /// How many Tests exist.
    pub total_tests: i64,
    /// How many are at or above the Flake threshold.
    pub flaky_tests: i64,
    /// How many are above zero but below the threshold.
    pub suspect_tests: i64,
    /// How many flew at least one Proven flake.
    pub confirmed_flaky_tests: i64,
    /// How many became flaky in the last seven days.
    pub new_flaky_7d: i64,
    /// How many Runs exist.
    pub total_runs: i64,
    /// Executions in the last thirty days.
    pub executions_30d: i64,
    /// Failures in the last thirty days.
    pub failures_30d: i64,
    /// The configured Flake threshold.
    pub flake_threshold: f64,
    /// Failing Executions per Failure category.
    pub category_counts: BTreeMap<String, i64>,
}

/// One Execution in a Test's history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ExecutionOut {
    /// The Execution's id.
    pub id: i64,
    /// What the Execution did.
    pub status: String,
    /// How long it took, in seconds.
    pub duration: f64,
    /// The Failure message.
    pub message: String,
    /// The Failure details.
    pub details: String,
    /// When the Execution was recorded.
    #[serde(with = "timestamp")]
    pub created_at: DateTime<Utc>,
    /// The commit the Execution ran.
    pub commit_sha: String,
    /// The branch the commit is on.
    pub branch: String,
    /// The CI provider's run id.
    pub ci_run_id: String,
    /// Which retry inside the Run this is, numbered from 0.
    pub attempt: i64,
    /// The Failure category, when the Execution failed.
    pub failure_category: Option<String>,
}

/// Where a Test is defined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LocationOut {
    /// The file joined onto the Project root, repo-relative.
    pub path: String,
    /// The line the Test is defined on, when reported.
    pub line: Option<i64>,
    /// A GitHub permalink at the last failing commit, when one exists.
    pub url: Option<String>,
}

/// One Job a Test has run in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TestJobLinkOut {
    /// The Job's id.
    pub job_id: i64,
    /// The Job's Pipeline.
    pub pipeline: String,
    /// The Job's name.
    pub name: String,
}

/// One day of a Test's or Job's Score history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScorePointOut {
    /// The day the snapshot was taken.
    pub day: NaiveDate,
    /// The Flakiness score at the end of that day.
    pub flakiness_score: f64,
    /// The Proven flake count at the end of that day.
    pub confirmed_flake_count: i64,
    /// Executions that day.
    pub executions: i64,
    /// Failures that day.
    pub failures: i64,
}

/// A Test's detail page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HistoryOut {
    /// The Test itself.
    pub test: TestOut,
    /// Where the Test is defined, when the runner reported a file.
    pub location: Option<LocationOut>,
    /// The commit of the newest failing Execution.
    pub last_failing_sha: Option<String>,
    /// The branch of the newest failing Execution.
    pub last_failing_branch: Option<String>,
    /// The newest Executions first.
    pub executions: Vec<ExecutionOut>,
    /// The Jobs whose Executions came from these Runs.
    pub jobs: Vec<TestJobLinkOut>,
    /// The last ninety days, oldest first.
    pub score_history: Vec<ScorePointOut>,
}

/// One Job as the leaderboard serves it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JobOut {
    /// The Job's id.
    pub id: i64,
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The CI provider's label.
    pub provider: String,
    /// The Job's Pipeline.
    pub pipeline: String,
    /// The Job's name.
    pub name: String,
    /// The Flakiness score.
    pub flakiness_score: f64,
    /// The tier the score falls in.
    pub tier: Tier,
    /// How many commit SHAs recorded both outcomes.
    pub confirmed_flake_count: i64,
    /// Non-skipped Executions since the newest failure.
    pub clean_streak: i64,
    /// The score's direction over the trend window.
    #[serde(default)]
    pub trend: Option<Trend>,
    /// The newest Job execution's status.
    pub last_status: String,
    /// When the Job last ran.
    #[serde(with = "timestamp")]
    pub last_seen_at: DateTime<Utc>,
    /// Whether the Job is a Summary Job.
    pub summary: bool,
    /// The GitHub issue the Job is linked to.
    pub github_issue_number: Option<i64>,
    /// The linked issue's url, when the Pipeline is on GitHub.
    pub github_issue_url: Option<String>,
    /// Whether the linked issue was closed as not planned.
    pub github_issue_closed: bool,
}

/// One page of Jobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JobPage {
    /// The Jobs on this page.
    pub items: Vec<JobOut>,
    /// How many Jobs the filter matched.
    pub total: i64,
    /// The page number, numbered from 1.
    pub page: i64,
    /// How many items a page holds.
    pub page_size: i64,
}

/// One Test execution that explains a failed Job execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ExplainingTestOut {
    /// The Test's id.
    pub test_id: i64,
    /// The Test's Project.
    pub project: String,
    /// The Test's classname.
    pub classname: String,
    /// The Test's name.
    pub name: String,
    /// The failing status: `failed` or `error`.
    pub status: String,
}

/// One sibling Job that explains a Summary Job's failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ExplainingJobOut {
    /// The sibling Job's id.
    pub job_id: i64,
    /// The sibling Job's Pipeline.
    pub pipeline: String,
    /// The sibling Job's name.
    pub name: String,
    /// The sibling Execution's CI url.
    pub url: String,
}

/// One Job execution in a Job's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct JobExecutionOut {
    /// The Job execution's id.
    pub id: i64,
    /// The stored status.
    pub status: String,
    /// What the execution counts as once Explained failures are attributed.
    pub outcome: JobOutcome,
    /// The commit the execution ran.
    pub commit_sha: String,
    /// The branch the commit is on.
    pub branch: String,
    /// The CI provider's run id.
    pub ci_run_id: String,
    /// Which attempt of the CI run this was.
    pub ci_run_attempt: i64,
    /// The CI provider's Job id.
    pub ci_job_id: String,
    /// The Job's CI url.
    pub url: String,
    /// The runner's name.
    pub runner_name: String,
    /// The runner's labels.
    pub runner_labels: Vec<String>,
    /// When the Job started.
    #[serde(with = "optional_timestamp")]
    pub started_at: Option<DateTime<Utc>>,
    /// When the Job finished.
    #[serde(with = "optional_timestamp")]
    pub completed_at: Option<DateTime<Utc>>,
    /// When the execution was recorded.
    #[serde(with = "timestamp")]
    pub created_at: DateTime<Utc>,
    /// The failing Tests that explain it; empty otherwise.
    pub explained_by: Vec<ExplainingTestOut>,
    /// The failed sibling Jobs that explain a Summary Job; empty otherwise.
    pub explained_by_jobs: Vec<ExplainingJobOut>,
}

/// One runner (or label set) a Job ran on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RunnerStatOut {
    /// The sorted label set.
    pub labels: Vec<String>,
    /// The runner's name, when it ran more than once.
    pub runner_name: Option<String>,
    /// How many executions.
    pub executions: i64,
    /// How many failures nothing explained.
    pub unexplained_failures: i64,
}

/// A Job's detail page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JobHistoryOut {
    /// The Job itself.
    pub job: JobOut,
    /// Unexplained failures among the returned executions.
    pub unexplained_failures: i64,
    /// Explained failures among the returned executions.
    pub explained_failures: i64,
    /// The newest executions first.
    pub executions: Vec<JobExecutionOut>,
    /// Per-runner counts over the newest score window, worst first.
    pub runners: Vec<RunnerStatOut>,
    /// The last ninety days, oldest first.
    pub score_history: Vec<ScorePointOut>,
}

/// The Jobs dashboard's headline counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JobSummaryOut {
    /// How many Jobs exist.
    pub total_jobs: i64,
    /// How many are at or above the Flake threshold.
    pub flaky_jobs: i64,
    /// How many are above zero but below the threshold.
    pub suspect_jobs: i64,
    /// How many flew at least one Proven flake.
    pub confirmed_flaky_jobs: i64,
    /// How many became flaky in the last seven days.
    pub new_flaky_7d: i64,
    /// Job executions in the last thirty days.
    pub job_executions_30d: i64,
    /// Job failures in the last thirty days.
    pub job_failures_30d: i64,
    /// The configured Flake threshold.
    pub flake_threshold: f64,
    /// Every Pipeline that has a Job.
    pub pipelines: Vec<String>,
}

/// A human's quarantine decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct QuarantineIn {
    /// Whether the Test is quarantined.
    pub quarantined: bool,
    /// Why the human quarantined it.
    #[serde(default)]
    pub reason: Option<String>,
    /// How many days the quarantine lasts.
    #[serde(default)]
    pub expires_in_days: Option<i64>,
}

/// One quarantined Test in the quarantine list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct QuarantineItem {
    /// The suite the runner reported.
    pub suite: String,
    /// The classname the runner reported.
    pub classname: String,
    /// The Test's name.
    pub name: String,
    /// The Test's identity hash.
    pub fingerprint: String,
    /// The file the Test is defined in.
    pub file: Option<String>,
    /// The line the Test is defined on.
    pub line: Option<i64>,
    /// When the quarantine started.
    #[serde(with = "optional_timestamp")]
    pub quarantined_at: Option<DateTime<Utc>>,
    /// Why the Test was quarantined.
    pub reason: Option<String>,
    /// When the quarantine lapses.
    #[serde(with = "optional_timestamp")]
    pub expires_at: Option<DateTime<Utc>>,
}

/// Report counts for one CI run attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReportCountsOut {
    /// Reports still waiting.
    pub pending: i64,
    /// Reports processed.
    pub processed: i64,
    /// Reports that failed.
    pub failed: i64,
}

/// One failing Test in a CI run attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CiRunTestFailureOut {
    /// The Test's id.
    pub test_id: i64,
    /// The Test's Project.
    pub project: String,
    /// The suite the runner reported.
    pub suite: String,
    /// The classname the runner reported.
    pub classname: String,
    /// The Test's name.
    pub name: String,
    /// The Run the failure came from.
    pub run_id: i64,
    /// The CI provider's Job id.
    pub ci_job_id: Option<String>,
    /// The Test's newest status in this Run.
    pub status: String,
    /// How many failing Executions this Run had.
    pub failed_attempts: i64,
    /// The last failing Execution's category.
    pub failure_category: Option<String>,
    /// The last failing Execution's message, first 1024 characters.
    pub message: String,
    /// The tier the score falls in.
    pub tier: Tier,
    /// The Flakiness score.
    pub flakiness_score: f64,
    /// How many commit SHAs recorded both outcomes.
    pub confirmed_flake_count: i64,
    /// Non-skipped Executions since the newest failure.
    pub clean_streak: i64,
    /// Consecutive failures, newest first.
    pub fail_streak: i64,
    /// Whether the Fail streak reached the broken threshold.
    pub broken: bool,
    /// Whether the Test is quarantined.
    pub quarantined: bool,
    /// The GitHub issue the Test is linked to.
    pub github_issue_number: Option<i64>,
    /// The linked issue's url.
    pub github_issue_url: Option<String>,
    /// Whether the linked issue was closed as not planned.
    pub github_issue_closed: bool,
}

/// One failed Job in a CI run attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CiRunFailedJobOut {
    /// The Job's id.
    pub job_id: i64,
    /// The Job's Pipeline.
    pub pipeline: String,
    /// The Job's name.
    pub name: String,
    /// The CI provider's Job id.
    pub ci_job_id: String,
    /// The Job's CI url.
    pub url: String,
    /// Whether something explains the failure.
    pub outcome: FailedJobOutcome,
    /// Whether the Job is a Summary Job.
    pub summary: bool,
    /// The tier the score falls in.
    pub tier: Tier,
    /// The Flakiness score.
    pub flakiness_score: f64,
    /// How many commit SHAs recorded both outcomes.
    pub confirmed_flake_count: i64,
    /// Non-skipped Executions since the newest failure.
    pub clean_streak: i64,
    /// The GitHub issue the Job is linked to.
    pub github_issue_number: Option<i64>,
    /// The linked issue's url, when the Pipeline is on GitHub.
    pub github_issue_url: Option<String>,
    /// Whether the linked issue was closed as not planned.
    pub github_issue_closed: bool,
}

/// Every failure in one CI run attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CiRunFailuresOut {
    /// The Repo's `owner/name`.
    pub repo: String,
    /// The CI provider's run id.
    pub ci_run_id: String,
    /// Which attempt this is.
    pub ci_run_attempt: i64,
    /// The configured Flake threshold.
    pub flake_threshold: f64,
    /// Report counts for both kinds, this attempt only.
    pub reports: ReportCountsOut,
    /// How many (Test, Run) pairs there were before the cap.
    pub total_failures: i64,
    /// Whether the failure list was capped.
    pub truncated: bool,
    /// The failing Tests.
    pub failures: Vec<CiRunTestFailureOut>,
    /// The failed Jobs.
    pub failed_jobs: Vec<CiRunFailedJobOut>,
}

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
