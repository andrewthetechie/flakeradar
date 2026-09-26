"""ci-jobs: pipelines, jobs, job executions and report enrichment (Postgres)

Adds the schema later tasks need while keeping every existing row:
repos.default_branch, Report kind/repo_id plus enrichment columns, TestRun
enrichment columns, and the new `pipelines`, `jobs` and `job_executions`
tables. `reports.repo_id` is added nullable, backfilled from each report's
Project's Repo, then made NOT NULL so existing data is preserved.

Revision ID: 0002
Revises: 0001
Create Date: 2026-09-25
"""

import sqlalchemy as sa
from alembic import op
from sqlalchemy.dialects import postgresql

revision = "0002"
down_revision = "0001"
branch_labels = None
depends_on = None

TS = sa.DateTime(timezone=True)


def upgrade() -> None:
    # 1. repos: default branch arrives from pipeline reports.
    op.add_column("repos", sa.Column("default_branch", sa.String(length=255), nullable=True))

    # 2. reports: kind, repo_id (backfilled), enrichment columns, nullable project_id.
    op.add_column("reports", sa.Column("kind", sa.String(length=16), nullable=False, server_default="junit"))
    op.add_column(
        "reports",
        sa.Column("repo_id", sa.Integer(), sa.ForeignKey("repos.id", ondelete="CASCADE"), nullable=True),
    )
    op.execute("UPDATE reports SET repo_id = projects.repo_id FROM projects WHERE projects.id = reports.project_id")
    op.alter_column("reports", "repo_id", existing_type=sa.Integer(), nullable=False)
    op.create_index("ix_reports_repo_id", "reports", ["repo_id"])
    op.alter_column("reports", "project_id", existing_type=sa.Integer(), nullable=True)
    op.add_column("reports", sa.Column("ci_job_id", sa.String(length=255), nullable=True))
    op.add_column("reports", sa.Column("ci_run_attempt", sa.Integer(), nullable=True))
    op.add_column("reports", sa.Column("pipeline", sa.String(length=512), nullable=True))
    op.add_column("reports", sa.Column("default_branch", sa.String(length=255), nullable=True))
    op.create_check_constraint(
        "ck_reports_junit_has_project",
        "reports",
        "kind <> 'junit' OR project_id IS NOT NULL",
    )

    # 3. test_runs: enrichment columns so a Run can name the Job that made it.
    op.add_column("test_runs", sa.Column("ci_job_id", sa.String(length=255), nullable=True))
    op.add_column("test_runs", sa.Column("ci_run_attempt", sa.Integer(), nullable=True))
    op.add_column("test_runs", sa.Column("pipeline", sa.String(length=512), nullable=True))
    op.create_index("ix_test_runs_ci_job_id", "test_runs", ["ci_job_id"])

    # 4. pipelines: named CI workflows, not scored, unique per repo+provider+name.
    op.create_table(
        "pipelines",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("repo_id", sa.Integer(), sa.ForeignKey("repos.id", ondelete="CASCADE"), nullable=False),
        sa.Column("provider", sa.String(length=32), nullable=False),
        sa.Column("name", sa.String(length=512), nullable=False),
        sa.Column("created_at", TS, nullable=False),
        sa.UniqueConstraint("repo_id", "provider", "name", name="uq_pipelines_repo_provider_name"),
    )
    op.create_index("ix_pipelines_repo_id", "pipelines", ["repo_id"])

    # 5. jobs: scored like tests, unique per pipeline+name.
    op.create_table(
        "jobs",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("pipeline_id", sa.Integer(), sa.ForeignKey("pipelines.id", ondelete="CASCADE"), nullable=False),
        sa.Column("name", sa.String(length=512), nullable=False),
        sa.Column("flakiness_score", sa.Float(), nullable=False, server_default="0"),
        sa.Column("confirmed_flake_count", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("last_status", sa.String(length=16), nullable=False, server_default="passed"),
        sa.Column("last_seen_at", TS, nullable=False),
        sa.Column("github_issue_number", sa.Integer(), nullable=True),
        sa.Column("created_at", TS, nullable=False),
        sa.UniqueConstraint("pipeline_id", "name", name="uq_jobs_pipeline_name"),
    )
    op.create_index("ix_jobs_pipeline_id", "jobs", ["pipeline_id"])
    op.create_index("ix_jobs_pipeline_score", "jobs", ["pipeline_id", "flakiness_score"])

    # 6. job_executions: one attempt of one Job at one SHA.
    op.create_table(
        "job_executions",
        sa.Column("id", sa.BigInteger(), primary_key=True),
        sa.Column("job_id", sa.Integer(), sa.ForeignKey("jobs.id", ondelete="CASCADE"), nullable=False),
        sa.Column("ci_job_id", sa.String(length=255), nullable=False),
        sa.Column("ci_run_id", sa.String(length=255), nullable=False, server_default=""),
        sa.Column("ci_run_attempt", sa.Integer(), nullable=False, server_default="1"),
        sa.Column("commit_sha", sa.String(length=64), nullable=False),
        sa.Column("branch", sa.String(length=255), nullable=False),
        sa.Column("status", sa.String(length=16), nullable=False),
        sa.Column("url", sa.Text(), nullable=False, server_default=""),
        sa.Column("runner_name", sa.String(length=255), nullable=False, server_default=""),
        sa.Column("runner_labels", postgresql.JSONB(), nullable=False, server_default=sa.text("'[]'::jsonb")),
        sa.Column("started_at", TS, nullable=True),
        sa.Column("completed_at", TS, nullable=True),
        sa.Column("created_at", TS, nullable=False),
        sa.UniqueConstraint("job_id", "ci_job_id", name="uq_job_executions_job_ci_job_id"),
    )
    op.create_index("ix_job_exec_job_id", "job_executions", ["job_id", "id"])
    op.create_index("ix_job_executions_ci_job_id", "job_executions", ["ci_job_id"])
    op.create_index("ix_job_executions_created_at", "job_executions", ["created_at"])


def downgrade() -> None:
    # Reverse in reverse order. Pipeline reports are deleted first so that
    # reports.project_id can go back to NOT NULL (a junit report requires one).
    op.drop_table("job_executions")
    op.drop_table("jobs")
    op.drop_table("pipelines")

    op.drop_index("ix_test_runs_ci_job_id", table_name="test_runs")
    op.drop_column("test_runs", "pipeline")
    op.drop_column("test_runs", "ci_run_attempt")
    op.drop_column("test_runs", "ci_job_id")

    op.drop_constraint("ck_reports_junit_has_project", "reports", type_="check")
    op.execute("DELETE FROM reports WHERE kind = 'pipeline'")
    op.alter_column("reports", "project_id", existing_type=sa.Integer(), nullable=False)
    op.drop_index("ix_reports_repo_id", table_name="reports")
    op.drop_column("reports", "default_branch")
    op.drop_column("reports", "pipeline")
    op.drop_column("reports", "ci_run_attempt")
    op.drop_column("reports", "ci_job_id")
    op.drop_column("reports", "repo_id")
    op.drop_column("reports", "kind")

    op.drop_column("repos", "default_branch")
