"""ingest line_base on the Report

pytest's `junit_family=xunit1` writes the JUnit `line` attribute 0-based, so
permalinks built from a stored line pointed one line above the test. Ingest now
takes a `line_base` parameter and stores it here, because the Report is only
parsed later by the processor (see docs/adr/0002-queued-ingest.md), which needs
the value to normalize the lines it stores on the TestCase.

Nullable and never backfilled: rows ingested before the parameter existed are
read as 1-based, exactly how they were processed until now.

Revision ID: 0013
Revises: 0012
Create Date: 2026-10-24
"""

import sqlalchemy as sa
from alembic import op

revision = "0013"
down_revision = "0012"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("reports", sa.Column("line_base", sa.Integer(), nullable=True))


def downgrade() -> None:
    op.drop_column("reports", "line_base")
