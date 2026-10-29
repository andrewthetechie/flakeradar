"""Startup wiring: the token check refuses the default, `get_db` yields a session,
and `lifespan` boots the worker and shuts it down again."""

import logging

import pytest
from app import main
from app.config import DEFAULT_INSECURE_TOKEN, Settings, assert_secure_token
from app.db import get_db, make_engine, make_session_factory
from sqlalchemy.ext.asyncio import AsyncSession

from tests.test_github_worker import _lock_is_free, _wait_for


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


async def test_the_lifespan_warns_about_the_default_token_and_holds_the_worker_lock(
    database_url, monkeypatch, caplog
):
    """Boot with the shipped default token: the warning is logged, the Report
    processor takes its advisory lock, and both are gone again after exit."""
    # A new engine, never the session `engine` fixture: the lifespan disposes it on exit.
    eng = make_engine(database_url)
    monkeypatch.setattr(main, "engine", eng)
    monkeypatch.setattr(main, "SessionLocal", make_session_factory(eng))
    with caplog.at_level(logging.WARNING, logger="flakeradar"):
        async with main.lifespan(main.app):  # only `lifespan`, not the combined MCP one

            async def lock_is_held() -> bool:
                return not await _lock_is_free(eng)

            await _wait_for(lock_is_held)  # the worker elects itself right after start()
            assert await lock_is_held()
    assert "FLAKERADAR_API_TOKEN is the default 'changeme'" in caplog.text
    assert await _lock_is_free(eng)  # stop() released the lock


async def test_a_real_token_boots_without_the_default_warning(database_url, monkeypatch, caplog):
    eng = make_engine(database_url)
    monkeypatch.setattr(main, "engine", eng)
    monkeypatch.setattr(main, "SessionLocal", make_session_factory(eng))
    monkeypatch.setattr(main, "get_settings", lambda: Settings(api_token="real-token"))
    with caplog.at_level(logging.WARNING, logger="flakeradar"):
        async with main.lifespan(main.app):
            pass
    assert "default 'changeme'" not in caplog.text
