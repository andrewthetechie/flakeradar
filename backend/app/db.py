"""Async database engine and session management (PostgreSQL + asyncpg)."""

from collections.abc import AsyncIterator

from sqlalchemy.ext.asyncio import (
    AsyncEngine,
    AsyncSession,
    async_sessionmaker,
    create_async_engine,
)

from .config import get_settings

# Postgres advisory-lock keys (any stable bigint; must not collide).
MIGRATION_LOCK_KEY = 726_300_001  # serializes startup migrations across uvicorn workers
WORKER_LOCK_KEY = 726_300_002  # elects the single Report processor (ADR 0002)


def make_engine(url: str | None = None) -> AsyncEngine:
    settings = get_settings()
    return create_async_engine(
        url or settings.database_url,
        pool_pre_ping=True,
        # Cancel any statement that hangs longer than this instead of tying up a
        # pooled connection (asyncpg command_timeout).
        connect_args={"command_timeout": settings.db_command_timeout_seconds},
    )


def make_session_factory(engine: AsyncEngine) -> async_sessionmaker[AsyncSession]:
    return async_sessionmaker(engine, expire_on_commit=False)


# Creating the engine does not connect; the first query does.
engine: AsyncEngine = make_engine()
SessionLocal: async_sessionmaker[AsyncSession] = make_session_factory(engine)


async def get_db() -> AsyncIterator[AsyncSession]:
    async with SessionLocal() as session:
        yield session
