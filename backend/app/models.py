"""ORM models — the final FlakeRadar schema (see CONTEXT.md for the terms).

Design notes:
- A Repo (``owner/name``) has Projects; a Test (``TestCase``) is unique by a
  stable fingerprint of (suite, classname, name) within one Project.
- A Report is the raw JUnit upload, queued until the processor turns it into
  a Run. Executions are keyed to a Run's commit SHA because a fail->pass flip
  on the SAME sha is proof of nondeterminism.
- Pipelines are named CI workflows, never scored. Jobs belong to a Pipeline
  (not a Project) and are scored like Tests. A Job execution is one attempt
  of one Job at one commit SHA, unique per (job_id, ci_job_id).
- No ORM relationships: async SQLAlchemy cannot lazy-load, so every read is
  an explicit select()/join.
"""

from datetime import datetime, timezone
from typing import Any, Literal, get_args

from sqlalchemy import (
    BigInteger,
    Boolean,
    CheckConstraint,
    DateTime,
    Float,
    ForeignKey,
    Index,
    Integer,
    LargeBinary,
    String,
    Text,
    UniqueConstraint,
    false,
    text,
)
from sqlalchemy.dialects.postgresql import JSONB
from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column

REPORT_PENDING = "pending"
REPORT_PROCESSED = "processed"
REPORT_FAILED = "failed"
REPORT_STATUSES = (REPORT_PENDING, REPORT_PROCESSED, REPORT_FAILED)

REPORT_KIND_JUNIT = "junit"
REPORT_KIND_PIPELINE = "pipeline"
REPORT_KINDS = (REPORT_KIND_JUNIT, REPORT_KIND_PIPELINE)

# A Job execution's status, already normalized by the CI reporter.
JobStatus = Literal["passed", "failed", "skipped"]
JOB_FAILED: JobStatus = "failed"
JOB_SKIPPED: JobStatus = "skipped"
JOB_STATUSES: tuple[JobStatus, ...] = get_args(JobStatus)


def utcnow() -> datetime:
    return datetime.now(timezone.utc)


class Base(DeclarativeBase):
    pass


class Repo(Base):
    __tablename__ = "repos"

    id: Mapped[int] = mapped_column(primary_key=True)
    name: Mapped[str] = mapped_column(String(255), unique=True)
    default_branch: Mapped[str | None] = mapped_column(String(255), default=None)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow)


class Project(Base):
    __tablename__ = "projects"
    __table_args__ = (UniqueConstraint("repo_id", "name", name="uq_projects_repo_name"),)

    id: Mapped[int] = mapped_column(primary_key=True)
    repo_id: Mapped[int] = mapped_column(ForeignKey("repos.id", ondelete="CASCADE"), index=True)
    name: Mapped[str] = mapped_column(String(100))
    root: Mapped[str] = mapped_column(String(1024), default="", server_default="")
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow)


class TestCase(Base):
    __tablename__ = "test_cases"
    __test__ = False  # stop pytest trying to collect this class
    __table_args__ = (
        UniqueConstraint("project_id", "fingerprint", name="uq_test_cases_project_fingerprint"),
        Index("ix_test_cases_project_score", "project_id", "flakiness_score"),
    )

    id: Mapped[int] = mapped_column(primary_key=True)
    project_id: Mapped[int] = mapped_column(ForeignKey("projects.id", ondelete="CASCADE"), index=True)
    fingerprint: Mapped[str] = mapped_column(String(40))
    suite: Mapped[str] = mapped_column(Text, default="", server_default="")
    classname: Mapped[str] = mapped_column(Text, default="", server_default="")
    name: Mapped[str] = mapped_column(Text)

    # Location: latest report wins; a report without `file` keeps the old one.
    file: Mapped[str | None] = mapped_column(Text, default=None)
    line: Mapped[int | None] = mapped_column(Integer, default=None)

    # Cached analytics, recomputed whenever a Run touches this test.
    flakiness_score: Mapped[float] = mapped_column(Float, default=0.0, server_default="0")
    confirmed_flake_count: Mapped[int] = mapped_column(Integer, default=0, server_default="0")
    last_status: Mapped[str] = mapped_column(String(16), default="passed", server_default="passed")
    last_seen_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow)

    # Manual quarantine: a human marks a test skippable by the runner.
    quarantined: Mapped[bool] = mapped_column(Boolean, default=False, server_default=false())
    quarantined_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), default=None)

    # GitHub issue automation bookkeeping.
    github_issue_number: Mapped[int | None] = mapped_column(Integer, default=None)


class TestRun(Base):
    __tablename__ = "test_runs"
    __test__ = False

    id: Mapped[int] = mapped_column(primary_key=True)
    project_id: Mapped[int] = mapped_column(ForeignKey("projects.id", ondelete="CASCADE"), index=True)
    commit_sha: Mapped[str] = mapped_column(String(64), index=True)
    branch: Mapped[str] = mapped_column(String(255), default="main")
    ci_run_id: Mapped[str] = mapped_column(String(255), default="", server_default="")
    # A Run can name the Job execution (ci_job_id) that produced it.
    ci_job_id: Mapped[str | None] = mapped_column(String(255), index=True, default=None)
    ci_run_attempt: Mapped[int | None] = mapped_column(Integer, default=None)
    pipeline: Mapped[str | None] = mapped_column(String(512), default=None)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow)


