"""Run Alembic migrations at startup, from inside the running event loop.

Several uvicorn workers start at once, so the upgrade runs under a
transaction-scoped advisory lock: the first worker migrates, the others wait
and then find nothing to do.
"""

from pathlib import Path

from alembic import command
from alembic.config import Config
from sqlalchemy import text
from sqlalchemy.engine import Connection
from sqlalchemy.ext.asyncio import AsyncEngine

from .db import MIGRATION_LOCK_KEY


def _alembic_config() -> Config:
    backend_root = Path(__file__).resolve().parents[1]  # backend/
    cfg = Config(str(backend_root / "alembic.ini"))
    cfg.set_main_option("script_location", str(backend_root / "migrations"))
    return cfg


def _upgrade(connection: Connection) -> None:
    cfg = _alembic_config()
    cfg.attributes["connection"] = connection  # see migrations/env.py
    command.upgrade(cfg, "head")


async def run_migrations(engine: AsyncEngine) -> None:
    async with engine.begin() as conn:
        await conn.execute(text("SELECT pg_advisory_xact_lock(:key)"), {"key": MIGRATION_LOCK_KEY})
        await conn.run_sync(_upgrade)
