//! Row builders with the documented fixture defaults.
//!
//! Each `make_*` function returns a builder whose defaults are the ones a row
//! written by the server would have, so a test states only the fields it cares
//! about. Building flushes nothing and opens no transaction: it inserts the rows
//! the test asked for on the connection it was given.

// Rust guideline compliant 2026-07-21

use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use flakeradar_db::entities::{
    job_executions, job_score_history, jobs, pipelines, projects, reports, repos, test_cases,
    test_executions, test_runs, test_score_history,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, IntoActiveModel,
    JsonValue, QueryFilter, Set, entity::prelude::DateTimeWithTimeZone,
};

use crate::db::hex;
use crate::error::TestError;

/// The time a row is stamped with when a builder is not given one.
///
/// A fixture's age is never what a test asserts, and a row stamped with the wall
/// clock makes a suite whose rows differ from run to run. This instant is fixed,
/// so two runs build the same rows; a test that cares when a row was written
/// says so with the builder's own setter.
fn now() -> DateTimeWithTimeZone {
    FIXTURE_INSTANT.fixed_offset()
}

/// The instant every fixture row is stamped with unless it is told otherwise:
/// 2020-09-13T12:26:40Z, which no test treats as special and every run can
/// therefore repeat.
pub const FIXTURE_INSTANT: DateTime<Utc> = DateTime::from_timestamp(1_600_000_000, 0)
    .expect("the fixture instant is a time the calendar has")
    .to_utc();

/// Inserts `active` and returns the stored row.
async fn insert<A, E>(db: &impl ConnectionTrait, active: A) -> Result<E::Model, TestError>
where
    A: ActiveModelTrait<Entity = E> + Send,
    E: EntityTrait,
    E::Model: IntoActiveModel<A>,
{
    Ok(E::insert(active).exec_with_returning(db).await?)
}

/// Builder for a Repo and a Project inside it.
#[derive(Debug)]
pub struct ProjectBuilder {
    repo: String,
    project: String,
    root: String,
}

/// Builds a Project named `default` in the Repo `acme/app`.
#[must_use]
pub fn make_project() -> ProjectBuilder {
    ProjectBuilder {
        repo: "acme/app".to_owned(),
        project: "default".to_owned(),
        root: String::new(),
    }
}

impl ProjectBuilder {
    /// Sets the Repo's `owner/name`, creating the Repo when it is new.
    #[must_use]
    pub fn repo(mut self, repo: &str) -> Self {
        repo.clone_into(&mut self.repo);
        self
    }

    /// Sets the Project's name within its Repo.
    #[must_use]
    pub fn project(mut self, project: &str) -> Self {
        project.clone_into(&mut self.project);
        self
    }

    /// Sets the Project root that reported file paths are relative to.
    #[must_use]
    pub fn root(mut self, root: &str) -> Self {
        root.clone_into(&mut self.root);
        self
    }

    /// Inserts the Project, reusing the Repo when it already exists.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Database`] when the insert violates a constraint.
    pub async fn build(self, db: &impl ConnectionTrait) -> Result<projects::Model, TestError> {
        let repo = get_or_create_repo(db, &self.repo).await?;
        insert(
            db,
            projects::ActiveModel {
                repo_id: Set(repo.id),
                name: Set(self.project),
                root: Set(self.root),
                created_at: Set(now()),
                ..Default::default()
            },
        )
        .await
    }
}

/// The Repo row named `name`, inserted when the database has none.
async fn get_or_create_repo(
    db: &impl ConnectionTrait,
    name: &str,
) -> Result<repos::Model, TestError> {
    if let Some(repo) = repos::Entity::find()
        .filter(repos::Column::Name.eq(name))
        .one(db)
        .await?
    {
        return Ok(repo);
    }
    insert(
        db,
        repos::ActiveModel {
            name: Set(name.to_owned()),
            created_at: Set(now()),
            ..Default::default()
        },
    )
    .await
}

/// Builder for a Test inside one Project.
#[derive(Debug)]
pub struct TestCaseBuilder {
    project_id: i32,
    name: String,
    classname: String,
    suite: String,
    fingerprint: Option<String>,
    flakiness_score: f64,
    confirmed_flake_count: i32,
    clean_streak: i32,
    fail_streak: i32,
    file: Option<String>,
    line: Option<i32>,
    failure_category: Option<String>,
    quarantined: bool,
    last_seen_at: Option<DateTimeWithTimeZone>,
    github_issue_fixed_noted_at: Option<DateTimeWithTimeZone>,
    github_issue_number: Option<i32>,
    quarantined_at: Option<DateTimeWithTimeZone>,
    quarantine_reason: Option<String>,
    quarantine_expires_at: Option<DateTimeWithTimeZone>,
}

