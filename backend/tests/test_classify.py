"""Failure classification rules and dominant-category caching (ADR 0006)."""

import pytest
from app.classify import classify_failure, dominant_category


@pytest.mark.parametrize(
    ("message", "details", "expected"),
    [
        ("Test timeout of 30000ms exceeded.", "", "timing"),
        ("Timed out 5000ms waiting for expect(locator).toBeVisible()", "", "timing"),
        ("Error: connect ECONNREFUSED 127.0.0.1:5432", "", "network"),
        ("Error: connect ETIMEDOUT 10.0.0.1:443", "", "network"),
        ("Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:3000/", "", "network"),
        ("expected 200 but got 503 Service Unavailable", "", "network"),
        ("requests.exceptions.ReadTimeout: read timed out", "", "timing"),
        ("OSError: [Errno 28] No space left on device", "", "environment"),
        ("Error: Target page, context or browser has been closed", "", "environment"),
        ("AssertionError: expected 3, got 4", "", "assertion"),
        ("assert 1 == 2", "", "assertion"),
        ("", "Traceback (most recent call last):\nAssertionError: boom", "assertion"),
        ("AssertionError: expected 3", "… ECONNRESET …", "assertion"),  # the message wins
        ("FAILURE", "", "other"),
        ("", "", "other"),
    ],
)
def test_classify_failure(message: str, details: str, expected: str):
    assert classify_failure(message, details) == expected


def test_dominant_category_majority():
    assert dominant_category(["timing", "assertion", "timing"]) == "timing"


def test_dominant_category_tie_newest_wins():
    assert dominant_category(["assertion", "timing"]) == "assertion"


def test_dominant_category_none_when_all_null():
    assert dominant_category([None, None]) is None


def test_dominant_category_skips_null():
    assert dominant_category([None, "network"]) == "network"
