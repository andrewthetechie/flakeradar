"""summary jobs

Adds `jobs.summary`: a summary Job runs no tests and only reports whether the
Jobs it `needs` succeeded (the alls-green pattern). Its failure is explained
when another failed Job exists in the same CI run attempt, and it is never
scored, so a red build caused by a known flake does not also raise the
summary Job's score (issue #9).

No backfill: a Job is a summary Job only once its reporter says so. Existing
Jobs stay non-summary and are scored as before.

Revision ID: 0010
Revises: 0009
Create Date: 2026-10-19
"""

import sqlalchemy as sa
from alembic import op

revision = "0010"
down_revision = "0009"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("jobs", sa.Column("summary", sa.Boolean(), nullable=False, server_default=sa.false()))


def downgrade() -> None:
    op.drop_column("jobs", "summary")
