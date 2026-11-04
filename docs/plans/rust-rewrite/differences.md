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
