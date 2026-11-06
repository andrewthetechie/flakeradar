"""`D6`: Retention and webhook delivery, run rather than read.

The retention profile leaves every window at its default and prunes every half
second, so the fixture is untouched while it is built. This phase then ages rows
with one statement pair (identical in both databases) until each window is
crossed, waits for a prune round on each server, and compares what was deleted,
what was left, and the scores the prune's own rescore wrote. The webhook
receiver recorded every Notification both servers delivered; those are compared
here too.
"""

from __future__ import annotations

import json
import os
import time
from pathlib import Path

from harness import API_KEY, Battery

from seed import PROJECT, REPO, upload

# One statement per window, run in both databases: the same rows become old
# enough for the windows the profile left at their defaults.
AGE = [
    "UPDATE reports SET processed_at = now() - interval '30 days', created_at = now() - interval '30 days' "
    "WHERE status = 'processed'",
    "UPDATE reports SET processed_at = now() - interval '30 days', created_at = now() - interval '30 days' "
    "WHERE status = 'failed'",
    "UPDATE test_executions SET created_at = now() - interval '200 days'",
    "UPDATE job_executions SET created_at = now() - interval '200 days'",
    "UPDATE test_runs SET created_at = now() - interval '200 days'",
    "UPDATE test_score_history SET day = current_date - 400",
    "UPDATE job_score_history SET day = current_date - 400",
    "UPDATE test_cases SET quarantine_expires_at = now() - interval '1 day' "
    "WHERE quarantined AND quarantine_expires_at IS NOT NULL",
]
# The failed Report the profile's `FAILED_REPORT_RETENTION_DAYS=1` window prunes.
UNSTORABLE = "retention_failed_report"


def retention(battery: Battery) -> None:
    battery.phase = "d6"
    # A failed Report, so the failed-Report window has something to age out.
    upload(
        battery,
        [f'<testcase name="{UNSTORABLE}" classname="broken" file="big_test.go" line="3000000000"/>'],
        repo=REPO,
        project=PROJECT,
        commit_sha="retention-failed",
    )
    battery.drain()

    # Counted before anything is old, so no prune can be part-way through a round
    # while the snapshot is taken.
    counts = (
        "SELECT (SELECT count(*) FROM reports), (SELECT count(*) FROM test_executions), "
        "(SELECT count(*) FROM job_executions), (SELECT count(*) FROM test_runs), "
        "(SELECT count(*) FROM test_score_history), (SELECT count(*) FROM job_score_history), "
        "(SELECT count(*) FROM test_cases WHERE quarantined)"
    )
    battery.rows(counts, label="d6: the rows the windows are about to remove")
    before = battery.psql("py", "SELECT count(*) FROM reports").strip()
    for database in ("py", "rs"):
        for statement in AGE:
            battery.psql(database, statement)

    # A prune round runs at most every `PRUNE_INTERVAL_SECONDS` (0.5 here), and a
    # round deletes at most `prune_batch` rows per table, so two rounds are enough
    # for this fixture. Both servers prune on their own schedule, so the counts are
    # read again until they have stopped moving on both sides.
    _settle(battery, counts)
    battery.rows(
        "SELECT (SELECT count(*) FROM reports), (SELECT count(*) FROM reports WHERE status = 'failed'), "
        "(SELECT count(*) FROM test_executions), (SELECT count(*) FROM job_executions), "
        "(SELECT count(*) FROM test_runs), (SELECT count(*) FROM test_score_history), "
        "(SELECT count(*) FROM job_score_history), "
        "(SELECT count(*) FROM test_cases WHERE quarantined)",
        label="d6: what the prune left",
    )
    battery.dump(["test_cases", "jobs"], label="d6: the scores the prune's own rescore wrote")
    battery.rows(
        "SELECT id, name, quarantined, quarantine_reason, quarantine_expires_at IS NULL "
        "FROM test_cases WHERE name = 'test_timing_flake'",
        label="d6: the expired quarantine the prune cleared",
    )
    battery.note(f"{before} Reports were planted; the prune aged out every window and cleared the expired quarantine")

    # The Notifications both servers delivered, in order. They were recorded by
    # the two receivers `run.sh` starts, one endpoint per server.
    out = Path(os.environ.get("BATTERY_OUT", Path(__file__).resolve().parent / "out"))
    python = _deliveries(out / "webhooks-py.jsonl")
    rust = _deliveries(out / "webhooks-rs.jsonl")
    battery.compare(f"webhook deliveries ({len(python)} python, {len(rust)} rust)", python, rust)
    battery.note(
        "the Notification bodies both servers posted are compared in order, with the recorded "
        "timestamp spelling normalised; a prune sends none, so these are the seed's tier crossings"
    )


def _settle(battery: Battery, counts: str, timeout: float = 90.0) -> None:
    """Wait until both servers' counts have stopped moving for one interval.

    A prune is a background round with no endpoint that triggers or reports it,
    so the only way to know one has finished is that nothing changed while a
    whole interval went by.
    """
    deadline = time.monotonic() + timeout
    previous = None
    stable_since = time.monotonic()
    while time.monotonic() < deadline:
        current = tuple(battery.psql(database, counts).strip() for database in ("py", "rs"))
        if current != previous:
            previous, stable_since = current, time.monotonic()
        elif time.monotonic() - stable_since > 2.0:
            return
        time.sleep(0.2)
    raise RuntimeError(f"the prune never settled: {previous}")


def _deliveries(path: Path) -> list:
    """Every recorded delivery, timestamps normalised."""
    from harness import normalize

    if not path.exists():
        return []
    return [normalize(json.loads(line)) for line in path.read_text().splitlines() if line.strip()]
