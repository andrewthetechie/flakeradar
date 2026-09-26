"""Trend reading (ADR 0007): the direction of a score versus 14 days ago."""

from app.score_history import trend_for


def test_trend_for_no_history_is_null():
    assert trend_for(0.5, None) is None


def test_trend_for_worsening():
    assert trend_for(0.5, 0.1) == "worsening"


def test_trend_for_improving():
    assert trend_for(0.1, 0.5) == "improving"


def test_trend_for_steady():
    assert trend_for(0.32, 0.30) == "steady"
