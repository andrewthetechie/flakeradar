"""The Issue-link pair's lifecycle rules, tested once at the issue_links module interface (issue #113).

One concept — the `(github_issue_number, github_issue_checked_at)` pair a Test or Job carries
— has two different "clear" semantics, and they are easy to confuse, so they are pinned side by
side here (see also `test_unlink_clears_both_fields_but_rearm_keeps_its_stamp`):

* `unlink_issue` clears BOTH halves: the entity has no link at all, so there is nothing to check.
* The sync's re-arm (`github_integration._clear_if_closed`) clears only the NUMBER and keeps the
  `utcnow()` stamp it wrote one line earlier, because the issue really was read this run. That is
  safe: `linked_for_check` and both partial indexes consider only rows that carry a number.
* A brand-new link clears BOTH halves through `store_filed_link` — `link_issue` with a different
  number, or the filer storing what GitHub just created — because a new link is checked first.

Everything here goes through the module interface: the entry points `link_issue` / `unlink_issue`
/ `linked_for_check`, the sync `sync_closed_issues`, and the filer `file_issues_for`, with the
proven `transport=httpx.MockTransport` stand-in and the real Postgres of conftest.py.
"""

import httpx
import pytest
from app import github_integration, issue_links
from app.config import Settings

from tests.factories import make_job, make_pipeline, make_project, make_test_case


@pytest.fixture()
def gh_settings(monkeypatch):
    """Mirror of `test_github.py`'s fixture: a configured GitHub, monkeypatched into the module
    so `sync_closed_issues` reads `github_sync_batch` from this object."""
    settings = Settings(
        github_token="tok",
        flake_threshold=0.3,
        github_issue_label="flakeradar",
        github_issue_min_score=0.3,
        github_issue_min_proven_flakes=0,
        github_issue_min_failures=0,
        score_window=50,
    )
    monkeypatch.setattr("app.github_integration.get_settings", lambda: settings)
    return settings


async def _test(db, name="t1", repo="acme/app", project="backend", **fields):
    """A Test above the filing gate, so the re-file half of the lifecycle has something to file."""
    proj = await make_project(db, repo, project)
    tc = await make_test_case(db, proj, name=name, flakiness_score=0.6, confirmed_flake_count=1, **fields)
    await db.commit()
    return tc


async def _job(db, name="e2e", repo="acme/app", pipeline=".github/workflows/ci.yml", provider="github", **fields):
    pipe = await make_pipeline(db, repo, pipeline, provider=provider)
    job = await make_job(db, pipe, name=name, flakiness_score=0.6, confirmed_flake_count=1, **fields)
    await db.commit()
    return job


def _recorder(*, state="open", created_number=77):
    """The MockTransport stand-in: issue GETs answer `state` (the sync), POSTs create `created_number`
    (the filer). Returns the recorded requests and the transport, as in test_github.py."""
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.method == "POST":
            return httpx.Response(201, json={"number": created_number})
        return httpx.Response(200, json={"state": state})

    return calls, httpx.MockTransport(handler)


async def test_link_sync_refile(db, gh_settings, session_factory):
    """The whole round trip: link through the module, let the sync re-arm it, then file it again."""
    tc = await _test(db)
    out = await issue_links.link_issue(db, "Test", tc.id, 300, threshold=0.3)
    assert out.github_issue_number == 300
    await db.refresh(tc)
    assert tc.github_issue_number == 300
    assert tc.github_issue_checked_at is None  # a new link is checked first

    checked, transport = _recorder(state="closed")
    await github_integration.sync_closed_issues(session_factory, transport=transport)
    assert [r.url.path for r in checked] == ["/repos/acme/app/issues/300"]
    await db.refresh(tc)
    assert tc.github_issue_number is None
    assert tc.github_issue_checked_at is not None  # the re-arm keeps the stamp it just wrote

    filed, transport = _recorder(state="open")
    await github_integration.file_issues_for(db, [tc.id], transport=transport)
    assert [r.url.path for r in filed] == ["/repos/acme/app/issues"]
    await db.refresh(tc)
    assert tc.github_issue_number == 77
    assert tc.github_issue_checked_at is None  # the fresh link is checked first again


