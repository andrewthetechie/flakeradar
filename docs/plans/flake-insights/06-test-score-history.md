# 06 — Test Score history, Clean streak, trend (backend + MCP)

## Tracer-Bullet Outcome
Every rescore of a Test writes that day's Score history row, and caches the Test's Clean streak. `GET /api/tests` and `GET /api/tests/{id}/history` return `clean_streak` and `trend` (`worsening` / `improving` / `steady` / `null`) on each Test, and the history response embeds `score_history` (the last 90 days, oldest first). The MCP `get_test` tool returns the same, and history rows older than `FLAKERADAR_SCORE_HISTORY_RETENTION_DAYS` (365) are pruned.

## User Story
As a maintainer who just fixed a flaky test, I want to see its score line going down and a growing clean streak, so that I know the fix held.

## Description
1. Migration `0005_test_score_history.py`: the table `test_score_history` and the column `test_cases.clean_streak`.
2. `scoring.py`: pure `clean_streak` and `branch_scoped_streak`.
3. New `backend/app/score_history.py`: trend constants, `trend_for`, and the upsert writer for Tests (task 07 adds the Job writer here).
4. `processing.py`: `rescore` writes the streak and the history row. `process_report` passes that day's counts.
5. `queries.py` + `schemas.py`: `clean_streak`, `trend`, `score_history`.
6. `retention.py` + `config.py`: prune old history.
7. `mcp_server.py`: expose it to agents.

## Context Pack
- Source decisions: `00-shared-context.md`, "Score history, Clean streak, trend". ADR 0007.
- Repo facts:
  - `backend/app/models.py` imports `BigInteger, Boolean, CheckConstraint, DateTime, Float, ForeignKey, Index, Integer, LargeBinary, String, Text, UniqueConstraint, false, text` from `sqlalchemy`. `Date` is not imported yet. `utcnow()` returns `datetime.now(timezone.utc)`, so `utcnow().date()` is the UTC date.
  - `scoring.py` has `FAILING = {"failed", "error"}`, `_is_fail(status) -> bool`, and:
    ```python
    def branch_scoped_score(
        history: list[tuple[str, str, str]],  # (commit_sha, branch, status), newest first
        default_branch: str | None,
        decay: float,
        window: int,
    ) -> tuple[float, int]:
        flips = [s for _, b, s in history if default_branch is None or b == default_branch]
        proven = [(sha, s) for sha, _, s in history[:window]]
        return combined_score(flips, proven, decay, window)
    ```
  - After task 03, `processing.rescore(db, test_case_ids)` builds `history[tc_id]` (newest first `(sha, branch, status)`), `default_branch[tc_id]` and `categories[tc_id]`, then:
    ```python
    updates: list[dict[str, Any]] = []
    for tc_id in test_case_ids:
        execs = history.get(tc_id, [])
        score, confirmed = scoring.branch_scoped_score(
            execs, default_branch.get(tc_id), settings.score_decay, settings.score_window,
        )
        updates.append({
            "id": tc_id, "flakiness_score": score, "confirmed_flake_count": confirmed,
            "failure_category": dominant_category(categories.get(tc_id, [])[: settings.score_window]),
        })
    for chunk in chunks(updates):
        await db.execute(update(TestCase), chunk)
    ```
    Callers: `process_report` (`await rescore(db, touched)`, where `touched = sorted(latest)`) and `rescore_repo` (`await rescore(db, test_ids)`). Both pass sorted, unique ids.
  - In `process_report`, `executions` is the list of dicts inserted into `test_executions`. Each has `"test_case_id"` and `"status"`.
  - `pg_insert` is already imported in `processing.py` as `from sqlalchemy.dialects.postgresql import insert as pg_insert`.
  - `queries.to_test_out(tc, project, repo, threshold) -> schemas.TestOut` is called by `list_tests` (for each row of a page), `get_test`, `search_tests` and `set_quarantine`. `schemas.HistoryOut` fields: `test, location, last_failing_sha, last_failing_branch, executions, jobs`.
  - `retention.py` today:
    ```python
    @dataclass(frozen=True)
    class PruneResult:
        reports: int
        executions: int
        runs: int
        job_executions: int

    async def prune(db: AsyncSession, *, now: datetime, report_days: int, execution_days: int) -> PruneResult: ...
    async def run_prune(session_factory) -> PruneResult:  # calls prune(db, now=utcnow(), report_days=…, execution_days=…) and logs
    ```
  - `config.Settings` has `execution_retention_days: int = 90  # Executions (and Runs left empty)` followed by `prune_interval_seconds: float = 3600.0`.
  - `conftest._TABLES = "reports, job_executions, jobs, pipelines, test_executions, test_runs, test_cases, projects, repos"`.
  - MCP `get_test` returns a dict with keys `test, location, last_failing_sha, last_failing_branch, latest_failure, executions, jobs`. `INSTRUCTIONS` is a module-level string (task 04 appended a failure_category paragraph).
  - The last migration is `0004` (task 03).
