"""failure categories: likely cause of a failing Execution (ADR 0006)

Adds `test_executions.failure_category` (set at ingest for failed/error
rows) and `test_cases.failure_category` (the dominant category over the
score window, cached at rescore). Both are nullable: there is no backfill,
so existing rows stay NULL.

Revision ID: 0004
Revises: 0003
Create Date: 2026-09-26
"""

import sqlalchemy as sa
from alembic import op

revision = "0004"
down_revision = "0003"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("test_executions", sa.Column("failure_category", sa.String(length=16), nullable=True))
    op.add_column("test_cases", sa.Column("failure_category", sa.String(length=16), nullable=True))


def downgrade() -> None:
    op.drop_column("test_cases", "failure_category")
    op.drop_column("test_executions", "failure_category")
