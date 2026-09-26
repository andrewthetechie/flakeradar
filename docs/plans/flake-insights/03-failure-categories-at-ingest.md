# 03 — Classify failures at ingest; cache a Test's category

## Tracer-Bullet Outcome
When a JUnit report is processed, every failing Execution gets a Failure category (`network`, `environment`, `timing`, `assertion`, `other`) from fixed regex rules, and each touched Test caches its dominant category. `GET /api/tests`, `GET /api/tests/{id}/history` and the MCP tools return `failure_category` on Tests and on Executions. For example, a report whose failure says `Test timeout of 30000ms exceeded.` gives that Execution and its Test `"failure_category": "timing"`.

## User Story
As a maintainer triaging flaky tests, I want each flaky test labeled with its likely cause, so that I can fix the timing flakes together and the network flakes together.

## Description
1. New pure module `backend/app/classify.py`: the rules, `classify_failure` and `dominant_category`.
2. Schema: migration `0004_failure_category.py`, adding `test_executions.failure_category` and `test_cases.failure_category`.
3. `processing.py`: classify during parsing (off the event loop), store the category on each Execution, and cache the dominant category in `rescore`.
4. `schemas.py`, `queries.py`, `mcp_server.py`: expose the fields.

## Context Pack
- Source decisions: `00-shared-context.md`, "Failure categories". ADR 0006 (rule order: network, environment, timing, assertion; the message before the details; no backfill).
- Repo facts:
  - `process_report` (`backend/app/processing.py`) starts with:
    ```python
    parsed = await asyncio.to_thread(parse_junit_xml, report.body)
    ```
    After task 01, the execution loop builds dicts with the keys `test_case_id, test_run_id, status, duration, message, details, created_at, attempt`.
  - `scoring.FAILING = {"failed", "error"}`.
  - `rescore(db, test_case_ids)` today (the parts you change):
    ```python
    history: dict[int, list[tuple[str, str, str]]] = defaultdict(list)  # id -> [(sha, branch, status)]
    default_branch: dict[int, str | None] = {}
    for chunk in chunks(test_case_ids):
        ranked = (
            select(
                TestExecution.id,
                TestExecution.test_case_id,
                TestExecution.status,
                TestRun.commit_sha,
                TestRun.branch,
                Repo.default_branch,
                rn_all,
                rn_def,
            )
            .join(TestRun, TestRun.id == TestExecution.test_run_id)
            .join(Project, Project.id == TestRun.project_id)
            .join(Repo, Repo.id == Project.repo_id)
            .where(TestExecution.test_case_id.in_(chunk))
            .subquery()
        )
        rows = await db.execute(
            select(
                ranked.c.test_case_id, ranked.c.commit_sha, ranked.c.branch, ranked.c.status, ranked.c.default_branch
            )
            .where((ranked.c.rn_all <= settings.score_window) | (ranked.c.rn_def <= settings.score_window))
            .order_by(ranked.c.test_case_id, ranked.c.id.desc())
        )
        for tc_id, sha, branch, status, def_branch in rows.all():
            history[tc_id].append((sha, branch, status))
            default_branch[tc_id] = def_branch

    updates: list[dict[str, Any]] = []
    for tc_id in test_case_ids:
        execs = history.get(tc_id, [])
        score, confirmed = scoring.branch_scoped_score(
            execs, default_branch.get(tc_id), settings.score_decay, settings.score_window,
        )
        updates.append({"id": tc_id, "flakiness_score": score, "confirmed_flake_count": confirmed})
    for chunk in chunks(updates):
        await db.execute(update(TestCase), chunk)
    ```
    The rows come back newest first per Test, and the first `score_window` rows of each Test's list are exactly its newest `score_window` Executions (any branch). Older Default-branch rows follow them.
  - `TestCase` model columns: `id, project_id, fingerprint, suite, classname, name, file, line, flakiness_score, confirmed_flake_count, last_status, last_seen_at, quarantined, quarantined_at, github_issue_number`.
  - `queries.to_test_out(tc, project, repo, threshold) -> schemas.TestOut` builds `TestOut` field by field. `schemas.TestOut` fields: `id, repo, project, fingerprint, suite, classname, name, file, line, flakiness_score, tier, confirmed_flake_count, last_status, last_seen_at, quarantined, quarantined_at, github_issue_number, github_issue_url`.
  - After task 01, `ExecutionOut` ends with `ci_run_id: str` and `attempt: int`. It is built in `queries.get_test` and `queries.latest_failure`. The MCP `get_test` builds executions as a dict with keys `status, commit_sha, branch, ci_run_id, created_at, duration, message, attempt`, and `latest_failure` as a dict with keys `status, message, details, commit_sha, branch, created_at`.
  - The last migration is `0003` (task 01).
- Non-goals: the filter and counts (task 04); the UI (task 05); Jobs; backfilling old Executions; user-configurable rules; a reclassify command.

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `feat/flake-insights` after this draft

