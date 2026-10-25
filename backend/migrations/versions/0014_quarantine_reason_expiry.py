"""quarantine reason and expiry on the TestCase

Quarantine was one boolean and a timestamp, so the person who decided could not
record why, nor when to look again. This adds the two values the decision comes
with: a free-text reason, and an optional expiry the leader's prune step can act
on (issue #30).

Both nullable and never backfilled: a quarantine set before this existed stays
exactly as it was, with no reason and no end date.

Revision ID: 0014
Revises: 0013
Create Date: 2026-10-25
"""

import sqlalchemy as sa
from alembic import op

revision = "0014"
down_revision = "0013"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("test_cases", sa.Column("quarantine_reason", sa.Text(), nullable=True))
    op.add_column("test_cases", sa.Column("quarantine_expires_at", sa.DateTime(timezone=True), nullable=True))


def downgrade() -> None:
    op.drop_column("test_cases", "quarantine_expires_at")
    op.drop_column("test_cases", "quarantine_reason")
