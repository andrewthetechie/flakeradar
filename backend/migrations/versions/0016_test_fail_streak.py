"""fail_streak on test_cases

A Test that fails every run scores 0, the same tier as one that always passes,
so the leaderboard hides it and a merge gate misreads its failure as new (issue
#29). The new `fail_streak` — consecutive non-skipped failing Executions on the
Default branch, newest first, the reverse of `clean_streak` — flags it as broken.
Existing rows get 0 and are correct after their next rescore.

Revision ID: 0016
Revises: 0015
Create Date: 2026-10-26
"""

import sqlalchemy as sa
from alembic import op

revision = "0016"
down_revision = "0015"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("test_cases", sa.Column("fail_streak", sa.Integer(), nullable=False, server_default="0"))


def downgrade() -> None:
    op.drop_column("test_cases", "fail_streak")