/// Builds a Test named `t1` of the suite `unit`.
#[must_use]
pub fn make_test_case(project: &projects::Model) -> TestCaseBuilder {
    TestCaseBuilder {
        project_id: project.id,
        name: "t1".to_owned(),
        classname: "tests.test_mod".to_owned(),
        suite: "unit".to_owned(),
        fingerprint: None,
        flakiness_score: 0.0,
        confirmed_flake_count: 0,
        clean_streak: 0,
        fail_streak: 0,
        file: None,
        line: None,
        failure_category: None,
        quarantined: false,
        last_seen_at: None,
        github_issue_fixed_noted_at: None,
        github_issue_number: None,
        quarantined_at: None,
        quarantine_reason: None,
        quarantine_expires_at: None,
    }
}

impl TestCaseBuilder {
    /// Sets the Test's name, which also changes its fingerprint.
    #[must_use]
    pub fn name(mut self, name: &str) -> Self {
        name.clone_into(&mut self.name);
        self
    }

    /// Sets the classname half of the Test's identity.
    #[must_use]
    pub fn classname(mut self, classname: &str) -> Self {
        classname.clone_into(&mut self.classname);
        self
    }

    /// Sets the suite half of the Test's identity.
    #[must_use]
    pub fn suite(mut self, suite: &str) -> Self {
        suite.clone_into(&mut self.suite);
        self
    }

    /// Sets the Test's cached Flakiness score.
    #[must_use]
    pub fn flakiness_score(mut self, score: f64) -> Self {
        self.flakiness_score = score;
        self
    }

    /// Sets the Test's fingerprint instead of deriving it from its identity.
    #[must_use]
    pub fn fingerprint(mut self, fingerprint: &str) -> Self {
        self.fingerprint = Some(fingerprint.to_owned());
        self
    }

    /// Sets the Test's Clean streak.
    #[must_use]
    pub fn clean_streak(mut self, clean_streak: i32) -> Self {
        self.clean_streak = clean_streak;
        self
    }

    /// Sets when the Test last ran.
    #[must_use]
    pub fn last_seen_at(mut self, at: DateTimeWithTimeZone) -> Self {
        self.last_seen_at = Some(at);
        self
    }

    /// Sets the Test's Proven flake count.
    #[must_use]
    pub fn confirmed_flake_count(mut self, count: i32) -> Self {
        self.confirmed_flake_count = count;
        self
    }

    /// Sets the Test's Fail streak.
    #[must_use]
    pub fn fail_streak(mut self, fail_streak: i32) -> Self {
        self.fail_streak = fail_streak;
        self
    }

    /// Sets the file the runner reported the Test in.
    #[must_use]
    pub fn file(mut self, file: &str) -> Self {
        self.file = Some(file.to_owned());
        self
    }

    /// Sets the line the runner reported the Test on.
    #[must_use]
    pub fn line(mut self, line: i32) -> Self {
        self.line = Some(line);
        self
    }

    /// Sets the Test's Failure category.
    #[must_use]
    pub fn failure_category(mut self, category: &str) -> Self {
        self.failure_category = Some(category.to_owned());
        self
    }

    /// Quarantines the Test without a reason or an expiry.
    #[must_use]
    pub fn quarantined(mut self) -> Self {
        self.quarantined = true;
        self
    }

    /// Sets when FlakeRadar noted that the Test's fix appears to hold.
    #[must_use]
    pub fn github_issue_fixed_noted_at(mut self, at: DateTimeWithTimeZone) -> Self {
        self.github_issue_fixed_noted_at = Some(at);
        self
    }

    /// Links the Test to a GitHub issue number.
    #[must_use]
    pub fn github_issue_number(mut self, number: i32) -> Self {
        self.github_issue_number = Some(number);
        self
    }

    /// Quarantines the Test at `at`. Any quarantine field implies the quarantine.
    #[must_use]
    pub fn quarantined_at(mut self, at: DateTimeWithTimeZone) -> Self {
        self.quarantined_at = Some(at);
        self
    }

    /// Records why a person quarantined the Test.
    #[must_use]
    pub fn quarantine_reason(mut self, reason: &str) -> Self {
        self.quarantine_reason = Some(reason.to_owned());
        self
    }

