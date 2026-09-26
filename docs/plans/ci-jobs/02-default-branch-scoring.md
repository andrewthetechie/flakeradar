# 02 — Default-branch rule for Test scoring

## Tracer-Bullet Outcome
JUnit ingest accepts an optional `default_branch`. When the Report is processed, that value is stored on the Repo. From then on, a Test's flip score looks only at Executions on that branch, and Proven flakes still count on every branch. When a Repo's Default branch changes, every Test in the Repo is rescored. A Repo that has never reported a Default branch scores exactly as before.

## User Story
As a maintainer, I want a Test that breaks on a PR and is then fixed to stop looking flaky. Only real instability on `main`, or a same-commit flip anywhere, should raise its score.

## Context Pack
- Read `00-shared-context.md`, section "Default-branch rule". ADR 0004 gives the reasons.
- Repo facts:
  - `scoring.combined_score(statuses_newest_first, executions_with_sha, decay, window)` slices `statuses_newest_first[:window]` itself, and counts same-SHA flips over **all** of `executions_with_sha`.
  - `processing.rescore(db, test_case_ids)` reads the newest `score_window` Executions per Test with `row_number() OVER (PARTITION BY test_case_id ORDER BY test_executions.id DESC)`.
  - The README ingest snippet sends `branch=${{ github.ref_name }}`. For `pull_request` events that is `<n>/merge`, which never equals the Default branch, and that is what we want.
- Non-goals: Jobs (task 04 reuses what you build here); UI changes.

## Implementation Contract

### `backend/app/scoring.py` (add; do not change existing functions)
```python
def branch_scoped_score(
    history: list[tuple[str, str, str]],   # (commit_sha, branch, status), newest first
    default_branch: str | None,
    decay: float,
    window: int,
) -> tuple[float, int]:
    """Flips on the Default branch only; Proven flakes on every branch.

    `history` must hold at least the newest `window` rows overall AND the
    newest `window` rows on the Default branch (see processing.rescore).
    default_branch=None means "unknown": every branch counts (old behaviour).
    """
    flips = [s for _, b, s in history if default_branch is None or b == default_branch]
    proven = [(sha, s) for sha, _, s in history[:window]]
    return combined_score(flips, proven, decay, window)
```
With `default_branch=None`, and `history` being exactly the newest `window` rows, the result must equal today's `combined_score(...)`. Add a unit test for that.

### `backend/app/processing.py`
1. **Rescoring history query.** `rescore(db, test_case_ids)` keeps its signature. For each Test it must now fetch the union of (a) its newest `score_window` Executions and (b) its newest `score_window` Executions on its Repo's Default branch, merged newest first by `TestExecution.id`, as `(commit_sha, branch, status)`. It then calls `scoring.branch_scoped_score`. A suggested query shape, joining `TestRun` → `Project` → `Repo` to get `Repo.default_branch`:
   ```python
   on_default = or_(Repo.default_branch.is_(None), TestRun.branch == Repo.default_branch)
   rn_all = func.row_number().over(partition_by=TestExecution.test_case_id, order_by=TestExecution.id.desc())
   rn_def = func.row_number().over(partition_by=(TestExecution.test_case_id, on_default), order_by=TestExecution.id.desc())
   # keep rows where rn_all <= window OR (on_default AND rn_def <= window); ORDER BY test_case_id, id DESC
   ```
   Also return each Test's `default_branch` from the same query (or from a small second query), so that `branch_scoped_score` gets it.
2. **Applying the Default branch.** Add these two functions:
   ```python
   async def apply_default_branch(db: AsyncSession, repo_id: int, value: str | None) -> bool:
       """Store a reported Default branch on the Repo. True when it changed (None never changes it)."""

   async def rescore_repo(db: AsyncSession, repo_id: int) -> list[int]:
       """Rescore every Test in the Repo (chunked). Returns their ids, sorted."""
   ```
   In `process_report`, call `apply_default_branch(db, report.repo_id, report.default_branch)` **before** inserting Executions. If it returns `True`, then after the normal rescore of touched Tests, call `rescore_repo` too. Task 04 extends `rescore_repo` to Jobs.
   `ProcessOutcome.touched_test_ids` stays "Tests in this Report". Do not add the repo-wide rescore to it, because GitHub filing should not run for the whole Repo because of a branch change.

### `backend/app/routers/reports.py`
- `POST /api/ingest` gains `default_branch: str | None = Query(default=None, max_length=255)`. Strip it, treat empty as `None`, and store it on the Report.

## Acceptance Criteria
- [ ] Repo with Default branch `main`: Executions `feat` fail@a, `feat` pass@b, `main` pass@c, `main` pass@d → score `0`, 0 Proven flakes.
- [ ] The same Repo, `feat` fail@x then `feat` pass@x → Proven flake 1, score ≥ 0.6.
- [ ] A Repo with no Default branch → identical scores to `main` @ `fea6f57` for the same history. Existing `test_processing.py` tests pass unchanged.
- [ ] Setting the Default branch on a Repo whose Tests have feature-branch flips lowers those Tests' scores in the same processing transaction, including Tests that are not in the Report.
- [ ] `default_branch` is only applied when processing. A Report still pending does not change the Repo.
- [ ] A Report without `default_branch` never clears a known one.
- [ ] Window correctness: with `score_window=3`, 5 `main` Executions followed by 10 newer `feat` Executions still flip-score the 3 newest `main` ones.

## Test Expectations
- `tests/test_scoring.py`: add `branch_scoped_score` cases (None equals the legacy result; feature-branch flips are ignored; a same-SHA flip on a feature branch still counts; history slicing).
- `tests/test_processing.py`: add the acceptance cases above, using `make_report(..., default_branch="main")` and `make_run(branch=...)`.
- `tests/test_ingest_api.py`: `default_branch` is stored on the Report, trimmed, and empty becomes `None`.

## Dependencies
- Blocked by: 01.
- Blocks: 04 (Job scoring reuses `branch_scoped_score`, `apply_default_branch` and `rescore_repo`).

## Estimate / Risk
Medium. Risk 3: it changes existing scores, and the window query must be right.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && ruff check backend && ruff format --check backend
```
