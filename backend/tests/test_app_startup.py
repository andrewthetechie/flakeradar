"""Startup wiring: the token check refuses the default, `get_db` yields a session."""

import pytest
from app.config import DEFAULT_INSECURE_TOKEN, assert_secure_token
from app.db import get_db
from sqlalchemy.ext.asyncio import AsyncSession


def test_a_real_token_needs_no_opt_in():
    assert assert_secure_token("a-real-token") is None


def test_the_default_token_is_allowed_when_explicitly_opted_in(monkeypatch):
    """The default token is allowed once the flag opts in; its value is stripped."""
    monkeypatch.setenv("FLAKERADAR_ALLOW_INSECURE", " 1 ")
    assert assert_secure_token(DEFAULT_INSECURE_TOKEN) is None


def test_the_default_token_is_refused_when_opted_out(monkeypatch):
    monkeypatch.setenv("FLAKERADAR_ALLOW_INSECURE", "0")
    with pytest.raises(RuntimeError, match="FLAKERADAR_API_TOKEN is still the default"):
        assert_secure_token(DEFAULT_INSECURE_TOKEN)


def test_the_default_token_is_refused_when_the_flag_is_unset(monkeypatch):
    # conftest sets the flag for the session; raising=False keeps this about "unset".
    monkeypatch.delenv("FLAKERADAR_ALLOW_INSECURE", raising=False)
    with pytest.raises(RuntimeError, match="FLAKERADAR_API_TOKEN is still the default"):
        assert_secure_token(DEFAULT_INSECURE_TOKEN)


async def test_get_db_yields_a_live_session(session_factory, monkeypatch):
    monkeypatch.setattr("app.db.SessionLocal", session_factory)
    gen = get_db()
    session = await anext(gen)
    assert isinstance(session, AsyncSession)
    await gen.aclose()