    /// Sets when the quarantine clears on its own.
    #[must_use]
    pub fn quarantine_expires_at(mut self, at: DateTimeWithTimeZone) -> Self {
        self.quarantine_expires_at = Some(at);
        self
    }

    /// Inserts the Test.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Database`] when the insert violates a constraint.
    pub async fn build(self, db: &impl ConnectionTrait) -> Result<test_cases::Model, TestError> {
        let quarantined = self.quarantined
            || self.quarantined_at.is_some()
            || self.quarantine_reason.is_some()
            || self.quarantine_expires_at.is_some();
        let fingerprint = self
            .fingerprint
            .unwrap_or_else(|| fingerprint(&self.suite, &self.classname, &self.name));
        insert(
            db,
            test_cases::ActiveModel {
                project_id: Set(self.project_id),
                fingerprint: Set(fingerprint),
                suite: Set(self.suite),
                classname: Set(self.classname),
                name: Set(self.name),
                flakiness_score: Set(self.flakiness_score),
                confirmed_flake_count: Set(self.confirmed_flake_count),
                failure_category: Set(self.failure_category),
                clean_streak: Set(self.clean_streak),
                fail_streak: Set(self.fail_streak),
                file: Set(self.file),
                line: Set(self.line),
                last_status: Set("passed".to_owned()),
                last_seen_at: Set(self.last_seen_at.unwrap_or_else(now)),
                quarantined: Set(quarantined),
                quarantined_at: Set(self.quarantined_at),
                quarantine_reason: Set(self.quarantine_reason),
                quarantine_expires_at: Set(self.quarantine_expires_at),
                github_issue_number: Set(self.github_issue_number),
                github_issue_filed: Set(None),
                github_issue_checked_at: Set(None),
                github_issue_closed: Set(false),
                github_issue_fixed_noted_at: Set(self.github_issue_fixed_noted_at),
                ..Default::default()
            },
        )
        .await
    }
}

/// The Test fingerprint, computed independently of the server's own copy.
///
/// The factory keeps its own copy rather than calling the server's, so a bug in
/// the server's fingerprint shows up as a failing test instead of a fixture that
/// agrees with it.
fn fingerprint(suite: &str, classname: &str, name: &str) -> String {
    use sha1::{Digest, Sha1};

    hex(Sha1::digest(
        format!("{suite}::{classname}::{name}").as_bytes(),
    ))
}

/// Builder for a Run of one Project at one commit SHA.
#[derive(Debug)]
pub struct RunBuilder {
    project_id: i32,
    commit_sha: String,
    branch: String,
    ci_run_id: String,
    ci_job_id: Option<String>,
    ci_run_attempt: Option<i32>,
    pipeline: Option<String>,
    created_at: Option<DateTimeWithTimeZone>,
}

/// Builds a Run at commit `sha1` on branch `main`.
#[must_use]
pub fn make_run(project: &projects::Model) -> RunBuilder {
    RunBuilder {
        project_id: project.id,
        commit_sha: "sha1".to_owned(),
        branch: "main".to_owned(),
        ci_run_id: String::new(),
        ci_job_id: None,
        ci_run_attempt: None,
        pipeline: None,
        created_at: None,
    }
}

impl RunBuilder {
    /// Sets the Run's commit SHA.
    #[must_use]
    pub fn commit_sha(mut self, commit_sha: &str) -> Self {
        commit_sha.clone_into(&mut self.commit_sha);
        self
    }

    /// Sets the Run's branch.
    #[must_use]
    pub fn branch(mut self, branch: &str) -> Self {
        branch.clone_into(&mut self.branch);
        self
    }

    /// Sets the Run's CI run ID.
    #[must_use]
    pub fn ci_run_id(mut self, ci_run_id: &str) -> Self {
        ci_run_id.clone_into(&mut self.ci_run_id);
        self
    }

    /// Sets the Job execution that produced the Run.
    #[must_use]
    pub fn ci_job_id(mut self, ci_job_id: &str) -> Self {
        self.ci_job_id = Some(ci_job_id.to_owned());
        self
    }

    /// Sets the CI run attempt the Run came from.
    #[must_use]
    pub fn ci_run_attempt(mut self, ci_run_attempt: i32) -> Self {
        self.ci_run_attempt = Some(ci_run_attempt);
        self
    }

