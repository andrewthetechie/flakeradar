# 07 — Job Score history, Clean streak, trend

## Tracer-Bullet Outcome
Jobs get what Tests got in task 06. Every Job rescore writes that day's `job_score_history` row and caches `jobs.clean_streak`. `GET /api/jobs` and `GET /api/jobs/{id}/history` return `clean_streak` and `trend`, and the history embeds `score_history`. MCP `get_job` returns the same. An explained Job failure does not break a Job's Clean streak, and old rows are pruned.

## User Story
As a maintainer watching a flaky CI job (say `e2e (ubuntu-latest)`), I want its score line and clean streak, so that I can confirm an infrastructure fix held.

## Description
Mirror task 06 for Jobs: migration `0006_job_score_history.py`, the `JobScoreHistory` model, `jobs.clean_streak`, `upsert_job_history`, `rescore_jobs(..., day_counts=)`, counts from `process_pipeline_report`, the read fields, retention and MCP.

## Context Pack
- Source decisions: `00-shared-context.md`, "Score history, Clean streak, trend". ADR 0007 ("A Job's failures count every failed Job execution, explained or not"; "An explained Job failure counts as skipped, so it does not break a Job's streak").
- Repo facts (after task 06):
  - `backend/app/score_history.py` has `TREND_DAYS = 14`, `TREND_DELTA = 0.05`, `HISTORY_DAYS = 90`, `Trend`, `trend_for(current, past) -> Trend | None`, and `upsert_test_history(db, rows)`, which runs `pg_insert(TestScoreHistory).values(chunk).on_conflict_do_update(index_elements=[TestScoreHistory.test_case_id, TestScoreHistory.day], set_={score and proven replaced, executions and failures added})`.
  - `models.TestScoreHistory` (columns `test_case_id` PK/FK, `day` `Date` PK indexed, `flakiness_score`, `confirmed_flake_count`, `executions`, `failures`) and `TestCase.clean_streak`.
  - `scoring.branch_scoped_streak(history, default_branch) -> int`.
  - `schemas.ScorePointOut(day, flakiness_score, confirmed_flake_count, executions, failures)`.
  - `queries.past_test_scores(db, test_ids) -> dict[int, float]` uses `DISTINCT ON` with the cutoff `utcnow().date() - timedelta(days=TREND_DAYS)`.
  - `retention.prune(db, *, now, report_days, execution_days, history_days=365)`, and `PruneResult.score_history`, which counts deleted Test history rows.
  - `conftest._TABLES = "reports, job_executions, jobs, pipelines, test_score_history, test_executions, test_runs, test_cases, projects, repos"`.
  - `factories.make_test_score(db, test_case, day, flakiness_score, **fields)`.
- Current Job code you change (`backend/app/processing.py`):
  ```python
  async def _job_history(db, job_ids) -> dict[int, tuple[str | None, list[tuple[str, str, str]]]]:
      # ... for job_id, sha, branch, status, is_explained, def_branch in rows.all():
      #         if is_explained and status == JOB_FAILED:
      #             status = JOB_SKIPPED
      #         history[job_id].append((sha, branch, status))
      #         branches.setdefault(job_id, def_branch)
      # return {jid: (branches.get(jid), history[jid]) for jid in job_ids}

  async def rescore_jobs(db: AsyncSession, job_ids: list[int]) -> None:
      """Recompute flakiness for Jobs with the Default-branch rule (see rescore)."""
      settings = get_settings()
      history = await _job_history(db, job_ids)
      updates: list[dict[str, Any]] = []
      for job_id in job_ids:
          def_branch, execs = history.get(job_id, (None, []))
          score, confirmed = scoring.branch_scoped_score(
              execs, def_branch, settings.score_decay, settings.score_window,
          )
          updates.append({"id": job_id, "flakiness_score": score, "confirmed_flake_count": confirmed})
      for chunk in chunks(updates):
          await db.execute(update(Job), chunk)
  ```
  Callers: `process_pipeline_report` (`await rescore_jobs(db, touched)`), `rescore_repo` and `_rescore_jobs_for_ci_job_id`. All pass sorted, unique ids.
  In `process_pipeline_report`, the insert loop is:
  ```python
  jobs_with_new_exec: set[int] = set()
  new_counts = dict.fromkeys(JOB_STATUSES, 0)
  for chunk in chunks(exec_rows):
      inserted = (
          await db.execute(
              pg_insert(JobExecution)
              .values(chunk)
              .on_conflict_do_nothing(constraint="uq_job_executions_job_ci_job_id")
              .returning(JobExecution.job_id, JobExecution.status)
          )
      ).all()
      for job_id, status in inserted:
          jobs_with_new_exec.add(job_id)
          new_counts[status] += 1
  ...
  touched = sorted(jobs_with_new_exec)
  await rescore_jobs(db, touched)
  ```
