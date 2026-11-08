# Parity fixtures

The frozen Python backend's answers, recorded once as committed files, replayed
by `crates/flakeradar/tests/parity.rs`. Python never runs in CI.

The golden files were recorded from the Python reference: the JUnit corpus and
the thirteen scenarios the recorder shipped with, from `97b933d`, the commit the
Rust rewrite branched from; and `github` (`449b29f`) and `failed-report`
(`8cea33f`) from `86c98804`, which is `97b933d` plus main's #130 and #131. Those
two were recorded with the recorder from `57b22b6`, the commit before `fd259c1`
deleted `tools/parity-record/` — it was already absent from the tree when they
were recorded — run against a checkout of `86c98804`'s `backend/` outside this
one. The recorder is gone with the Python code it drove. **A golden file changes
only for an intended behaviour change**; a difference the replay must accept
also needs an entry in `allowed-differences.json`.

```
junit/                    the JUnit XML corpus (inputs)
scenarios/                declarative request scripts (inputs)
golden/junit/             one parse result per corpus file, per line_base
golden/<scenario>/<nn>.json   one result per scenario step
schema.sql                pg_dump of Alembic head, minus alembic_version
allowed-differences.json  the only mismatches a replay may accept
```

At `bba16f2` this holds **15 scenarios with 316 steps**, a **33-file** JUnit corpus, **66** JUnit
goldens (one per corpus file at each `line_base`), **382 JSON goldens** in all and `schema.sql` —
383 golden files, one per step — beside `allowed-differences.json`, which is not a golden. A count
of `steps` in `scenarios/*.json` and of `golden/**/*.json` derives them; `every_scenario_step_has_a_golden_and_no_golden_is_orphaned`
fails when the two disagree, so the numbers cannot drift unnoticed. Nothing here changed after
`ac5e6f3` except `allowed-differences.json`'s two `rustValue`s (task 32's parser pre-scan refuses
`bad-nested-entities` with its own sentence) and this file (task 43 corrected which commit recorded
which scenario), so a diff against that review's head reads those two files and nothing else.

Nothing here is edited by hand: a replay compares against these files byte for
byte, so a hand edit is an unreviewed behaviour change. When a difference is
intended, add the narrowest matching entry to `allowed-differences.json` and
change the golden in the same PR.

## Recording (historical)

The files were produced by `tools/parity-record/record.py` against the reference
at `97b933d` and, for the two scenarios named above, at `86c98804`; task 15
deleted the recorder with `backend/`. The recipe, kept so the fixtures stay
readable: the recorder started a throwaway `postgres:17-alpine`, created a
fresh database per scenario, started one `uvicorn` process against it (the
lifespan ran the migrations), ran the steps, and stopped the server. If
`frontend/dist` existed it was moved aside for the run and put back afterwards:
with a built frontend mounted at `/`, a trailing-slash request is answered 404
by the static files instead of Starlette's 307, and a fresh checkout has no
`dist`. Every golden was recorded against the routes-only app. Recording was
deterministic: two runs produced byte-identical files. The parts were
reproducible on their own with `--only junit`, `--only scenarios` or
`--only schema`, and one scenario at a time with `--scenario NAME`.

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

`record.py` (deleted with the Python code) also failed when a route in
FastAPI's own route table (`app.openapi()["paths"]`) appeared in no scenario,
except the docs routes and `/mcp`, which the Rust binary serves differently on
purpose.

A PostgreSQL minor-version bump changes the version comment in `schema.sql`;
the file is now a fixture, so a bump means editing it by hand and saying why in
the commit.

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
  for this scenario's server only. A value holding `$WEBHOOK` gets the URL of
  the Webhook stand-in on each side, because the recorder's port is not the
  replay's and no golden holds it; `github` is the scenario that sets it, beside
  a token and two filing gates that are not the defaults.
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
replay binds a loopback port and speaks HTTP to `axum::serve`, as the recorder
spoke to `uvicorn`, so the framing, the headers and the connection behaviour a
client meets are part of what a recording proves; the writers-app CI upload
(`POST /api/ingest` with a raw `application/xml` body and its query parameters)
is the `consumers` scenario's first step. The rules:

