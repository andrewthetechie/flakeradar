"""Application configuration, sourced from environment variables (.env supported)."""

import os
from functools import lru_cache

from pydantic import field_validator
from pydantic_settings import BaseSettings, SettingsConfigDict

DEFAULT_INSECURE_TOKEN = "changeme"
DEFAULT_DATABASE_URL = "postgresql+asyncpg://flakeradar:flakeradar@localhost:5432/flakeradar"


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file=".env", env_prefix="FLAKERADAR_", extra="ignore")

    # Auth token CI systems send in the X-API-Key header (and MCP clients as a Bearer token).
    api_token: str = DEFAULT_INSECURE_TOKEN

    # PostgreSQL only (ADR 0003). postgresql:// and postgres:// are rewritten to asyncpg.
    database_url: str = DEFAULT_DATABASE_URL

    # Scoring parameters. window: how many recent executions to consider.
    # decay: geometric weight applied per step into the past (recent flips matter more).
    score_window: int = 50
    score_decay: float = 0.85

    # GitHub integration. Leave the token empty to disable (graceful no-op).
    # Issues are filed in each Test's own Repo, so the token needs
    # Issues: write on every Repo you ingest.
    github_token: str = ""

    # Label applied to every auto-filed GitHub issue.
    github_issue_label: str = "flakeradar"

    # Issue-filing gate. A test is filed only when it meets EVERY configured
    # (non-zero) minimum: flakiness score, proven flakes (same-commit fail+pass),/
    # and failure count over the recent window. Set a signal's minimum to 0 to
    # skip that gate entirely, so you can tune on score, proven flakes, failures,
    # or any combination. Defaults match the historical behavior (score only).
    github_issue_min_score: float = 0.30
    github_issue_min_proven_flakes: int = 0
    github_issue_min_failures: int = 0

    # Tier classification threshold for the UI/API ("flaky" vs "suspect").
    # Independent of the GitHub filing gate above.
    flake_threshold: float = 0.30

    cors_origins: str = "http://localhost:5173"

    # Report processor: idle poll interval when the queue is empty.
    worker_poll_seconds: float = 1.0

    # Retention (pruned hourly by the Report processor).
    report_retention_days: int = 7  # processed Reports; failed ones are kept
    execution_retention_days: int = 90  # Executions (and Runs left empty)
    prune_interval_seconds: float = 3600.0

    @field_validator("database_url")
    @classmethod
    def _require_asyncpg(cls, value: str) -> str:
        for prefix in ("postgresql://", "postgres://"):
            if value.startswith(prefix):
                return "postgresql+asyncpg://" + value[len(prefix) :]
        if not value.startswith("postgresql+asyncpg://"):
            raise ValueError(
                "FLAKERADAR_DATABASE_URL must be a PostgreSQL URL (postgresql+asyncpg://user:pass@host:5432/db)"
            )
        return value


@lru_cache
def get_settings() -> Settings:
    return Settings()


def assert_secure_token(token: str) -> None:
    """Refuse to boot on the shipped default token unless explicitly allowed.

    Tests and throwaway local demos set ``FLAKERADAR_ALLOW_INSECURE=1``.
    Docker/production must set a real ``FLAKERADAR_API_TOKEN``.
    """
    if token != DEFAULT_INSECURE_TOKEN:
        return
    if os.getenv("FLAKERADAR_ALLOW_INSECURE", "").strip() == "1":
        return
    raise RuntimeError(
        "FLAKERADAR_API_TOKEN is still the default 'changeme'. Set a real "
        "token (see .env.example) or set FLAKERADAR_ALLOW_INSECURE=1 for a "
        "throwaway local demo."
    )
