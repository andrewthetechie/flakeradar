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
from sqlalchemy.exc import DBAPIError, InvalidRequestError
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
            got = (await conn.execute(text("SELECT pg_try_advisory_lock(:key)"), {"key": WORKER_LOCK_KEY})).scalar()
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
            await conn.execute(text("SELECT pg_advisory_unlock(:key)"), {"key": WORKER_LOCK_KEY})
            await conn.close()
        except (DBAPIError, InvalidRequestError):
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
                        logger.exception("on_processed hook failed for Report %s", outcome.report_id)
            except asyncio.CancelledError:
                raise
            except Exception:
                logger.exception("Report worker error; dropping leadership and retrying")
                await self._release()
                await asyncio.sleep(self._poll_seconds)