async def test_same_number_link_keeps_the_stamp(db, gh_settings, session_factory):
    """Re-linking the number a Test already has is a no-op: only a changed number re-arms the pair."""
    tc = await _test(db)
    await issue_links.link_issue(db, "Test", tc.id, 300, threshold=0.3)
    _, transport = _recorder(state="open")
    await github_integration.sync_closed_issues(session_factory, transport=transport)
    await db.refresh(tc)
    before = tc.github_issue_checked_at
    assert before is not None

    await issue_links.link_issue(db, "Test", tc.id, 300, threshold=0.3)
    await db.refresh(tc)
    assert tc.github_issue_number == 300
    assert tc.github_issue_checked_at == before

    await issue_links.link_issue(db, "Test", tc.id, 44, replace=True, threshold=0.3)
    await db.refresh(tc)
    assert tc.github_issue_number == 44
    assert tc.github_issue_checked_at is None  # a different number: the pair is written anew


async def test_unlink_clears_both_fields_but_rearm_keeps_its_stamp(db, gh_settings, session_factory):
    """The two "clear" semantics in one place: unlink drops the whole pair, the re-arm only the number."""
    unlinked = await _test(db, project="p1", name="t1")
    rearmed = await _test(db, project="p2", name="t2")
    await issue_links.link_issue(db, "Test", unlinked.id, 11, threshold=0.3)
    await issue_links.link_issue(db, "Test", rearmed.id, 22, threshold=0.3)

    await issue_links.unlink_issue(db, "Test", unlinked.id, threshold=0.3)
    await db.refresh(unlinked)
    assert unlinked.github_issue_number is None
    assert unlinked.github_issue_checked_at is None  # unlink: both halves of the pair go

    calls, transport = _recorder(state="closed")
    await github_integration.sync_closed_issues(session_factory, transport=transport)
    assert [r.url.path for r in calls] == ["/repos/acme/app/issues/22"]  # the unlinked row is not checked
    await db.refresh(rearmed)
    assert rearmed.github_issue_number is None
    assert rearmed.github_issue_checked_at is not None  # the re-arm: number only, stamp kept
    await db.refresh(unlinked)
    assert unlinked.github_issue_checked_at is None


async def test_job_provider_gate(db):
    """Only a Job of a GitHub Pipeline can hold a link (ADR 0008), and the gate is the provider."""
    job = await _job(db, provider="gitlab")
    with pytest.raises(issue_links.IssueNotLinkable) as info:
        await issue_links.link_issue(db, "Job", job.id, 4, threshold=0.3)
    assert str(info.value) == "Only Jobs of GitHub Pipelines can link a GitHub issue"
    assert info.value.status_code == 422
    # The refused call still holds the FOR UPDATE row lock; release it.
    await db.rollback()
    await db.refresh(job)
    assert job.github_issue_number is None
    assert job.github_issue_checked_at is None

    linked = await _job(db, name="lint")
    assert (await issue_links.link_issue(db, "Job", linked.id, 5, threshold=0.3)).github_issue_number == 5


async def test_rotation_never_checked_first_and_past_the_cap(db, gh_settings, session_factory):
    """`linked_for_check` orders checked_at ASC NULLS FIRST then id, and the cap rotates rows through."""
    gh_settings.github_sync_batch = 2
    first = await _test(db, project="p1", name="t1")
    second = await _test(db, project="p2", name="t2")
    third = await _test(db, project="p3", name="t3")
    for tc, number in ((first, 11), (second, 22), (third, 33)):
        await issue_links.link_issue(db, "Test", tc.id, number, threshold=0.3)

    rows = await issue_links.linked_for_check(db, issue_links.TEST, 2)
    assert [tc.id for tc, _ in rows] == [first.id, second.id]  # nothing checked yet: lowest ids first
    assert [repo for _, repo in rows] == ["acme/app", "acme/app"]  # the Repo name the sync logs

    calls, transport = _recorder(state="open")
    await github_integration.sync_closed_issues(session_factory, transport=transport)
    assert [r.url.path for r in calls] == ["/repos/acme/app/issues/11", "/repos/acme/app/issues/22"]
    await db.refresh(third)
    assert third.github_issue_checked_at is None  # past the cap: not reached by this sync

    rows = await issue_links.linked_for_check(db, issue_links.TEST, 2)
    assert [tc.id for tc, _ in rows] == [third.id, first.id]  # the unstamped row leads the stamped ones

    calls.clear()
    await github_integration.sync_closed_issues(session_factory, transport=transport)
    assert [r.url.path for r in calls] == ["/repos/acme/app/issues/33", "/repos/acme/app/issues/11"]
    await db.refresh(third)
    assert third.github_issue_checked_at is not None