## Implementation Contract
- Expected files:
  - `backend/app/classify.py` (new)
  - `backend/migrations/versions/0004_failure_category.py` (new)
  - `backend/app/models.py`, `backend/app/processing.py`, `backend/app/schemas.py`, `backend/app/queries.py`, `backend/app/mcp_server.py` (edit)
  - `backend/tests/test_classify.py` (new), `backend/tests/test_processing.py`, `backend/tests/test_test_detail_api.py`, `backend/tests/test_mcp.py` (edit), `backend/tests/factories.py` (edit: `make_execution` gains `failure_category: str | None = None`)
- Interfaces and names — `backend/app/classify.py` (write it like this):
  ```python
  """Failure categories: the likely cause of a failing Execution (ADR 0006).

  Fixed, ordered regex rules, tried from most to least specific. The Failure
  message is checked first; the Failure details only when the message matches
  nothing. A heuristic: the UI calls the result "likely cause".
  """

  import re
  from collections import Counter
  from typing import Literal

  FailureCategory = Literal["network", "environment", "timing", "assertion", "other"]
  CATEGORIES: tuple[FailureCategory, ...] = ("network", "environment", "timing", "assertion", "other")

  _RULES: tuple[tuple[FailureCategory, re.Pattern[str]], ...] = tuple(
      (category, re.compile("|".join(patterns), re.IGNORECASE))
      for category, patterns in (
          ("network", (
              r"ECONNREFUSED", r"ECONNRESET", r"ENOTFOUND", r"EAI_AGAIN", r"ETIMEDOUT",
              r"EHOSTUNREACH", r"ENETUNREACH", r"socket hang up", r"net::ERR_",
              r"connection (?:refused|reset|aborted)", r"ConnectionError",
              r"ConnectionResetError", r"ConnectionRefusedError",
              r"Name or service not known", r"Temporary failure in name resolution", r"getaddrinfo",
              r"\b50[234] (?:Bad Gateway|Service Unavailable|Gateway Time-?out)",
          )),
          ("environment", (
              r"ENOSPC", r"No space left on device", r"MemoryError", r"out of memory", r"\bOOM\b",
              r"\bKilled\b", r"SIGKILL", r"signal 9\b", r"Permission denied", r"EACCES",
              r"Target (?:page, context or browser )?(?:has been )?closed", r"browser has been closed",
              r"ModuleNotFoundError", r"Cannot find module", r"No such file or directory", r"ENOENT",
              r"command not found",
          )),
          ("timing", (r"timeout", r"timed out", r"times out", r"deadline exceeded")),
          ("assertion", (r"AssertionError", r"\bassert", r"expect\(", r"\bexpected\b")),
      )
  )


  def _match(text: str) -> FailureCategory | None:
      for category, pattern in _RULES:
          if pattern.search(text):
              return category
      return None


  def classify_failure(message: str, details: str) -> FailureCategory:
      """The Failure category of one failing Execution."""
      return _match(message) or _match(details) or "other"


  def dominant_category(categories_newest_first: list[str | None]) -> FailureCategory | None:
      """Most common non-null category; the newest wins a tie; None when there is none."""
      present = [c for c in categories_newest_first if c is not None]
      if not present:
          return None
      counts = Counter(present)
      best = max(counts.values())
      return next(c for c in present if counts[c] == best)  # type: ignore[return-value]
  ```
- `models.py`:
  - `TestExecution` gains `failure_category: Mapped[str | None] = mapped_column(String(16), default=None)  # NULL unless failed/error`.
  - `TestCase` gains `failure_category: Mapped[str | None] = mapped_column(String(16), default=None)  # dominant over the score window` (place it after `confirmed_flake_count`).
- Migration `0004_failure_category.py`: `revision = "0004"`, `down_revision = "0003"`. `upgrade()` adds both columns as `sa.String(length=16), nullable=True`. `downgrade()` drops both. The docstring names ADR 0006 and says "no backfill: existing rows stay NULL".
- `processing.py`:
  ```python
  from .classify import classify_failure, dominant_category

  def _parse_and_classify(body: bytes) -> list[tuple[ParsedCase, str | None]]:
      """Parse a JUnit body and classify each failing case (CPU work: runs in a thread)."""
      return [
          (pc, classify_failure(pc.message, pc.details) if pc.status in scoring.FAILING else None)
          for pc in parse_junit_xml(body)
      ]
  ```
  In `process_report`, replace the `to_thread(parse_junit_xml, …)` call with `classified = await asyncio.to_thread(_parse_and_classify, report.body)` and `parsed = [pc for pc, _ in classified]`. `_upsert_test_cases` still takes `parsed`. The execution loop iterates `for pc, category in classified:` and adds `"failure_category": category` to each execution dict.
  In `rescore`: add `TestExecution.failure_category` to the `ranked` select and `ranked.c.failure_category` to the outer select. Collect `categories: dict[int, list[str | None]] = defaultdict(list)` in the same loop (append for every row, in row order). Each update dict gains `"failure_category": dominant_category(categories.get(tc_id, [])[: settings.score_window])`.