    /// Sets the Pipeline the Run came from.
    #[must_use]
    pub fn pipeline(mut self, pipeline: &str) -> Self {
        self.pipeline = Some(pipeline.to_owned());
        self
    }

    /// Sets when the Run was created.
    #[must_use]
    pub fn created_at(mut self, created_at: DateTime<FixedOffset>) -> Self {
        self.created_at = Some(created_at);
        self
    }

    /// Inserts the Run.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Database`] when the insert violates a constraint.
    pub async fn build(self, db: &impl ConnectionTrait) -> Result<test_runs::Model, TestError> {
        insert(
            db,
            test_runs::ActiveModel {
                project_id: Set(self.project_id),
                commit_sha: Set(self.commit_sha),
                branch: Set(self.branch),
                ci_run_id: Set(self.ci_run_id),
                ci_job_id: Set(self.ci_job_id),
                ci_run_attempt: Set(self.ci_run_attempt),
                pipeline: Set(self.pipeline),
                created_at: Set(self.created_at.unwrap_or_else(now)),
                ..Default::default()
            },
        )
        .await
    }
}

/// Builder for one Execution of a Test.
#[derive(Debug)]
pub struct TestExecutionBuilder {
    test_case_id: i32,
    test_run_id: i32,
    status: String,
    message: String,
    details: String,
    attempt: i32,
    failure_category: Option<String>,
    created_at: Option<DateTimeWithTimeZone>,
}

/// Builds a passing Execution of `test_case` in `run`, attempt 0.
#[must_use]
pub fn make_execution(
    test_case: &test_cases::Model,
    run: &test_runs::Model,
) -> TestExecutionBuilder {
    TestExecutionBuilder {
        test_case_id: test_case.id,
        test_run_id: run.id,
        status: "passed".to_owned(),
        message: String::new(),
        details: String::new(),
        attempt: 0,
        failure_category: None,
        created_at: None,
    }
}

impl TestExecutionBuilder {
    /// Sets the Execution's outcome: `passed`, `failed`, `error` or `skipped`.
    #[must_use]
    pub fn status(mut self, status: &str) -> Self {
        status.clone_into(&mut self.status);
        self
    }

    /// Sets the Execution's Failure message.
    #[must_use]
    pub fn message(mut self, message: &str) -> Self {
        message.clone_into(&mut self.message);
        self
    }

    /// Sets the Execution's Failure details.
    #[must_use]
    pub fn details(mut self, details: &str) -> Self {
        details.clone_into(&mut self.details);
        self
    }

    /// Sets the retry number, counted from 0 inside one Run.
    #[must_use]
    pub fn attempt(mut self, attempt: i32) -> Self {
        self.attempt = attempt;
        self
    }

    /// Sets the Execution's Failure category.
    #[must_use]
    pub fn failure_category(mut self, failure_category: &str) -> Self {
        self.failure_category = Some(failure_category.to_owned());
        self
    }

    /// Sets when the Execution ran.
    #[must_use]
    pub fn created_at(mut self, created_at: DateTime<FixedOffset>) -> Self {
        self.created_at = Some(created_at);
        self
    }

    /// Inserts the Execution.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Database`] when the insert violates a constraint.
    pub async fn build(
        self,
        db: &impl ConnectionTrait,
    ) -> Result<test_executions::Model, TestError> {
        insert(
            db,
            test_executions::ActiveModel {
                test_case_id: Set(self.test_case_id),
                test_run_id: Set(self.test_run_id),
                status: Set(self.status),
                duration: Set(0.0),
                message: Set(self.message),
                details: Set(self.details),
                attempt: Set(self.attempt),
                failure_category: Set(self.failure_category),
                created_at: Set(self.created_at.unwrap_or_else(now)),
                ..Default::default()
            },
        )
        .await
    }
}

/// Builder for a Pipeline of a Repo.
#[derive(Debug)]
pub struct PipelineBuilder {
    repo: String,
    provider: String,
    name: String,
}

/// Builds the GitHub Actions Pipeline `.github/workflows/ci.yml` of `acme/app`.
#[must_use]
pub fn make_pipeline() -> PipelineBuilder {
    PipelineBuilder {
        repo: "acme/app".to_owned(),
        provider: "github".to_owned(),
        name: ".github/workflows/ci.yml".to_owned(),
    }
}

impl PipelineBuilder {
    /// Sets the Repo the Pipeline belongs to, creating the Repo when it is new.
    #[must_use]
    pub fn repo(mut self, repo: &str) -> Self {
        repo.clone_into(&mut self.repo);
        self
    }

