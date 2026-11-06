"""`D3r`: the retry path of a failed Report, run last because it is the one
difference the sweep leaves open.

A `BEFORE INSERT` trigger on `test_executions`, created identically in both
databases, raises `serialization_failure` (SQLSTATE 40001) for one named Test's
Execution. The reference's own docstring calls a serialization failure transient,
but SQLAlchemy does not wrap asyncpg's `SerializationError` as `OperationalError`,
so its processor fails the Report at once: `retry_count` 0. This port classifies
it as transient and spends the retry budget: `retry_count` 3. `differences.md`
records that, with the measurement, so both the elapsed retries and their effect
on the ids are a known outcome rather than a new difference.

The phase is last because each retry consumes sequence values the earlier phases'
ids would otherwise be compared against.
"""

from __future__ import annotations

from harness import API_KEY, Battery

from seed import PROJECT, REPO, report_id_of, upload

TRANSIENT_CASE = "staged_transient_failure"


def _stage(battery: Battery, statement: str) -> None:
    for database in ("py", "rs"):
        battery.psql(database, statement)


def retries(battery: Battery) -> None:
    battery.phase = "d3r"
    _stage(
        battery,
        "CREATE OR REPLACE FUNCTION staged_transient() RETURNS trigger AS $$ "
        "BEGIN IF NEW.message = 'staged transient failure' THEN "
        "RAISE EXCEPTION 'staged transient failure' USING ERRCODE = '40001'; "
        "END IF; RETURN NEW; END $$ LANGUAGE plpgsql",
    )
    _stage(battery, "DROP TRIGGER IF EXISTS staged_transient ON test_executions")
    _stage(
        battery,
        "CREATE TRIGGER staged_transient BEFORE INSERT ON test_executions "
        "FOR EACH ROW EXECUTE FUNCTION staged_transient()",
    )
    upload(
        battery,
        [f'<testcase name="{TRANSIENT_CASE}" classname="staged"><failure message="staged transient failure">x</failure></testcase>'],
        repo=REPO,
        project=PROJECT,
        commit_sha="fail-c2",
    )
    battery.drain()
    _stage(battery, "DROP TRIGGER IF EXISTS staged_transient ON test_executions")

    failed = report_id_of(battery, "failed")
    battery.call(
        "GET",
        f"/api/reports/{failed}",
        mask=("error", "retry_count"),
        known="K-transient-retry",
        label="the Report a transient failure failed",
    )
    battery.rows(
        f"SELECT status, run_id FROM reports WHERE id = {failed}",
        label="d3r: the Report a transient failure failed",
    )
    battery.rows(
        f"SELECT count(*) FROM test_executions WHERE message = 'staged transient failure'",
        label="d3r: no Execution of the retried Report is stored",
    )
    battery.rows(
        f"SELECT count(*) FROM test_cases WHERE name = '{TRANSIENT_CASE}'",
        label="d3r: no Test the retried Report would have created",
    )
    # Both give up in the end, and both answer the dismiss route for it.
    battery.call(
        "DELETE",
        f"/api/reports/{failed}",
        headers=API_KEY,
        known="K-delete-204-content-type",
        label="dismiss the failed Report",
    )
    battery.note(
        "a staged 40001 made the Report fail on both sides; the reference failed it at once "
        "(retry_count 0) and this port spent its three retries, a recorded difference"
    )
