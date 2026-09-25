"""fresh baseline: repos, projects, tests, reports, runs, executions (Postgres)

Revision ID: 0001
Revises:
Create Date: 2026-09-25
"""

import sqlalchemy as sa
from alembic import op
from sqlalchemy.dialects import postgresql

revision = "0001"
down_revision = None
branch_labels = None
depends_on = None

TS = sa.DateTime(timezone=True)


def upgrade() -> None:
    op.create_table(
        "repos",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("name", sa.String(length=255), nullable=False),
        sa.Column("created_at", TS, nullable=False),
        sa.UniqueConstraint("name", name="uq_repos_name"),
    )

    op.create_table(
        "projects",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("repo_id", sa.Integer(), sa.ForeignKey("repos.id", ondelete="CASCADE"), nullable=False),
        sa.Column("name", sa.String(length=100), nullable=False),
        sa.Column("root", sa.String(length=1024), nullable=False, server_default=""),
        sa.Column("created_at", TS, nullable=False),
        sa.UniqueConstraint("repo_id", "name", name="uq_projects_repo_name"),
    )
    op.create_index("ix_projects_repo_id", "projects", ["repo_id"])

    op.create_table(
        "test_cases",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("project_id", sa.Integer(), sa.ForeignKey("projects.id", ondelete="CASCADE"), nullable=False),
        sa.Column("fingerprint", sa.String(length=40), nullable=False),
        sa.Column("suite", sa.Text(), nullable=False, server_default=""),
        sa.Column("classname", sa.Text(), nullable=False, server_default=""),
        sa.Column("name", sa.Text(), nullable=False),
        sa.Column("file", sa.Text(), nullable=True),
        sa.Column("line", sa.Integer(), nullable=True),
        sa.Column("flakiness_score", sa.Float(), nullable=False, server_default="0"),
        sa.Column("confirmed_flake_count", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("last_status", sa.String(length=16), nullable=False, server_default="passed"),
        sa.Column("last_seen_at", TS, nullable=False),
        sa.Column("quarantined", sa.Boolean(), nullable=False, server_default=sa.false()),
        sa.Column("quarantined_at", TS, nullable=True),
        sa.Column("github_issue_number", sa.Integer(), nullable=True),
        sa.UniqueConstraint("project_id", "fingerprint", name="uq_test_cases_project_fingerprint"),
    )
    op.create_index("ix_test_cases_project_id", "test_cases", ["project_id"])
    op.create_index("ix_test_cases_project_score", "test_cases", ["project_id", "flakiness_score"])

    op.create_table(
        "test_runs",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("project_id", sa.Integer(), sa.ForeignKey("projects.id", ondelete="CASCADE"), nullable=False),
        sa.Column("commit_sha", sa.String(length=64), nullable=False),
        sa.Column("branch", sa.String(length=255), nullable=False),
        sa.Column("ci_run_id", sa.String(length=255), nullable=False, server_default=""),
        sa.Column("created_at", TS, nullable=False),
    )
    op.create_index("ix_test_runs_project_id", "test_runs", ["project_id"])
    op.create_index("ix_test_runs_commit_sha", "test_runs", ["commit_sha"])

    op.create_table(
        "test_executions",
        sa.Column("id", sa.BigInteger(), primary_key=True),
        sa.Column("test_case_id", sa.Integer(), sa.ForeignKey("test_cases.id", ondelete="CASCADE"), nullable=False),
        sa.Column("test_run_id", sa.Integer(), sa.ForeignKey("test_runs.id", ondelete="CASCADE"), nullable=False),
        sa.Column("status", sa.String(length=16), nullable=False),
        sa.Column("duration", sa.Float(), nullable=False, server_default="0"),
        sa.Column("message", sa.Text(), nullable=False, server_default=""),
        sa.Column("details", sa.Text(), nullable=False, server_default=""),
        sa.Column("created_at", TS, nullable=False),
    )
    op.create_index("ix_exec_case_id", "test_executions", ["test_case_id", "id"])
    op.create_index("ix_test_executions_test_run_id", "test_executions", ["test_run_id"])
    op.create_index("ix_test_executions_created_at", "test_executions", ["created_at"])

    op.create_table(
        "reports",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("project_id", sa.Integer(), sa.ForeignKey("projects.id", ondelete="CASCADE"), nullable=False),
        sa.Column("commit_sha", sa.String(length=64), nullable=False),
        sa.Column("branch", sa.String(length=255), nullable=False),
        sa.Column("ci_run_id", sa.String(length=255), nullable=False, server_default=""),
        sa.Column("root", sa.String(length=1024), nullable=True),
        sa.Column("body", sa.LargeBinary(), nullable=False),
        sa.Column("status", sa.String(length=16), nullable=False, server_default="pending"),
        sa.Column("error", sa.Text(), nullable=True),
        sa.Column("counts", postgresql.JSONB(), nullable=True),
        sa.Column("run_id", sa.Integer(), sa.ForeignKey("test_runs.id", ondelete="SET NULL"), nullable=True),
        sa.Column("created_at", TS, nullable=False),
        sa.Column("processed_at", TS, nullable=True),
    )
    op.create_index("ix_reports_status_id", "reports", ["status", "id"])
    op.create_index("ix_reports_project_id", "reports", ["project_id"])


def downgrade() -> None:
    op.drop_table("reports")
    op.drop_table("test_executions")
    op.drop_table("test_runs")
    op.drop_table("test_cases")
    op.drop_table("projects")
    op.drop_table("repos")