- Non-goals: Jobs (task 07); UI (task 08); backfilling history for past days; a separate history endpoint; a "mark as fixed" action.

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `feat/flake-insights` after this draft

## Implementation Contract
- Expected files:
  - `backend/migrations/versions/0005_test_score_history.py` (new), `backend/app/score_history.py` (new)
  - `backend/app/models.py`, `backend/app/scoring.py`, `backend/app/processing.py`, `backend/app/schemas.py`, `backend/app/queries.py`, `backend/app/retention.py`, `backend/app/config.py`, `backend/app/mcp_server.py`, `.env.example` (edit)
  - `backend/tests/conftest.py`, `backend/tests/factories.py`, `backend/tests/test_scoring.py`, `backend/tests/test_processing.py`, `backend/tests/test_tests_api.py`, `backend/tests/test_test_detail_api.py`, `backend/tests/test_retention.py`, `backend/tests/test_mcp.py` (edit), `backend/tests/test_score_history.py` (new)
- Interfaces and names:
  - `models.py` (import `Date`, and `date` from `datetime`):
    ```python
    # on TestCase, after failure_category:
    clean_streak: Mapped[int] = mapped_column(Integer, default=0, server_default="0")

    class TestScoreHistory(Base):
        """One row per Test per UTC day: end-of-day score plus that day's counts (ADR 0007)."""

        __tablename__ = "test_score_history"
        __test__ = False

        test_case_id: Mapped[int] = mapped_column(ForeignKey("test_cases.id", ondelete="CASCADE"), primary_key=True)
        day: Mapped[date] = mapped_column(Date, primary_key=True, index=True)
        flakiness_score: Mapped[float] = mapped_column(Float)
        confirmed_flake_count: Mapped[int] = mapped_column(Integer, default=0, server_default="0")
        executions: Mapped[int] = mapped_column(Integer, default=0, server_default="0")
        failures: Mapped[int] = mapped_column(Integer, default=0, server_default="0")
    ```
  - Migration `0005`: `revision = "0005"`, `down_revision = "0004"`. `upgrade()`: `op.add_column("test_cases", sa.Column("clean_streak", sa.Integer(), nullable=False, server_default="0"))`, then `op.create_table("test_score_history", …)` with the columns above (`test_case_id` `sa.Integer()` FK `test_cases.id` `ondelete="CASCADE"`, `day` `sa.Date()`, `sa.PrimaryKeyConstraint("test_case_id", "day")`), then `op.create_index("ix_test_score_history_day", "test_score_history", ["day"])`. `downgrade()` reverses these. The docstring names ADR 0007 and says "no backfill".
  - `scoring.py`, add:
    ```python
    def clean_streak(statuses_newest_first: list[str]) -> int:
        """Non-skipped executions since the newest failure (all of them when none failed)."""
        n = 0
        for s in statuses_newest_first:
            if s == "skipped":
                continue
            if _is_fail(s):
                break
            n += 1
        return n


    def branch_scoped_streak(history: list[tuple[str, str, str]], default_branch: str | None) -> int:
        """Clean streak over Default-branch rows only (every row when the branch is unknown)."""
        return clean_streak([s for _, b, s in history if default_branch is None or b == default_branch])
    ```
  - `backend/app/score_history.py`:
    ```python
    """Score history (ADR 0007): daily snapshots, and the trend read from them.

    Written in the same transaction as each rescore. The score and Proven flake
    count are replaced (end-of-day value); executions and failures are added.
    """

    from datetime import date
    from typing import Any, Literal

    from sqlalchemy.dialects.postgresql import insert as pg_insert
    from sqlalchemy.ext.asyncio import AsyncSession

    from .batching import chunks
    from .models import TestScoreHistory

    TREND_DAYS = 14
    TREND_DELTA = 0.05
    HISTORY_DAYS = 90  # how much history the detail responses embed

    Trend = Literal["worsening", "improving", "steady"]


    def trend_for(current: float, past: float | None) -> Trend | None:
        if past is None:
            return None
        delta = current - past
        if delta >= TREND_DELTA:
            return "worsening"
        if delta <= -TREND_DELTA:
            return "improving"
        return "steady"


    async def upsert_test_history(db: AsyncSession, rows: list[dict[str, Any]]) -> None:
        """rows: {test_case_id, day, flakiness_score, confirmed_flake_count, executions, failures}.

        (test_case_id, day) must be unique within `rows`.
        """
        for chunk in chunks(rows):
            stmt = pg_insert(TestScoreHistory).values(list(chunk))
            stmt = stmt.on_conflict_do_update(
                index_elements=[TestScoreHistory.test_case_id, TestScoreHistory.day],
                set_={
                    "flakiness_score": stmt.excluded.flakiness_score,
                    "confirmed_flake_count": stmt.excluded.confirmed_flake_count,
                    "executions": TestScoreHistory.executions + stmt.excluded.executions,
                    "failures": TestScoreHistory.failures + stmt.excluded.failures,
                },
            )
            await db.execute(stmt)
    ```
    Floating-point note: `trend_for(0.35, 0.30)` gives `delta = 0.04999…` (float), so it is `steady`. The tests below avoid values that sit exactly on the edge.
  - `processing.py`:
    - `async def rescore(db: AsyncSession, test_case_ids: list[int], *, day_counts: dict[int, tuple[int, int]] | None = None) -> None`. `day_counts` maps a test id to `(executions, failures)` added today, and a missing id means `(0, 0)`.
    - Each update dict gains `"clean_streak": scoring.branch_scoped_streak(execs, default_branch.get(tc_id))`.
    - After the `update(TestCase)` loop, when `test_case_ids` is non-empty:
      ```python
      today = utcnow().date()
      counts = day_counts or {}
      await upsert_test_history(db, [
          {"test_case_id": u["id"], "day": today, "flakiness_score": u["flakiness_score"],
           "confirmed_flake_count": u["confirmed_flake_count"],
           "executions": counts.get(u["id"], (0, 0))[0], "failures": counts.get(u["id"], (0, 0))[1]}
          for u in updates
      ])
      ```
    - In `process_report`, build `day_counts` from `executions` before the rescore:
      ```python
      day_counts: dict[int, tuple[int, int]] = {}
      for ex in executions:
          n, f = day_counts.get(ex["test_case_id"], (0, 0))
          day_counts[ex["test_case_id"]] = (n + 1, f + (ex["status"] in scoring.FAILING))
      await rescore(db, touched, day_counts=day_counts)
      ```
      `rescore_repo` keeps calling `rescore(db, test_ids)` with no counts.
  - `schemas.py`:
    ```python
    class ScorePointOut(BaseModel):
        day: date
        flakiness_score: float
        confirmed_flake_count: int
        executions: int
        failures: int
    ```
    `TestOut` gains `clean_streak: int` and `trend: Literal["worsening", "improving", "steady"] | None = None` (after `failure_category`). `HistoryOut` gains `score_history: list[ScorePointOut]` (oldest first, the last `HISTORY_DAYS` days).
  - `queries.py`:
    - `to_test_out(tc, project, repo, threshold, trend: Trend | None = None)` sets `clean_streak=tc.clean_streak, trend=trend`.
    - Add:
      ```python
      async def past_test_scores(db: AsyncSession, test_ids: list[int]) -> dict[int, float]:
          """Score of each Test's newest history row at least TREND_DAYS old."""
          cutoff = utcnow().date() - timedelta(days=TREND_DAYS)
          found: dict[int, float] = {}
          for chunk in chunks(test_ids):
              rows = await db.execute(
                  select(TestScoreHistory.test_case_id, TestScoreHistory.flakiness_score)
                  .where(TestScoreHistory.test_case_id.in_(chunk), TestScoreHistory.day <= cutoff)
                  .order_by(TestScoreHistory.test_case_id, TestScoreHistory.day.desc())
                  .distinct(TestScoreHistory.test_case_id)
              )
              found.update(dict(rows.all()))
          return found
      ```
    - `list_tests`, `search_tests` and `get_test` compute `past = await past_test_scores(db, [ids…])` and pass `trend=trend_for(tc.flakiness_score, past.get(tc.id))` to `to_test_out`. `set_quarantine` passes no trend.
    - `get_test` also loads `score_history`: `select(TestScoreHistory).where(TestScoreHistory.test_case_id == test_id, TestScoreHistory.day > utcnow().date() - timedelta(days=HISTORY_DAYS)).order_by(TestScoreHistory.day)`, mapped to `ScorePointOut`.
  - `config.py`: after `execution_retention_days`, add `score_history_retention_days: int = 365  # Score history rows (ADR 0007)`. `.env.example`: add a commented line `# FLAKERADAR_SCORE_HISTORY_RETENTION_DAYS=365` under the tuning section.
  - `retention.py`: `PruneResult` gains `score_history: int = 0` (last field). `prune(db, *, now, report_days, execution_days, history_days: int = 365)` also runs `delete(TestScoreHistory).where(TestScoreHistory.day < (now - timedelta(days=history_days)).date())` and returns its rowcount as `score_history`. `run_prune` passes `history_days=settings.score_history_retention_days` and logs the count (`"… %s score history rows"`).
  - `mcp_server.py`: `get_test` gains the key `"score_history": [{"day": p.day.isoformat(), "flakiness_score": p.flakiness_score} for p in history.score_history[-30:]]`. Append to `INSTRUCTIONS`:
    ```
    Tests also carry clean_streak (passing runs on the default branch since the
    last failure) and trend (worsening/improving/steady versus 14 days ago, or
    null when there is not enough history). get_test returns score_history: the
    daily score for up to the last 30 days. A fix that held shows a growing
    clean_streak and a falling score.
    ```
  - Tests support: `conftest._TABLES` becomes `"reports, job_executions, jobs, pipelines, test_score_history, test_executions, test_runs, test_cases, projects, repos"`. `factories.py` adds:
    ```python
    async def make_test_score(db: AsyncSession, test_case: TestCase, day: date, flakiness_score: float, **fields) -> TestScoreHistory:
        row = TestScoreHistory(test_case_id=test_case.id, day=day, flakiness_score=flakiness_score, **fields)
        db.add(row)
        await db.flush()
        return row
    ```
