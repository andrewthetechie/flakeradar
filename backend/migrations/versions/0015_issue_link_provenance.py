"""provenance marker on a stored GitHub issue number

`github_issue_number` holds two different links: an issue FlakeRadar filed it-
self, and one a human linked through the Test/Job link endpoints or MCP (issue
#33). The admin delete auto-closes every number in the deleted scope, so with no
marker it would comment on and close issues it never filed.

The nullable boolean separates them wherever a number exists: true = filed by
FlakeRadar, false = linked by a human, and NULL counts as linked. A row written
before this revision can only carry a number the filer stored, so existing rows
are backfilled as filed.

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

# Only rows with a stored number get a provenance; a Test/Job without one keeps
# a NULL flag, exactly as the code writes it.
BACKFILL_TESTS_SQL = "UPDATE test_cases SET github_issue_filed = true WHERE github_issue_number IS NOT NULL"
BACKFILL_JOBS_SQL = "UPDATE jobs SET github_issue_filed = true WHERE github_issue_number IS NOT NULL"


def upgrade() -> None:
    op.add_column("test_cases", sa.Column("github_issue_filed", sa.Boolean(), nullable=True))
    op.add_column("jobs", sa.Column("github_issue_filed", sa.Boolean(), nullable=True))
    op.execute(BACKFILL_TESTS_SQL)
    op.execute(BACKFILL_JOBS_SQL)


def downgrade() -> None:
    op.drop_column("jobs", "github_issue_filed")
    op.drop_column("test_cases", "github_issue_filed")
