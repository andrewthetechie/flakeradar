# 05 — Server shell

Read `00-shared-context.md` first. Tasks 01–04 have landed.

Python reference: `backend/app/main.py`, `config.py`, `auth.py`, `routers/ops.py`, `backend/tests/test_app_startup.py`, `test_ops_api.py`.

## Goal

The Axum app that later routes plug into: settings, errors, extractors, auth, health, metrics, API docs, and the parity replay harness.

## Deliverables

### Settings

A `Settings` struct with every field and default from `config.py`, plus `host` (default `127.0.0.1`) and `port` (default `8000`).

- Names are `FLAKERADAR_<FIELD>` in uppercase. Load `.env` from the working directory with `dotenvy`, with real env vars taking precedence. Ignore unknown vars.
- `FLAKERADAR_ALLOW_INSECURE` stays a separate check. Booleans accept `1/0`, `true/false`, `yes/no`, `on/off`, in any case. A bad value exits with an error naming the variable.
- `Settings::from_map` takes an explicit map, so tests set values with no process-wide env changes. Pass `Arc<Settings>` through app state. No globals.
- `assert_secure_token`: same rule and text as Python. Log the same warning when the default token is allowed.

### App construction

`build_app(state) -> Router`, usable in tests with no socket.

- `AppState` holds the pool and settings.
- Middleware: CORS from `cors_origins` split on commas, with all methods and all headers, no credentials. Tracing that **never logs a query string or a full outbound URL**. Webhook URLs are credentials (ADR 0009).
- `GET /api/health` returns `{"status":"ok"}`.

### Errors and extractors

- `ApiError` renders `{"detail": "<text>"}` with a status code.
- `ApiQuery<T>`, `ApiPath<T>` and `ApiJson<T>` wrap Axum's extractors. On a failure they return **422** with `{"detail":[{"type","loc","msg"}]}`, with `loc` as FastAPI sets it, such as `["query","limit"]`, `["path","test_id"]`, `["body","number"]`. Malformed JSON is 422 with `loc: ["body", <position>]` and `type: "json_invalid"`. Add a `Validate` trait, so range and length rules give the same shape.
- `require_token` middleware: compares `X-API-Key` in constant time. A missing or wrong key is 401 with `Invalid or missing X-API-Key`.
- Set a default body limit of 20 MB on JSON routes, so a large pipeline report is not rejected by Axum's 2 MB default.

### Fallback

In order:

1. If the path ends in `/` and is not `/`, return 307 to the path with the slash removed, keeping the query string, as Starlette's `redirect_slashes` does. It is fine for this to redirect paths that then 404.
2. Otherwise return `{"detail":"Not Found"}` with 404.

Task 12 adds static assets ahead of step 2. `/mcp` to `/mcp/` is handled in task 11.

### Ops routes

- `GET /api/health/ready`: Python's behaviour, including `{"database":false}` with 503. Use the `pg_locks` query from `ops.py`: `classid = 0`, `objid = key`, scoped to the current database.
- `GET /metrics`: same metric names, HELP text, TYPE lines, label order and media type `text/plain; version=0.0.4; charset=utf-8`. 503 with an empty body on a database failure. The `last_report_processed` line is absent until a Report has been processed.

### API docs

- `utoipa::OpenApi` collects routes and DTOs, title `FlakeRadar`, version from the crate. Every later route must add its `#[utoipa::path]`.
- `GET /openapi.json`.
- `GET /docs`: `utoipa-redoc` with `custom_html`, loading `/docs/redoc.standalone.js` from the binary. Vendor a pinned Redoc release at `crates/flakeradar/assets/redoc.standalone.js`, and record its version, source URL and SHA-256 in an adjacent `README.md`. Add its licence to `NOTICE`. Test that the HTML references no external host.

### Parity replay harness

`crates/flakeradar/tests/parity.rs`: implements the contract in `tests/parity/README.md`. It runs each listed scenario against `build_app` on a `TestDb`, using the settings the recorder used. It has an `ENABLED` list. Later tasks add scenarios to it as their routes land. Implement step types now, including `wait_processed` and `dump_db`, even though later tasks use them. Enable the health and metrics scenarios in this task.

## Tests to port

`test_ops_api` (10) and `test_app_startup` (10). Lifespan tests that need the worker move to task 08, and `mount_frontend` tests move to task 12. Change their manifest rows.

Add tests for each extractor's 422 shape, the 20 MB JSON limit, the redirect and its query string, every setting's default, and a bad value for each type.

## Done when

- [ ] All verification in `00-shared-context.md` passes.
- [ ] Enabled parity scenarios pass.
- [ ] `cargo run -p flakeradar` serves `/api/health`, `/docs` and `/openapi.json` on a fresh database.
- [ ] Manifest rows for this task are not `pending`, and gaps are logged.
- [ ] `STATUS.md` is updated, and the branch is pushed.
