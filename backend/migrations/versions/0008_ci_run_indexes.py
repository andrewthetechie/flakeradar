"""ci run indexes

Indexes `ci_run_id` on `test_runs`, `reports` (with `repo_id`) and
`job_executions`, for `GET /api/ci-runs/{ci_run_id}/failures`. Plain
`CREATE INDEX`: these tables are small next to `test_executions`.

Revision ID: 0008
Revises: 0007
Create Date: 2026-10-16
"""

from alembic import op

revision = "0008"
down_revision = "0007"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_index("ix_test_runs_ci_run_id", "test_runs", ["ci_run_id"])
    op.create_index("ix_reports_repo_ci_run_id", "reports", ["repo_id", "ci_run_id"])
    op.create_index("ix_job_executions_ci_run_id", "job_executions", ["ci_run_id"])


def downgrade() -> None:
    op.drop_index("ix_job_executions_ci_run_id", table_name="job_executions")
    op.drop_index("ix_reports_repo_ci_run_id", table_name="reports")
    op.drop_index("ix_test_runs_ci_run_id", table_name="test_runs")
