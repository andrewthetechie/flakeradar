"""job score history: daily snapshots per Job (ADR 0007)

Adds `jobs.clean_streak` and the `job_score_history` table (one row per Job
per UTC day). No backfill: history starts the day this ships.

Revision ID: 0006
Revises: 0005
Create Date: 2026-09-26
"""

import sqlalchemy as sa
from alembic import op

revision = "0006"
down_revision = "0005"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("jobs", sa.Column("clean_streak", sa.Integer(), nullable=False, server_default="0"))
    op.create_table(
        "job_score_history",
        sa.Column("job_id", sa.Integer(), sa.ForeignKey("jobs.id", ondelete="CASCADE"), primary_key=True),
        sa.Column("day", sa.Date(), primary_key=True),
        sa.Column("flakiness_score", sa.Float(), nullable=False),
        sa.Column("confirmed_flake_count", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("executions", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("failures", sa.Integer(), nullable=False, server_default="0"),
        sa.PrimaryKeyConstraint("job_id", "day"),
    )
    op.create_index("ix_job_score_history_day", "job_score_history", ["day"])


def downgrade() -> None:
    op.drop_index("ix_job_score_history_day", table_name="job_score_history")
    op.drop_table("job_score_history")
    op.drop_column("jobs", "clean_streak")
