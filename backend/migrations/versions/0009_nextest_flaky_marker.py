"""nextest flaky marker

cargo-nextest with `flaky-result = "fail"` writes the final try of a flaky
test, which passed, as a <failure> with the message "test passed on attempt
N/M but is configured to fail when flaky". Reports parsed before issue #8
stored that try as failed. This marks those Executions passed, the same as
the parser now does. Each Test is rescored from its stored Executions on its
next Report, so no rescore runs here. Score history and report counts keep
their old values.

Revision ID: 0009
Revises: 0008
Create Date: 2026-10-16
"""

from alembic import op

revision = "0009"
down_revision = "0008"
branch_labels = None
depends_on = None

# The parser's rule, in SQL: a marker message on a try that follows a failing
# try of the same Test in the same Run passed. `~` is Postgres regex match.
BACKFILL_SQL = """
UPDATE test_executions AS e
SET status = 'passed', message = '', details = '', failure_category = NULL
WHERE e.status IN ('failed', 'error')
  AND e.attempt > 0
  AND e.message ~ '^test passed on attempt [0-9]+/[0-9]+ but is configured to fail when flaky$'
  AND EXISTS (
    SELECT 1 FROM test_executions AS prev
    WHERE prev.test_run_id = e.test_run_id
      AND prev.test_case_id = e.test_case_id
      AND prev.attempt < e.attempt
      AND prev.status IN ('failed', 'error')
  )
"""


def upgrade() -> None:
    op.execute(BACKFILL_SQL)


def downgrade() -> None:
    # Data only: the old (wrong) statuses are not restored.
    pass