    /// Sets the CI provider that names the Pipeline.
    #[must_use]
    pub fn provider(mut self, provider: &str) -> Self {
        provider.clone_into(&mut self.provider);
        self
    }

    /// Sets the Pipeline's name within its Repo.
    #[must_use]
    pub fn name(mut self, name: &str) -> Self {
        name.clone_into(&mut self.name);
        self
    }

    /// Inserts the Pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Database`] when the insert violates a constraint.
    pub async fn build(self, db: &impl ConnectionTrait) -> Result<pipelines::Model, TestError> {
        let repo = get_or_create_repo(db, &self.repo).await?;
        insert(
            db,
            pipelines::ActiveModel {
                repo_id: Set(repo.id),
                provider: Set(self.provider),
                name: Set(self.name),
                created_at: Set(now()),
                ..Default::default()
            },
        )
        .await
    }
}

/// Builder for a Job of a Pipeline.
#[derive(Debug)]
pub struct JobBuilder {
    pipeline_id: i32,
    name: String,
    summary: bool,
    flakiness_score: f64,
    confirmed_flake_count: i32,
    clean_streak: i32,
    last_seen_at: Option<DateTimeWithTimeZone>,
    github_issue_fixed_noted_at: Option<DateTimeWithTimeZone>,
    github_issue_number: Option<i32>,
}

/// Builds a non-summary Job named `test`.
#[must_use]
pub fn make_job(pipeline: &pipelines::Model) -> JobBuilder {
    JobBuilder {
        pipeline_id: pipeline.id,
        name: "test".to_owned(),
        summary: false,
        flakiness_score: 0.0,
        confirmed_flake_count: 0,
        clean_streak: 0,
        last_seen_at: None,
        github_issue_fixed_noted_at: None,
        github_issue_number: None,
    }
}

impl JobBuilder {
    /// Sets the Job's name within its Pipeline.
    #[must_use]
    pub fn name(mut self, name: &str) -> Self {
        name.clone_into(&mut self.name);
        self
    }

    /// Sets the Job's cached Flakiness score.
    #[must_use]
    pub fn flakiness_score(mut self, score: f64) -> Self {
        self.flakiness_score = score;
        self
    }

    /// Sets the Job's Proven flake count.
    #[must_use]
    pub fn confirmed_flake_count(mut self, count: i32) -> Self {
        self.confirmed_flake_count = count;
        self
    }

    /// Sets the Job's Clean streak.
    #[must_use]
    pub fn clean_streak(mut self, clean_streak: i32) -> Self {
        self.clean_streak = clean_streak;
        self
    }

    /// Sets when FlakeRadar noted that the Job's fix appears to hold.
    #[must_use]
    pub fn github_issue_fixed_noted_at(mut self, at: DateTimeWithTimeZone) -> Self {
        self.github_issue_fixed_noted_at = Some(at);
        self
    }

    /// Marks the Job as a Summary Job.
    #[must_use]
    pub fn summary(mut self, summary: bool) -> Self {
        self.summary = summary;
        self
    }

    /// Links the Job to a GitHub issue number.
    #[must_use]
    pub fn github_issue_number(mut self, number: i32) -> Self {
        self.github_issue_number = Some(number);
        self
    }

    /// Inserts the Job.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Database`] when the insert violates a constraint.
    pub async fn build(self, db: &impl ConnectionTrait) -> Result<jobs::Model, TestError> {
        insert(
            db,
            jobs::ActiveModel {
                pipeline_id: Set(self.pipeline_id),
                name: Set(self.name),
                summary: Set(self.summary),
                flakiness_score: Set(self.flakiness_score),
                confirmed_flake_count: Set(self.confirmed_flake_count),
                clean_streak: Set(self.clean_streak),
                last_status: Set("passed".to_owned()),
                last_seen_at: Set(self.last_seen_at.unwrap_or_else(now)),
                github_issue_number: Set(self.github_issue_number),
                github_issue_filed: Set(None),
                github_issue_checked_at: Set(None),
                github_issue_closed: Set(false),
                github_issue_fixed_noted_at: Set(self.github_issue_fixed_noted_at),
                created_at: Set(now()),
                ..Default::default()
            },
        )
        .await
    }
}

