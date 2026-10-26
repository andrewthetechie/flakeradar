"""provenance marker on a stored GitHub issue number

`github_issue_number` holds two different links: an issue FlakeRadar filed it-
self, and one a human linked through the Test/Job link endpoints or MCP (issue
#33). The admin delete auto-closes every number in the deleted scope, so with no
marker it would comment on and close issues it never filed.

The nullable boolean separates them wherever a number exists: true = filed by
FlakeRadar, false = linked by a human, and NULL counts as linked. A row written
before this revision can carry either kind — the human-link endpoints (issue
#11) predate it — and after the fact nothing tells the two apart, so no row is
backfilled: an existing number keeps NULL, which reads as linked. The cost is
that an issue FlakeRadar filed before this revision is not auto-closed by an
admin delete either; it is only listed in the response for manual close, the
same fallback the README documents for an unconfigured GitHub token.

Revision ID: 0015
Revises: 0014
Create Date: 2026-10-26
"""

import sqlalchemy as sa
from alembic import op

revision = "0015"
down_revision = "0014"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("test_cases", sa.Column("github_issue_filed", sa.Boolean(), nullable=True))
    op.add_column("jobs", sa.Column("github_issue_filed", sa.Boolean(), nullable=True))


def downgrade() -> None:
    op.drop_column("jobs", "github_issue_filed")
    op.drop_column("test_cases", "github_issue_filed")
