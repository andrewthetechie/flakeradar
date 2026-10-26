"""Operational reads: the readiness probe and the Prometheus scrape (ADR 0002).

Both endpoints answer from the database alone, so any uvicorn worker returns the
same result and neither needs a token, like the other read APIs.
"""

from fastapi import APIRouter, Depends
from fastapi.responses import JSONResponse, PlainTextResponse
from sqlalchemy import text
from sqlalchemy.exc import SQLAlchemyError
from sqlalchemy.ext.asyncio import AsyncSession

from .. import schemas
from ..config import get_settings
from ..db import WORKER_LOCK_KEY, get_db
from ..models import REPORT_PENDING, REPORT_STATUSES

router = APIRouter()

# Prometheus text exposition format, version 0.0.4.
PROMETHEUS_MEDIA_TYPE = "text/plain; version=0.0.4; charset=utf-8"

# The tiers are the cached score columns, classified exactly as queries.tier_for
# does: at or above the Flake threshold is flaky, above 0 suspect, 0 stable.
TIER_ORDER: tuple[schemas.Tier, ...] = ("flaky", "suspect", "stable")

# One roundtrip for the queue facts. A one-argument session advisory lock (the
# form ReportWorker holds) appears in pg_locks as classid = 0, objid = key, which
# is distinct from the transaction-scoped migration lock on another key. The probe
# is scoped to the current database because pg_locks is a cluster-wide view: the
# same key in another database of the cluster belongs to that environment's leader.
_QUEUE_SQL = text(
    f"""
    SELECT
      EXISTS(SELECT 1 FROM pg_locks
             WHERE locktype = 'advisory' AND classid = 0 AND objid = :lock_key
               AND database = (SELECT oid FROM pg_database WHERE datname = current_database())) AS leader,
      (SELECT count(*) FROM reports WHERE status = '{REPORT_PENDING}') AS pending,
      (SELECT coalesce(extract(epoch FROM (now() - min(created_at))), 0)
         FROM reports WHERE status = '{REPORT_PENDING}') AS oldest
    """
)

_TIER_COUNT_SQL = """
    SELECT CASE WHEN flakiness_score >= :threshold THEN 'flaky'
                WHEN flakiness_score > 0 THEN 'suspect'
                ELSE 'stable' END AS tier,
           count(*)
    FROM {table} GROUP BY 1
"""

# Long HELP lines, kept as named constants so the render below stays readable.
_OLDEST_PENDING_HELP = (
    "# HELP flakeradar_oldest_pending_report_age_seconds Age in seconds of the oldest pending Report;"
    " 0 when the queue is empty."
)
_LAST_PROCESSED_HELP = (
    "# HELP flakeradar_last_report_processed_timestamp_seconds Unix time of the most recently processed"
    " Report; absent until the first Report is processed."
)


@router.get("/api/health/ready", response_model=schemas.ReadinessOut)
async def ready(db: AsyncSession = Depends(get_db)):
    try:
        await db.execute(text("SELECT 1"))
    except SQLAlchemyError:
        # Nothing else is knowable without a database.
        return JSONResponse(status_code=503, content={"database": False})
    row = (await db.execute(_QUEUE_SQL, {"lock_key": WORKER_LOCK_KEY})).one()
    # A missing leader is body information only: during a redeploy the lock
    # moves between processes, and the replacement must not be kept unready.
    return schemas.ReadinessOut(
        database=True,
        processor_leader=row.leader,
        pending_reports=row.pending,
        oldest_pending_age_seconds=float(row.oldest),
    )


@router.get("/metrics")
async def metrics(db: AsyncSession = Depends(get_db)):
    threshold = get_settings().flake_threshold
    try:
        queue = (await db.execute(_QUEUE_SQL, {"lock_key": WORKER_LOCK_KEY})).one()
        statuses = dict((await db.execute(text("SELECT status, count(*) FROM reports GROUP BY status"))).all())
        tests = dict(
            (await db.execute(text(_TIER_COUNT_SQL.format(table="test_cases")), {"threshold": threshold})).all()
        )
        jobs = dict((await db.execute(text(_TIER_COUNT_SQL.format(table="jobs")), {"threshold": threshold})).all())
        last_processed = (await db.execute(text("SELECT extract(epoch FROM max(processed_at)) FROM reports"))).scalar()
    except SQLAlchemyError:
        # A scrape during a database outage fails like the ready check.
        return PlainTextResponse("", status_code=503, media_type=PROMETHEUS_MEDIA_TYPE)

    lines = [
        "# HELP flakeradar_reports Reports by current status.",
        "# TYPE flakeradar_reports gauge",
    ]
    lines += [f'flakeradar_reports{{status="{status}"}} {statuses.get(status, 0)}' for status in REPORT_STATUSES]
    lines += [
        _OLDEST_PENDING_HELP,
        "# TYPE flakeradar_oldest_pending_report_age_seconds gauge",
        f"flakeradar_oldest_pending_report_age_seconds {float(queue.oldest)}",
        "# HELP flakeradar_processor_leader 1 when a process holds the Report processor advisory lock.",
        "# TYPE flakeradar_processor_leader gauge",
        f"flakeradar_processor_leader {1 if queue.leader else 0}",
    ]
    for metric, counts, help_text in (
        ("flakeradar_tests", tests, "Tests by tier (flaky, suspect, stable)."),
        ("flakeradar_jobs", jobs, "Jobs by tier (flaky, suspect, stable)."),
    ):
        lines += [f"# HELP {metric} {help_text}", f"# TYPE {metric} gauge"]
        lines += [f'{metric}{{tier="{tier}"}} {counts.get(tier, 0)}' for tier in TIER_ORDER]
    lines += [
        _LAST_PROCESSED_HELP,
        "# TYPE flakeradar_last_report_processed_timestamp_seconds gauge",
    ]
    if last_processed is not None:
        lines.append(f"flakeradar_last_report_processed_timestamp_seconds {float(last_processed)}")
    return PlainTextResponse("\n".join(lines) + "\n", media_type=PROMETHEUS_MEDIA_TYPE)
