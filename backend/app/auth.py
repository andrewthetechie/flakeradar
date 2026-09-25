"""Shared-token auth for CI-facing endpoints."""

import secrets

from fastapi import Header, HTTPException

from .config import get_settings


def require_token(x_api_key: str = Header(default="")) -> None:
    expected = get_settings().api_token
    if not secrets.compare_digest(x_api_key.encode(), expected.encode()):
        raise HTTPException(status_code=401, detail="Invalid or missing X-API-Key")
