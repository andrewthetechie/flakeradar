# Parity fixtures

What the frozen Python backend does, captured once as committed files. The Rust
rewrite replays them, so Python never runs in CI. Task 02 records them; task 15
deletes the recorder and keeps these files as regression tests.

```
junit/                    the JUnit XML corpus (inputs)
scenarios/                declarative request scripts (inputs)
golden/junit/             one parse result per corpus file, per line_base
golden/<scenario>/<nn>.json   one result per scenario step
schema.sql                pg_dump of Alembic head, minus alembic_version
allowed-differences.json  the only mismatches a replay may accept
```

Nothing here is edited by hand. `tools/parity-record/record.py` writes all of
it, and `record.py --verify` fails when a committed file no longer matches what
the Python reference produces.

## Recording

Docker must be running. From the repo root:

```bash
backend/.venv/bin/python tools/parity-record/record.py            # rewrite
backend/.venv/bin/python tools/parity-record/record.py --verify   # check
```

Each run starts a throwaway `postgres:17-alpine`, creates a fresh database per
scenario, starts one `uvicorn` process against it (the lifespan runs the
migrations), runs the steps, and stops the server. If `frontend/dist` exists it
is moved aside for the run and put back afterwards: with a built frontend
mounted at `/`, a trailing-slash request is answered 404 by the static files
instead of Starlette's 307, and a fresh checkout has no `dist`. Every golden is
recorded against the routes-only app. Recording is deterministic:
two runs produce byte-identical files. The parts are reproducible on their own
with `--only junit`, `--only scenarios` or `--only schema`, and one scenario at
a time with `--scenario NAME`.

Values the server takes from the clock are replaced after being checked to
parse:

- `"<timestamp>"` matches any value in the timestamp rule
  (`YYYY-MM-DDTHH:MM:SS`, an optional fraction, then `Z` or `+00:00`).
- `"<date>"` matches `YYYY-MM-DD`.
- Substitution also happens inside a string, because an MCP tool result carries
  its whole payload as one JSON text block.
- A timestamp or date the *scenario* wrote is kept verbatim, so input values
  stay readable.
- Ids, scores and fingerprints are kept exact. Sequence values start at 1 on
  every fresh database, so ids are stable.

`record.py` also fails when a route in FastAPI's own route table (`app.openapi()["paths"]`)
appears in no scenario, except the docs routes and `/mcp`, which the Rust binary
serves differently on purpose (see `docs/plans/rust-rewrite/differences.md`).

A PostgreSQL minor-version bump changes the version comment in `schema.sql`;
re-record and commit it.

## The JUnit corpus

`junit/*.xml` are inputs, several of them binary (ISO-8859-1, UTF-16, a UTF-8
BOM, invalid bytes). `golden/junit/<file>.line_base<0|1>.json` is what
`app.parsing.parse_junit_xml(bytes, line_base)` returned for that file:

```json
{
  "source": "junit/pytest-xunit1.xml",
  "line_base": 1,
  "cases": [
    {
      "suite": "unit", "classname": "tests.test_cart", "name": "test_total",
      "status": "failed", "duration": 0.01,
      "message": "assert 3 == 2", "details": "...",
      "file": "tests/test_cart.py", "line": 120,
      "fingerprint": "3f2c...", "category": "assertion"
    }
  ]
}
```