- Current Job read code (`backend/app/queries.py`): `to_job_out(job, pipeline, repo, threshold) -> schemas.JobOut`, called by `list_jobs` (for each page row), `get_job` and `search_jobs`. `schemas.JobOut` fields: `id, repo, provider, pipeline, name, flakiness_score, tier, confirmed_flake_count, last_status, last_seen_at, github_issue_number, github_issue_url`. `schemas.JobHistoryOut` fields: `job, unexplained_failures, explained_failures, executions`.
- Current MCP `get_job` return statement:
  ```python
  return history.model_dump(mode="json", include={"job", "unexplained_failures", "explained_failures"}) | {
      "executions": [ ... ]
  }
  ```
- The `Job` model has `flakiness_score, confirmed_flake_count, last_status, last_seen_at, github_issue_number, created_at`.
- The last migration is `0005` (task 06).
- Non-goals: UI (task 08); Failure categories for Jobs; history for Pipelines.

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `feat/flake-insights` after this draft

## Implementation Contract
- Expected files: `backend/migrations/versions/0006_job_score_history.py` (new). Edits: `backend/app/models.py`, `backend/app/score_history.py`, `backend/app/processing.py`, `backend/app/schemas.py`, `backend/app/queries.py`, `backend/app/retention.py`, `backend/app/mcp_server.py`, `backend/tests/conftest.py`, `backend/tests/factories.py`, `backend/tests/test_pipeline_processing.py`, `backend/tests/test_attribution.py`, `backend/tests/test_jobs_api.py`, `backend/tests/test_retention.py`, `backend/tests/test_mcp.py`.
- Interfaces and names:
  - `models.py`:
    ```python
    # on Job, after confirmed_flake_count:
    clean_streak: Mapped[int] = mapped_column(Integer, default=0, server_default="0")

    class JobScoreHistory(Base):
        """One row per Job per UTC day (ADR 0007). failures = raw failed Job executions."""

        __tablename__ = "job_score_history"
        __test__ = False

        job_id: Mapped[int] = mapped_column(ForeignKey("jobs.id", ondelete="CASCADE"), primary_key=True)
        day: Mapped[date] = mapped_column(Date, primary_key=True, index=True)
        flakiness_score: Mapped[float] = mapped_column(Float)
        confirmed_flake_count: Mapped[int] = mapped_column(Integer, default=0, server_default="0")
        executions: Mapped[int] = mapped_column(Integer, default=0, server_default="0")
        failures: Mapped[int] = mapped_column(Integer, default=0, server_default="0")
    ```
  - Migration `0006`: `revision = "0006"`, `down_revision = "0005"`. Add `jobs.clean_streak` (`nullable=False, server_default="0"`), create `job_score_history` (PK `job_id, day`; FK `jobs.id` `ondelete="CASCADE"`), and create the index `ix_job_score_history_day`. `downgrade()` reverses these.
  - `score_history.py`: add `upsert_job_history(db, rows)`, the same as `upsert_test_history` but with `JobScoreHistory` and `index_elements=[JobScoreHistory.job_id, JobScoreHistory.day]`. Rows are `{job_id, day, flakiness_score, confirmed_flake_count, executions, failures}`.
  - `processing.py`:
    - `async def rescore_jobs(db: AsyncSession, job_ids: list[int], *, day_counts: dict[int, tuple[int, int]] | None = None) -> None`. Each update gains `"clean_streak": scoring.branch_scoped_streak(execs, def_branch)`. The `execs` statuses already have explained failures turned into `JOB_SKIPPED`, so they do not break the streak. After the update loop, when `job_ids` is non-empty, call `upsert_job_history` with `day = utcnow().date()` and counts from `day_counts` (missing means `(0, 0)`), mirroring task 06.
    - In `process_pipeline_report`, extend the insert loop:
      ```python
      day_counts: dict[int, tuple[int, int]] = {}
      ...
      for job_id, status in inserted:
          jobs_with_new_exec.add(job_id)
          new_counts[status] += 1
          n, f = day_counts.get(job_id, (0, 0))
          day_counts[job_id] = (n + 1, f + (status == JOB_FAILED))
      ...
      await rescore_jobs(db, touched, day_counts=day_counts)
      ```
      `rescore_repo` and `_rescore_jobs_for_ci_job_id` pass no counts.
  - `schemas.py`: `JobOut` gains `clean_streak: int` and `trend: Literal["worsening", "improving", "steady"] | None = None` (after `confirmed_flake_count`). `JobHistoryOut` gains `score_history: list[ScorePointOut]`.
  - `queries.py`: `to_job_out(job, pipeline, repo, threshold, trend: Trend | None = None)` sets `clean_streak=job.clean_streak, trend=trend`. Add `past_job_scores(db, job_ids) -> dict[int, float]`, the same as `past_test_scores` over `JobScoreHistory.job_id`. `list_jobs`, `search_jobs` and `get_job` pass `trend=trend_for(job.flakiness_score, past.get(job.id))`. `get_job` loads `score_history` for the last `HISTORY_DAYS` days, oldest first.
  - `retention.py`: `prune` also deletes `JobScoreHistory` rows with `day < (now - timedelta(days=history_days)).date()`. `PruneResult.score_history` becomes the sum of the Test and Job rows deleted.
  - `mcp_server.py`: `get_job` adds `"score_history": [{"day": p.day.isoformat(), "flakiness_score": p.flakiness_score} for p in history.score_history[-30:]]` to the returned dict. `JobOut` dumps pick up `clean_streak` and `trend` without a change. Append to `INSTRUCTIONS`: `Jobs have clean_streak, trend and score_history too (get_job).`
  - Tests support: `conftest._TABLES` becomes `"reports, job_score_history, job_executions, jobs, pipelines, test_score_history, test_executions, test_runs, test_cases, projects, repos"`. `factories.py` adds `make_job_score(db, job, day, flakiness_score, **fields) -> JobScoreHistory` (same shape as `make_test_score`).
