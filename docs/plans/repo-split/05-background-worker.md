# 05 — Background processor with leader lock

## Tracer-Bullet Outcome
With the app running (`uvicorn app.main:app --workers 2`), a CI upload to `/api/ingest` is processed automatically: within about a second, `GET /api/reports/{id}` shows `"status": "processed"` with counts. Exactly one uvicorn worker processes Reports at a time; the log line `This process is now the Report processor` appears once. If that worker dies, another takes over.

## User Story
As the maintainer, I want queued Reports processed continuously by exactly one processor so that scoring stays correct and nothing piles up.

## Description
Create `backend/app/worker.py` with a `ReportWorker` class. It runs an asyncio task that tries to become leader by taking a Postgres **session** advisory lock on a dedicated AUTOCOMMIT connection. As leader, it loops calling `process_next` from task 04. When it is not leader, it retries every `standby_seconds`. The lifespan in `main.py` starts it after migrations and stops it on shutdown. There is a new setting, `worker_poll_seconds`, and the Dockerfile runs 2 uvicorn workers.

Verified end-to-end: a live run of `uvicorn --workers 2` against `postgres:17-alpine` gave `202` on ingest, and the Report was `processed` 2 s later with `counts {"passed":1,"failed":1,...}`. The "now the Report processor" log line appeared exactly once.

## Context Pack
- Source decisions: a single processor via advisory lock; every web worker is on standby (ADR 0002). `--workers 2` in the Dockerfile.
- Repo facts:
  - `app/db.py` (task 01): `WORKER_LOCK_KEY = 726_300_002`, `engine: AsyncEngine`, `SessionLocal: async_sessionmaker[AsyncSession]`.
  - `app/processing.py` (task 04): `async def process_next(session_factory) -> ProcessOutcome | None`, where `ProcessOutcome(report_id: int, status: str, run_id: int | None, counts: dict[str, int] | None, touched_test_ids: list[int], error: str | None)`.
  - Current `backend/app/main.py` lifespan (from task 01):
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
    yield
    await engine.dispose()
```
- Verified external contracts (`00-shared-context.md`):
  - A session advisory lock **survives returning the connection to the pool**, so we hold a dedicated connection and call `pg_advisory_unlock` explicitly.
  - `await conn.execution_options(isolation_level="AUTOCOMMIT")` keeps the holder `idle` rather than `idle in transaction`.
  - `AsyncConnection.invalidate()` discards a broken connection.
- Non-goals: retention pruning (06 adds it into this loop); GitHub filing (07 passes `on_processed`); any metrics endpoint.

## Delivery Strategy
- Shape: Wide refactor: Migrate (integration branch).
- Valid-state scope: Named integration branch `feat/repo-split`.

## Implementation Contract
- Expected files:
  - Create `backend/app/worker.py` and `backend/tests/test_worker.py`.
  - Edit `backend/app/main.py`, `backend/app/config.py` and `backend/Dockerfile`.
- Interfaces and names — `backend/app/worker.py` (verified):
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


class ReportWorker:
    def __init__(
        self,
        engine: AsyncEngine,
        session_factory: async_sessionmaker[AsyncSession],
        *,
        poll_seconds: float = 1.0,
        standby_seconds: float = 5.0,
        on_processed: OnProcessed | None = None,
    ) -> None:
        self._engine = engine
        self._session_factory = session_factory
        self._poll_seconds = poll_seconds
        self._standby_seconds = standby_seconds
        self._on_processed = on_processed
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

    async def _run(self) -> None:
        while True:
            try:
                if not self.is_leader and not await self._try_lead():
                    await asyncio.sleep(self._standby_seconds)
                    continue
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
- `backend/app/config.py`: add this field directly below `cors_origins`:
```python
    # Report processor: idle poll interval when the queue is empty.
    worker_poll_seconds: float = 1.0
```
- `backend/app/main.py`:
  - Change `from .db import engine` to `from .db import SessionLocal, engine`.
  - Add `from .worker import ReportWorker` with the other imports.
  - Replace the lifespan tail so that it reads:
```python
    await run_migrations(engine)
    worker = ReportWorker(engine, SessionLocal, poll_seconds=settings.worker_poll_seconds)
    await worker.start()
    try:
        yield
    finally:
        await worker.stop()
        await engine.dispose()
```
- `backend/Dockerfile`: change the last line to
  `CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000", "--workers", "2"]`
