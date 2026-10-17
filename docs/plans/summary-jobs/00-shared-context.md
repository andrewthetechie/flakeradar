# Shared context — Summary Jobs (issue #9)

Source: [andrewthetechie/flakeradar#9](https://github.com/andrewthetechie/flakeradar/issues/9).
Amends ADR 0004 (pushed CI jobs and attribution).

## Problem

Some workflows have a summary Job that runs no tests and only checks whether the
Jobs it `needs` succeeded (the [alls-green](https://github.com/marketplace/actions/alls-green#why)
pattern, used for branch protection). When a test Job fails, the summary Job
fails too. FlakeRadar finds no failing Test in it, so it is always
**unexplained**. Consequences:

- The summary Job's score counts every one of those failures. In
  womens-fantasy-sports, `alls-green-playwright` is tier `flaky` at score `0.7`
  without ever having failed for a reason of its own.
- `GET /api/ci-runs/{ci_run_id}/failures` lists it as `unexplained` in every red
  CI run, so a merge gate that blocks on any unexplained failed Job can never
  let through a red run that failed only on known flakes.

ADR 0004 defines an explained failure as "a failing Test execution in a Run with
the same `ci_job_id`". A Job with no tests of its own can never meet that.

## Decisions

1. **Mark the Job, not the execution.** A summary Job is a trait of the Job in
   its pipeline, so `summary` lives on `jobs` and arrives on `JobResultIn`. The
   field is `summary: bool = False`; existing callers are unchanged.
2. **A failed non-summary sibling explains it.** A failed summary Job is
   explained when another failed **non-summary** Job exists in the same
   `(repo, ci_run_id, ci_run_attempt)`. Requiring a non-summary sibling keeps a
   chain of summary Jobs honest: if an inner summary Job breaks, the outer one
   is not blamed on it. The sibling itself may be explained or unexplained — a
   flaky test that explains the test Job also explains the summary Job.
3. **Never scored.** Explained summary failures are fed to scoring as `skipped`,
   exactly like test-explained failures. No new scoring path.
4. **Read-only surfaces agree.** `explained_ci_job_ids`, `_job_history` (scoring),
   `unexplained_failure_counts` (issue filing) and `_ci_run_failed_jobs` (the CI
   run endpoint) must all use the same rule — they already share
   `attribution.py`, so the new clause is added there once.
5. **Visible in responses.** `summary` is added to `JobOut` and
   `CiRunFailedJobOut`, so a CI gate can see why a Job was explained. Not added
   to `JobExecutionOut`: it is a Job trait, not an execution trait.
6. **The reporter must name its summary Jobs.** No CI API exposes a Job's `needs`, so
   the sample reporter gets a `SUMMARY_JOBS` list. Option 1 from the issue (a
   per-repo ignore list) is rejected: each repo would have to know the list and
   FlakeRadar still could not tell.

## Rejected alternatives

- **Sample exclude list only** (issue option 1): cheap, but per-repo knowledge
  and FlakeRadar still cannot tell. Kept only as the reporter's naming mechanism.
- **Infer from "failed Job with no Run"** (issue option 3): hides real setup
  failures, e.g. a Job whose tests crashed before writing JUnit, by blaming an
  unrelated flaky test in the same attempt.

## Repo facts

- `backend/app/attribution.py`: `explained_clause(ci_job_id, repo_id)`,
  `explained_ci_job_ids(db, repo_id, ci_job_ids)`, `unexplained_failure_counts(db, job_ids, window)`.
- `backend/app/processing.py`: `_job_history` feeds an explained failed Job to
  scoring as `JOB_SKIPPED`; `process_pipeline_report` upserts `Job` rows.
- `backend/app/queries.py`: `to_job_out` (L472), `_ci_run_failed_jobs` (L945).
- `backend/app/schemas.py`: `JobResultIn`, `JobOut`, `CiRunFailedJobOut`.
- `backend/migrations/versions/`: head is `0009`; the new revision is `0010`.
- `backend/tests/test_db.py::test_migrations_are_idempotent` pins the head
  revision and must be bumped.

## Attribution rule, in SQL terms

```sql
-- explains a failed Job execution when true
(EXISTS (failing Test execution from a Run with this ci_job_id in this Repo))
OR (jobs.summary AND EXISTS (
      SELECT 1 FROM job_executions sib
      JOIN jobs sj ON sj.id = sib.job_id
      JOIN pipelines sp ON sp.id = sj.pipeline_id
      WHERE sp.repo_id = pipelines.repo_id
        AND sib.ci_run_id = job_executions.ci_run_id
        AND sib.ci_run_attempt = job_executions.ci_run_attempt
        AND sib.status = 'failed'
        AND sj.summary = false))
```

The sibling tables are aliased, so the EXISTS never correlates with the outer
`job_executions` / `jobs` / `pipelines` joins.
