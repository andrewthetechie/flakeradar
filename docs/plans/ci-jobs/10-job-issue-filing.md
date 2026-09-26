# 10 — GitHub issue filing for Jobs

## Tracer-Bullet Outcome
When a Job on a `github` Pipeline passes the existing filing gate, FlakeRadar files one issue in the Job's Repo. The gate is the minimum score, the minimum Proven flakes, and the minimum **unexplained** failures in the window. The issue shows the evidence: the recent Job executions with CI links and runners, and how many failures were explained by tests. Deduplication and the re-arm after an issue closes work as they do for Tests.

## User Story
As a maintainer, I want a flaky CI job to get an issue automatically, like a flaky test does, so that infrastructure flakes get an owner.

## Context Pack
- Read `00-shared-context.md`, section "GitHub issues".
- `backend/app/github_integration.py` already has:
  - the module docstring;
  - `_FilingGate` (the gate settings `github_issue_min_score`, `github_issue_min_proven_flakes`, `github_issue_min_failures`);
  - `_headers`, `_chunks`, `file_issues_for(db, test_case_ids, *, transport=None)`, `sync_closed_issues(session_factory, *, transport=None)` and `on_report_processed(session_factory, outcome)`.
  - Error rules: it never raises, 403/429 stops the batch, and anything else is logged and skipped.
- `backend/tests/test_github.py` uses `httpx.MockTransport`. Copy its patterns.
- `ProcessOutcome.touched_job_ids` comes from tasks 04 and 05. `queries.get_job` comes from task 06. You can reuse it to build the issue body, or query directly.
- Non-goals: filing for non-GitHub providers; new settings.

## Implementation Contract
```python
async def file_job_issues_for(db: AsyncSession, job_ids: list[int], *,
                              transport: httpx.AsyncBaseTransport | None = None) -> None:
    """File issues for github-provider Jobs that now meet the filing gate. Never raises."""
```
- Candidates are Jobs in `job_ids` whose Pipeline `provider == "github"`, whose `github_issue_number IS NULL`, and whose score and Proven flakes meet the gate. When `min_failures > 0`, they also need at least that many **unexplained** failures in the newest `score_window` Job executions. That uses the same `explained` definition as scoring (task 05). Do not re-derive it.
- The title is `[FlakeRadar] Flaky CI job: {pipeline} / {name}`. The label is `github_issue_label`.
- The body is self-contained markdown. It includes:
  - the Repo, Pipeline and Job;
  - the score and Proven flakes;
  - the unexplained vs explained counts;
  - the last 10 Job executions as a table: outcome, short SHA, branch, attempt, runner, and a `[job](url)` link;
  - a short "what to look at" paragraph (setup steps, network, runner capacity, timeouts, and whether failures cluster on one runner).
- Store the returned number on `Job.github_issue_number`, and commit after each issue, as for Tests.
- `sync_closed_issues` also re-arms Jobs. Apply the same 2,000-row cap **separately** to Tests and Jobs, and still stop on 403/429.
- `on_report_processed` calls `file_issues_for` for `touched_test_ids` (unchanged) and `file_job_issues_for` for `touched_job_ids`.
- Update the module docstring to say that Jobs are covered too.

## Acceptance Criteria
- [ ] A github-provider Job over the gate → one POST to `/repos/{repo}/issues` with the expected title and label, and the number is stored.
- [ ] Processing it again with no change → no second POST (dedup).
- [ ] A `gitlab`-provider Job over the gate → no POST.
- [ ] With `min_failures=2`: a Job with 3 failures, all explained → not filed. With 2 unexplained → filed.
- [ ] A closed issue → the number is cleared by `sync_closed_issues`, and the Job is filed again on the next touch.
- [ ] A 403 → the batch stops, and nothing raises.
- [ ] No token configured → a no-op.

## Test Expectations
Extend `backend/tests/test_github.py` with the criteria above, using `MockTransport`.

## Dependencies
- Blocked by: 05, 06.
- Blocks: 11.

## Estimate / Risk
Small–medium. Risk 2: it writes to GitHub, but the gate and the dedup contain that.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && ruff check backend && ruff format --check backend
```
