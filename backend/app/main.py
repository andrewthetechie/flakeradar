"""FastAPI application: lifespan, API routers, static frontend hosting."""

import logging
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.staticfiles import StaticFiles
from fastmcp.utilities.lifespan import combine_lifespans

from . import github_integration
from .config import DEFAULT_INSECURE_TOKEN, assert_secure_token, get_settings
from .db import SessionLocal, engine
from .mcp_server import build_mcp
from .migrate import run_migrations
from .retention import run_prune
from .routers import reports, tests
from .worker import ReportWorker

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("flakeradar")
# Alembic's fileConfig sets the root logger to WARNING at startup; pin our
# own level so flakeradar.* INFO logs (e.g. processor election) still show.
logger.setLevel(logging.INFO)


@asynccontextmanager
async def lifespan(app: FastAPI):
    settings = get_settings()
    assert_secure_token(settings.api_token)
    if settings.api_token == DEFAULT_INSECURE_TOKEN:
        logger.warning("FLAKERADAR_API_TOKEN is the default 'changeme' — set a real token.")
    await run_migrations(engine)

    async def _maintenance() -> None:
        """Leader-only housekeeping: retention prune + re-arm closed GitHub issues."""
        await run_prune(SessionLocal)
        await github_integration.sync_closed_issues(SessionLocal)

    worker = ReportWorker(
        engine,
        SessionLocal,
        poll_seconds=settings.worker_poll_seconds,
        on_processed=lambda outcome: github_integration.on_report_processed(SessionLocal, outcome),
        prune=_maintenance,
        prune_interval_seconds=settings.prune_interval_seconds,
    )
    await worker.start()
    try:
        yield
    finally:
        await worker.stop()
        await engine.dispose()


# Read-only MCP server for agents (Bearer token = FLAKERADAR_API_TOKEN).
mcp = build_mcp(SessionLocal, api_token=get_settings().api_token)
mcp_app = mcp.http_app(path="/")

app = FastAPI(
    title="FlakeRadar",
    version="2.0.0",
    lifespan=combine_lifespans(lifespan, mcp_app.lifespan),
)

app.add_middleware(
    CORSMiddleware,
    allow_origins=get_settings().cors_origins.split(","),
    allow_methods=["*"],
    allow_headers=["*"],
)


@app.get("/api/health")
async def health() -> dict:
    return {"status": "ok"}


# --- API routers: include them HERE, above the static mount. -------------
# A Mount("/") registered earlier would swallow every later route.
app.include_router(reports.router)
app.include_router(tests.router)
app.mount("/mcp", mcp_app)  # endpoint: /mcp/ (POST /mcp redirects there)


# Serve the built frontend (frontend/dist) if present — single-container self-host.
_dist = Path(__file__).resolve().parents[2] / "frontend" / "dist"
if _dist.is_dir():
    app.mount("/", StaticFiles(directory=_dist, html=True), name="frontend")
