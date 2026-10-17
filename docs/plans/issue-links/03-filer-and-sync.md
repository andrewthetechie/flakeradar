# 03 — Prove the filer skips a linked entity and the sync re-arms it

## Tracer-Bullet Outcome
Tests show that FlakeRadar's filer files no issue for a Test or Job that another filer linked, and that the closed-issue sync clears such a link when GitHub reports the issue closed, after which the filer files again. This covers two of issue #6's "Done when" points. No production code changes, unless a test finds a bug.

## User Story
As the maintainer, I want to turn FlakeRadar's own filing on in a repo where fabro's scout already filed issues, and get no duplicates.

## Description
New tests in `backend/tests/test_github.py` that use the task 01/02 service functions to link, then run the real filer and sync against an `httpx.MockTransport`.

## Context Pack
- Source decisions: `00-shared-context.md`, "Filer and sync (no code change; tests only)". ADR 0008, "Consequences".
- Repo facts (`backend/tests/test_github.py`):
  - Fixture `gh_settings` patches `app.github_integration.get_settings` to a `Settings` with `github_token="tok"`, `flake_threshold=0.3`, `github_issue_min_score=0.3`, and the other minimums at 0. Use it in every test here: without it the filer and the sync are no-ops.
  - `_flaky_test(db, repo="acme/app", project="backend", name="t_flaky", flakiness_score=0.6, confirmed_flake_count=1, **fields)` makes a Project, a Test that meets the filing gate, a Run and two Executions, and **commits**. It returns the `TestCase`.
  - `_flaky_job(db, provider="github", name="e2e (ubuntu-latest)", *, pipeline=".github/workflows/ci.yml", **fields)` makes a github Pipeline in `acme/app` and a Job that meets the score gate, and commits. `_job_exec(db, job, ci_job_id, explained=False)` adds a failed Job execution and commits.
  - `_recorder(status=201, number=77)` returns `(calls, transport)`. Each request is appended to `calls`, and the response is `{"number": 77}` with that status.
  - `github_integration.file_issues_for(db, test_ids, transport=...)`, `file_job_issues_for(db, job_ids, transport=...)`, `sync_closed_issues(session_factory, transport=...)`. The sync uses its own session, so call `await db.refresh(obj)` before you assert.
  - Existing tests to copy: `test_sync_rearms_closed_issue_only` and `test_job_sync_rearms_closed_issue_and_refiles`.
  - Task 01/02 service functions: `issue_links.link_test_issue(db, test_id, number, *, url=None, replace=False, threshold)` and `issue_links.link_job_issue(...)`. They commit.
- Non-goals: closing the filer race described in ADR 0008; any change to `github_integration.py` (if a test below fails because of a real bug there, fix it in its own commit and say why).

## Delivery Strategy
- Shape: Normal tracer bullet (behavior proof; the code path exists)
- Valid-state scope: `feat/issue-links`

## Implementation Contract
- Expected files: `backend/tests/test_github.py`.
- Interfaces and names: add `from app import issue_links` (next to `from app import github_integration`). Put the new tests at the end of the file under a comment `# --- Issue links from other filers (issue #6) ---`.
- Behavior rules: none new.
- Error and security rules: None.

## Steps
1. Add the tests below.
2. Run them. They are expected to pass at once. If one fails, find out whether the test or the code is wrong before you change either.
3. Run the Validator.

## Acceptance Criteria
- [ ] Every test below passes.
- [ ] `backend/app/github_integration.py` is unchanged, or each change is a separate commit with a reason.

## Test Expectations
1. `test_linked_test_is_not_filed(db, gh_settings)`: `linked = await _flaky_test(db, name="linked")`, `other = await _flaky_test(db, repo="acme/other", name="other")`. `await issue_links.link_test_issue(db, linked.id, 300, threshold=0.3)`. `calls, transport = _recorder()`, `await github_integration.file_issues_for(db, [linked.id, other.id], transport=transport)`. Exactly one call, `calls[0].url.path == "/repos/acme/other/issues"`. `await db.refresh(linked)` → `linked.github_issue_number == 300`.
2. `test_linked_job_is_not_filed(db, gh_settings)`: `job = await _flaky_job(db)`, `await _job_exec(db, job, "1")`, `await issue_links.link_job_issue(db, job.id, 301, threshold=0.3)`. `file_job_issues_for(db, [job.id], transport=transport)` → `calls == []`. The Job still has `301`.
3. `test_sync_rearms_a_linked_test_and_the_filer_files_again(db, gh_settings, session_factory)`: `tc = await _flaky_test(db)`, link `300`. A handler that appends each request to a list and returns `httpx.Response(200, json={"state": "closed"})`. `await github_integration.sync_closed_issues(session_factory, transport=httpx.MockTransport(handler))`. One request, `path == "/repos/acme/app/issues/300"`. `await db.refresh(tc)` → `None`. Then `calls, transport = _recorder()` and `file_issues_for(db, [tc.id], transport=transport)` → one call, and after `db.refresh(tc)` the number is `77`.
4. `test_sync_keeps_a_linked_issue_while_it_is_open(db, gh_settings, session_factory)`: link `300`, the handler returns `200 {"state": "open"}` → after the sync and a refresh, still `300`.
5. `test_sync_rearms_a_linked_job(db, gh_settings, session_factory)`: `job = await _flaky_job(db)`, link `301` with `link_job_issue`, the handler returns `200 {"state": "closed"}` → after the sync and a refresh, `job.github_issue_number is None`.

## Validator
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```

## Dependencies
- Blocked by: 02.
- Blocks: 05.

## Labels
`test`, `backend`, `priority:high`
