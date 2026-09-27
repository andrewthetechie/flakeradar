"""report retry cap

Adds `reports.retry_count` so a Report cannot be retried (auto or manually)
more than `max_report_retries` times. Existing rows default to 0.

Revision ID: 0007
Revises: 0006
Create Date: 2026-10-xx
"""

import sqlalchemy as sa
from alembic import op

revision = "0007"
down_revision = "0006"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("reports", sa.Column("retry_count", sa.Integer(), server_default="0", nullable=False))


def downgrade() -> None:
    op.drop_column("reports", "retry_count")
