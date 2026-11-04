# Intentional behaviour differences

Every way the Rust server intentionally differs from the Python server at commit `97b933d`. The rules are in `00-shared-context.md`, under "The compatibility rule". The parity tests treat each entry here as an allowed exception.

Add a row when you make a change. Removing or renaming a field, or changing a status code, needs the user's approval first.

## Pre-approved

| Area | Python | Rust | Why |
|---|---|---|---|
| Process model | `uvicorn --workers 2` | One process with a Tokio multi-thread runtime | Single binary. Leader election stays, for multiple replicas |
| MCP sessions | Session IDs per process | Stateless, no `Mcp-Session-Id` | Works across replicas. No tool uses session state |
| API docs | `/docs` Swagger UI and `/redoc` | Redoc at `/docs`. `/redoc` is gone | User decision. Redoc is read-only |
| Env var names | Case-insensitive | Uppercase only | Standard for a binary |
| Database URL | `?ssl=` (asyncpg) | `?sslmode=` (SQLx), with no translation | Fresh install. `postgresql+asyncpg://` prefixes are still accepted |
| Default bind | Set by the `uvicorn` command line | `127.0.0.1:8000` unless configured | Reads are unauthenticated by design |
| JSON numbers | `pydantic` float text | `serde_json` float text | Same values |
| Timestamps | Fraction omitted when zero | Always six fractional digits | Fabro and the frontend accept both |
| `{raw!r}` in error text | Python escaping | Single quotes around the raw value | Only unusual characters differ |
| `line` attribute | Unicode digits pass `isdigit()`, and some then crash `int()` | ASCII digits only, anything else is no line | Removes a crash |
| Report `error` text | `ExceptionType: message` | A Rust error description, same 2000-character cap | Text is for humans |
| Startup on Alembic databases | Upgrades them | Refuses to start | Fresh-install cutover (ADR 0010) |

## Added during the port

| Task | Area | Python | Rust | Why |
|---|---|---|---|---|
| 02 | A `<testcase time>` attribute that is not a number | junitparser's `time` property runs `float(...)`, so `parse_junit_xml` raises an unhandled `ValueError` and `POST /api/ingest` answers 500 and stores nothing | Validates the attribute and answers 400 with the usual `{"detail": …}` JSON body (`application/json`) | **Approved by the user on 2026-11-04.** A 400 is deliberate: the code standards forbid panicking on untrusted CI input, and a 500 is not a contract anyone can rely on. A missing, empty, negative or fractional `time` keeps Python's behaviour (0.0, or the value parsed). Because the status changes, the golden's `body` (`Internal Server Error`) and `content-type` (`text/plain; charset=utf-8`) differ too; the allowance names those fields of that one golden, no others |
| 04 | The `line` attribute | Unicode digits pass `isdigit()`, so the Arabic-Indic `line="١٢"` in `non-numeric-line.xml` becomes line 12 (13 with `line_base=0`) | ASCII digits only, so that value is no line (`null`) | The pre-approved rule above, recorded here because two JUnit goldens change in exactly this field. The allowance in `allowed-differences.json` names the case and both `line_base` values |
| 02 | The suffix of a JUnit parse-error detail | `"Not a valid JUnit XML report: "` + the ElementTree/expat message, such as `syntax error: line 1, column 0` | `"Not a valid JUnit XML report: "` + the `roxmltree` message | The prefix and the 422 status are the contract, and they stay. The parsers' own wording cannot be reproduced, and rewriting expat's messages would be worse than saying which parser rejected the file. `"Report parsed but contained no test cases."` is hand-written and stays identical |
