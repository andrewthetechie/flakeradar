"""Background tasks with Postgres advisory-lock leader election.

Every uvicorn worker runs one ReportWorker. Only the one holding
WORKER_LOCK_KEY is the leader, and the leader runs the two background tasks
(ADR 0002: scoring depends on order, so exactly one processor); the rest stay
on standby and take over if the leader's process or connection dies.

- The processor loop: one Report at a time (``process_next``), plus the
  database-only retention prune.
- The GitHub task (``GitHubWorker``): drains the queue of Test and Job ids the
  processor touched, filing issues for them, and re-checks stored issue links
  every ``prune_interval_seconds``.
- The Notification task (``WebhookWorker``): posts each Report's Tier changes to
  every configured Webhook URL (ADR 0009). It holds no database session and
  stores nothing, so a batch that is not delivered is lost.

The processor hands outcomes over with the non-blocking ``GitHubWorker.enqueue``
and never waits for an API call, so a slow or hanging GitHub cannot stop Report
processing. The same applies to ``WebhookWorker.enqueue``: a slow endpoint
cannot delay processing either, and its queue drops the oldest batch when full.

Queue contents live in the process only. If it dies, the ids queued but not yet
filed are lost — acceptable, because the next Report that touches the Test or
Job files it, the same as when GitHub cannot be reached today. A Notification
queued but not yet delivered is lost the same way, and nothing re-sends it
(ADR 0009). GitHub work never rescores and only writes ``github_issue_number``,
so the one-processor rule of ADR 0002 is untouched.

The lock is session-level and SURVIVES a connection being returned to the
pool, so the leader keeps one dedicated AUTOCOMMIT connection for as long as
it leads and unlocks explicitly before closing it. Losing leadership stops the
GitHub task before the unlock: a replacement leader must never see the lock
free while this process still has an issue POST in flight, or both tasks could
file the same issue.
"""

import asyncio
import logging
from collections.abc import Awaitable, Callable, Sequence
from urllib.parse import urlsplit

import httpx
from sqlalchemy import text
from sqlalchemy.exc import DBAPIError, InvalidRequestError
from sqlalchemy.ext.asyncio import AsyncConnection, AsyncEngine, AsyncSession, async_sessionmaker

from . import github_integration, notify
from .config import WebhookFormat
from .db import WORKER_LOCK_KEY
from .models import REPORT_PENDING
from .processing import ProcessOutcome, process_next

logger = logging.getLogger("flakeradar.worker")

# Ceiling for the wait after a transiently failed Report, in seconds: the
# exponential backoff from ``poll_seconds`` stops growing here.
MAX_RETRY_BACKOFF_SECONDS = 30

# Notification delivery (ADR 0009): 3 attempts, 5 s each, and a wait between
# them. A rate limit's `retry-after` replaces the wait, capped so one endpoint
# cannot park the queue for minutes.
WEBHOOK_RETRY_DELAYS = (0.5, 1.0)
WEBHOOK_MAX_RETRY_AFTER = 10.0
WEBHOOK_TIMEOUT = 5.0
WEBHOOK_RETRYABLE = {408, 429}

OnProcessed = Callable[[ProcessOutcome], Awaitable[None]]
Prune = Callable[[], Awaitable[object]]