class TestExecution(Base):
    __tablename__ = "test_executions"
    __test__ = False
    __table_args__ = (Index("ix_exec_case_id", "test_case_id", "id"),)

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True)
    test_case_id: Mapped[int] = mapped_column(ForeignKey("test_cases.id", ondelete="CASCADE"))
    test_run_id: Mapped[int] = mapped_column(ForeignKey("test_runs.id", ondelete="CASCADE"), index=True)
    status: Mapped[str] = mapped_column(String(16))  # passed | failed | error | skipped
    duration: Mapped[float] = mapped_column(Float, default=0.0, server_default="0")
    message: Mapped[str] = mapped_column(Text, default="", server_default="")
    details: Mapped[str] = mapped_column(Text, default="", server_default="")
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow, index=True)


class Report(Base):
    __tablename__ = "reports"
    __table_args__ = (
        Index("ix_reports_status_id", "status", "id"),
        CheckConstraint("kind <> 'junit' OR project_id IS NOT NULL", name="ck_reports_junit_has_project"),
    )

    id: Mapped[int] = mapped_column(primary_key=True)
    kind: Mapped[str] = mapped_column(String(16), default=REPORT_KIND_JUNIT, server_default=REPORT_KIND_JUNIT)
    project_id: Mapped[int | None] = mapped_column(ForeignKey("projects.id", ondelete="CASCADE"), index=True)
    repo_id: Mapped[int] = mapped_column(ForeignKey("repos.id", ondelete="CASCADE"), index=True)
    commit_sha: Mapped[str] = mapped_column(String(64))
    branch: Mapped[str] = mapped_column(String(255), default="main")
    ci_run_id: Mapped[str] = mapped_column(String(255), default="", server_default="")
    # Project root sent with this upload; None means "leave the Project's root alone".
    root: Mapped[str | None] = mapped_column(String(1024), default=None)
    # A JUnit Report can name the Job execution (ci_job_id) that produced it.
    ci_job_id: Mapped[str | None] = mapped_column(String(255), default=None)
    ci_run_attempt: Mapped[int | None] = mapped_column(Integer, default=None)
    pipeline: Mapped[str | None] = mapped_column(String(512), default=None)
    default_branch: Mapped[str | None] = mapped_column(String(255), default=None)
    body: Mapped[bytes] = mapped_column(LargeBinary)
    status: Mapped[str] = mapped_column(String(16), default=REPORT_PENDING, server_default=REPORT_PENDING)
    error: Mapped[str | None] = mapped_column(Text, default=None)
    counts: Mapped[dict[str, Any] | None] = mapped_column(JSONB, default=None)
    run_id: Mapped[int | None] = mapped_column(ForeignKey("test_runs.id", ondelete="SET NULL"), default=None)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow)
    processed_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), default=None)


class Pipeline(Base):
    """A named CI workflow in a Repo; groups Jobs and is never scored."""

    __tablename__ = "pipelines"
    __table_args__ = (UniqueConstraint("repo_id", "provider", "name", name="uq_pipelines_repo_provider_name"),)

    id: Mapped[int] = mapped_column(primary_key=True)
    repo_id: Mapped[int] = mapped_column(ForeignKey("repos.id", ondelete="CASCADE"), index=True)
    provider: Mapped[str] = mapped_column(String(32))
    name: Mapped[str] = mapped_column(String(512))
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow)


class Job(Base):
    __tablename__ = "jobs"
    __table_args__ = (
        UniqueConstraint("pipeline_id", "name", name="uq_jobs_pipeline_name"),
        Index("ix_jobs_pipeline_score", "pipeline_id", "flakiness_score"),
    )

    id: Mapped[int] = mapped_column(primary_key=True)
    pipeline_id: Mapped[int] = mapped_column(ForeignKey("pipelines.id", ondelete="CASCADE"), index=True)
    name: Mapped[str] = mapped_column(String(512))
    flakiness_score: Mapped[float] = mapped_column(Float, default=0.0, server_default="0")
    confirmed_flake_count: Mapped[int] = mapped_column(Integer, default=0, server_default="0")
    last_status: Mapped[str] = mapped_column(String(16), default="passed", server_default="passed")
    last_seen_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow)
    github_issue_number: Mapped[int | None] = mapped_column(Integer, default=None)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow)


class JobExecution(Base):
    __tablename__ = "job_executions"
    __test__ = False
    __table_args__ = (
        UniqueConstraint("job_id", "ci_job_id", name="uq_job_executions_job_ci_job_id"),
        Index("ix_job_exec_job_id", "job_id", "id"),
    )

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True)
    job_id: Mapped[int] = mapped_column(ForeignKey("jobs.id", ondelete="CASCADE"))
    ci_job_id: Mapped[str] = mapped_column(String(255), index=True)
    ci_run_id: Mapped[str] = mapped_column(String(255), default="", server_default="")
    ci_run_attempt: Mapped[int] = mapped_column(Integer, default=1, server_default="1")
    commit_sha: Mapped[str] = mapped_column(String(64))
    branch: Mapped[str] = mapped_column(String(255))
    status: Mapped[str] = mapped_column(String(16))
    url: Mapped[str] = mapped_column(Text, default="", server_default="")
    runner_name: Mapped[str] = mapped_column(String(255), default="", server_default="")
    runner_labels: Mapped[list[str]] = mapped_column(JSONB, default=list, server_default=text("'[]'::jsonb"))
    started_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), default=None)
    completed_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), default=None)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow, index=True)