- **JSON is compared by value**, ignoring key order and whitespace.
- **Numbers compare numerically**: `1`, `1.0` and `1e0` are the same value.
  Floats compare at the precision the golden prints.
- **`"<timestamp>"`** matches any string in the timestamp rule;
  **`"<date>"`** matches any `YYYY-MM-DD`. A timestamp the scenario itself
  supplied is compared as an instant, not as text, so `Z` and `+00:00` are
  interchangeable there.
- **A `dump_db` step's clock values keep the order the reference writes them
  in**, which a placeholder alone cannot show: `created_at` never decreases down
  a table listed by `id` (Reports are processed oldest first), a row's
  `processed_at` is never before its own `created_at`, and every Execution of one
  Report carries one `created_at`, because the reference takes a single clock
  value per Report.
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
  `body.detail`; a numeric part indexes an array, such as `cases.2.line`),
  optionally narrowed:
  - `pythonValue`/`rustValue`: the two exact values, for a status-code change.
  - `match: {"kind": "prefix", "value": "..."}`: the Rust value must start with
    that prefix and may differ after it, for parser wording. Every entry carries a `reason`, and
  the replay's reconciliation checks each entry against the golden it names.

## What a scenario cannot reach, and where it is covered instead

- **A Report that fails in the processor, not in the parser.** `failed-report`
  reaches the `failed` state itself: its one case carries a `line` past the
  `int4` column beside a `file`, which the reference stores only with a `file`
  and fails the Report on. Its `wait_processed` and `/api/ci-runs/{id}/failures`
  goldens hold the reference's `failed: 1`. A Report that fails for a reason the
  public API can still express is therefore reachable; one that needs a
  statement to fail mid-Report is not. `POST /api/reports/{id}/retry` and
  `DELETE /api/reports/{id}` are recorded for their hand-written errors (404 and
  409); their success paths are covered by the ported tests
  (`crates/flakeradar/tests/reports.rs`'s `retry` and `delete`, task 06).
- **A background tick a step cannot wait for.** A sync, a filing and a
  Notification run on the leader's own loop, and a step can only wait for the
  queue to drain (`wait_processed`), not for one tick of that loop. `github`
  therefore records what the *service* answers and leaves the calls to the
  GitHub and Webhook stand-ins to the Rust test that replays it
  (`a_scenario_with_a_github_token_webhooks_and_gates_files_only_the_test_that_qualifies`
  in `crates/flakeradar/tests/parity.rs`), and the tick itself is owned by
  `crates/flakeradar/tests/github_worker.rs` — including
  `a_database_restart_under_the_leader_files_a_real_issue_and_syncs_again`,
  which ends every session of the database under a leading worker and asserts the
  new leader files a real issue and syncs again.
- **A database that is down.** `GET /api/health/ready` and `GET /metrics`
  answer 503 only when a query fails, which a scenario cannot arrange without
  stopping its own database mid-request. `crates/flakeradar/src/ops.rs`'s tests
  own those cases (task 05).
- **A delete that deadlocks with the report processor.**
  `DELETE /api/repos/{owner}/{name}` and
  `DELETE /api/repos/{owner}/{name}/projects/{project}` answer 503 with the
  hand-written `"Delete raced with the report processor; retry."` only when
  Postgres aborts the delete for a lock cycle with the processor
  (`crates/flakeradar/src/routes/admin.rs`). A scenario would have to hold
  a Report mid-processing while it deletes the Repo, which it cannot arrange.
  `crates/flakeradar/tests/admin.rs` owns both endpoints (task 10).
- **Time.** Trends, retention and `new_within_days` depend on the clock; the
  scenarios record their current-day values with `<date>`/`<timestamp>`
  placeholders, and the ported unit tests own the time-dependent behaviour.
- **A real GitHub or Webhook endpoint.** `github` is the one scenario that sets
  a token, so a replay of it points `FLAKERADAR_GITHUB_TOKEN` at a `wiremock`
  server and `$WEBHOOK` at a second one, at both ends: the recorder started the
  `review-135/repro/fake-github` stand-ins, and the replay starts its own. A
  scenario whose `FLAKERADAR_GITHUB_TOKEN` is empty files and syncs nothing, as
  the default does.
