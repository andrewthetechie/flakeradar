# 06 — Retention pruning

## Tracer-Bullet Outcome
While the app runs, the processor that holds the lock deletes, once an hour:
- processed Reports whose `processed_at` is more than 7 days old;
- Executions more than 90 days old;
- Runs left with no Executions.

Failed Reports are never pruned automatically. The log shows `Pruned N reports, N executions, N runs`.

## User Story
As the maintainer of a small self-hosted instance, I want old raw uploads and ancient history deleted automatically, so that the database stays small and the dashboard queries stay fast.

## Description
Add `backend/app/retention.py` with a pure-DB `prune()` and a `run_prune(session_factory)` wrapper that reads the settings. Extend `ReportWorker` (task 05) with an optional `prune` hook that runs at most once per `prune_interval_seconds`, only while leading. Add three settings, and wire the hook in `main.py`.

## Context Pack
- Source decisions: processed Reports are kept 7 days and Executions 90 days (Runs left empty go with them); failed Reports are kept; pruning runs hourly on the leader (`00-shared-context.md` → Retention).
- Repo facts:
  - Models (task 01): `Report.status`/`processed_at`, `TestExecution.created_at` (indexed), `TestRun.created_at`.
  - `reports.run_id` is `ON DELETE SET NULL`, so deleting a Run never deletes a Report.
  - `test_executions.test_run_id` is `ON DELETE CASCADE`.
  - Factories (task 01): `make_project`, `make_test_case`, `make_run(db, project, commit_sha="sha1", branch="main", ci_run_id="", created_at=None)`, `make_execution(db, test_case, run, status="passed", message="", details="", created_at=None)`, `make_report(db, project, body, …, status="pending")`.
  - The `ReportWorker` from task 05 is in `backend/app/worker.py`. This task edits it; the full target file is below.
- Verified external contracts: `delete(Model).where(...)` returns a result whose `.rowcount` is the number of deleted rows (asyncpg; verified by the test below). The correlated `~exists(select(...).where(...))` works in a DELETE's WHERE clause (verified).
- Non-goals: pruning Tests (they keep their cached score); deleting failed Reports; an admin endpoint to trigger pruning.

## Delivery Strategy
- Shape: Wide refactor: Migrate (integration branch).
- Valid-state scope: Named integration branch `feat/repo-split`.

## Implementation Contract
- Expected files:
  - Create `backend/app/retention.py` and `backend/tests/test_retention.py`.
  - Edit `backend/app/worker.py`, `backend/app/config.py` and `backend/app/main.py`.
