# Postgres only, fully async

**Update:** ADR 0010 replaces the stack named here (SQLAlchemy, `asyncpg`, Alembic) with Rust and SeaORM. The decision to support only Postgres, and to be fully async, still stands.

We dropped SQLite and made the backend fully async (SQLAlchemy `AsyncSession` with `asyncpg`, and an async Alembic `env.py`). The queued ingest depends on `FOR UPDATE SKIP LOCKED` and advisory locks, which SQLite lacks. If dev ran on one dialect and production on another, some bugs would only show up in production, so tests also run against a real Postgres. The data was wiped, so we squashed the old Alembic history into a new baseline and removed the startup code that stamped pre-Alembic SQLite databases. Full async is a bigger rewrite than sync endpoints running in the threadpool, and at our load it is not required. We chose it so that the whole app uses one consistent concurrency model.