- Behavior rules:
  - The loop:
    1. Not leader → try to lead. On failure, sleep `standby_seconds` and try again.
    2. Leader → `process_next`.
    3. `None` (queue empty) → run `SELECT 1` on the lock connection (this detects a lost connection), then sleep `poll_seconds`.
    4. An outcome → `await on_processed(outcome)` if set. An exception from the hook is logged and swallowed.
  - Any other exception is logged. The worker then drops leadership (`_release`), sleeps `poll_seconds` and loops. `CancelledError` always propagates.
  - `stop()` cancels the task, waits for it, then releases the lock. It is safe to call when never started.
- Error and security rules: log with `logging.getLogger("flakeradar.worker")`. Do not log connection URLs.

## Acceptance Criteria
- [ ] With two pending Reports, a started worker processes both within 10 s, and `on_processed` receives report ids `[1, 2]`.
- [ ] With two workers, exactly one `is_leader` 0.3 s after both start. After `first.stop()`, the second becomes leader.
- [ ] After `stop()`, a fresh connection's `pg_try_advisory_lock(726300002)` returns `True`.
- [ ] Manual smoke (optional; needs Docker): `docker compose up --build`, then `curl -X POST "localhost:8000/api/ingest?repo=acme/app&commit_sha=abc" -H "X-API-Key: $TOKEN" --data-binary @junit.xml`. `GET /api/reports/1` shows `processed` within a few seconds.

## Test Expectations
- Framework: pytest + pytest-asyncio, with the `engine`, `session_factory` and `db` fixtures (real Postgres). Small intervals keep the tests fast.
- `backend/tests/test_worker.py` (verified, 3 passing):
```python
"""ReportWorker: processes the queue; exactly one leader; clean hand-over."""
import asyncio

from sqlalchemy import select, text

from app.db import WORKER_LOCK_KEY
from app.models import Report
from app.worker import ReportWorker
from tests.conftest import make_junit
from tests.factories import make_project, make_report


async def _wait_for(predicate, timeout: float = 10.0) -> None:
    deadline = asyncio.get_running_loop().time() + timeout
    while not await predicate():
        assert asyncio.get_running_loop().time() < deadline, "timed out"
        await asyncio.sleep(0.05)


def _worker(engine, session_factory, **kw) -> ReportWorker:
    return ReportWorker(engine, session_factory, poll_seconds=0.05,
                        standby_seconds=0.1, **kw)


async def test_worker_processes_pending_reports(engine, session_factory, db):
    proj = await make_project(db)
    for sha in ("a", "b"):
        await make_report(db, proj, make_junit([("t", "passed")]), commit_sha=sha)
    await db.commit()
    seen = []

    async def on_processed(outcome):
        seen.append(outcome.report_id)

    worker = _worker(engine, session_factory, on_processed=on_processed)
    await worker.start()
    try:
        async def all_done():
            async with session_factory() as s:
                statuses = (await s.execute(select(Report.status))).scalars().all()
            return statuses == ["processed", "processed"]
        await _wait_for(all_done)
    finally:
        await worker.stop()
    assert sorted(seen) == [1, 2]


async def test_single_leader_and_takeover(engine, session_factory):
    first = _worker(engine, session_factory)
    second = _worker(engine, session_factory)
    await first.start()

    async def first_leads():
        return first.is_leader
    await _wait_for(first_leads)
    await second.start()
    await asyncio.sleep(0.3)
    assert first.is_leader and not second.is_leader

    await first.stop()

    async def second_leads():
        return second.is_leader
    await _wait_for(second_leads)
    await second.stop()


async def test_stop_releases_the_lock(engine, session_factory):
    worker = _worker(engine, session_factory)
    await worker.start()

    async def leads():
        return worker.is_leader
    await _wait_for(leads)
    await worker.stop()
    async with engine.connect() as conn:
        got = (await conn.execute(text("SELECT pg_try_advisory_lock(:k)"),
                                  {"k": WORKER_LOCK_KEY})).scalar()
        assert got is True
        await conn.execute(text("SELECT pg_advisory_unlock(:k)"), {"k": WORKER_LOCK_KEY})
```

## Dependencies
- Blocked by: 04 — Report processor
- Why blocked: the loop calls `process_next` and passes `ProcessOutcome` to the hook.
- Blocks: 06 (adds pruning to the loop), 07 (supplies `on_processed`)

## Labels
`feature`, `backend`, `ingest`, `priority:high`

## Estimate
Medium

## Risk
3 - Concurrency and leader election. The lock-survives-pool gotcha is covered by `test_stop_releases_the_lock` and the takeover test.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q tests/test_worker.py tests/test_processing.py tests/test_ingest_api.py tests/test_reports_api.py tests/test_identity.py tests/test_parsing.py tests/test_db.py tests/test_scoring.py
# expect: 61 passed
```
