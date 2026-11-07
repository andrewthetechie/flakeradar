---
status: accepted
---

# Replace the Python backend with one Rust binary, cut over by fresh install

FlakeRadar should run from a single binary plus a PostgreSQL database, on Linux, macOS and Windows, so we replace the FastAPI backend with Rust: Axum for HTTP, SeaORM 2 for SQL, `rmcp` for MCP, and the React frontend embedded in the binary at compile time. The frontend stays a separate app in `frontend/`. The Python server is deleted in the same PR that adds the Rust one, so `main` never has two backends.

The goal is functional parity, not identical bytes. A response may change only if the frontend, the actions, the samples, MCP clients, Fabro and other CI reporters keep working, and each change is recorded. Test fingerprints stay byte-identical because Fabro's issue keys depend on them.

The cutover is a fresh install: there is no import tool, and the binary refuses to start on a database that Alembic created. FlakeRadar has one operator, who accepted losing the history. Porting 18 migrations and proving a data upgrade would cost more than rebuilding scores from new reports.

## Considered Options

- **Diesel:** rejected. Its strength is compile-time query checking, but the leaderboard builds its query at runtime from filters, a sort key and an optional lateral join, shared across Tests and Jobs. Async support is also a separate crate.
- **SQLx alone:** workable, but we would maintain every query as SQL. SeaORM runs on SQLx and still allows raw statements for the few queries its builder cannot express, such as advisory locks and `pg_locks`.
- **Adding SQLite for a database-free install:** rejected. The queue needs `FOR UPDATE SKIP LOCKED` and advisory locks (ADR 0002, ADR 0003).
- **Porting incrementally on `main`:** rejected. One PR gives one review and a clean cutover.

## Consequences

- The supported targets are Linux, macOS and Windows on x86-64, and Linux and macOS on ARM64. Windows ARM64 is not a target, because its GitHub runner image has no PostgreSQL to smoke test against.
- The MCP server is stateless, so it works across replicas with no sticky routing. No tool uses session state.
- The bare binary binds to `127.0.0.1` by default, because the read APIs and quarantine are unauthenticated by design. The container image binds to `0.0.0.0`.
- Rust coverage replaces the Python gate: line and branch coverage on a pinned nightly, with floors and a no-decrease check against `main`.
- After merge, FlakeRadar needs a new Fabro sandbox image with Rust, Node and PostgreSQL before Fabro can work on this repo again.
