"""fixed-note stamp on the stored GitHub issue link

A linked issue says nothing about whether a fix held: the Clean streak lives on
the dashboard only, and a person closes the issue as soon as the PR merges,
before there is evidence (issue #33). `github_issue_fixed_noted_at` records when
FlakeRadar commented that the fix appears to hold, so it posts exactly once per
link. It is NULL for every existing row — nothing has been noted yet — and a
later task sets it back to NULL when the streak resets or the link changes.

Revision ID: 0018
Revises: 0017
Create Date: 2026-11-03
"""

import sqlalchemy as sa
from alembic import op

revision = "0018"
down_revision = "0017"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("test_cases", sa.Column("github_issue_fixed_noted_at", sa.DateTime(timezone=True), nullable=True))
    op.add_column("jobs", sa.Column("github_issue_fixed_noted_at", sa.DateTime(timezone=True), nullable=True))


def downgrade() -> None:
    op.drop_column("jobs", "github_issue_fixed_noted_at")
    op.drop_column("test_cases", "github_issue_fixed_noted_at")
