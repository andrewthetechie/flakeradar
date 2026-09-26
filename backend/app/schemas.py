"""Pydantic response models — the typed contract the frontend and MCP consume."""

from datetime import datetime
from typing import Annotated, Any, Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

from .identity import normalize_provider, normalize_repo
from .models import JobStatus


class IngestAccepted(BaseModel):
    report_id: int
    status: str  # always "pending"


class JobResultIn(BaseModel):
    # Strip before the length checks, so a blank name is rejected, not stored as "".
    model_config = ConfigDict(str_strip_whitespace=True)

    ci_job_id: str = Field(min_length=1, max_length=255)
    name: str = Field(min_length=1, max_length=512)
    status: JobStatus
    url: str = Field(default="", max_length=2048)
    runner_name: str = Field(default="", max_length=255)
    runner_labels: list[Annotated[str, Field(max_length=255)]] = Field(default_factory=list, max_length=50)
    started_at: datetime | None = None
    completed_at: datetime | None = None


class PipelineReportIn(BaseModel):
    """One attempt of one Pipeline run. Validation also normalizes, so the
    stored body is canonical and processing can trust it."""

    model_config = ConfigDict(str_strip_whitespace=True)

    repo: str = Field(max_length=255)
    provider: str = Field(max_length=32)
    pipeline: str = Field(min_length=1, max_length=512)
    commit_sha: str = Field(min_length=1, max_length=64)
    branch: str = Field(min_length=1, max_length=255)
    default_branch: str | None = Field(default=None, max_length=255)
    ci_run_id: str = Field(default="", max_length=255)
    ci_run_attempt: int = Field(default=1, ge=1)
    jobs: list[JobResultIn] = Field(min_length=1, max_length=1000)

    @field_validator("repo")
    @classmethod
    def _normalize_repo(cls, v: str) -> str:
        return normalize_repo(v)

    @field_validator("provider")
    @classmethod
    def _normalize_provider(cls, v: str) -> str:
        return normalize_provider(v)

    @field_validator("default_branch")
    @classmethod
    def _blank_default_branch_is_unknown(cls, v: str | None) -> str | None:
        return v or None

    @model_validator(mode="after")
    def _unique_ci_job_ids(self) -> "PipelineReportIn":
        if len({j.ci_job_id for j in self.jobs}) != len(self.jobs):
            raise ValueError("duplicate ci_job_id in jobs")
        return self


class ReportOut(BaseModel):
    id: int
    kind: str  # junit | pipeline
    repo: str
    project: str | None
    commit_sha: str
    branch: str
    ci_run_id: str
    status: str  # pending | processed | failed
    error: str | None
    counts: dict[str, Any] | None
    run_id: int | None
    created_at: datetime
    processed_at: datetime | None


class ReportSummaryOut(BaseModel):
    pending: int
    failed: int


# --- Tests, Repos, summary (task 08) ------------------------------------

Tier = Literal["flaky", "suspect", "stable"]


class ProjectOut(BaseModel):
    name: str
    root: str


class RepoOut(BaseModel):
    name: str
    projects: list[ProjectOut]


class TestOut(BaseModel):
    __test__ = False  # not a pytest class

    id: int
    repo: str
    project: str
    fingerprint: str
    suite: str
    classname: str
    name: str
    file: str | None
    line: int | None
    flakiness_score: float
    tier: Tier
    confirmed_flake_count: int
    last_status: str
    last_seen_at: datetime
    quarantined: bool
    quarantined_at: datetime | None
    github_issue_number: int | None
    github_issue_url: str | None


class TestPage(BaseModel):
    __test__ = False

    items: list[TestOut]
    total: int
    page: int
    page_size: int


class SummaryOut(BaseModel):
    total_tests: int
    flaky_tests: int
    suspect_tests: int
    confirmed_flaky_tests: int
    total_runs: int
    total_executions: int
    flake_threshold: float


# --- Test detail and quarantine (task 09) -------------------------------


class ExecutionOut(BaseModel):
    id: int
    status: str
    duration: float
    message: str  # Failure message
    details: str  # Failure details (traceback + captured output)
    created_at: datetime
    commit_sha: str
    branch: str
    ci_run_id: str


class LocationOut(BaseModel):
    path: str  # file joined onto the Project root, repo-relative
    line: int | None
    url: str | None  # GitHub permalink at the last failing SHA, when one exists


class TestJobLinkOut(BaseModel):
    __test__ = False

    job_id: int
    pipeline: str
    name: str


class HistoryOut(BaseModel):
    test: TestOut
    location: LocationOut | None  # None when the runner never reported a file
    last_failing_sha: str | None
    last_failing_branch: str | None
    executions: list[ExecutionOut]  # newest first
    jobs: list[TestJobLinkOut]  # "seen in Jobs": Jobs whose executions share a ci_job_id with these Runs


# --- Jobs (task 06) ----------------------------------------------------


class JobOut(BaseModel):
    __test__ = False

    id: int
    repo: str
    provider: str
    pipeline: str
    name: str
    flakiness_score: float
    tier: Tier
    confirmed_flake_count: int
    last_status: str
    last_seen_at: datetime
    github_issue_number: int | None
    github_issue_url: str | None  # only when provider == "github"


class JobPage(BaseModel):
    __test__ = False

    items: list[JobOut]
    total: int
    page: int
    page_size: int


class ExplainingTestOut(BaseModel):
    __test__ = False

    test_id: int
    project: str
    classname: str
    name: str
    status: str  # failed | error


class JobExecutionOut(BaseModel):
    __test__ = False

    id: int
    status: str  # passed | failed | skipped (as stored)
    outcome: Literal["passed", "failed", "explained", "skipped"]  # failed = unexplained
    commit_sha: str
    branch: str
    ci_run_id: str
    ci_run_attempt: int
    ci_job_id: str
    url: str
    runner_name: str
    runner_labels: list[str]
    started_at: datetime | None
    completed_at: datetime | None
    created_at: datetime
    explained_by: list[ExplainingTestOut]  # empty unless outcome == "explained"


class JobHistoryOut(BaseModel):
    __test__ = False

    job: JobOut
    unexplained_failures: int  # over the returned executions
    explained_failures: int
    executions: list[JobExecutionOut]  # newest first


class JobSummaryOut(BaseModel):
    __test__ = False

    total_jobs: int
    flaky_jobs: int
    suspect_jobs: int
    confirmed_flaky_jobs: int
    total_job_executions: int
    flake_threshold: float


class QuarantineIn(BaseModel):
    quarantined: bool


class QuarantineItem(BaseModel):
    suite: str
    classname: str
    name: str
    fingerprint: str
    file: str | None
    line: int | None
    quarantined_at: datetime | None
