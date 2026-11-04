"""WebhookWorker: Notification delivery runs off the processor's hot path."""

import asyncio
import json

import httpx
import pytest
from app.processing import ProcessOutcome
from app.worker import WebhookWorker

from tests.test_github_worker import _wait_for
from tests.test_webhook import _job_change, _test_change

URL = "https://hooks.example.com/services/SECRETTOKEN"


def _outcome(*changes) -> ProcessOutcome:
    return ProcessOutcome(
        report_id=7,
        status="processed",
        run_id=1,
        counts={},
        touched_test_ids=[],
        error=None,
        tier_changes=list(changes),
    )


def _worker(transport, urls=(URL,), **kw) -> WebhookWorker:
    kw.setdefault("public_url", "")
    kw.setdefault("retry_delays", (0.0, 0.0))
    return WebhookWorker(urls, transport=transport, **kw)


async def test_enqueue_posts_one_request_per_report():
    """A Report that crosses several tiers is one request carrying a list."""
    calls: list[httpx.Request] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        await asyncio.sleep(0.2)  # a slow endpoint must not delay enqueue
        return httpx.Response(200)

    worker = _worker(httpx.MockTransport(handler))
    await worker.start()
    try:
        await worker.enqueue(_outcome(_test_change(), _job_change()))
        assert calls == []  # enqueue returned before the POST was issued
        await _wait_for(lambda: len(calls) == 1)
        assert calls[0].method == "POST"
        assert calls[0].url.host == "hooks.example.com"
        body = json.loads(calls[0].content)
        assert [event["event"] for event in body] == ["test.flaky", "job.flaky"]
    finally:
        await worker.stop()


async def test_an_outcome_without_events_posts_nothing():
    calls: list[httpx.Request] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        return httpx.Response(200)

    worker = _worker(httpx.MockTransport(handler))
    await worker.start()
    try:
        await worker.enqueue(_outcome())
        await asyncio.sleep(0.05)
        assert calls == []
    finally:
        await worker.stop()


async def test_retries_a_transient_failure_then_succeeds():
    statuses = [503, 500, 200]
    calls: list[int] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(1)
        return httpx.Response(statuses[len(calls) - 1])

    worker = _worker(httpx.MockTransport(handler))
    await worker.start()
    try:
        await worker.enqueue(_outcome(_test_change()))
        await _wait_for(lambda: len(calls) == 3)
    finally:
        await worker.stop()


async def test_gives_up_after_three_attempts_and_keeps_serving():
    calls: list[int] = []
    failing = True

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(1)
        return httpx.Response(503 if failing else 200)

    worker = _worker(httpx.MockTransport(handler))
    await worker.start()
    try:
        await worker.enqueue(_outcome(_test_change()))
        await _wait_for(lambda: len(calls) == 3)
        await asyncio.sleep(0.1)
        assert len(calls) == 3  # no fourth attempt, and no crash
        failing = False
        await worker.enqueue(_outcome(_job_change()))
        await _wait_for(lambda: len(calls) == 4)
    finally:
        await worker.stop()


@pytest.mark.parametrize("status", [400, 401, 403, 404])
async def test_a_client_error_is_not_retried(status):
    calls: list[int] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(1)
        return httpx.Response(status)

    worker = _worker(httpx.MockTransport(handler))
    await worker.start()
    try:
        await worker.enqueue(_outcome(_test_change()))
        await _wait_for(lambda: len(calls) == 1)
        await asyncio.sleep(0.1)
        assert len(calls) == 1
    finally:
        await worker.stop()


async def test_a_rate_limit_waits_as_long_as_retry_after_says():
    calls: list[int] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(1)
        return httpx.Response(429, headers={"retry-after": "1"})

    worker = _worker(httpx.MockTransport(handler), retry_delays=(0.0, 0.0))
    await worker.start()
    try:
        loop = asyncio.get_running_loop()
        started = loop.time()
        await worker.enqueue(_outcome(_test_change()))
        await _wait_for(lambda: loop.time() - started > 0.9 and len(calls) == 2)
    finally:
        await worker.stop()


async def test_an_unreachable_endpoint_never_raises():
    calls: list[int] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(1)
        raise httpx.ConnectError("unreachable", request=request)

    worker = _worker(httpx.MockTransport(handler))
    await worker.start()
    try:
        await worker.enqueue(_outcome(_test_change()))
        await _wait_for(lambda: len(calls) == 3)  # connection errors are retried
        assert worker.is_running
    finally:
        await worker.stop()


