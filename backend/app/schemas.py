"""Pydantic response models — the typed contract the frontend and MCP consume."""

from datetime import datetime
from typing import Any, Literal

from pydantic import BaseModel


class IngestAccepted(BaseModel):
    report_id: int
    status: str  # always "pending"


class ReportOut(BaseModel):
    id: int
    repo: str
    project: str
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


class HistoryOut(BaseModel):
    test: TestOut
    location: LocationOut | None  # None when the runner never reported a file
    last_failing_sha: str | None
    last_failing_branch: str | None
    executions: list[ExecutionOut]  # newest first


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
