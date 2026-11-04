"""Webhook Notifications: one message per Tier change, as ``json`` or ``slack``.

A Notification is best effort (ADR 0009). The leader renders it and posts it from
its own task, so a slow or unreachable endpoint never delays Report processing.
Nothing is stored: a Notification that is not delivered is lost.

The payload is built from a `TierChange`, the snapshot a rescore took when the
Test or Job crossed the Flake threshold. Rendering never reads the database, so
the numbers can never contradict the direction of the event that carries them.
"""

from dataclasses import dataclass
from typing import Any, Literal
from urllib.parse import quote

from .classify import FailureCategory
from .config import WebhookFormat
from .schemas import Tier

Kind = Literal["test", "job"]


@dataclass(frozen=True)
class TierChange:
    """One Test's or Job's crossing of the Flake threshold, snapshotted at detection.

    `tier` is the new tier, and the direction of the event follows from it, so
    the two can never disagree. Test values come from the same rescore that
    wrote them; a Job has no Project and no Failure category (CONTEXT.md).
    """

    kind: Kind
    id: int
    repo: str
    project: str | None
    name: str
    flakiness_score: float
    tier: Tier
    confirmed_flake_count: int
    classname: str | None = None  # tests only
    pipeline: str | None = None  # jobs only
    failure_category: FailureCategory | None = None  # tests only

    @property
    def event(self) -> str:
        """`test.flaky`, `test.recovered`, `job.flaky` or `job.recovered`."""
        return f"{self.kind}.{'flaky' if self.tier == 'flaky' else 'recovered'}"


def parse_urls(raw: str) -> list[str]:
    """Split the comma-separated setting, dropping blanks and stray whitespace."""
    return [url.strip() for url in raw.split(",") if url.strip()]


def _dashboard_url(change: TierChange, public_url: str) -> str | None:
    """The dashboard deep link for one Test or Job, or None without a public URL.

    Mirrors frontend/src/urlstate.ts, so the link opens the drawer.
    """
    base = public_url.rstrip("/")
    if not base:
        return None
    repo = quote(change.repo, safe="")
    if change.kind == "job":
        return f"{base}/?view=jobs&repo={repo}&job={change.id}"
    project = quote(change.project or "", safe="")
    return f"{base}/?repo={repo}&project={project}&test={change.id}"


def _json_payload(change: TierChange, public_url: str) -> dict[str, Any]:
    entity: dict[str, Any]
    if change.kind == "job":
        entity = {
            "id": change.id,
            "pipeline": change.pipeline,
            "name": change.name,
            "flakiness_score": change.flakiness_score,
            "tier": change.tier,
            "confirmed_flake_count": change.confirmed_flake_count,
        }
    else:
        entity = {
            "id": change.id,
            "classname": change.classname,
            "name": change.name,
            "flakiness_score": change.flakiness_score,
            "tier": change.tier,
            "confirmed_flake_count": change.confirmed_flake_count,
            "failure_category": change.failure_category,
        }
    payload: dict[str, Any] = {
        "event": change.event,
        "repo": change.repo,
        "project": change.project,
        change.kind: entity,
    }
    url = _dashboard_url(change, public_url)
    if url is not None:
        payload["url"] = url
    return payload


def render_json(changes: list[TierChange], public_url: str) -> list[dict[str, Any]]:
    """A list of event objects: one shape whether the Report crossed one tier or many."""
    return [_json_payload(change, public_url) for change in changes]


def _slack_line(change: TierChange, public_url: str) -> str:
    subject = f"{change.repo} / {change.project}" if change.kind == "test" else change.repo
    noun = "test" if change.kind == "test" else "job"
    where = f" in {change.pipeline}" if change.kind == "job" else ""
    verb = "became flaky" if change.tier == "flaky" else "recovered"
    line = f'FlakeRadar: {subject} — {noun} "{change.name}"{where} {verb} (score {change.flakiness_score:.2f})'
    url = _dashboard_url(change, public_url)
    return f"{line} {url}" if url is not None else line


def render_slack(changes: list[TierChange], public_url: str) -> dict[str, str]:
    """One ``text`` with a line per event: Slack renders the newlines as lines."""
    return {"text": "\n".join(_slack_line(change, public_url) for change in changes)}


def render(changes: list[TierChange], fmt: WebhookFormat, public_url: str) -> dict[str, Any] | list[dict[str, Any]]:
    """The request body for one Report's batch of Tier changes, in `fmt`."""
    return render_slack(changes, public_url) if fmt == "slack" else render_json(changes, public_url)