class GitHubWorker:
    """Leader-only GitHub task: files issues for queued Report outcomes and re-arms closed links.

    Every GitHub call runs here, never in the processor loop, so a slow or
    hanging GitHub API cannot delay Report processing. ``enqueue`` is a
    non-blocking ``put_nowait``; the worker's own task drains the queue and runs
    ``sync_closed_issues`` on ``sync_interval_seconds``.

    Queue contents live in this process only: if it dies, outcomes that were
    queued but not filed are lost. That is acceptable — the next Report that
    touches the Test or Job files it, the same as when GitHub cannot be reached.
    """

    def __init__(
        self,
        session_factory: async_sessionmaker[AsyncSession],
        *,
        sync_interval_seconds: float = 3600.0,
        transport: httpx.AsyncBaseTransport | None = None,
    ) -> None:
        self._session_factory = session_factory
        self._sync_interval_seconds = sync_interval_seconds
        self._transport = transport
        self._queue: asyncio.Queue[ProcessOutcome] = asyncio.Queue()
        self._last_sync: float | None = None  # loop.time() of the last sync
        self._task: asyncio.Task | None = None

    @property
    def is_running(self) -> bool:
        return self._task is not None and not self._task.done()

    async def start(self) -> None:
        self._task = asyncio.create_task(self._run(), name="flakeradar-github-worker")

    async def stop(self) -> None:
        if self._task is not None:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass
            self._task = None

    async def enqueue(self, outcome: ProcessOutcome) -> None:
        """Hand a processed Report to the GitHub task. Never waits for an API call."""
        self._queue.put_nowait(outcome)

    def _seconds_until_sync(self) -> float | None:
        """Seconds until the next sync, or None when one is due right now."""
        if self._last_sync is None:
            return None
        elapsed = asyncio.get_running_loop().time() - self._last_sync
        if elapsed >= self._sync_interval_seconds:
            return None
        return self._sync_interval_seconds - elapsed

    async def _file(self, outcome: ProcessOutcome) -> None:
        try:
            await github_integration.on_report_processed(self._session_factory, outcome, transport=self._transport)
        except Exception:
            logger.exception("GitHub issue filing failed for Report %s", outcome.report_id)

    async def _sync(self) -> None:
        self._last_sync = asyncio.get_running_loop().time()
        try:
            await github_integration.sync_closed_issues(self._session_factory, transport=self._transport)
        except Exception:
            logger.exception("GitHub issue-state sync failed; will retry next interval")

    async def _run(self) -> None:
        while True:
            seconds = self._seconds_until_sync()
            if seconds is None:
                await self._sync()
                continue
            try:
                outcome = await asyncio.wait_for(self._queue.get(), seconds)
            except TimeoutError:
                continue  # nothing queued before the sync came due
            await self._file(outcome)


def _retry_after_seconds(resp: httpx.Response) -> float | None:
    """The `retry-after` a rate limit asked for, capped; None when absent or unusable."""
    raw = resp.headers.get("retry-after")
    if raw is None:
        return None
    try:
        seconds = float(raw)
    except ValueError:
        return None  # an HTTP-date, which is rare enough to ignore
    return max(0.0, min(seconds, WEBHOOK_MAX_RETRY_AFTER))


