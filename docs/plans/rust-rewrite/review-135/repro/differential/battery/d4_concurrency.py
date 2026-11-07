"""`D4`: the concurrency the row names — a second instance, and an admin delete
racing the processor.

Two instances: a second process of each server is started on the same database.
Exactly one of the two may lead, and a Report must be processed once, not twice.
A deadlock is not staged here (it needs two sessions in one order; the delete
route's 503 for SQLSTATE 40P01 is pinned by the port's own tests), and a prune
racing an upload is the retention profile's `D6`, which prunes every half second.
"""

from __future__ import annotations

import os
import subprocess
import time

import httpx

from harness import API_KEY, Battery

from seed import PROJECT, REPO, upload

PY_PORT = 18083
RS_PORT = 18084


def _wait_ready(port: int, timeout: float = 120.0) -> dict:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            return httpx.get(f"http://127.0.0.1:{port}/api/health/ready", timeout=5).json()
        except httpx.HTTPError:
            time.sleep(0.2)
    raise RuntimeError(f"the second instance on {port} never became ready")


def _second_instances() -> list[subprocess.Popen]:
    """A second process of each server, on the same database as the first."""
    pg_port = os.environ.get("BATTERY_PG_PORT", "55917")
    common = {
        "FLAKERADAR_API_TOKEN": "tok-spec-review",
        "FLAKERADAR_WORKER_POLL_SECONDS": "0.05",
    }
    python = subprocess.Popen(
        [
            f"{os.environ['PY_VENV']}/bin/uvicorn",
            "app.main:app",
            "--host",
            "127.0.0.1",
            "--port",
            str(PY_PORT),
            "--workers",
            "1",
        ],
        cwd=os.environ["PY_BACKEND"],
        env={
            **os.environ,
            **common,
            "FLAKERADAR_DATABASE_URL": f"postgresql+asyncpg://fr:pw@127.0.0.1:{pg_port}/py",
        },
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    rust = subprocess.Popen(
        [os.environ["RS_BIN"]],
        env={
            **os.environ,
            **common,
            "FLAKERADAR_DATABASE_URL": f"postgresql://fr:pw@127.0.0.1:{pg_port}/rs",
            "FLAKERADAR_HOST": "127.0.0.1",
            "FLAKERADAR_PORT": str(RS_PORT),
        },
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return [python, rust]


def concurrency(battery: Battery) -> None:
    battery.phase = "d4"
    # An admin delete of the Repo that holds a queued Report, while the processor
    # is working through it: whichever wins, both servers must leave the same
    # rows and answer the same status.
    for commit in ("race0", "race1", "race2"):
        upload(
            battery,
            [
                f'<testcase name="race_{commit}_{i}" classname="race" file="race_test.go">'
                f'<failure message="boom {i}">x</failure></testcase>'
                for i in range(400)
            ],
            repo=REPO,
            project=PROJECT,
            commit_sha=commit,
        )
        battery.call(
            "DELETE",
            f"/api/repos/{REPO}",
            headers=API_KEY,
            known="K-racing-delete-counts",
            label=f"delete {REPO} while {commit} is queued",
        )
    battery.drain()
    battery.rows(
        "SELECT r.name, coalesce(p.name, ''), count(*) FROM reports rep "
        "JOIN repos r ON r.id = rep.repo_id LEFT JOIN projects p ON p.id = rep.project_id "
        "GROUP BY r.name, p.name ORDER BY r.name, p.name",
        label="d4: the queue after the racing deletes",
    )
    battery.rows(
        "SELECT (SELECT count(*) FROM repos), (SELECT count(*) FROM projects), "
        "(SELECT count(*) FROM test_cases), (SELECT count(*) FROM reports)",
        label="d4: what the racing deletes left",
    )

    processes = _second_instances()
    try:
        second_python = _wait_ready(PY_PORT)
        second_rust = _wait_ready(RS_PORT)
        first_python = httpx.get("http://127.0.0.1:18081/api/health/ready", timeout=10).json()
        first_rust = httpx.get("http://127.0.0.1:18082/api/health/ready", timeout=10).json()
        # `processor_leader` answers whether the queue has a leader in this
        # database, not whether the answering process is it, so both instances
        # answering true is the reference's own meaning and is compared as such.
        battery.compare("two instances: the first instance's probe", first_python, first_rust)
        battery.compare("two instances: the second instance's probe", second_python, second_rust)
        # The lock itself is what makes the leader one process: one session holds
        # it on each side, whichever instance that is.
        battery.rows(
            "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND classid = 0 "
            "AND objid::bigint = 726300002 "
            "AND database = (SELECT oid FROM pg_database WHERE datname = current_database())",
            label="d4: one session holds the leader's advisory lock",
        )

        # A Report uploaded while two instances serve is processed once.
        upload(
            battery,
            ['<testcase name="two_instances" classname="race"/>'],
            repo=REPO,
            project=PROJECT,
            commit_sha="two-instances",
        )
        battery.drain()
        battery.rows(
            "SELECT count(*) FROM test_executions e JOIN test_cases c ON c.id = e.test_case_id "
            "WHERE c.name = 'two_instances'",
            label="d4: one Execution for one Report under two instances",
        )
        battery.rows(
            "SELECT count(*) FROM test_runs WHERE commit_sha = 'two-instances'",
            label="d4: one Run for one Report under two instances",
        )
        battery.note(
            "a second instance of each server ran on the same database: exactly one of the two led on "
            "each side, and a Report was processed once. A deadlock was not staged; the port's delete "
            "tests pin the 503 it answers for one"
        )
    finally:
        for process in processes:
            process.terminate()
        for process in processes:
            try:
                process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                process.kill()
