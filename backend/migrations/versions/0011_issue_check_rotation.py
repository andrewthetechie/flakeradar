"""issue check rotation

sync_closed_issues claimed to check "the 2000 most stale rows" but ordered
by id, so linked Tests and Jobs past the cap were never checked: closed
issues never re-armed the filer's deduplication, and the same low-id rows
spent the GitHub API budget on every run. This adds the per-row check
timestamp the sync's ordering rotates on (NULLS FIRST, then id), plus a
partial index per table so the hourly sync's batch select walks the index
instead of sorting every row (the same role ix_reports_status_id plays for
the pending-Report claim).

No backfill: existing links were never checked, and NULL means "check
first", which is the correct starting state for them.

Revision ID: 0011
Revises: 0010
Create Date: 2026-10-24
"""

import sqlalchemy as sa
from alembic import op
from sqlalchemy import text

revision = "0011"
down_revision = "0010"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("test_cases", sa.Column("github_issue_checked_at", sa.DateTime(timezone=True), nullable=True))
    op.add_column("jobs", sa.Column("github_issue_checked_at", sa.DateTime(timezone=True), nullable=True))
    op.create_index(
        "ix_test_cases_issue_check",
        "test_cases",
        [text("github_issue_checked_at NULLS FIRST"), "id"],
        postgresql_where=text("github_issue_number IS NOT NULL"),
    )
    op.create_index(
        "ix_jobs_issue_check",
        "jobs",
        [text("github_issue_checked_at NULLS FIRST"), "id"],
        postgresql_where=text("github_issue_number IS NOT NULL"),
    )


def downgrade() -> None:
    op.drop_index("ix_jobs_issue_check", table_name="jobs")
    op.drop_index("ix_test_cases_issue_check", table_name="test_cases")
    op.drop_column("jobs", "github_issue_checked_at")
    op.drop_column("test_cases", "github_issue_checked_at")