- `backend/app/retention.py` (verified):
```python
"""Retention: delete old processed Reports and old Executions.

Scores are cached on each Test, so pruning history never changes a score
until the Test's next Run rescores it from what remains. Failed Reports are
kept until a human deals with them.
"""
import logging
from dataclasses import dataclass
from datetime import datetime, timedelta

from sqlalchemy import delete, exists, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from .config import get_settings
from .models import REPORT_PROCESSED, Report, TestExecution, TestRun, utcnow

logger = logging.getLogger("flakeradar.retention")


@dataclass(frozen=True)
class PruneResult:
    reports: int
    executions: int
    runs: int


async def prune(
    db: AsyncSession, *, now: datetime, report_days: int, execution_days: int
) -> PruneResult:
    """Delete expired rows. Caller owns the transaction."""
    report_cutoff = now - timedelta(days=report_days)
    execution_cutoff = now - timedelta(days=execution_days)

    reports = await db.execute(
        delete(Report).where(
            Report.status == REPORT_PROCESSED, Report.processed_at < report_cutoff
        )
    )
    executions = await db.execute(
        delete(TestExecution).where(TestExecution.created_at < execution_cutoff)
    )
    runs = await db.execute(
        delete(TestRun).where(
            TestRun.created_at < execution_cutoff,
            ~exists(select(TestExecution.id).where(TestExecution.test_run_id == TestRun.id)),
        )
    )
    return PruneResult(reports=reports.rowcount, executions=executions.rowcount,
                       runs=runs.rowcount)


async def run_prune(session_factory: async_sessionmaker[AsyncSession]) -> PruneResult:
    settings = get_settings()
    async with session_factory() as db, db.begin():
        result = await prune(
            db, now=utcnow(),
            report_days=settings.report_retention_days,
            execution_days=settings.execution_retention_days,
        )
    logger.info("Pruned %s reports, %s executions, %s runs",
                result.reports, result.executions, result.runs)
    return result
```
- `backend/app/worker.py` — full target file after this task (adds `Prune`, the constructor params `prune`/`prune_interval_seconds`, `_maybe_prune`, and one call in `_run`):
```python
"""Background Report processor with Postgres advisory-lock leader election.

Every uvicorn worker runs one ReportWorker. Only the one holding
WORKER_LOCK_KEY processes Reports (ADR 0002: scoring depends on order, so
exactly one processor); the rest stay on standby and take over if the
leader's process or connection dies.

The lock is session-level and SURVIVES a connection being returned to the
pool, so the leader keeps one dedicated AUTOCOMMIT connection for as long as
it leads and unlocks explicitly before closing it.
"""
import asyncio
import logging
from collections.abc import Awaitable, Callable

from sqlalchemy import text
from sqlalchemy.ext.asyncio import AsyncConnection, AsyncEngine, AsyncSession, async_sessionmaker

from .db import WORKER_LOCK_KEY
from .processing import ProcessOutcome, process_next

logger = logging.getLogger("flakeradar.worker")

OnProcessed = Callable[[ProcessOutcome], Awaitable[None]]
Prune = Callable[[], Awaitable[object]]


class ReportWorker:
    def __init__(
        self,
        engine: AsyncEngine,
        session_factory: async_sessionmaker[AsyncSession],
        *,
        poll_seconds: float = 1.0,
        standby_seconds: float = 5.0,
        on_processed: OnProcessed | None = None,
        prune: Prune | None = None,
        prune_interval_seconds: float = 3600.0,
    ) -> None:
        self._engine = engine
        self._session_factory = session_factory
        self._poll_seconds = poll_seconds
        self._standby_seconds = standby_seconds
        self._on_processed = on_processed
        self._prune = prune
        self._prune_interval_seconds = prune_interval_seconds
        self._last_prune: float | None = None  # loop.time() of the last prune
        self._lock_conn: AsyncConnection | None = None
        self._task: asyncio.Task | None = None

    @property
    def is_leader(self) -> bool:
        return self._lock_conn is not None

    async def start(self) -> None:
        self._task = asyncio.create_task(self._run(), name="flakeradar-report-worker")

    async def stop(self) -> None:
        if self._task is not None:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass
            self._task = None
        await self._release()

    async def _try_lead(self) -> bool:
        conn = await self._engine.connect()
        try:
            await conn.execution_options(isolation_level="AUTOCOMMIT")
            got = (await conn.execute(
                text("SELECT pg_try_advisory_lock(:key)"), {"key": WORKER_LOCK_KEY}
            )).scalar()
        except BaseException:
            await conn.close()
            raise
        if not got:
            await conn.close()
            return False
        self._lock_conn = conn
        logger.info("This process is now the Report processor")
        return True

    async def _release(self) -> None:
        conn, self._lock_conn = self._lock_conn, None
        if conn is None:
            return
        try:
            await conn.execute(
                text("SELECT pg_advisory_unlock(:key)"), {"key": WORKER_LOCK_KEY}
            )
            await conn.close()
        except Exception:
            # Connection is broken: the server already dropped the lock.
            await conn.invalidate()

    async def _lock_alive(self) -> None:
        assert self._lock_conn is not None
        await self._lock_conn.execute(text("SELECT 1"))

    async def _maybe_prune(self) -> None:
        """Leader-only housekeeping, at most once per prune interval."""
        if self._prune is None:
            return
        now = asyncio.get_running_loop().time()
        if self._last_prune is not None and now - self._last_prune < self._prune_interval_seconds:
            return
        self._last_prune = now
        try:
            await self._prune()
        except Exception:
            logger.exception("Retention prune failed; will retry next interval")

    async def _run(self) -> None:
        while True:
            try:
                if not self.is_leader and not await self._try_lead():
                    await asyncio.sleep(self._standby_seconds)
                    continue
                await self._maybe_prune()
                outcome = await process_next(self._session_factory)
                if outcome is None:
                    await self._lock_alive()
                    await asyncio.sleep(self._poll_seconds)
                    continue
                if self._on_processed is not None:
                    try:
                        await self._on_processed(outcome)
                    except Exception:
                        logger.exception("on_processed hook failed for Report %s",
                                         outcome.report_id)
            except asyncio.CancelledError:
                raise
            except Exception:
                logger.exception("Report worker error; dropping leadership and retrying")
                await self._release()
                await asyncio.sleep(self._poll_seconds)
```
- `backend/app/config.py`: add below `worker_poll_seconds`:
```python
    # Retention (pruned hourly by the Report processor).
    report_retention_days: int = 7        # processed Reports; failed ones are kept
    execution_retention_days: int = 90    # Executions (and Runs left empty)
    prune_interval_seconds: float = 3600.0
```
- `backend/app/main.py`: add `from .retention import run_prune` to the imports. The lifespan becomes:
```python
@asynccontextmanager
async def lifespan(app: FastAPI):
    settings = get_settings()
    assert_secure_token(settings.api_token)
    if settings.api_token == DEFAULT_INSECURE_TOKEN:
        logger.warning(
            "FLAKERADAR_API_TOKEN is the default 'changeme' — set a real token."
        )
    await run_migrations(engine)
    worker = ReportWorker(
        engine, SessionLocal,
        poll_seconds=settings.worker_poll_seconds,
        prune=lambda: run_prune(SessionLocal),
        prune_interval_seconds=settings.prune_interval_seconds,
    )
    await worker.start()
    try:
        yield
    finally:
        await worker.stop()
        await engine.dispose()
```
- Behavior rules:
  - The cutoffs are strict (`<`): a row exactly at the cutoff stays.
  - The first prune runs on the leader's first loop pass, and after that at most once per interval.
  - A prune failure is logged (`Retention prune failed; will retry next interval`) and never stops processing.