/// Builder for one Job execution.
#[derive(Debug)]
pub struct JobExecutionBuilder {
    job_id: i32,
    status: String,
    ci_job_id: Option<String>,
    commit_sha: String,
    branch: String,
    ci_run_id: String,
    ci_run_attempt: i32,
    url: String,
    runner_name: String,
    runner_labels: Vec<String>,
    created_at: Option<DateTimeWithTimeZone>,
}

/// Builds a passing Job execution with a fresh CI job ID.
#[must_use]
pub fn make_job_execution(job: &jobs::Model) -> JobExecutionBuilder {
    static NEXT_CI_JOB_ID: AtomicU64 = AtomicU64::new(1);
    JobExecutionBuilder {
        job_id: job.id,
        status: "passed".to_owned(),
        ci_job_id: Some(NEXT_CI_JOB_ID.fetch_add(1, Ordering::Relaxed).to_string()),
        commit_sha: "sha1".to_owned(),
        branch: "main".to_owned(),
        ci_run_id: "1".to_owned(),
        ci_run_attempt: 1,
        url: String::new(),
        runner_name: String::new(),
        runner_labels: Vec::new(),
        created_at: None,
    }
}

impl JobExecutionBuilder {
    /// Sets the Job execution's outcome: `passed`, `failed` or `skipped`.
    #[must_use]
    pub fn status(mut self, status: &str) -> Self {
        status.clone_into(&mut self.status);
        self
    }

    /// Sets the CI job ID the executor reported.
    #[must_use]
    pub fn ci_job_id(mut self, ci_job_id: &str) -> Self {
        self.ci_job_id = Some(ci_job_id.to_owned());
        self
    }

    /// Sets the commit SHA the Job executed at.
    #[must_use]
    pub fn commit_sha(mut self, commit_sha: &str) -> Self {
        commit_sha.clone_into(&mut self.commit_sha);
        self
    }

    /// Sets the branch the Job executed on.
    #[must_use]
    pub fn branch(mut self, branch: &str) -> Self {
        branch.clone_into(&mut self.branch);
        self
    }

    /// Sets the CI run ID the Job executed in.
    #[must_use]
    pub fn ci_run_id(mut self, ci_run_id: &str) -> Self {
        ci_run_id.clone_into(&mut self.ci_run_id);
        self
    }

    /// Sets the CI run attempt the Job executed in.
    #[must_use]
    pub fn ci_run_attempt(mut self, ci_run_attempt: i32) -> Self {
        self.ci_run_attempt = ci_run_attempt;
        self
    }

    /// Sets the Job execution's CI url.
    #[must_use]
    pub fn url(mut self, url: &str) -> Self {
        url.clone_into(&mut self.url);
        self
    }

    /// Sets the runner's name.
    #[must_use]
    pub fn runner_name(mut self, runner_name: &str) -> Self {
        runner_name.clone_into(&mut self.runner_name);
        self
    }

    /// Sets the runner's labels.
    #[must_use]
    pub fn runner_labels(mut self, labels: &[&str]) -> Self {
        self.runner_labels = labels.iter().map(|label| (*label).to_owned()).collect();
        self
    }

    /// Sets when the Job execution was recorded.
    #[must_use]
    pub fn created_at(mut self, created_at: DateTime<FixedOffset>) -> Self {
        self.created_at = Some(created_at);
        self
    }

    /// Inserts the Job execution.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Database`] when the status is not one of the
    /// allowed outcomes, or when the insert violates a constraint.
    pub async fn build(
        self,
        db: &impl ConnectionTrait,
    ) -> Result<job_executions::Model, TestError> {
        if !matches!(self.status.as_str(), "passed" | "failed" | "skipped") {
            return Err(TestError::Fixture(format!(
                "bad job status: {:?}",
                self.status
            )));
        }
        insert(
            db,
            job_executions::ActiveModel {
                job_id: Set(self.job_id),
                ci_job_id: Set(self.ci_job_id.unwrap_or_else(|| "0".to_owned())),
                ci_run_id: Set(self.ci_run_id),
                ci_run_attempt: Set(self.ci_run_attempt),
                commit_sha: Set(self.commit_sha),
                branch: Set(self.branch),
                status: Set(self.status),
                url: Set(self.url),
                runner_name: Set(self.runner_name),
                runner_labels: Set(JsonValue::Array(
                    self.runner_labels
                        .into_iter()
                        .map(JsonValue::from)
                        .collect(),
                )),
                started_at: Set(None),
                completed_at: Set(None),
                created_at: Set(self.created_at.unwrap_or_else(now)),
                ..Default::default()
            },
        )
        .await
    }
}

