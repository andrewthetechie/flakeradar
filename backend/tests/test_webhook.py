"""Webhook Notifications: settings parsing, deep links, and the json/slack payloads."""

import pytest
from app import notify
from app.config import Settings
from app.notify import TierChange
from pydantic import ValidationError

PUBLIC_URL = "https://flakeradar.example.com"


def _test_change(**kw) -> TierChange:
    base = {
        "kind": "test",
        "id": 42,
        "repo": "acme/app",
        "project": "backend",
        "name": "test_login",
        "classname": "tests.test_auth",
        "flakiness_score": 0.42,
        "tier": "flaky",
        "confirmed_flake_count": 2,
        "failure_category": "timing",
    }
    base.update(kw)
    return TierChange(**base)


def _job_change(**kw) -> TierChange:
    base = {
        "kind": "job",
        "id": 3,
        "repo": "acme/app",
        "project": None,
        "name": "test (ubuntu, 3.12)",
        "pipeline": ".github/workflows/ci.yml",
        "flakiness_score": 0.51,
        "tier": "flaky",
        "confirmed_flake_count": 1,
        "failure_category": None,
    }
    base.update(kw)
    return TierChange(**base)


def test_parse_urls_splits_and_strips():
    assert notify.parse_urls(" https://a.example/hook , ,https://b.example/hook, ") == [
        "https://a.example/hook",
        "https://b.example/hook",
    ]


@pytest.mark.parametrize("raw", ["", "   ", ",", " , , "])
def test_parse_urls_of_nothing_is_empty(raw):
    assert notify.parse_urls(raw) == []


def test_an_unknown_webhook_format_is_refused():
    with pytest.raises(ValidationError):
        Settings(webhook_format="xml")


def test_webhook_settings_default_to_off():
    settings = Settings()
    assert settings.webhook_urls == ""
    assert settings.webhook_format == "json"
    assert settings.public_url == ""


def test_json_payload_for_a_test_carries_every_field():
    payload = notify.render_json([_test_change()], PUBLIC_URL)
    assert payload == [
        {
            "event": "test.flaky",
            "repo": "acme/app",
            "project": "backend",
            "test": {
                "id": 42,
                "classname": "tests.test_auth",
                "name": "test_login",
                "flakiness_score": 0.42,
                "tier": "flaky",
                "confirmed_flake_count": 2,
                "failure_category": "timing",
            },
            "url": "https://flakeradar.example.com/?repo=acme%2Fapp&project=backend&test=42",
        }
    ]


def test_json_payload_for_a_job_has_no_project_and_names_its_pipeline():
    payload = notify.render_json([_job_change(tier="suspect", flakiness_score=0.1)], PUBLIC_URL)
    assert payload == [
        {
            "event": "job.recovered",
            "repo": "acme/app",
            "project": None,
            "job": {
                "id": 3,
                "pipeline": ".github/workflows/ci.yml",
                "name": "test (ubuntu, 3.12)",
                "flakiness_score": 0.1,
                "tier": "suspect",
                "confirmed_flake_count": 1,
            },
            "url": "https://flakeradar.example.com/?view=jobs&repo=acme%2Fapp&job=3",
        }
    ]


def test_json_payload_omits_the_url_without_a_public_url():
    (payload,) = notify.render_json([_test_change()], "")
    assert "url" not in payload


def test_json_payload_is_a_list_for_one_event():
    """One shape for every batch: consumers never branch on the request size."""
    assert len(notify.render_json([_test_change()], PUBLIC_URL)) == 1


def test_slack_payload_has_one_line_per_event():
    slack = notify.render_slack([_test_change(), _job_change()], PUBLIC_URL)
    assert slack == {
        "text": (
            'FlakeRadar: acme/app / backend — test "test_login" became flaky (score 0.42) '
            "https://flakeradar.example.com/?repo=acme%2Fapp&project=backend&test=42\n"
            'FlakeRadar: acme/app — job "test (ubuntu, 3.12)" in .github/workflows/ci.yml '
            "became flaky (score 0.51) "
            "https://flakeradar.example.com/?view=jobs&repo=acme%2Fapp&job=3"
        )
    }


def test_slack_payload_wording_for_a_recovery():
    slack = notify.render_slack([_test_change(tier="suspect", flakiness_score=0.1)], PUBLIC_URL)
    assert slack["text"] == (
        'FlakeRadar: acme/app / backend — test "test_login" recovered (score 0.10) '
        "https://flakeradar.example.com/?repo=acme%2Fapp&project=backend&test=42"
    )


def test_slack_payload_without_a_public_url_ends_the_line_cleanly():
    slack = notify.render_slack([_test_change()], "")
    assert slack["text"] == 'FlakeRadar: acme/app / backend — test "test_login" became flaky (score 0.42)'
