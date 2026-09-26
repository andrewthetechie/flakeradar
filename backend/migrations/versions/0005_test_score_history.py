"""test score history: daily snapshots per Test (ADR 0007)

Adds `test_cases.clean_streak` and the `test_score_history` table (one row
per Test per UTC day). No backfill: history starts the day this ships.

Revision ID: 0005
Revises: 0004
Create Date: 2026-09-26
"""

import sqlalchemy as sa
from alembic import op

revision = "0005"
down_revision = "0004"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("test_cases", sa.Column("clean_streak", sa.Integer(), nullable=False, server_default="0"))
    op.create_table(
        "test_score_history",
        sa.Column("test_case_id", sa.Integer(), sa.ForeignKey("test_cases.id", ondelete="CASCADE"), primary_key=True),
        sa.Column("day", sa.Date(), primary_key=True),
        sa.Column("flakiness_score", sa.Float(), nullable=False),
        sa.Column("confirmed_flake_count", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("executions", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("failures", sa.Integer(), nullable=False, server_default="0"),
        sa.PrimaryKeyConstraint("test_case_id", "day"),
    )
    op.create_index("ix_test_score_history_day", "test_score_history", ["day"])


def downgrade() -> None:
    op.drop_index("ix_test_score_history_day", table_name="test_score_history")
    op.drop_table("test_score_history")
    op.drop_column("test_cases", "clean_streak")