/// Builder for one day of a Test's Score history.
#[derive(Debug)]
pub struct TestScoreBuilder {
    test_case_id: i32,
    day: NaiveDate,
    flakiness_score: f64,
    confirmed_flake_count: i32,
    executions: i32,
    failures: i32,
}

/// Builds a Score history row for `test_case` on `day`.
#[must_use]
pub fn make_test_score(
    test_case: &test_cases::Model,
    day: NaiveDate,
    flakiness_score: f64,
) -> TestScoreBuilder {
    TestScoreBuilder {
        test_case_id: test_case.id,
        day,
        flakiness_score,
        confirmed_flake_count: 0,
        executions: 0,
        failures: 0,
    }
}

impl TestScoreBuilder {
    /// Sets how many Executions ran that day.
    #[must_use]
    pub fn executions(mut self, executions: i32) -> Self {
        self.executions = executions;
        self
    }

    /// Sets how many Executions failed that day.
    #[must_use]
    pub fn failures(mut self, failures: i32) -> Self {
        self.failures = failures;
        self
    }

    /// Inserts the Score history row.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Database`] when the insert violates a constraint.
    pub async fn build(
        self,
        db: &impl ConnectionTrait,
    ) -> Result<test_score_history::Model, TestError> {
        insert(
            db,
            test_score_history::ActiveModel {
                test_case_id: Set(self.test_case_id),
                day: Set(self.day),
                flakiness_score: Set(self.flakiness_score),
                confirmed_flake_count: Set(self.confirmed_flake_count),
                executions: Set(self.executions),
                failures: Set(self.failures),
            },
        )
        .await
    }
}

/// Builder for one day of a Job's Score history.
#[derive(Debug)]
pub struct JobScoreBuilder {
    job_id: i32,
    day: NaiveDate,
    flakiness_score: f64,
    confirmed_flake_count: i32,
    executions: i32,
    failures: i32,
}

/// Builds a Score history row for `job` on `day`.
#[must_use]
pub fn make_job_score(job: &jobs::Model, day: NaiveDate, flakiness_score: f64) -> JobScoreBuilder {
    JobScoreBuilder {
        job_id: job.id,
        day,
        flakiness_score,
        confirmed_flake_count: 0,
        executions: 0,
        failures: 0,
    }
}

impl JobScoreBuilder {
    /// Sets how many Job executions ran that day.
    #[must_use]
    pub fn executions(mut self, executions: i32) -> Self {
        self.executions = executions;
        self
    }

    /// Sets how many Job executions failed that day.
    #[must_use]
    pub fn failures(mut self, failures: i32) -> Self {
        self.failures = failures;
        self
    }

    /// Inserts the Score history row.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Database`] when the insert violates a constraint.
    pub async fn build(
        self,
        db: &impl ConnectionTrait,
    ) -> Result<job_score_history::Model, TestError> {
        insert(
            db,
            job_score_history::ActiveModel {
                job_id: Set(self.job_id),
                day: Set(self.day),
                flakiness_score: Set(self.flakiness_score),
                confirmed_flake_count: Set(self.confirmed_flake_count),
                executions: Set(self.executions),
                failures: Set(self.failures),
            },
        )
        .await
    }
}

/// Builder for a Report, pending unless a test says otherwise.
#[derive(Debug)]
pub struct ReportBuilder {
    project_id: Option<i32>,
    repo_id: i32,
    body: Vec<u8>,
    commit_sha: String,
    branch: String,
    ci_run_id: String,
    ci_run_attempt: Option<i32>,
    status: String,
    error: Option<String>,
    retry_count: i32,
    processed_at: Option<DateTime<FixedOffset>>,
    kind: String,
    root: Option<String>,
    default_branch: Option<String>,
    ci_job_id: Option<String>,
    pipeline: Option<String>,
    line_base: Option<i32>,
}

/// Builds a pending JUnit Report of `acme/app`'s `default` Project at `sha1`.
#[must_use]
pub fn make_report(project: &projects::Model, repo_id: i32) -> ReportBuilder {
    ReportBuilder {
        project_id: Some(project.id),
        repo_id,
        body: b"<testsuite/>".to_vec(),
        commit_sha: "sha1".to_owned(),
        branch: "main".to_owned(),
        ci_run_id: String::new(),
        ci_run_attempt: None,
        status: "pending".to_owned(),
        error: None,
        retry_count: 0,
        processed_at: None,
        kind: "junit".to_owned(),
        root: None,
        default_branch: None,
        ci_job_id: None,
        pipeline: None,
        line_base: None,
    }
}

