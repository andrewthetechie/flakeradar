"""summary jobs

Adds `jobs.summary`: a summary Job runs no tests and only reports whether the
Jobs it `needs` succeeded (the alls-green pattern). Its failure is explained
when a non-summary Job failed in the same CI run attempt, and an explained
failure is never scored. So a red CI run caused by a known flake does not also
raise the summary Job's score (issue #9).

No backfill: a Job is a summary Job only once its reporter says so. Existing
Jobs stay non-summary and are scored as before.

Revision ID: 0010
Revises: 0009
Create Date: 2026-10-17
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
