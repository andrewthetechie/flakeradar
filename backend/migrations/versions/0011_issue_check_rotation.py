"""issue check rotation

sync_closed_issues claimed to check "the 2000 most stale rows" but ordered
by id, so linked Tests and Jobs past the cap were never checked: closed
issues never re-armed the filer's deduplication, and the same low-id rows
spent the GitHub API budget on every run. This adds the per-row check
timestamp the sync's ordering rotates on (NULLS FIRST, then id).

No backfill: existing links were never checked, and NULL means "check
first", which is the correct starting state for them.

Revision ID: 0011
Revises: 0010
Create Date: 2026-10-24
"""

import sqlalchemy as sa
from alembic import op

revision = "0011"
down_revision = "0010"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("test_cases", sa.Column("github_issue_checked_at", sa.DateTime(timezone=True), nullable=True))
    op.add_column("jobs", sa.Column("github_issue_checked_at", sa.DateTime(timezone=True), nullable=True))


def downgrade() -> None:
    op.drop_column("jobs", "github_issue_checked_at")
    op.drop_column("test_cases", "github_issue_checked_at")