- `schemas.py`: `TestOut` gains `failure_category: str | None` (after `confirmed_flake_count`). `ExecutionOut` gains `failure_category: str | None` (after `attempt`).
- `queries.py`: `to_test_out` passes `failure_category=tc.failure_category`. Both `ExecutionOut(...)` builders pass `failure_category=e.failure_category`.
- `mcp_server.py`: in `get_test`, the execution dict gains `"failure_category": e.failure_category`, and the `latest_failure` dict gains `"failure_category": failure.failure_category`. `TestOut` dumps pick up the field without a change.
- Verified external contracts: None (stdlib `re`, `collections.Counter`).
- Behavior rules:
  - Only `failed` and `error` Executions get a category. `passed` and `skipped` store `NULL`.
  - Old Executions (before this change) keep `NULL`, and `dominant_category` skips them.
  - Classification and scoring stay independent: the category never changes a score.
- Error and security rules: classification never raises for any string input (empty strings give `"other"`).

## Acceptance Criteria
- [ ] `classify_failure` returns the category in each row of the table in Test Expectations.
- [ ] `dominant_category(["timing", "assertion", "timing"]) == "timing"`, `dominant_category(["assertion", "timing"]) == "assertion"` (tie, newest first), `dominant_category([None, None]) is None`, `dominant_category([None, "network"]) == "network"`.
- [ ] After processing a report with a failure `message="Test timeout of 30000ms exceeded."`, that Execution's `failure_category == "timing"`, a passing Execution in the same report has `NULL`, and the Test's `failure_category == "timing"`.
- [ ] `GET /api/tests/{id}/history` returns `test.failure_category` and `executions[*].failure_category`. MCP `get_test` returns them too.
- [ ] Migration `0004` applies on top of `0003`.

## Test Expectations
- Framework: pytest (pure tests are plain `def test_…`, DB tests are `async def`). Run: `cd backend && .venv/bin/python -m pytest -q`.
- `tests/test_classify.py` (new). Parametrize `classify_failure(message, details)` with exactly these rows:
  | message | details | expected |
  |---|---|---|
  | `"Test timeout of 30000ms exceeded."` | `""` | `"timing"` |
  | `"Timed out 5000ms waiting for expect(locator).toBeVisible()"` | `""` | `"timing"` |
  | `"Error: connect ECONNREFUSED 127.0.0.1:5432"` | `""` | `"network"` |
  | `"Error: connect ETIMEDOUT 10.0.0.1:443"` | `""` | `"network"` |
  | `"Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:3000/"` | `""` | `"network"` |
  | `"expected 200 but got 503 Service Unavailable"` | `""` | `"network"` |
  | `"requests.exceptions.ReadTimeout: read timed out"` | `""` | `"timing"` |
  | `"OSError: [Errno 28] No space left on device"` | `""` | `"environment"` |
  | `"Error: Target page, context or browser has been closed"` | `""` | `"environment"` |
  | `"AssertionError: expected 3, got 4"` | `""` | `"assertion"` |
  | `"assert 1 == 2"` | `""` | `"assertion"` |
  | `""` | `"Traceback (most recent call last):\nAssertionError: boom"` | `"assertion"` |
  | `"AssertionError: expected 3"` | `"… ECONNRESET …"` | `"assertion"` (the message wins) |
  | `"FAILURE"` | `""` | `"other"` |
  | `""` | `""` | `"other"` |

  Also add the four `dominant_category` cases from the Acceptance Criteria.
- `tests/test_processing.py`: add `test_failures_are_classified_and_test_gets_dominant_category`. Queue this XML with `_queue` and `process_next`: `<testsuite name="s"><testcase classname="c" name="t"><failure message="Test timeout of 30000ms exceeded.">trace</failure></testcase><testcase classname="c" name="ok"/></testsuite>`. Assert the `t` Execution has `failure_category == "timing"`, the `ok` Execution has `None`, and the `t` Test has `failure_category == "timing"`.
- `tests/test_test_detail_api.py`: in `_seed`, the failing `make_execution` gets `failure_category="assertion"` and the Test gets `failure_category="assertion"` (via `make_test_case(..., failure_category="assertion")`). Assert `body["test"]["failure_category"] == "assertion"` and that the failing execution in `body["executions"]` has `"failure_category": "assertion"`.
- `tests/test_mcp.py`: in the `get_test` test, assert every execution has a `"failure_category"` key.

## Dependencies
- Blocked by: 01.
- Why blocked: migration `0004` revises `0003`, and the execution loop you edit is the one 01 changed.
- Blocks: 04, 06.

## Labels
`feature`, `backend`, `priority:high`

## Estimate
Medium

## Risk
2 - Additive columns. The regex rules are heuristic but isolated and fully unit-tested.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```