- Verified external contracts: same SQLAlchemy PostgreSQL APIs as task 06 (already used there).
- Behavior rules:
  - One row per (Job, UTC day). A Pipeline report adds each new (non-duplicate) Job execution to `executions`, and each new `failed` one to `failures`. Duplicates (ignored by `on_conflict_do_nothing`) add nothing.
  - A later JUnit report that explains a failure rescores the Job (attribution), which replaces today's score with `+0` counts.
  - `clean_streak` uses Default-branch rows only (every row when the branch is unknown), treats explained failures as skipped, and stops at the newest unexplained failure.
- Error and security rules: None new.

## Acceptance Criteria
- [ ] Processing a Pipeline report with Job `e2e` `failed`, then another with `e2e` `passed` (different `ci_job_id`, same day), leaves one `job_score_history` row for `e2e` with `executions == 2`, `failures == 1`, and the score equal to the Job's `flakiness_score`.
- [ ] Re-sending the second report (same `ci_job_id`) leaves `executions == 2`.
- [ ] Job executions newest first `passed`, `failed` (explained by a failing Test with the same `ci_job_id`), `passed` on `main` give `clean_streak == 2`. Without the explaining Test, the same sequence gives `1`.
- [ ] `GET /api/jobs?repo=…` items include `clean_streak` and `trend`. A Job with score 0.6 and a history row 20 days ago at 0.1 has `trend == "worsening"`.
- [ ] `GET /api/jobs/{id}/history` includes `score_history` oldest first.
- [ ] `prune` deletes Job history rows older than `history_days` and counts them in `score_history`.
- [ ] MCP `get_job` returns `score_history`, and its `job` has `clean_streak` and `trend`.

## Test Expectations
- Framework: pytest + pytest-asyncio, real Postgres. Run: `cd backend && .venv/bin/python -m pytest -q`.
- `tests/test_pipeline_processing.py`: `test_same_day_pipeline_reports_share_one_history_row` (use `make_pipeline_report(db, report_json={...})` twice with job `{"ci_job_id": "1", "name": "e2e", "status": "failed"}`, then `{"ci_job_id": "2", …, "status": "passed"}`; commit; run `process_next` twice; assert the counts; queue the second one again and assert `executions` is still 2). The default `report_json` in `make_pipeline_report` shows every required key: `repo, provider, pipeline, commit_sha, branch, default_branch, ci_run_id, ci_run_attempt, jobs`.
- `tests/test_attribution.py`: `test_explained_failure_does_not_break_clean_streak`. Seed the Job with `make_job_execution` rows in insertion order `passed` (ci `J1`), `failed` (ci `J2`), `passed` (ci `J3`), all `branch="main"`. Add `make_run(db, proj, ci_job_id="J2")` plus a failing `make_execution`. Commit, call `await rescore_jobs(db, [job.id])`, commit, refresh, and assert `job.clean_streak == 2`. Repeat without the explaining run and expect `1`.
- `tests/test_jobs_api.py`: the trend case above (use `make_job_score`) and the `score_history` ordering.
- `tests/test_retention.py`: one Job history row 400 days old is deleted and one 10 days old is kept.
- `tests/test_mcp.py`: in the `get_job` test, assert `"score_history" in result` and `"clean_streak" in result["job"]`.

## Dependencies
- Blocked by: 06.
- Why blocked: 06 creates `score_history.py`, `ScorePointOut`, `branch_scoped_streak`, the `prune(history_days=)` argument, and migration `0005`, which `0006` revises.
- Blocks: 08.

## Labels
`feature`, `backend`, `priority:medium`

## Estimate
Medium

## Risk
2 - Mirrors a pattern that task 06 already proved. The attribution interaction is covered by a test.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```
