"""closed-as-not-planned flag on the stored GitHub issue link

An issue closed as `not_planned` means "known, accepted, will not fix", but the
sync clears the link like any other closed issue, so the filer opens a fresh one
for the same Test — and the team closes it again (issue #32). The new
`github_issue_closed` says the linked issue is closed as not planned while the
link stays: the filer skips any row that has a number, so it files no new issue.
A reopen, or a manual unlink, clears the flag.

Revision ID: 0017
Revises: 0016
Create Date: 2026-11-03
"""

import sqlalchemy as sa
from alembic import op

revision = "0017"
down_revision = "0016"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "test_cases", sa.Column("github_issue_closed", sa.Boolean(), nullable=False, server_default=sa.false())
    )
    op.add_column("jobs", sa.Column("github_issue_closed", sa.Boolean(), nullable=False, server_default=sa.false()))


def downgrade() -> None:
    op.drop_column("jobs", "github_issue_closed")
    op.drop_column("test_cases", "github_issue_closed")
