# The differential sweep (task 23)

Both servers, side by side, each on its own PostgreSQL database, sent the same
requests. Every answer is compared on its status, on the headers a consumer reads
and on its body, and the rows the requests left are compared too. `D1`–`D8` in
`../ledger.md` are the surface it covers.

The battery was a scratch script in the review (`../t1.py`–`../t10.py`), with
paths and ports edited by hand, and the earlier run of `../t8.py` compared two
databases whose ids had drifted. This directory is the rerunnable replacement: one
`run.sh`, one report, and every known difference named by its `differences.md`
row.

## Running it

```bash
./run.sh              # the default profile: D1, D2, D3, D5, D4, D3r
./run.sh retention    # the retention profile: D6
./run.sh all          # both, one after the other
```

What it needs, all of it outside the repo:

- Docker, for a `postgres:17-alpine` container it creates and removes itself.
- The reference at commit `86c98804`: `git archive 86c98804 backend | tar -x -C "$TMP/python-base"`.
- A virtualenv with that `backend/requirements.txt`: `PY_VENV=/tmp/pyref`.
- A built binary: `cargo build` writes `target/debug/flakeradar`.

Override any of it with `PY_BASE`, `PY_VENV`, `PG_CONTAINER`, `PG_PORT`,
`PY_PORT`, `RS_PORT`, `WEBHOOK_{PY,RS}_PORT` or `OUT`.

The run creates both databases fresh, starts both servers on the same settings,
builds the fixture through the servers' own routes (so both databases hold the
same rows with the same ids), runs the phases, and writes
`out/report-<profile>.md` and `out/report-<profile>.json`. It exits non-zero when
a difference is not one `differences.md` records.

## The phases

| Phase | What it does |
|---|---|
| `seed` | The fixture, built only through the API: `acme/app` with two Projects over commits c1-c8, a feature branch, five Failure categories, Proven flakes, a broken, a suspect, a stable, a skipped and a retried Test, four Pipeline runs (explained, unexplained, summary and cancelled Jobs), a quarantine and an Issue link. Also `acme/other`, so a scoped read is not the whole database |
| `d2` | `D2`: every read route, generated from the route's own parameter list — each parameter alone, at both ends of its range, refused, and in the combinations the dashboard sends — then the rows behind the reads |
| `d1` | `D1`: all eleven MCP tools through `fastmcp`'s own client, with valid, string-typed, missing, out-of-range and unknown arguments, the two write tools' `replace` and single-target rules, and the rows the writes left |
| `d3` | `D3`: a Report that cannot be stored (`line` past `int4` beside a `file`), its `failed` state, the rows it leaves, and every status the retry and dismiss routes answer |
| `d5` | `D5`: ingest at and past every boundary — the 34 multipart shapes `t8.py` sent, eight raw content types, gzip, the writers-app `curl`, both ingest-route limits, the query parameters, and the order the route checks things in |
| `d4` | `D4`: an admin delete racing the processor three times, then a second instance of each server on the same database: exactly one of the two holds the advisory lock, and a Report is processed once |
| `d3r` | `D3`: the retry path, staged with a `BEFORE INSERT` trigger raising SQLSTATE 40001. Last, because the retries consume sequence values the earlier phases compare |
| `d6` | `D6`, retention profile only: every window stays at its default while the fixture is built, then one statement pair ages rows past each window, and what the prune deleted, what it left, the scores its rescore wrote and the Notifications both servers posted are compared |

## How a difference is classified

`harness.py` holds `KNOWN`: every difference the sweep may find, as a
`differences.md` row. A case that names one is recorded with its id and never
fails the run; a case that does not is a new difference and does. The report
lists both, so a rerun shows the known differences by name and nothing else.

A field whose *value* a row allows to differ (a Report's `error` text, for one) is
masked before the comparison, so the rest of that answer is still compared.

Timestamps are replaced by `<ts>` everywhere: the reference writes `+00:00` and
omits a zero fraction where this port writes six digits and `Z`, which is one
recorded difference with one meaning.

## What it does not cover

- `D7` (the acceptance criteria of tasks 01, 02, 03, 12 and 13) and `D8` (scope)
  are reviews, not requests; their evidence is in the ledger rows.
- A deadlock between two deletes and the processor is not staged: it needs two
  sessions in one order, and the port's own delete tests pin the 503 it answers.
- A real Postgres restart is not staged here either; task 22's
  `a_database_restart_under_the_leader_files_a_real_issue_and_syncs_again` pins
  it, and `../t9.py` is the review's own record of the behaviour.
- The frontend is not exercised: task 12's browser check is task 22's remaining
  unverified item.
