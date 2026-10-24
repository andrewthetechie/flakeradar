"""report body digest for ingest deduplication

`POST /api/ingest` stored every upload, so a CI retry whose response was lost
(`curl --retry 5 --retry-all-errors`) created a second Report and a second Run
with the same Executions: duplicate failures, doubled score history counts, and
a duplicated GitHub issue gate. Real reports differ in timestamps and durations,
so identical bytes mean a retried upload. This stores the SHA-256 of the body
next to it, plus the index the duplicate lookup walks.

No backfill: rows without a digest simply never match, and no unique constraint:
the check is best effort, and two parallel identical uploads may both insert.

Revision ID: 0012
Revises: 0011
Create Date: 2026-10-24
"""

import sqlalchemy as sa
from alembic import op

revision = "0012"
down_revision = "0011"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("reports", sa.Column("body_sha256", sa.String(64), nullable=True))
    op.create_index("ix_reports_project_body_sha256", "reports", ["project_id", "body_sha256"])


def downgrade() -> None:
    op.drop_index("ix_reports_project_body_sha256", table_name="reports")
    op.drop_column("reports", "body_sha256")
