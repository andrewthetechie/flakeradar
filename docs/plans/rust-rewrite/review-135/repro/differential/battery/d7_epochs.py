"""`D7`: the epoch timestamps — `SP8` and `SP9` of the second review.

A number past `2e10` in magnitude is a millisecond count, so the value
`Date.now()` gives is 2026 and not the year 57971, and a string holding a plain
decimal number is the same instant a number is. Every accepted case is a fresh CI
run, so the Job rows it leaves can be compared as rows as well as by its answer;
the refused ones are past the years a datetime holds, or no number at all.

The phase is its own so the Reports it leaves never reach `d5`'s report dump,
whose byte-comparison cannot normalise the timestamp spelling both servers
record. The instants are compared where they are stored: `job_executions`.
"""

from __future__ import annotations

from harness import API_KEY, Battery

# The reference's own answers, measured from it: the accepted values, and the
# `type` each refused one answers with.
ACCEPTED = [
    20000000000,
    20000000001,
    -20000000001,
    1767225600000,
    1767225600123,
    253402300799,
    253402300800,
    1e14,
    "1767225600",
    "1767225600.5",
    "1767225600000",
    "1700000000",
    "0",
    "-5",
]
REFUSED = [1e15, 253402300800000, "253402300800000", "1e3", "1_000"]


def _body(index: str, started_at: object) -> dict:
    return {
        "repo": "acme/app",
        "provider": "github",
        "pipeline": "ci.yml",
        "commit_sha": f"ts{index}",
        "branch": "main",
        "ci_run_id": f"ts{index}",
        "jobs": [
            {
                "ci_job_id": f"ts{index}",
                "name": "epoch",
                "status": "passed",
                "started_at": started_at,
                "completed_at": started_at,
            }
        ],
    }


def epochs(battery: Battery) -> None:
    battery.phase = "d7"
    for index, value in enumerate(ACCEPTED):
        battery.call(
            "POST",
            "/api/ingest/pipeline",
            headers={**API_KEY, "Content-Type": "application/json"},
            json_body=_body(str(index), value),
            label=f"started_at {value!r}",
        )
    for index, value in enumerate(REFUSED):
        battery.call(
            "POST",
            "/api/ingest/pipeline",
            headers={**API_KEY, "Content-Type": "application/json"},
            json_body=_body(f"bad{index}", value),
            label=f"refused started_at {value!r}",
        )
    battery.drain()
    battery.rows(
        "SELECT ci_run_id, started_at, completed_at FROM job_executions "
        "WHERE ci_run_id LIKE 'ts%' ORDER BY ci_run_id",
        label="d7: the Job time each epoch timestamp left",
    )
    battery.rows(
        "SELECT ci_run_id, status FROM reports WHERE ci_run_id LIKE 'ts%' ORDER BY ci_run_id",
        label="d7: the Reports the refused timestamps left",
    )
    battery.note(
        f"{len(ACCEPTED)} epoch timestamps accepted and {len(REFUSED)} refused, "
        "each answer and each stored Job time compared"
    )