`category` is `classify_failure(message, details)` for `failed` and `error`
cases and `null` otherwise. A report Python rejects has `error` instead of
`cases`; the string is Python's message verbatim, prefixed with `unhandled
<ExceptionType>: ` when the frozen code raises something that is not a
`ParseError` (one corpus file does: see `allowed-differences.json`).

Every corpus file is recorded with both `line_base` values, so a replay can
check the attribute's shift rule without a second corpus.

XML 1.0 cannot carry U+000B, U+000C or U+001C–U+001F, so the corpus covers the
whitespace trap characters with TAB, CR, NEL (U+0085), LINE SEPARATOR (U+2028),
PARAGRAPH SEPARATOR (U+2029), NBSP and leading/trailing spaces. The characters
XML cannot carry are covered by the ported unit tests of `py_strip` and
`py_splitlines`.

## Scenarios

A scenario is a JSON file in `scenarios/`:

```json
{"name": "health", "settings": {"FLAKERADAR_BROKEN_STREAK": "2"}, "steps": [
  {"http": {"method": "GET", "path": "/api/tests", "query": {"repo": "o/r"}}},
  {"wait_processed": true},
  {"dump_db": ["test_cases"]}
]}
```

- `name` names the directory under `golden/`. It may not be `junit`, which
  owns `golden/junit/`.
- `settings`, optional, overrides the recorder's pinned `FLAKERADAR_*` settings
  for this scenario's server only.
- Every step writes `golden/<scenario>/<nn>.json`, numbered from 1 in step
  order, so a golden file always names the step that produced it.

Every scenario starts on an empty database and seeds only through the public
API, so both servers run identical steps. The fixed token is `parity-token`; a
step that needs it sends `"X-API-Key": "$TOKEN"`, and a step with no `headers`
sends none (which is how the 401 cases are recorded).

### Step types

`http` — one request:

| key | meaning |
|---|---|
| `method`, `path` | required |
| `query` | query parameters; values are strings, `$TOKEN` is substituted |
| `headers` | request headers; `$TOKEN` is substituted; omitted means none |
| `body_text` | the raw body, UTF-8 encoded |
| `body_json` | the body as JSON (the recorder serializes it) |
| `body_file` | a body read from `tests/parity/<body_file>` |
| `body_size` | a filler body of that many `a` bytes, for size-limit cases |
| `multipart_file` | `{"field", "filename", "body_file"}`: one file field, so httpx writes the multipart body |

The recorded file holds `request` (method, path and query, never headers),
`status`, `content-type`, `location` when present, and `body`: parsed JSON when
the response is JSON, otherwise the text. A recorded `location` is reduced to
its path and query: the server answers with an absolute URL, its port is not
stable, and a redirect target is what the contract is about.

`wait_processed` — polls `GET /api/reports/summary` until `pending` is 0, then
records `{"processed": true, "reports": {...}}`. This is the only waiting a
scenario does: everything else assumes the queue is drained.

`dump_db` — every row of the listed tables, as JSON. An entry is a table name
(ordered by `id`) or `{"table", "order_by"}` for a table with no `id`, such as
the Score history tables.

`mcp` — one call through the real FastMCP client (legacy protocol mode, so the
`initialize` handshake is the one `rmcp` performs):

```json
{"mcp": {"call": "initialize"}}
{"mcp": {"call": "tools/list"}}
{"mcp": {"call": "tools/call", "tool": "get_test", "arguments": {"test_id": 1}}}
```

`tools/call` records the raw protocol `CallToolResult` (`content`,
`structuredContent`, `isError`), so an error is an ordinary result with
`isError: true`, exactly as a non-FastMCP client sees it.

## Comparing a replay

A later task compares the Rust server's response with the golden file. The
rules:

- **JSON is compared by value**, ignoring key order and whitespace.
- **Numbers compare numerically**: `1`, `1.0` and `1e0` are the same value.
  Floats compare at the precision the golden prints.
- **`"<timestamp>"`** matches any string in the timestamp rule;
  **`"<date>"`** matches any `YYYY-MM-DD`. A timestamp the scenario itself
  supplied is compared as an instant, not as text, so `Z` and `+00:00` are
  interchangeable there.
- **`status` and the recorded headers** (`content-type`, `location`) must match
  exactly.
- **Framework 422 messages** are compared loosely: the `{"detail": [{...}]}`
  shape, each entry's `type` and `loc`, and the status code must match, but the
  `msg` text may differ. Hand-written `detail` strings (a bare
  `{"detail": "..."}`) must match exactly, apart from the entries in
  `allowed-differences.json`.
- **MCP** compares the contract, not the framework envelope:
  `initialize` → `protocolVersion`, `serverInfo.name`, `capabilities.tools` and
  `instructions`; `tools/list` → each tool's `name`, `description`,
  `inputSchema` and `annotations`; `tools/call` → `isError` and the text of
  every result, with `structuredContent` compared as JSON. FastMCP's envelope
  (`_meta.fastmcp`, `outputSchema`/`x-fastmcp-wrap-result`, `icons`,
  `execution`, `resultType`) is not part of the contract and is ignored.
- **Anything else fails**, unless `allowed-differences.json` has an entry that
  covers exactly that file and field. An allowance is a list of
  `{file, field}` entries (`field` is a dotted path into the golden, such as
  `body.detail`), optionally narrowed:
  - `pythonValue`/`rustValue`: the two exact values, for a status-code change.
  - `match: {"kind": "prefix", "value": "..."}`: the Rust value must start with
    that prefix and may differ after it, for parser wording. Every entry names a row in
  `docs/plans/rust-rewrite/differences.md`; an entry with no matching row is a
  bug, and so is a `differences.md` row that changes a recorded value without an
  entry here.

## What a scenario cannot reach, and where it is covered instead

- **A failed Report.** Only the public API seeds data, and every request the
  API accepts is one the processor can parse, so no upload produces a `failed`
  Report. `POST /api/reports/{id}/retry` and `DELETE /api/reports/{id}` are
  recorded for their hand-written errors (404 and 409); their success paths are
  covered by the ported unit tests in the manifest
  (`tests/test_reports_api.py::test_retry`, `::test_delete`, task 06).
- **A database that is down.** `GET /api/health/ready` and `GET /metrics`
  answer 503 only when a query fails, which a scenario cannot arrange without
  stopping its own database mid-request. `tests/test_ops_api.py` owns those
  cases (task 05).
- **Time.** Trends, retention and `new_within_days` depend on the clock; the
  scenarios record their current-day values with `<date>`/`<timestamp>`
  placeholders, and the ported unit tests own the time-dependent behaviour.
- **GitHub.** No scenario talks to GitHub: `FLAKERADAR_GITHUB_TOKEN` is empty,
  so filing and syncing are no-ops. Task 10 covers GitHub with `wiremock`.
