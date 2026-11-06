"""`D3`: the paths a Report that cannot be stored takes — its `failed` state, the
rows it leaves behind, and the retry and dismiss routes.

The Report is staged with a `line` past the `int4` column beside a `file`, which
the reference only writes when a file is present, so its Execution cannot be
inserted. That is a data error in both servers: it fails the Report at once
without spending retry budget, so both databases consume the same sequence values
and every id after this phase still matches.

A Report that fails *transiently* is `d3r`, which runs last: this port retries one
and the reference does not (a recorded difference), and the retries consume
sequence values of their own.
"""

from __future__ import annotations

from harness import API_KEY, Battery

from seed import PROJECT, REPO, report_id_of, upload

UNSTORABLE_CASE = "staged_unstorable_line"


def failed_reports(battery: Battery) -> None:
    battery.phase = "d3"
    upload(
        battery,
        [f'<testcase name="{UNSTORABLE_CASE}" classname="broken" file="big_test.go" line="3000000000"/>'],
        repo=REPO,
        project=PROJECT,
        commit_sha="fail-c1",
    )
    battery.drain()

    failed = report_id_of(battery, "failed")
    # The Report the processor failed, with the error text masked: `ExceptionType:
    # message` against a Rust description is a recorded difference
    # (differences.md 06).
    battery.call("GET", f"/api/reports/{failed}", mask=("error",), label="the failed Report")
    battery.rows(
        "SELECT id, kind, status, retry_count, run_id, project_id IS NULL, counts "
        "FROM reports WHERE id = " + str(failed),
        label="d3: the failed Report row",
    )
    battery.rows(
        f"SELECT count(*) FROM test_executions WHERE message = 'staged unstorable line'",
        label="d3: no Execution of a failed Report is stored",
    )
    battery.rows(
        f"SELECT count(*) FROM test_cases WHERE name = '{UNSTORABLE_CASE}'",
        label="d3: no Test a failed Report would have created",
    )
    battery.rows(
        f"SELECT count(*) FROM test_runs WHERE commit_sha = 'fail-c1'",
        label="d3: no Run of a failed Report",
    )

    # A retry of a data error fails it again at once, on both servers.
    battery.call("POST", f"/api/reports/{failed}/retry", headers=API_KEY, mask=("error",), label="retry the failed Report")
    battery.drain()
    battery.call("GET", f"/api/reports/{failed}", mask=("error",), label="the Report after its retry")
    battery.rows(
        f"SELECT status, retry_count FROM reports WHERE id = {failed}",
        label="d3: the Report row after its retry",
    )

    # Every status the two routes answer for a Report they may not touch.
    processed = report_id_of(battery, "processed")
    battery.call("POST", f"/api/reports/{processed}/retry", headers=API_KEY, label="retry a processed Report")
    battery.call(
        "DELETE",
        f"/api/reports/{processed}",
        headers=API_KEY,
        known="K-delete-204-content-type",
        label="delete a processed Report",
    )
    battery.call("POST", "/api/reports/999999/retry", headers=API_KEY, label="retry a missing Report")
    battery.call("DELETE", "/api/reports/999999", headers=API_KEY, label="delete a missing Report")

    # Dismissing a failed Report deletes it, and only once.
    battery.call(
        "DELETE",
        f"/api/reports/{failed}",
        headers=API_KEY,
        known="K-delete-204-content-type",
        label="dismiss the failed Report",
    )
    battery.call("GET", f"/api/reports/{failed}", label="read a dismissed Report")
    battery.call("DELETE", f"/api/reports/{failed}", headers=API_KEY, label="dismiss it twice")
    battery.call("GET", "/api/reports/summary", label="the queue after the dismissals")
    battery.rows(
        "SELECT status, count(*) FROM reports GROUP BY status ORDER BY status",
        label="d3: the Reports left behind",
    )
    battery.note(
        "one Report could not be stored at all (a line past int4 beside a file) and failed at once; "
        "its retry, dismiss, 409 and 404 paths all compared, and it left no Test, Run or Execution"
    )
