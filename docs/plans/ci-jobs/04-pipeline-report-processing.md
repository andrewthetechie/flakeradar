# 04 — Process Pipeline reports, score Jobs, and prune Job executions

## Tracer-Bullet Outcome
The processor turns a `pipeline` Report into Job executions:
- It creates any new Pipeline and Jobs.
- It ignores Job executions it has already seen, keyed by `ci_job_id`.
- It updates each Job's last status.
- It scores each touched Job with the Default-branch rule.

A failed attempt followed by a passing re-run on the same SHA makes the Job a Proven flake. At this stage **every** failure counts (attribution is task 05). Retention prunes old Job executions.

## User Story
As a maintainer, I want flaky CI jobs scored the same way as flaky tests, so that "re-run failed jobs" stops being invisible toil.

## Context Pack
- Read `00-shared-context.md`, sections "Identity", "Default-branch rule" and "Retention".
- From task 02: `scoring.branch_scoped_score(history, default_branch, decay, window)`, `processing.apply_default_branch(db, repo_id, value) -> bool` and `processing.rescore_repo(db, repo_id) -> list[int]`.
- From task 03: the Report `body` of a Pipeline report is `schemas.PipelineReportIn(...).model_dump_json().encode()`, already normalized. It is parsed with `schemas.PipelineReportIn.model_validate_json(report.body)`.
- The existing `process_next` SAVEPOINT and failure handling is kept exactly as it is.
- `github_integration.on_report_processed` reads `outcome.touched_test_ids`. It must keep working, and a Pipeline Report has no touched Tests.
- Non-goals: attribution (05); Job issue filing (10); read API (06).

## Implementation Contract

### `ProcessOutcome`
Add `touched_job_ids: list[int] = field(default_factory=list)` as the **last** field, so that existing constructors keep working. For Pipeline reports, `run_id=None` and `touched_test_ids=[]`.

### `backend/app/processing.py`
Replace the task 03 guard with a dispatch:
```python
async def process_report(db, report) -> ProcessOutcome:
    if report.kind == REPORT_KIND_PIPELINE:
        return await process_pipeline_report(db, report)
    ...existing JUnit path...

async def process_pipeline_report(db: AsyncSession, report: Report) -> ProcessOutcome:
    """Persist one Pipeline report as Job executions. Caller owns the transaction."""

async def rescore_jobs(db: AsyncSession, job_ids: list[int]) -> None:
    """Recompute flakiness for Jobs with the Default-branch rule (see rescore)."""
```
Steps for `process_pipeline_report`:
1. `payload = PipelineReportIn.model_validate_json(report.body)`. `now = utcnow()`.
2. `changed = await apply_default_branch(db, report.repo_id, payload.default_branch)`.
3. Upsert the Pipeline (`repo_id`, `provider`, `name=payload.pipeline`) with `ON CONFLICT DO NOTHING` on `uq_pipelines_repo_provider_name`, then select its id.
4. Upsert the Jobs by (`pipeline_id`, `name`), inserting `last_seen_at=now`, then read back `{name: id}`. Chunk the statements.
5. Insert the Job executions with `pg_insert(JobExecution).values(chunk).on_conflict_do_nothing(constraint="uq_job_executions_job_ci_job_id").returning(JobExecution.id, JobExecution.job_id)`. The returned rows are the **new** executions. The rest are duplicates. Each row gets:
   - from the job: `ci_job_id`, `status`, `url`, `runner_name`, `runner_labels`, `started_at` and `completed_at`;
   - from the payload: `ci_run_id`, `ci_run_attempt`, `commit_sha` and `branch`;
   - `created_at=now`.
6. For the Jobs with a **new** execution, bulk-update `last_status` (this report's status for that Job) and `last_seen_at=now`.
7. `touched = sorted(job ids with a new execution)`, then `await rescore_jobs(db, touched)`.
8. If `changed`: `await rescore_repo(db, report.repo_id)`.
9. Mark the Report processed with `counts = {"passed": n, "failed": n, "skipped": n, "duplicate": n}`. The first three count new executions only. Return `ProcessOutcome(..., run_id=None, touched_test_ids=[], touched_job_ids=touched)`.

`rescore_jobs` mirrors task 02's `rescore`. It uses the same union-of-two-windows query over `job_executions`, joined through `jobs` → `pipelines` → `repos` to get `default_branch`, and ordered by `JobExecution.id DESC`. The history tuples are `(commit_sha, branch, status)`. It then calls `branch_scoped_score` and bulk-updates `flakiness_score` and `confirmed_flake_count`. Keep the history-building in a helper that task 05 can extend, for example `_job_history(db, job_ids) -> dict[int, tuple[str | None, list[tuple[str, str, str]]]]`.

Extend `rescore_repo` so that it also rescores every Job in the Repo (through `pipelines.repo_id`). It still returns only Test ids, or change it to return `(test_ids, job_ids)` and update the task 02 callers. Either is fine.

Scoring note: `scoring.FAILING = {"failed", "error"}`, so a Job's `failed` counts as failing and `skipped` is ignored. No change to `scoring.py` is needed.

### `backend/app/retention.py`
Also delete `JobExecution` rows with `created_at < execution_cutoff`. `PruneResult` gains `job_executions: int`, and the log line includes it. Pipeline Reports are already covered by the existing Report prune (status and `processed_at`), whatever their kind.

## Acceptance Criteria
- [ ] One Pipeline report with jobs `lint` (passed) and `test (ubuntu, 3.12)` (failed) creates 1 Pipeline, 2 Jobs and 2 Job executions. The Report is `processed` with `counts == {"passed": 1, "failed": 1, "skipped": 0, "duplicate": 0}`.
- [ ] Attempt 1 `failed`, then attempt 2 (new `ci_job_id`) `passed`, same SHA → `confirmed_flake_count == 1` and score ≥ 0.6.
- [ ] Re-posting the same payload → no new rows, `counts["duplicate"] == 2`, `touched_job_ids == []`, and scores unchanged.
- [ ] The same Job name in two Pipelines is two Jobs. The same Pipeline name under two providers is two Pipelines.
- [ ] Default-branch rule: on a PR branch, fail@a then pass@b → score 0. On `main` the same pair gives score > 0.
- [ ] A Pipeline report that sets a new `default_branch` rescores the Repo's Tests **and** Jobs.
- [ ] A malformed body (hand-inserted Report with `kind="pipeline"`, `body=b"{}"`) → Report `failed` with a `ValidationError: …` error, and the queue moves on.
- [ ] `on_report_processed` is a no-op for Pipeline reports (no Tests touched).
- [ ] `prune` deletes old Job executions and keeps new ones.

## Test Expectations
- New `backend/tests/test_pipeline_processing.py` for the criteria above. Queue Pipeline reports with a helper that builds `PipelineReportIn(...).model_dump_json().encode()` and calls `make_report(db, ..., kind="pipeline")`. `make_report` needs a Project today, so either add a `repo`-based variant (`make_pipeline_report(db, repo, payload)`) to `factories.py` or post through the API with `client`. Prefer the factory.
- `tests/test_retention.py`: a Job execution pruning case.
- `tests/test_worker.py`: if it asserts on `ProcessOutcome` fields positionally, update it.

## Dependencies
- Blocked by: 01, 02, 03.
- Blocks: 05, 06, 10.

## Estimate / Risk
Medium. Risk 3: the scoring correctness and the dedupe.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && ruff check backend && ruff format --check backend
```