- Verified external contracts: SQLAlchemy 2 PostgreSQL `insert(...).on_conflict_do_update(index_elements=[...], set_={...})` and `stmt.excluded.<col>`, and `select(...).distinct(col)` rendering `DISTINCT ON (col)` on PostgreSQL. Both are standard SQLAlchemy 2.x PostgreSQL dialect APIs, and the repo already uses `pg_insert(...).on_conflict_do_nothing(constraint=...)`.
- Behavior rules:
  - One history row per (Test, UTC day). A second Report on the same day replaces the score and Proven flake count and adds to `executions`/`failures`.
  - A repo-wide rescore (Default branch changed) writes rows with `+0` counts for Tests that did not run. This is intended: their score changed.
  - `clean_streak` counts Default-branch rows only (every row when the Default branch is `NULL`), ignores `skipped`, and stops at the newest `failed`/`error`.
  - `trend` is `null` when there is no history row at least 14 days old.
- Error and security rules: None new. History writes happen inside the processing SAVEPOINT, so a failure rolls them back with the Report.

## Acceptance Criteria
- [ ] `clean_streak(["passed", "skipped", "passed", "failed", "passed"]) == 2`, `clean_streak(["error", "passed"]) == 0`, `clean_streak([]) == 0`. `branch_scoped_streak([("a", "feat", "failed"), ("b", "main", "passed"), ("c", "main", "passed")], "main") == 2`, and the same history with `None` gives `0`.
- [ ] `trend_for(0.5, None) is None`, `trend_for(0.5, 0.1) == "worsening"`, `trend_for(0.1, 0.5) == "improving"`, `trend_for(0.32, 0.30) == "steady"`.
- [ ] Processing two JUnit Reports on the same day for Test `t` (first `failed`, then `passed`, same SHA) leaves exactly one `test_score_history` row for `t` for today, with `executions == 2`, `failures == 1`, and `flakiness_score`/`confirmed_flake_count` equal to the Test's current values.
- [ ] After a Report where `t` passed on `main` twice after a failure, `t.clean_streak == 2`.
- [ ] `GET /api/tests` items include `clean_streak` and `trend`. A Test with score 0.5 and a history row 20 days ago at 0.1 has `trend == "worsening"`. A Test with only a row 3 days ago has `trend is None`.
- [ ] `GET /api/tests/{id}/history` includes `score_history` oldest first, and excludes rows older than 90 days.
- [ ] `prune(..., history_days=365)` deletes a row dated 400 days ago, keeps one dated 10 days ago, and reports `score_history == 1`.
- [ ] MCP `get_test` returns `score_history`, and its `test` has `clean_streak` and `trend`.

