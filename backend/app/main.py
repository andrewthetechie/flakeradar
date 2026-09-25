"""FastAPI application: lifespan, API routers, static frontend hosting."""
import logging
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.staticfiles import StaticFiles

from .config import DEFAULT_INSECURE_TOKEN, assert_secure_token, get_settings
from .db import engine
from .migrate import run_migrations

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
        logger.warning(
            "FLAKERADAR_API_TOKEN is the default 'changeme' — set a real token."
        )
    await run_migrations(engine)
    yield
    await engine.dispose()


app = FastAPI(title="FlakeRadar", version="2.0.0", lifespan=lifespan)

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


# Serve the built frontend (frontend/dist) if present — single-container self-host.
_dist = Path(__file__).resolve().parents[2] / "frontend" / "dist"
if _dist.is_dir():
    app.mount("/", StaticFiles(directory=_dist, html=True), name="frontend")