impl ReportBuilder {
    /// Sets the uploaded body.
    #[must_use]
    pub fn body(mut self, body: &[u8]) -> Self {
        body.clone_into(&mut self.body);
        self
    }

    /// Sets the commit the Report is for.
    #[must_use]
    pub fn commit_sha(mut self, commit_sha: &str) -> Self {
        commit_sha.clone_into(&mut self.commit_sha);
        self
    }

    /// Sets the branch the Report is for.
    #[must_use]
    pub fn branch(mut self, branch: &str) -> Self {
        branch.clone_into(&mut self.branch);
        self
    }

    /// Sets the CI run the Report came from.
    #[must_use]
    pub fn ci_run_id(mut self, ci_run_id: &str) -> Self {
        ci_run_id.clone_into(&mut self.ci_run_id);
        self
    }

    /// Sets the Report's status: `pending`, `processed` or `failed`.
    #[must_use]
    pub fn status(mut self, status: &str) -> Self {
        status.clone_into(&mut self.status);
        self
    }

    /// Sets the Report's kind: `junit` or `pipeline`, with no Project.
    #[must_use]
    pub fn kind(mut self, kind: &str) -> Self {
        kind.clone_into(&mut self.kind);
        self.project_id = None;
        self
    }

    /// Sets why processing failed.
    #[must_use]
    pub fn error(mut self, error: &str) -> Self {
        self.error = Some(error.to_owned());
        self
    }

    /// Sets how many automatic retries the Report has spent.
    #[must_use]
    pub fn retry_count(mut self, retry_count: i32) -> Self {
        self.retry_count = retry_count;
        self
    }

    /// Sets which attempt of the CI run the Report came from.
    #[must_use]
    pub fn ci_run_attempt(mut self, ci_run_attempt: i32) -> Self {
        self.ci_run_attempt = Some(ci_run_attempt);
        self
    }

    /// Sets the Project root the upload named.
    #[must_use]
    pub fn root(mut self, root: &str) -> Self {
        self.root = Some(root.to_owned());
        self
    }

    /// Sets the Repo's Default branch the upload named.
    #[must_use]
    pub fn default_branch(mut self, default_branch: &str) -> Self {
        self.default_branch = Some(default_branch.to_owned());
        self
    }

    /// Sets the CI Job id the Report came from.
    #[must_use]
    pub fn ci_job_id(mut self, ci_job_id: &str) -> Self {
        self.ci_job_id = Some(ci_job_id.to_owned());
        self
    }

    /// Sets the Pipeline the Report came from.
    #[must_use]
    pub fn pipeline(mut self, pipeline: &str) -> Self {
        self.pipeline = Some(pipeline.to_owned());
        self
    }

    /// Sets the base the runner used for line numbers.
    #[must_use]
    pub fn line_base(mut self, line_base: i32) -> Self {
        self.line_base = Some(line_base);
        self
    }

    /// Sets when processing finished.
    #[must_use]
    pub fn processed_at(mut self, processed_at: DateTime<FixedOffset>) -> Self {
        self.processed_at = Some(processed_at);
        self
    }

    /// Inserts the Report.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Database`] when the insert violates a constraint.
    pub async fn build(self, db: &impl ConnectionTrait) -> Result<reports::Model, TestError> {
        insert(
            db,
            reports::ActiveModel {
                id: ActiveValue::default(),
                project_id: Set(self.project_id),
                repo_id: Set(self.repo_id),
                body: Set(self.body),
                body_sha256: Set(None),
                commit_sha: Set(self.commit_sha),
                branch: Set(self.branch),
                ci_run_id: Set(self.ci_run_id),
                ci_job_id: Set(self.ci_job_id),
                ci_run_attempt: Set(self.ci_run_attempt),
                pipeline: Set(self.pipeline),
                default_branch: Set(self.default_branch),
                line_base: Set(self.line_base),
                root: Set(self.root),
                status: Set(self.status),
                error: Set(self.error),
                counts: Set(None),
                run_id: Set(None),
                created_at: Set(now()),
                processed_at: Set(self.processed_at.map(|at| at.fixed_offset())),
                retry_count: Set(self.retry_count),
                kind: Set(self.kind),
            },
        )
        .await
    }
}