## Test Expectations
- Framework: pytest + pytest-asyncio, real Postgres. Run: `cd backend && .venv/bin/python -m pytest -q`.
- `tests/test_scoring.py`: the `clean_streak` / `branch_scoped_streak` literals above.
- `tests/test_score_history.py` (new): the `trend_for` literals above (plain `def` tests).
- `tests/test_processing.py`: `test_same_day_reports_share_one_history_row` (queue `make_junit([("t", "failed")])` and `make_junit([("t", "passed")])` with `commit_sha="s1"` and `branch="main"`, process both, then `select(TestScoreHistory)`: one row, `(executions, failures) == (2, 1)`, `day == utcnow().date()`, and the score equals the Test's `flakiness_score`). Also `test_clean_streak_counts_passes_since_last_failure` (three reports on `main`: failed, passed, passed; `clean_streak == 2`).
- `tests/test_tests_api.py`: `test_trend_from_history` (seed Test A score 0.5 with `make_test_score(db, a, utcnow().date() - timedelta(days=20), 0.1)`, and Test B score 0.5 with only a row 3 days ago; both come back from `GET /api/tests?repo=…`; A's trend is `"worsening"`, B's is `None`, and both have a `clean_streak` key).
- `tests/test_test_detail_api.py`: seed rows at `today - 100`, `today - 5` and `today`, and assert `[p["day"] for p in body["score_history"]] == [(today - timedelta(days=5)).isoformat(), today.isoformat()]`.
- `tests/test_retention.py`: the prune case above (use `make_test_score`, and pass `history_days=365` to `prune`).
- `tests/test_mcp.py`: in the `get_test` test, assert `"score_history" in by_id` and `"clean_streak" in by_id["test"]`.

## Dependencies
- Blocked by: 03.
- Why blocked: migration `0005` revises `0004`, and the `rescore` update dict you extend is the one 03 changed.
- Blocks: 07, 08.

## Labels
`feature`, `backend`, `priority:high`

## Estimate
Large

## Risk
3 - A new write on every rescore (one upsert per touched Test), plus changes in the shared `to_test_out`. It is bounded by chunking, and tested end to end.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```