async def test_fans_out_to_every_endpoint_independently():
    seen: dict[str, int] = {}

    async def handler(request: httpx.Request) -> httpx.Response:
        host = request.url.host
        seen[host] = seen.get(host, 0) + 1
        return httpx.Response(500 if host == "broken.example.com" else 200)

    worker = _worker(httpx.MockTransport(handler), urls=(URL, "https://broken.example.com/hook"))
    await worker.start()
    try:
        await worker.enqueue(_outcome(_test_change()))
        await _wait_for(lambda: seen.get("hooks.example.com") == 1 and seen.get("broken.example.com") == 3)
    finally:
        await worker.stop()


async def test_drops_the_oldest_batch_when_the_queue_is_full(caplog):
    bodies: list[bytes] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        bodies.append(request.content)
        return httpx.Response(200)

    worker = _worker(httpx.MockTransport(handler), maxsize=1)
    with caplog.at_level("WARNING"):
        await worker.enqueue(_outcome(_test_change()))
        await worker.enqueue(_outcome(_test_change(tier="stable")))
    assert "dropped" in caplog.text
    await worker.start()
    try:
        await _wait_for(lambda: bodies)
        assert b"test.recovered" in bodies[0]  # the newest batch survived
    finally:
        await worker.stop()


async def test_a_webhook_url_is_never_logged(caplog):
    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(400)

    worker = _worker(httpx.MockTransport(handler))
    with caplog.at_level("WARNING"):
        await worker.start()
        try:
            await worker.enqueue(_outcome(_test_change()))
            await _wait_for(lambda: "400" in caplog.text)
        finally:
            await worker.stop()
    assert "SECRETTOKEN" not in caplog.text
    assert "hooks.example.com" in caplog.text  # the host is enough to find the endpoint


async def test_stop_ends_the_task():
    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200)

    worker = _worker(httpx.MockTransport(handler))
    await worker.start()
    assert worker.is_running
    await worker.stop()
    assert not worker.is_running


async def test_a_retry_after_date_is_ignored_and_the_default_wait_used():
    """`retry-after` may be an HTTP date; the batch still retries on the normal delays."""
    calls: list[int] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(1)
        return httpx.Response(429, headers={"retry-after": "Wed, 21 Oct 2015 07:28:00 GMT"})

    worker = _worker(httpx.MockTransport(handler))
    await worker.start()
    try:
        await worker.enqueue(_outcome(_test_change()))
        await _wait_for(lambda: len(calls) == 3)
    finally:
        await worker.stop()


async def test_the_delivery_task_survives_a_post_that_raises(monkeypatch, caplog):
    """_post never raises, but if it ever does the task must keep serving."""
    calls: list[int] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        calls.append(1)
        return httpx.Response(200)

    worker = _worker(httpx.MockTransport(handler))
    real_post = worker._post
    boom = {"left": 1}

    async def flaky_post(url, batch):
        if boom["left"]:
            boom["left"] -= 1
            raise RuntimeError("boom")
        await real_post(url, batch)

    monkeypatch.setattr(worker, "_post", flaky_post)
    await worker.start()
    try:
        with caplog.at_level("ERROR", logger="flakeradar.worker"):
            await worker.enqueue(_outcome(_test_change()))
            await _wait_for(lambda: "Webhook delivery failed" in caplog.text)
        await worker.enqueue(_outcome(_job_change()))
        await _wait_for(lambda: len(calls) == 1)
        assert worker.is_running
    finally:
        await worker.stop()


async def test_slack_format_posts_one_text_with_a_line_per_event():
    bodies: list[bytes] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        bodies.append(request.content)
        return httpx.Response(200)

    worker = _worker(httpx.MockTransport(handler), fmt="slack")
    await worker.start()
    try:
        await worker.enqueue(_outcome(_test_change(), _job_change()))
        await _wait_for(lambda: bodies)
        body = json.loads(bodies[0])
        assert list(body) == ["text"]
        assert body["text"].count("\n") == 1  # one line per event
        assert body["text"].startswith("FlakeRadar: acme/app")
    finally:
        await worker.stop()
