"""execution attempt: number retries of a Test within one Run (ADR 0005)

Adds `test_executions.attempt` (0 = first try in the Run) so retries from
Playwright `flaky*` / `rerun*` elements and Surefire each become their own
Execution. Existing rows default to 0.

Revision ID: 0003
Revises: 0002
Create Date: 2026-09-26
"""

import sqlalchemy as sa
from alembic import op

revision = "0003"
down_revision = "0002"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("test_executions", sa.Column("attempt", sa.Integer(), nullable=False, server_default="0"))


def downgrade() -> None:
    op.drop_column("test_executions", "attempt")
