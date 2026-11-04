"""End-to-end test for the `actions/report` composite action.

`report.sh` is bash, `curl` and `jq`, so the only honest test runs it the way a
composite step does — same script, runner-like environment — against a real
backend. It has to be a real *process*: the ASGI test client never runs the
lifespan, and only the lifespan's `ReportWorker` turns an uploaded Report into
`processed`. This is the locally runnable half of issue #35's "Done when"; the CI
half is the job that uses the action on the two fixture reports.
"""

import asyncio
import os
import shutil
import socket
import subprocess
import sys
import time
from collections.abc import Iterator
from pathlib import Path

import httpx
import pytest
from app import models
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncEngine, AsyncSession, async_sessionmaker

from tests.conftest import TOKEN

REPO_ROOT = Path(__file__).resolve().parents[2]
BACKEND_ROOT = REPO_ROOT / "backend"
ACTION_DIR = REPO_ROOT / "actions" / "report"
FIXTURES = ("fixture-alpha.xml", "fixture-beta.xml")

# The four cases the two fixture files report, as (suite, test name): both files
# must be uploaded and ingested, not just the first one.
INGESTED = {
    ("alpha-suite", "test_cart_total_sums_line_items"),
    ("alpha-suite", "test_refund_requires_an_order"),
    ("beta-suite", "test_password_is_never_echoed"),
    ("beta-suite", "test_locked_account_unlocks_after_cooldown"),
}

# The action's one passing plus one failing case, per file.
FIXTURE_COUNTS = {"passed": 1, "failed": 1, "error": 0, "skipped": 0, "retried": 0}

# What report.sh prints once per accepted file; its no-match warning also
# contains the word "uploaded", so the notices are what prove an upload happened.
UPLOAD_NOTICE = "::notice::FlakeRadar report: uploaded"

_unavailable = [
    what
    for what, present in (
        ("FLAKERADAR_TEST_DATABASE_URL", bool(os.environ.get("FLAKERADAR_TEST_DATABASE_URL"))),
        ("curl", shutil.which("curl") is not None),
        ("jq", shutil.which("jq") is not None),
    )
    if not present
]
if _unavailable:
    pytest.skip(f"needs {', '.join(_unavailable)}", allow_module_level=True)