- Error and security rules: none beyond logging counts only.

## Acceptance Criteria
- [ ] With `now`, `report_days=7` and `execution_days=90`:
  - a processed Report 8 days old is deleted, and one 6 days old stays;
  - a failed Report 30 days old stays;
  - an Execution and Run 100 days old are deleted, and ones 1 day old stay;
  - the result is `PruneResult(reports=1, executions=1, runs=1)`.
- [ ] A worker with `prune_interval_seconds=3600` calls `prune` exactly once in 0.5 s of running.

## Test Expectations
- Framework: pytest + pytest-asyncio against a real Postgres.
- `backend/tests/test_retention.py` (verified, 2 passing):
```python
"""Retention: old processed Reports and old Executions go; the rest stays."""
import asyncio
from datetime import timedelta

from sqlalchemy import func, select

from app.models import REPORT_FAILED, REPORT_PROCESSED, Report, TestExecution, TestRun, utcnow
from app.retention import prune
from app.worker import ReportWorker
from tests.factories import (
    make_execution, make_project, make_report, make_run, make_test_case,
)


async def _count(db, column) -> int:
    return (await db.execute(select(func.count(column)))).scalar()


async def test_prune_deletes_only_expired_rows(db):
    now = utcnow()
    old, recent = now - timedelta(days=100), now - timedelta(days=1)
    proj = await make_project(db)
    tc = await make_test_case(db, proj)

    old_run = await make_run(db, proj, commit_sha="old", created_at=old)
    await make_execution(db, tc, old_run, created_at=old)
    new_run = await make_run(db, proj, commit_sha="new", created_at=recent)
    await make_execution(db, tc, new_run, created_at=recent)

    stale = await make_report(db, proj, b"<x/>", status=REPORT_PROCESSED)
    stale.processed_at, stale.run_id = now - timedelta(days=8), old_run.id
    fresh = await make_report(db, proj, b"<x/>", status=REPORT_PROCESSED)
    fresh.processed_at = now - timedelta(days=6)
    failed = await make_report(db, proj, b"<x/>", status=REPORT_FAILED)
    failed.processed_at = now - timedelta(days=30)
    await db.commit()

    result = await prune(db, now=now, report_days=7, execution_days=90)
    await db.commit()

    assert (result.reports, result.executions, result.runs) == (1, 1, 1)
    remaining = set((await db.execute(select(Report.id))).scalars())
    assert remaining == {fresh.id, failed.id}
    assert await _count(db, TestExecution.id) == 1
    assert (await db.execute(select(TestRun.commit_sha))).scalars().all() == ["new"]


async def test_worker_prunes_once_per_interval(engine, session_factory):
    calls = []

    async def fake_prune():
        calls.append(1)

    worker = ReportWorker(engine, session_factory, poll_seconds=0.05, standby_seconds=0.1,
                          prune=fake_prune, prune_interval_seconds=3600)
    await worker.start()
    try:
        await asyncio.sleep(0.5)
    finally:
        await worker.stop()
    assert calls == [1]
```

## Dependencies
- Blocked by: 05 — Background processor with leader lock
- Why blocked: pruning runs inside the leader's loop, which `ReportWorker` provides.
- Blocks: 14 (documents the retention settings)

## Labels
`feature`, `backend`, `database`, `priority:medium`

## Estimate
Small

## Risk
2 - Deletes data. Bounded by strict cutoffs and a test that checks exactly what survives.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q   # the whole backend suite; expect: 63 passed
```
(From this task on, every backend test file present should pass. `app/github_integration.py` is still unimported and has no tests until task 07.)