class WebhookWorker:
    """Leader-only Notification task: posts a Report's Tier changes to every Webhook URL.

    ``enqueue`` is a non-blocking ``put_nowait``; every POST happens here, so a
    slow or unreachable endpoint never delays Report processing. Delivery is
    best effort (ADR 0009): the queue holds at most ``maxsize`` batches and
    drops the oldest when full, the task stores nothing, and a batch queued
    when the process dies is lost. Three attempts per endpoint with a short
    wait between them; a client error is permanent and is not retried. Never
    raises, and never logs a Webhook URL: the URL is the credential.
    """

    def __init__(
        self,
        urls: Sequence[str],
        *,
        fmt: WebhookFormat = "json",
        public_url: str = "",
        maxsize: int = 1000,
        transport: httpx.AsyncBaseTransport | None = None,
        retry_delays: Sequence[float] = WEBHOOK_RETRY_DELAYS,
    ) -> None:
        self._urls = list(urls)
        self._format = fmt
        self._public_url = public_url
        self._transport = transport
        self._retry_delays = tuple(retry_delays)
        self._queue: asyncio.Queue[list[notify.TierChange]] = asyncio.Queue(maxsize=maxsize)
        self._dropped = 0
        self._task: asyncio.Task | None = None

    @property
    def is_running(self) -> bool:
        return self._task is not None and not self._task.done()

    async def start(self) -> None:
        self._task = asyncio.create_task(self._run(), name="flakeradar-webhook-worker")

    async def stop(self) -> None:
        if self._task is not None:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass
            self._task = None

    async def enqueue(self, outcome: ProcessOutcome) -> None:
        """Queue one Report's Tier changes. Never waits for an endpoint."""
        if not outcome.tier_changes:
            return
        if self._queue.full():
            self._queue.get_nowait()  # the oldest batch is the stalest news
            self._dropped += 1
            logger.warning("Webhook queue full; dropped the oldest batch (%d dropped so far)", self._dropped)
        self._queue.put_nowait(list(outcome.tier_changes))

    async def _run(self) -> None:
        while True:
            batch = await self._queue.get()
            try:
                await asyncio.gather(*(self._post(url, batch) for url in self._urls))
            except Exception:  # _post never raises; never let the task die either
                logger.exception("Webhook delivery failed for a batch of %d event(s)", len(batch))

    @staticmethod
    def _endpoint(url: str) -> str:
        """The host an endpoint belongs to. Never log the URL itself: it is a credential."""
        return urlsplit(url).hostname or "endpoint"

    async def _post(self, url: str, batch: list[notify.TierChange]) -> None:
        body = notify.render(batch, self._format, self._public_url)
        label = self._endpoint(url)
        attempts = len(self._retry_delays) + 1
        for attempt in range(attempts):
            delay = self._retry_delays[attempt] if attempt < len(self._retry_delays) else 0.0
            try:
                async with httpx.AsyncClient(timeout=WEBHOOK_TIMEOUT, transport=self._transport) as client:
                    resp = await client.post(url, json=body)
            except httpx.HTTPError as exc:
                logger.warning(
                    "Webhook %s unreachable (attempt %d/%d): %s", label, attempt + 1, attempts, type(exc).__name__
                )
            else:
                if 200 <= resp.status_code < 300:
                    logger.info("Sent %d Notification(s) to %s", len(batch), label)
                    return
                if resp.status_code not in WEBHOOK_RETRYABLE and resp.status_code < 500:
                    logger.warning("Webhook %s rejected the batch: HTTP %d", label, resp.status_code)
                    return
                logger.warning(
                    "Webhook %s failed: HTTP %d (attempt %d/%d)", label, resp.status_code, attempt + 1, attempts
                )
                delay = _retry_after_seconds(resp) or delay
            if attempt + 1 < attempts:
                await asyncio.sleep(delay)


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
        github: GitHubWorker | None = None,
        webhook: WebhookWorker | None = None,
    ) -> None:
        self._engine = engine
        self._session_factory = session_factory
        self._poll_seconds = poll_seconds
        self._standby_seconds = standby_seconds
        self._on_processed = on_processed
        self._prune = prune
        self._prune_interval_seconds = prune_interval_seconds
        self._github = github
        self._webhook = webhook
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
        if self._github is not None:
            await self._github.start()
        if self._webhook is not None:
            await self._webhook.start()
        logger.info("This process is now the Report processor")
        return True

    async def _release(self) -> None:
        """End leadership: the leader-only tasks stop first, then the lock is given up.

        The order matters. Unlocking while the GitHub task still has a call in
        flight lets a replacement leader take the lock and file the same issue
        twice, so the task is cancelled and awaited before the unlock. The
        Notification task holds no shared state, but it stops here too so a
        shutdown never leaves a POST in flight. Stopping is best effort: if it
        fails, the lock is released anyway rather than be held by a process
        that no longer processes.
        """
        if self._github is not None:
            try:
                await self._github.stop()
            except Exception:
                logger.exception("Stopping the GitHub task failed; releasing the lock anyway")
        if self._webhook is not None:
            try:
                await self._webhook.stop()
            except Exception:
                logger.exception("Stopping the Notification task failed; releasing the lock anyway")
        conn, self._lock_conn = self._lock_conn, None
        if conn is not None:
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
                if self._webhook is not None:
                    # A non-blocking queue put: an outcome with no Tier change is
                    # dropped by WebhookWorker.enqueue itself.
                    await self._webhook.enqueue(outcome)
                if outcome.status == REPORT_PENDING:
                    # A transient (DB-level) failure re-queued this Report: it is
                    # still the oldest pending one, so claiming again right away
                    # would spend the whole retry budget in milliseconds. Wait
                    # out the backoff before the next claim.
                    await asyncio.sleep(min(self._poll_seconds * 2**outcome.retry_count, MAX_RETRY_BACKOFF_SECONDS))
            except asyncio.CancelledError:
                raise
            except Exception:
                logger.exception("Report worker error; dropping leadership and retrying")
                await self._release()
                await asyncio.sleep(self._poll_seconds)