def _free_port() -> int:
    """An OS-assigned port, released before the server binds it (as ci.yml does with 8000)."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


@pytest.fixture(scope="module")
def backend_url(database_url: str, engine: AsyncEngine, tmp_path_factory: pytest.TempPathFactory) -> Iterator[str]:
    """A live uvicorn process on the test database, as the CI job brings one up."""
    del engine  # requested so the suite's migrations are applied before the server boots

    port = _free_port()
    base_url = f"http://127.0.0.1:{port}"
    log = tmp_path_factory.mktemp("report-action") / "uvicorn.log"
    env = {
        **os.environ,
        "FLAKERADAR_DATABASE_URL": database_url,
        "FLAKERADAR_API_TOKEN": TOKEN,
        "FLAKERADAR_ALLOW_INSECURE": "1",
        "FLAKERADAR_GITHUB_TOKEN": "",  # a test server must never reach GitHub
    }
    with log.open("w", encoding="utf-8") as handle:
        server = subprocess.Popen(
            [sys.executable, "-m", "uvicorn", "app.main:app", "--host", "127.0.0.1", "--port", str(port)],
            cwd=BACKEND_ROOT,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
        )
        try:
            _wait_for_health(base_url, server, log)
            yield base_url
        finally:
            server.terminate()
            try:
                server.wait(timeout=30)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=30)


def _wait_for_health(base_url: str, server: subprocess.Popen[None], log: Path) -> None:
    """The ci.yml bring-up check: /api/health answers once the lifespan is through."""
    deadline = time.monotonic() + 60
    while True:
        if server.poll() is None:
            try:
                if httpx.get(f"{base_url}/api/health", timeout=1.0).status_code == 200:
                    return
            except httpx.HTTPError:
                pass
        if time.monotonic() > deadline:
            raise RuntimeError(f"the test backend did not start (exit {server.poll()}):\n{log.read_text()}")
        time.sleep(0.5)


def _run_report_script(base_url: str, workspace: Path, output: Path, files: str) -> subprocess.CompletedProcess[str]:
    """Run report.sh from `workspace` with the environment a composite step gives it.

    Explicit env, not the ambient one: the run's own values come from GITHUB_*,
    exactly as they would on a runner, and `$GITHUB_OUTPUT` is where the
    `report_ids` output has to land.
    """
    env = {
        **os.environ,
        # The action's inputs (see action.yml).
        "FR_URL": base_url,
        "FR_TOKEN": TOKEN,
        "FR_PROJECT": "fixture",
        "FR_FILES": files,
        # ...and the values the action computes from the runner environment.
        "GITHUB_WORKSPACE": str(workspace),
        "GITHUB_REPOSITORY": "flakeradar-selftest/demo",
        "GITHUB_SHA": "b" * 40,
        "GITHUB_REF_NAME": "main",
        "GITHUB_RUN_ID": "4242",
        "GITHUB_RUN_ATTEMPT": "1",
        "GITHUB_WORKFLOW_REF": "flakeradar-selftest/demo/.github/workflows/ci.yml@refs/heads/main",
        "GITHUB_OUTPUT": str(output),
        # The action's job-context inputs, which report.sh falls back to the event
        # payload for; empty here so the fallback is what runs.
        "FR_JOB_ID": "",
        "FR_DEFAULT_BRANCH": "",
        "GITHUB_EVENT_PATH": str(workspace / "event.json"),
    }
    (workspace / "event.json").write_text('{"repository": {"default_branch": "main"}}', encoding="utf-8")
    return subprocess.run(
        ["bash", str(ACTION_DIR / "report.sh")],
        cwd=workspace,
        env=env,
        capture_output=True,
        text=True,
        timeout=120,
        check=False,  # each test asserts on the exit code itself: exit 0 is part of what is tested
    )


def _report_ids(output: Path) -> list[str]:
    """The ids on the `report_ids` line the script wrote to $GITHUB_OUTPUT."""
    lines = [ln for ln in output.read_text(encoding="utf-8").splitlines() if ln.startswith("report_ids=")]
    assert len(lines) == 1, f"expected one report_ids line in {str(output)!r}, got {lines!r}"
    return [part for part in lines[0].removeprefix("report_ids=").split(",") if part]


async def _processed_report(client: httpx.AsyncClient, report_id: str, timeout: float = 30.0) -> dict:
    """Poll GET /api/reports/{id} — the read the CI job does — until the worker is done."""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while True:
        response = await client.get(f"/api/reports/{report_id}")
        assert response.status_code == 200, response.text
        body = response.json()
        assert body["status"] != "failed", f"Report {report_id} failed: {body['error']}"
        if body["status"] == "processed":
            return body
        if loop.time() > deadline:
            pytest.fail(f"Report {report_id} was still {body['status']!r} after {timeout}s")
        await asyncio.sleep(0.25)


async def test_both_fixture_files_are_uploaded_and_processed(
    backend_url: str, session_factory: async_sessionmaker[AsyncSession], tmp_path: Path
) -> None:
    """Two files, two Reports, both processed: the headline criterion of issue #35."""
    for name in FIXTURES:
        shutil.copy(ACTION_DIR / "test" / name, tmp_path / name)
    output = tmp_path / "github_output"
    output.write_text("", encoding="utf-8")

    # Two patterns that both match the first file: it is uploaded once, not twice.
    run = _run_report_script(backend_url, tmp_path, output, "fixture-*.xml\nfixture-alpha.xml")
    assert run.returncode == 0, f"stdout:\n{run.stdout}\nstderr:\n{run.stderr}"
    assert run.stdout.count(UPLOAD_NOTICE) == 2, run.stdout

    ids = _report_ids(output)
    # Two ids, not one: identical bytes under one identity dedupe to a single
    # Report, so distinct ids also prove the two files stayed two files.
    assert len(ids) == 2 and ids[0] != ids[1], f"expected two Report ids, report.sh wrote: {run.stdout}"

    async with httpx.AsyncClient(base_url=backend_url, timeout=5.0) as client:
        for report_id in ids:
            body = await _processed_report(client, report_id)
            assert body["counts"] == FIXTURE_COUNTS, f"Report {report_id}: {body}"

    # And the four cases of the two files really are in the database, as two Runs.
    async with session_factory() as session:
        rows = (await session.execute(select(models.TestCase.suite, models.TestCase.name))).all()
        runs = (await session.execute(select(func.count()).select_from(models.TestRun))).scalar()
    assert {(suite, name) for suite, name in rows} == INGESTED, f"ingested {sorted(rows)}"
    assert runs == 2, f"expected one TestRun per uploaded file, got {runs}"


async def test_no_match_warns_without_failing_or_uploading(backend_url: str, tmp_path: Path) -> None:
    """A pattern that matches nothing is a warning, not an error: reporting must not break a build."""
    output = tmp_path / "github_output"
    output.write_text("", encoding="utf-8")

    run = _run_report_script(backend_url, tmp_path, output, "missing-*.xml")
    assert run.returncode == 0, f"stdout:\n{run.stdout}\nstderr:\n{run.stderr}"
    assert "::warning::" in run.stdout, run.stdout
    assert "no files matched" in run.stdout, run.stdout
    assert UPLOAD_NOTICE not in run.stdout, run.stdout
    assert _report_ids(output) == []
