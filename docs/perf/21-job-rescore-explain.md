# Job rescore: attribution for the score window only (#21)

`EXPLAIN (ANALYZE, BUFFERS)` before and after the change, for a Job with about 5,000
executions. This file is the measured record of the change to `job_execution_window_ids`
(`backend/app/attribution.py`); the PR description carries the same numbers and plans.

## Fixture

PostgreSQL 17.11, alembic-head schema, one Repo at 90-day retention scale:

| table | rows | notes |
| --- | --- | --- |
| `job_executions` | 200,000 | 40 Jobs x 5,000 executions; the measured Job has exactly 5,000 |
| `test_runs` | 100,500 | 500 of them carry the `ci_job_id` of a failed execution of the measured Job |
| `test_executions` | 400,500 | 1 in 16 failing, plus one failing execution per explaining Run |

The measured Job (id 1) has 1,000 failed executions, 500 of them explained by a failing
Test in a Run with the same `ci_job_id` (checked with a join over
`test_runs`/`test_executions`); one of the other Jobs (id 5) is a Summary Job whose
failures are explained by a failed non-summary sibling in the same CI run attempt.
`ANALYZE`d, warm cache, `score_window = 50` (the default), one chunk of Job ids.

## Method

The statements were captured from the running code - `processing._job_history` and
`attribution.unexplained_failure_counts`, through SQLAlchemy's `before_cursor_execute`
hook - once from the base commit ("before") and once from this head ("after"), and then run
as asyncpg prepared statements with the same bind values the worker sends, so Postgres plans
the real query. Three runs each, the last (warm) plan kept.

## Result

| read | what it rescores | before | after | shared buffers |
| --- | --- | --- | --- | --- |
| `_job_history` (Job rescore) | 1 Job, 5,000 executions | 1,039.2 ms | 5.0 ms | 6,367 -> 468 |
| `unexplained_failure_counts` (issue filer) | the same Job | 904.8 ms | 1.4 ms | 6,268 -> 276 |
| `_job_history` (Job rescore) | one Pipeline report's 5 Jobs, 25,000 executions | 1,809.2 ms | 399.9 ms | 10,283 -> 10,979 |
| `unexplained_failure_counts` (issue filer) | the same 5 Jobs | 1,430.0 ms | 5.2 ms | 10,264 -> 1,198 |

The same statements in a separate run with `jit = off`, so the comparison is not about JIT
(which the before plans trip): `_job_history` 509.4 ms -> 4.9 ms,
`unexplained_failure_counts` 409.8 ms -> 1.5 ms.

## Why it got faster

Before, `WindowAgg` evaluated the attribution expression for all 5,000 retained executions
of the Job and `(rn_all | rn_def) <= 50` discarded 4,900 of them afterwards (`Rows Removed
by Filter: 4900`): the cost tracked the Job's retained history. After, the two
`LATERAL ... LIMIT 50` arms read the window ids by Index Only Scan Backward on
`ix_job_exec_job_id` (`Heap Fetches: 0`), those 60 rows are fetched by primary key and the
attribution `SubPlan` runs for them only (`loops=60`): the cost tracks the window.

In the 5-Job row most of the remaining time is the one-time build of the explaining sets
(the `SubPlan` parallel hash join over `test_executions`, and the failed siblings of the
Summary Job's Runs); the before plan pays those builds as well, on top of the per-execution
work for 25,000 rows. For one Job the after `unexplained_failure_counts` plan goes further:
with only 50 candidate ids it probes `ix_test_runs_ci_job_id` per failed window row (10
probes, 276 buffers) instead of building the whole explaining set.

## The two reads return a different number of rows

100 vs 60 for the single Job. The old `row_number() OVER (PARTITION BY job_id, <on the
Default branch>)` also admitted the newest `window` off-Default rows that lay beyond the
overall window. Nothing consumed them: `branch_scoped_score` takes flips on the Default
branch only, Proven flakes come from the overall window, and `branch_scoped_streak` filters
to the Default branch. Checked over all 40 Jobs of this fixture: the newest-`window` slice
and the Default-branch subsequence of the history, the score, confirmed-flake count and
clean streak derived from them, and the `unexplained_failure_counts` result are identical
before and after. With the Default branch set to `feat/x` - the branch this fixture's
failures land on, so 5 Jobs score above 0 - the same comparison is still identical.

## The plans

Each block is the whole `EXPLAIN (ANALYZE, BUFFERS)` output, with the captured statement and
its binds on top.

### before - base commit: attribution computed for every retained execution

`_job_history`, one Job with 5,000 executions, `job_ids = [1]`:

<details>
<summary>full plan</summary>

```
-- statement: SELECT anon_1.job_id, anon_1.commit_sha, anon_1.branch, anon_1.status, anon_1.explained, anon_1.default_branch FROM (SELECT job_executions.id AS id, job_executions.job_id AS job_id, job_executions.commit_sha AS commit_sha, job_executions.branch AS branch, job_executions.status AS status, (EXISTS (SELECT 1 FROM test_runs JOIN projects ON projects.id = test_runs.project_id JOIN test_executions ON test_executions.test_run_id = test_runs.id WHERE test_runs.ci_job_id = job_executions.ci_job_id AND projects.repo_id = pipelines.repo_id AND test_executions.status IN ($5::VARCHAR, $6::VARCHAR))) OR job_executions.ci_run_id != $1::VARCHAR AND jobs.summary IS true AND (EXISTS (SELECT 1 FROM job_executions AS job_executions_1 JOIN jobs AS jobs_1 ON jobs_1.id = job_executions_1.job_id JOIN pipelines AS pipelines_1 ON pipelines_1.id = jobs_1.pipeline_id WHERE job_executions_1.ci_run_id = job_executions.ci_run_id AND job_executions_1.ci_run_attempt = job_executions.ci_run_attempt AND job_executions_1.status = $2::VARCHAR AND jobs_1.summary IS false AND pipelines_1.repo_id = pipelines.repo_id)) AS explained, repos.default_branch AS default_branch, row_number() OVER (PARTITION BY job_executions.job_id ORDER BY job_executions.id DESC) AS rn_all, row_number() OVER (PARTITION BY job_executions.job_id, repos.default_branch IS NULL OR job_executions.branch = repos.default_branch ORDER BY job_executions.id DESC) AS rn_def FROM job_executions JOIN jobs ON jobs.id = job_executions.job_id JOIN pipelines ON pipelines.id = jobs.pipeline_id JOIN repos ON repos.id = pipelines.repo_id WHERE job_executions.job_id IN ($7::INTEGER)) AS anon_1 WHERE anon_1.rn_all <= $3::INTEGER OR anon_1.rn_def <= $4::INTEGER ORDER BY anon_1.job_id, anon_1.id DESC
-- binds: ['', 'failed', 50, 50, 'error', 'failed', 1]

Sort  (cost=925172.15..925179.09 rows=2774 width=63) (actual time=1035.342..1036.203 rows=100 loops=1)
  Sort Key: anon_1.job_id, anon_1.id DESC
  Sort Method: quicksort  Memory: 33kB
  Buffers: shared hit=6367
  ->  Subquery Scan on anon_1  (cost=4405.13..925013.51 rows=2774 width=63) (actual time=1003.113..1035.851 rows=100 loops=1)
        Filter: ((anon_1.rn_all <= 50) OR (anon_1.rn_def <= 50))
        Rows Removed by Filter: 4900
        Buffers: shared hit=6367
        ->  WindowAgg  (cost=4405.13..924938.62 rows=4993 width=80) (actual time=1003.096..1028.137 rows=5000 loops=1)
              Buffers: shared hit=6367
              ->  Sort  (cost=4405.11..4417.59 rows=4993 width=97) (actual time=555.611..563.150 rows=5000 loops=1)
                    Sort Key: job_executions.id DESC
                    Sort Method: quicksort  Memory: 785kB
                    Buffers: shared hit=107
                    ->  WindowAgg  (cost=3986.07..4098.40 rows=4993 width=97) (actual time=521.973..546.525 rows=5000 loops=1)
                          Buffers: shared hit=107
                          ->  Sort  (cost=3986.05..3998.54 rows=4993 width=89) (actual time=521.953..529.934 rows=5000 loops=1)
                                Sort Key: (((repos.default_branch IS NULL) OR ((job_executions.branch)::text = (repos.default_branch)::text))), job_executions.id DESC
                                Sort Method: quicksort  Memory: 746kB
                                Buffers: shared hit=107
                                ->  Nested Loop  (cost=120.63..3679.34 rows=4993 width=89) (actual time=487.819..512.235 rows=5000 loops=1)
                                      Buffers: shared hit=107
                                      ->  Nested Loop  (cost=1.51..3.65 rows=1 width=14) (actual time=487.640..487.691 rows=1 loops=1)
                                            Join Filter: (pipelines.repo_id = repos.id)
                                            Buffers: shared hit=3
                                            ->  Hash Join  (cost=1.51..2.62 rows=1 width=9) (actual time=487.608..487.650 rows=1 loops=1)
                                                  Hash Cond: (pipelines.id = jobs.pipeline_id)
                                                  Buffers: shared hit=2
                                                  ->  Seq Scan on pipelines  (cost=0.00..1.08 rows=8 width=8) (actual time=0.007..0.021 rows=8 loops=1)
                                                        Buffers: shared hit=1
                                                  ->  Hash  (cost=1.50..1.50 rows=1 width=9) (actual time=487.577..487.583 rows=1 loops=1)
                                                        Buckets: 1024  Batches: 1  Memory Usage: 9kB
                                                        Buffers: shared hit=1
                                                        ->  Seq Scan on jobs  (cost=0.00..1.50 rows=1 width=9) (actual time=487.556..487.562 rows=1 loops=1)
                                                              Filter: (id = 1)
                                                              Rows Removed by Filter: 39
                                                              Buffers: shared hit=1
                                            ->  Seq Scan on repos  (cost=0.00..1.01 rows=1 width=9) (actual time=0.015..0.017 rows=1 loops=1)
                                                  Buffers: shared hit=1
                                      ->  Bitmap Heap Scan on job_executions  (cost=119.12..3613.28 rows=4993 width=78) (actual time=0.164..8.560 rows=5000 loops=1)
                                            Recheck Cond: (job_id = 1)
                                            Heap Blocks: exact=82
                                            Buffers: shared hit=104
                                            ->  Bitmap Index Scan on ix_job_exec_job_id  (cost=0.00..117.87 rows=4993 width=0) (actual time=0.147..0.149 rows=5000 loops=1)
                                                  Index Cond: (job_id = 1)
                                                  Buffers: shared hit=22
              SubPlan 2
                ->  Gather  (cost=7769.34..13070.01 rows=25031 width=36) (actual time=135.220..392.484 rows=25500 loops=1)
                      Workers Planned: 1
                      Workers Launched: 1
                      Buffers: shared hit=6260
                      ->  Hash Join  (cost=6769.34..9566.91 rows=14724 width=36) (actual time=124.789..364.696 rows=12750 loops=2)
                            Hash Cond: (test_runs.project_id = projects.id)
                            Buffers: shared hit=6260
                            ->  Parallel Hash Join  (cost=6768.31..9363.43 rows=14724 width=19) (actual time=58.114..253.788 rows=12750 loops=2)
                                  Hash Cond: (test_runs.id = test_executions.test_run_id)
                                  Buffers: shared hit=6199
                                  ->  Parallel Seq Scan on test_runs  (cost=0.00..2238.18 rows=59118 width=23) (actual time=0.009..85.517 rows=50250 loops=2)
                                        Buffers: shared hit=1647
                                  ->  Parallel Hash  (cost=6637.94..6637.94 rows=10430 width=4) (actual time=57.778..57.783 rows=12750 loops=2)
                                        Buckets: 32768  Batches: 1  Memory Usage: 1280kB
                                        Buffers: shared hit=4552
                                        ->  Parallel Seq Scan on test_executions  (cost=0.00..6637.94 rows=10430 width=4) (actual time=11.015..35.133 rows=12750 loops=2)
                                              Filter: ((status)::text = ANY ('{error,failed}'::text[]))
                                              Rows Removed by Filter: 187500
                                              Buffers: shared hit=4552
                            ->  Hash  (cost=1.01..1.01 rows=1 width=8) (actual time=66.599..66.604 rows=1 loops=2)
                                  Buckets: 1024  Batches: 1  Memory Usage: 9kB
                                  Buffers: shared hit=2
                                  ->  Seq Scan on projects  (cost=0.00..1.01 rows=1 width=8) (actual time=66.575..66.580 rows=1 loops=2)
                                        Buffers: shared hit=2
              SubPlan 4
                ->  Gather  (cost=1003.07..5658.83 rows=3256 width=40) (never executed)
                      Workers Planned: 2
                      Workers Launched: 0
                      ->  Hash Join  (cost=3.07..4333.23 rows=1357 width=40) (never executed)
                            Hash Cond: (jobs_1.pipeline_id = pipelines_1.id)
                            ->  Hash Join  (cost=1.89..4326.60 rows=1357 width=16) (never executed)
                                  Hash Cond: (job_executions_1.job_id = jobs_1.id)
                                  ->  Parallel Seq Scan on job_executions job_executions_1  (cost=0.00..4320.67 rows=1392 width=16) (never executed)
                                        Filter: ((status)::text = 'failed'::text)
                                  ->  Hash  (cost=1.40..1.40 rows=39 width=8) (never executed)
                                        ->  Seq Scan on jobs jobs_1  (cost=0.00..1.40 rows=39 width=8) (never executed)
                                              Filter: (summary IS FALSE)
                            ->  Hash  (cost=1.08..1.08 rows=8 width=8) (never executed)
                                  ->  Seq Scan on pipelines pipelines_1  (cost=0.00..1.08 rows=8 width=8) (never executed)
Planning:
  Buffers: shared hit=51
Planning Time: 1.013 ms
JIT:
  Functions: 109
  Options: Inlining true, Optimization true, Expressions true, Deforming true
  Timing: Generation 3.467 ms (Deform 1.738 ms), Inlining 62.641 ms, Optimization 335.141 ms, Emission 231.278 ms, Total 632.528 ms
Execution Time: 1039.224 ms
```

</details>

`unexplained_failure_counts`, the same Job, `window = 50`:

<details>
<summary>full plan</summary>

```
-- statement: SELECT anon_1.job_id FROM (SELECT job_executions.job_id AS job_id, job_executions.status AS status, (EXISTS (SELECT 1 FROM test_runs JOIN projects ON projects.id = test_runs.project_id JOIN test_executions ON test_executions.test_run_id = test_runs.id WHERE test_runs.ci_job_id = job_executions.ci_job_id AND projects.repo_id = pipelines.repo_id AND test_executions.status IN ($5::VARCHAR, $6::VARCHAR))) OR job_executions.ci_run_id != $1::VARCHAR AND jobs.summary IS true AND (EXISTS (SELECT 1 FROM job_executions AS job_executions_1 JOIN jobs AS jobs_1 ON jobs_1.id = job_executions_1.job_id JOIN pipelines AS pipelines_1 ON pipelines_1.id = jobs_1.pipeline_id WHERE job_executions_1.ci_run_id = job_executions.ci_run_id AND job_executions_1.ci_run_attempt = job_executions.ci_run_attempt AND job_executions_1.status = $2::VARCHAR AND jobs_1.summary IS false AND pipelines_1.repo_id = pipelines.repo_id)) AS explained, row_number() OVER (PARTITION BY job_executions.job_id ORDER BY job_executions.id DESC) AS rn FROM job_executions JOIN jobs ON jobs.id = job_executions.job_id JOIN pipelines ON pipelines.id = jobs.pipeline_id WHERE job_executions.job_id IN ($7::INTEGER)) AS anon_1 WHERE anon_1.rn <= $3::INTEGER AND anon_1.status = $4::VARCHAR AND anon_1.explained IS false
-- binds: ['', 'failed', 50, 'failed', 'error', 'failed', 1]

Subquery Scan on anon_1  (cost=3.00..925812.85 rows=42 width=4) (actual time=895.338..901.851 rows=5 loops=1)
  Filter: ((anon_1.explained IS FALSE) AND ((anon_1.status)::text = 'failed'::text))
  Rows Removed by Filter: 45
  Buffers: shared hit=6268
  ->  WindowAgg  (cost=3.00..925750.44 rows=4993 width=28) (actual time=895.190..901.738 rows=50 loops=1)
        Run Condition: (row_number() OVER (?) <= 50)
        Buffers: shared hit=6268
        ->  Nested Loop  (cost=1.93..5241.90 rows=4993 width=45) (actual time=432.907..433.571 rows=51 loops=1)
              Buffers: shared hit=8
              ->  Index Scan Backward using ix_job_exec_job_id on job_executions  (cost=0.42..5176.86 rows=4993 width=40) (actual time=0.015..0.116 rows=51 loops=1)
                    Index Cond: (job_id = 1)
                    Buffers: shared hit=6
              ->  Materialize  (cost=1.51..2.63 rows=1 width=9) (actual time=8.489..8.492 rows=1 loops=51)
                    Buffers: shared hit=2
                    ->  Hash Join  (cost=1.51..2.62 rows=1 width=9) (actual time=432.876..432.919 rows=1 loops=1)
                          Hash Cond: (pipelines.id = jobs.pipeline_id)
                          Buffers: shared hit=2
                          ->  Seq Scan on pipelines  (cost=0.00..1.08 rows=8 width=8) (actual time=0.006..0.022 rows=8 loops=1)
                                Buffers: shared hit=1
                          ->  Hash  (cost=1.50..1.50 rows=1 width=9) (actual time=432.840..432.845 rows=1 loops=1)
                                Buckets: 1024  Batches: 1  Memory Usage: 9kB
                                Buffers: shared hit=1
                                ->  Seq Scan on jobs  (cost=0.00..1.50 rows=1 width=9) (actual time=432.817..432.824 rows=1 loops=1)
                                      Filter: (id = 1)
                                      Rows Removed by Filter: 39
                                      Buffers: shared hit=1
        SubPlan 2
          ->  Gather  (cost=7769.34..13070.01 rows=25031 width=36) (actual time=142.304..412.134 rows=25500 loops=1)
                Workers Planned: 1
                Workers Launched: 1
                Buffers: shared hit=6260
                ->  Hash Join  (cost=6769.34..9566.91 rows=14724 width=36) (actual time=131.575..377.986 rows=12750 loops=2)
                      Hash Cond: (test_runs.project_id = projects.id)
                      Buffers: shared hit=6260
                      ->  Parallel Hash Join  (cost=6768.31..9363.43 rows=14724 width=19) (actual time=60.653..262.605 rows=12750 loops=2)
                            Hash Cond: (test_runs.id = test_executions.test_run_id)
                            Buffers: shared hit=6199
                            ->  Parallel Seq Scan on test_runs  (cost=0.00..2238.18 rows=59118 width=23) (actual time=0.012..88.762 rows=50250 loops=2)
                                  Buffers: shared hit=1647
                            ->  Parallel Hash  (cost=6637.94..6637.94 rows=10430 width=4) (actual time=60.261..60.266 rows=12750 loops=2)
                                  Buckets: 32768  Batches: 1  Memory Usage: 1280kB
                                  Buffers: shared hit=4552
                                  ->  Parallel Seq Scan on test_executions  (cost=0.00..6637.94 rows=10430 width=4) (actual time=11.811..37.031 rows=12750 loops=2)
                                        Filter: ((status)::text = ANY ('{error,failed}'::text[]))
                                        Rows Removed by Filter: 187500
                                        Buffers: shared hit=4552
                      ->  Hash  (cost=1.01..1.01 rows=1 width=8) (actual time=70.833..70.838 rows=1 loops=2)
                            Buckets: 1024  Batches: 1  Memory Usage: 9kB
                            Buffers: shared hit=2
                            ->  Seq Scan on projects  (cost=0.00..1.01 rows=1 width=8) (actual time=70.808..70.813 rows=1 loops=2)
                                  Buffers: shared hit=2
        SubPlan 4
          ->  Gather  (cost=1003.07..5658.83 rows=3256 width=40) (never executed)
                Workers Planned: 2
                Workers Launched: 0
                ->  Hash Join  (cost=3.07..4333.23 rows=1357 width=40) (never executed)
                      Hash Cond: (jobs_1.pipeline_id = pipelines_1.id)
                      ->  Hash Join  (cost=1.89..4326.60 rows=1357 width=16) (never executed)
                            Hash Cond: (job_executions_1.job_id = jobs_1.id)
                            ->  Parallel Seq Scan on job_executions job_executions_1  (cost=0.00..4320.67 rows=1392 width=16) (never executed)
                                  Filter: ((status)::text = 'failed'::text)
                            ->  Hash  (cost=1.40..1.40 rows=39 width=8) (never executed)
                                  ->  Seq Scan on jobs jobs_1  (cost=0.00..1.40 rows=39 width=8) (never executed)
                                        Filter: (summary IS FALSE)
                      ->  Hash  (cost=1.08..1.08 rows=8 width=8) (never executed)
                            ->  Seq Scan on pipelines pipelines_1  (cost=0.00..1.08 rows=8 width=8) (never executed)
Planning:
  Buffers: shared hit=51
Planning Time: 1.038 ms
JIT:
  Functions: 103
  Options: Inlining true, Optimization true, Expressions true, Deforming true
  Timing: Generation 3.519 ms (Deform 1.737 ms), Inlining 67.749 ms, Optimization 292.244 ms, Emission 223.304 ms, Total 586.816 ms
Execution Time: 904.782 ms
```

</details>

### after - this head: window ids first, attribution only for the window rows

`_job_history`, one Job with 5,000 executions, `job_ids = [1]`:

<details>
<summary>full plan</summary>

```
-- statement: SELECT job_executions.job_id, job_executions.commit_sha, job_executions.branch, job_executions.status, (EXISTS (SELECT 1 FROM test_runs JOIN projects ON projects.id = test_runs.project_id JOIN test_executions ON test_executions.test_run_id = test_runs.id WHERE test_runs.ci_job_id = job_executions.ci_job_id AND projects.repo_id = pipelines.repo_id AND test_executions.status IN ($7::VARCHAR, $8::VARCHAR))) OR job_executions.ci_run_id != $1::VARCHAR AND jobs.summary IS true AND (EXISTS (SELECT 1 FROM job_executions AS job_executions_1 JOIN jobs AS jobs_1 ON jobs_1.id = job_executions_1.job_id JOIN pipelines AS pipelines_1 ON pipelines_1.id = jobs_1.pipeline_id WHERE job_executions_1.ci_run_id = job_executions.ci_run_id AND job_executions_1.ci_run_attempt = job_executions.ci_run_attempt AND job_executions_1.status = $2::VARCHAR AND jobs_1.summary IS false AND pipelines_1.repo_id = pipelines.repo_id)) AS explained, repos.default_branch FROM job_executions JOIN jobs ON jobs.id = job_executions.job_id JOIN pipelines ON pipelines.id = jobs.pipeline_id JOIN repos ON repos.id = pipelines.repo_id WHERE job_executions.id IN (SELECT anon_1.id FROM (SELECT anon_2.job_id AS job_id, anon_3.id AS id FROM unnest($3::INTEGER[]) AS anon_2(job_id) JOIN LATERAL (SELECT job_executions.id AS id FROM job_executions WHERE job_executions.job_id = anon_2.job_id ORDER BY job_executions.id DESC LIMIT $4::INTEGER) AS anon_3 ON true UNION SELECT anon_4.job_id AS job_id, anon_5.id AS id FROM unnest($5::INTEGER[]) AS anon_4(job_id) JOIN LATERAL (SELECT job_executions.id AS id FROM job_executions JOIN jobs ON jobs.id = job_executions.job_id JOIN pipelines ON pipelines.id = jobs.pipeline_id JOIN repos ON repos.id = pipelines.repo_id WHERE job_executions.job_id = anon_4.job_id AND (repos.default_branch IS NULL OR job_executions.branch = repos.default_branch) ORDER BY job_executions.id DESC LIMIT $6::INTEGER) AS anon_5 ON true) AS anon_1) ORDER BY job_executions.job_id, job_executions.id DESC
-- binds: ['', 'failed', [1], 50, [1], 50, 'error', 'failed']

Incremental Sort  (cost=1391.76..19369.73 rows=100 width=63) (actual time=4.606..4.816 rows=60 loops=1)
  Sort Key: job_executions.job_id, job_executions.id DESC
  Presorted Key: job_executions.job_id
  Full-sort Groups: 1  Sort Method: quicksort  Average Memory: 29kB  Peak Memory: 29kB
  Buffers: shared hit=468
  ->  Merge Join  (cost=930.84..19367.02 rows=100 width=63) (actual time=3.453..4.589 rows=60 loops=1)
        Merge Cond: (jobs.id = job_executions.job_id)
        Buffers: shared hit=468
        ->  Sort  (cost=5.32..5.42 rows=40 width=14) (actual time=0.476..0.500 rows=2 loops=1)
              Sort Key: jobs.id
              Sort Method: quicksort  Memory: 26kB
              Buffers: shared hit=3
              ->  Nested Loop  (cost=1.18..4.25 rows=40 width=14) (actual time=0.060..0.422 rows=40 loops=1)
                    Join Filter: (pipelines.repo_id = repos.id)
                    Buffers: shared hit=3
                    ->  Seq Scan on repos  (cost=0.00..1.01 rows=1 width=9) (actual time=0.005..0.009 rows=1 loops=1)
                          Buffers: shared hit=1
                    ->  Hash Join  (cost=1.18..2.74 rows=40 width=9) (actual time=0.049..0.260 rows=40 loops=1)
                          Hash Cond: (jobs.pipeline_id = pipelines.id)
                          Buffers: shared hit=2
                          ->  Seq Scan on jobs  (cost=0.00..1.40 rows=40 width=9) (actual time=0.004..0.071 rows=40 loops=1)
                                Buffers: shared hit=1
                          ->  Hash  (cost=1.08..1.08 rows=8 width=8) (actual time=0.035..0.040 rows=8 loops=1)
                                Buckets: 1024  Batches: 1  Memory Usage: 9kB
                                Buffers: shared hit=1
                                ->  Seq Scan on pipelines  (cost=0.00..1.08 rows=8 width=8) (actual time=0.003..0.017 rows=8 loops=1)
                                      Buffers: shared hit=1
        ->  Sort  (cost=925.53..925.78 rows=100 width=78) (actual time=2.940..3.107 rows=60 loops=1)
              Sort Key: job_executions.job_id
              Sort Method: quicksort  Memory: 30kB
              Buffers: shared hit=255
              ->  Nested Loop  (cost=113.87..922.20 rows=100 width=78) (actual time=2.273..2.897 rows=60 loops=1)
                    Buffers: shared hit=255
                    ->  HashAggregate  (cost=113.45..114.45 rows=100 width=8) (actual time=2.262..2.423 rows=60 loops=1)
                          Group Key: job_executions_1.id
                          Batches: 1  Memory Usage: 24kB
                          Buffers: shared hit=15
                          ->  HashAggregate  (cost=111.20..112.20 rows=100 width=12) (actual time=2.053..2.209 rows=60 loops=1)
                                Group Key: anon_2.job_id, job_executions_1.id
                                Batches: 1  Memory Usage: 24kB
                                Buffers: shared hit=15
                                ->  Append  (cost=0.42..110.70 rows=100 width=12) (actual time=0.024..1.930 rows=100 loops=1)
                                      Buffers: shared hit=15
                                      ->  Nested Loop  (cost=0.42..2.61 rows=50 width=12) (actual time=0.020..0.447 rows=50 loops=1)
                                            Buffers: shared hit=5
                                            ->  Function Scan on unnest anon_2  (cost=0.00..0.01 rows=1 width=4) (actual time=0.004..0.008 rows=1 loops=1)
                                            ->  Limit  (cost=0.42..2.10 rows=50 width=8) (actual time=0.011..0.268 rows=50 loops=1)
                                                  Buffers: shared hit=5
                                                  ->  Index Only Scan Backward using ix_job_exec_job_id on job_executions job_executions_1  (cost=0.42..167.92 rows=5000 width=8) (actual time=0.008..0.098 rows=50 loops=1)
                                                        Index Cond: (job_id = anon_2.job_id)
                                                        Heap Fetches: 0
                                                        Buffers: shared hit=5
                                      ->  Nested Loop  (cost=1.93..107.59 rows=50 width=12) (actual time=0.104..1.149 rows=50 loops=1)
                                            Buffers: shared hit=10
                                            ->  Function Scan on unnest anon_4  (cost=0.00..0.01 rows=1 width=4) (actual time=0.002..0.005 rows=1 loops=1)
                                            ->  Limit  (cost=1.93..107.08 rows=50 width=8) (actual time=0.096..0.965 rows=50 loops=1)
                                                  Buffers: shared hit=10
                                                  ->  Nested Loop  (cost=1.93..5259.13 rows=2500 width=8) (actual time=0.092..0.797 rows=50 loops=1)
                                                        Join Filter: ((repos_1.default_branch IS NULL) OR ((job_executions_2.branch)::text = (repos_1.default_branch)::text))
                                                        Rows Removed by Join Filter: 13
                                                        Buffers: shared hit=10
                                                        ->  Index Scan Backward using ix_job_exec_job_id on job_executions job_executions_2  (cost=0.42..5180.48 rows=5000 width=17) (actual time=0.005..0.115 rows=63 loops=1)
                                                              Index Cond: (job_id = anon_4.job_id)
                                                              Buffers: shared hit=7
                                                        ->  Materialize  (cost=1.51..3.65 rows=1 width=9) (actual time=0.002..0.005 rows=1 loops=63)
                                                              Buffers: shared hit=3
                                                              ->  Nested Loop  (cost=1.51..3.65 rows=1 width=9) (actual time=0.034..0.083 rows=1 loops=1)
                                                                    Join Filter: (pipelines_1.repo_id = repos_1.id)
                                                                    Buffers: shared hit=3
                                                                    ->  Hash Join  (cost=1.51..2.62 rows=1 width=8) (actual time=0.025..0.065 rows=1 loops=1)
                                                                          Hash Cond: (pipelines_1.id = jobs_1.pipeline_id)
                                                                          Buffers: shared hit=2
                                                                          ->  Seq Scan on pipelines pipelines_1  (cost=0.00..1.08 rows=8 width=8) (actual time=0.003..0.017 rows=8 loops=1)
                                                                                Buffers: shared hit=1
                                                                          ->  Hash  (cost=1.50..1.50 rows=1 width=8) (actual time=0.012..0.018 rows=1 loops=1)
                                                                                Buckets: 1024  Batches: 1  Memory Usage: 9kB
                                                                                Buffers: shared hit=1
                                                                                ->  Seq Scan on jobs jobs_1  (cost=0.00..1.50 rows=1 width=8) (actual time=0.004..0.009 rows=1 loops=1)
                                                                                      Filter: (id = anon_4.job_id)
                                                                                      Rows Removed by Filter: 39
                                                                                      Buffers: shared hit=1
                                                                    ->  Seq Scan on repos repos_1  (cost=0.00..1.01 rows=1 width=9) (actual time=0.003..0.004 rows=1 loops=1)
                                                                          Buffers: shared hit=1
                    ->  Index Scan using job_executions_pkey on job_executions  (cost=0.42..8.08 rows=1 width=78) (actual time=0.003..0.003 rows=1 loops=60)
                          Index Cond: (id = job_executions_1.id)
                          Buffers: shared hit=240
        SubPlan 1
          ->  Nested Loop  (cost=0.84..29.21 rows=1 width=0) (actual time=0.011..0.011 rows=0 loops=60)
                Buffers: shared hit=210
                ->  Nested Loop  (cost=0.42..9.46 rows=1 width=4) (actual time=0.007..0.007 rows=0 loops=60)
                      Join Filter: (test_runs.project_id = projects.id)
                      Buffers: shared hit=190
                      ->  Index Scan using ix_test_runs_ci_job_id on test_runs  (cost=0.42..8.44 rows=1 width=8) (actual time=0.003..0.003 rows=0 loops=60)
                            Index Cond: ((ci_job_id)::text = (job_executions.ci_job_id)::text)
                            Buffers: shared hit=185
                      ->  Seq Scan on projects  (cost=0.00..1.01 rows=1 width=4) (actual time=0.002..0.003 rows=1 loops=5)
                            Filter: (repo_id = pipelines.repo_id)
                            Buffers: shared hit=5
                ->  Index Scan using ix_test_executions_test_run_id on test_executions  (cost=0.42..19.74 rows=1 width=4) (actual time=0.003..0.004 rows=1 loops=5)
                      Index Cond: (test_run_id = test_runs.id)
                      Filter: ((status)::text = ANY ('{error,failed}'::text[]))
                      Buffers: shared hit=20
        SubPlan 4
          ->  Gather  (cost=1003.07..5658.83 rows=3256 width=40) (never executed)
                Workers Planned: 2
                Workers Launched: 0
                ->  Hash Join  (cost=3.07..4333.23 rows=1357 width=40) (never executed)
                      Hash Cond: (jobs_1_1.pipeline_id = pipelines_1_1.id)
                      ->  Hash Join  (cost=1.89..4326.60 rows=1357 width=16) (never executed)
                            Hash Cond: (job_executions_1_1.job_id = jobs_1_1.id)
                            ->  Parallel Seq Scan on job_executions job_executions_1_1  (cost=0.00..4320.67 rows=1392 width=16) (never executed)
                                  Filter: ((status)::text = 'failed'::text)
                            ->  Hash  (cost=1.40..1.40 rows=39 width=8) (never executed)
                                  ->  Seq Scan on jobs jobs_1_1  (cost=0.00..1.40 rows=39 width=8) (never executed)
                                        Filter: (summary IS FALSE)
                      ->  Hash  (cost=1.08..1.08 rows=8 width=8) (never executed)
                            ->  Seq Scan on pipelines pipelines_1_1  (cost=0.00..1.08 rows=8 width=8) (never executed)
Planning:
  Buffers: shared hit=60
Planning Time: 1.307 ms
Execution Time: 4.993 ms
```

</details>

`unexplained_failure_counts`, the same Job, `window = 50`:

<details>
<summary>full plan</summary>

```
-- statement: SELECT job_executions.job_id FROM job_executions JOIN jobs ON jobs.id = job_executions.job_id JOIN pipelines ON pipelines.id = jobs.pipeline_id WHERE job_executions.id IN (SELECT anon_1.id FROM (SELECT anon_2.job_id AS job_id, anon_3.id AS id FROM unnest($1::INTEGER[]) AS anon_2(job_id) JOIN LATERAL (SELECT job_executions.id AS id FROM job_executions WHERE job_executions.job_id = anon_2.job_id ORDER BY job_executions.id DESC LIMIT $2::INTEGER) AS anon_3 ON true) AS anon_1) AND job_executions.status = $3::VARCHAR AND ((EXISTS (SELECT 1 FROM test_runs JOIN projects ON projects.id = test_runs.project_id JOIN test_executions ON test_executions.test_run_id = test_runs.id WHERE test_runs.ci_job_id = job_executions.ci_job_id AND projects.repo_id = pipelines.repo_id AND test_executions.status IN ($6::VARCHAR, $7::VARCHAR))) OR job_executions.ci_run_id != $4::VARCHAR AND jobs.summary IS true AND (EXISTS (SELECT 1 FROM job_executions AS job_executions_1 JOIN jobs AS jobs_1 ON jobs_1.id = job_executions_1.job_id JOIN pipelines AS pipelines_1 ON pipelines_1.id = jobs_1.pipeline_id WHERE job_executions_1.ci_run_id = job_executions.ci_run_id AND job_executions_1.ci_run_attempt = job_executions.ci_run_attempt AND job_executions_1.status = $5::VARCHAR AND jobs_1.summary IS false AND pipelines_1.repo_id = pipelines.repo_id))) IS false
-- binds: [[1], 50, 'failed', '', 'failed', 'error', 'failed']

Hash Join  (cost=4.33..420.55 rows=1 width=4) (actual time=0.700..1.323 rows=5 loops=1)
  Hash Cond: (jobs.pipeline_id = pipelines.id)
  Join Filter: ((EXISTS(SubPlan 1) OR (((job_executions.ci_run_id)::text <> ''::text) AND (jobs.summary IS TRUE) AND EXISTS(SubPlan 3))) IS FALSE)
  Rows Removed by Join Filter: 5
  Buffers: shared hit=276
  ->  Nested Loop  (cost=3.15..419.36 rows=1 width=30) (actual time=0.603..1.062 rows=10 loops=1)
        Join Filter: (job_executions.job_id = jobs.id)
        Buffers: shared hit=215
        ->  Nested Loop  (cost=3.15..417.46 rows=1 width=25) (actual time=0.593..0.971 rows=10 loops=1)
              Buffers: shared hit=205
              ->  HashAggregate  (cost=2.73..3.23 rows=50 width=8) (actual time=0.535..0.634 rows=50 loops=1)
                    Group Key: job_executions_1.id
                    Batches: 1  Memory Usage: 24kB
                    Buffers: shared hit=5
                    ->  Nested Loop  (cost=0.42..2.61 rows=50 width=8) (actual time=0.020..0.455 rows=50 loops=1)
                          Buffers: shared hit=5
                          ->  Function Scan on unnest anon_2  (cost=0.00..0.01 rows=1 width=4) (actual time=0.004..0.007 rows=1 loops=1)
                          ->  Limit  (cost=0.42..2.10 rows=50 width=8) (actual time=0.011..0.277 rows=50 loops=1)
                                Buffers: shared hit=5
                                ->  Index Only Scan Backward using ix_job_exec_job_id on job_executions job_executions_1  (cost=0.42..167.92 rows=5000 width=8) (actual time=0.007..0.099 rows=50 loops=1)
                                      Index Cond: (job_id = anon_2.job_id)
                                      Heap Fetches: 0
                                      Buffers: shared hit=5
              ->  Index Scan using job_executions_pkey on job_executions  (cost=0.42..8.28 rows=1 width=33) (actual time=0.003..0.003 rows=0 loops=50)
                    Index Cond: (id = job_executions_1.id)
                    Filter: ((status)::text = 'failed'::text)
                    Rows Removed by Filter: 1
                    Buffers: shared hit=200
        ->  Seq Scan on jobs  (cost=0.00..1.40 rows=40 width=9) (actual time=0.002..0.002 rows=1 loops=10)
              Buffers: shared hit=10
  ->  Hash  (cost=1.08..1.08 rows=8 width=8) (actual time=0.037..0.042 rows=8 loops=1)
        Buckets: 1024  Batches: 1  Memory Usage: 9kB
        Buffers: shared hit=1
        ->  Seq Scan on pipelines  (cost=0.00..1.08 rows=8 width=8) (actual time=0.006..0.021 rows=8 loops=1)
              Buffers: shared hit=1
  SubPlan 1
    ->  Nested Loop  (cost=0.84..29.21 rows=1 width=0) (actual time=0.015..0.016 rows=0 loops=10)
          Buffers: shared hit=60
          ->  Nested Loop  (cost=0.42..9.46 rows=1 width=4) (actual time=0.009..0.010 rows=0 loops=10)
                Join Filter: (test_runs.project_id = projects.id)
                Buffers: shared hit=40
                ->  Index Scan using ix_test_runs_ci_job_id on test_runs  (cost=0.42..8.44 rows=1 width=8) (actual time=0.003..0.003 rows=0 loops=10)
                      Index Cond: ((ci_job_id)::text = (job_executions.ci_job_id)::text)
                      Buffers: shared hit=35
                ->  Seq Scan on projects  (cost=0.00..1.01 rows=1 width=4) (actual time=0.002..0.002 rows=1 loops=5)
                      Filter: (repo_id = pipelines.repo_id)
                      Buffers: shared hit=5
          ->  Index Scan using ix_test_executions_test_run_id on test_executions  (cost=0.42..19.74 rows=1 width=4) (actual time=0.003..0.003 rows=1 loops=5)
                Index Cond: (test_run_id = test_runs.id)
                Filter: ((status)::text = ANY ('{error,failed}'::text[]))
                Buffers: shared hit=20
  SubPlan 3
    ->  Nested Loop  (cost=4.60..155.13 rows=1 width=0) (never executed)
          Join Filter: (pipelines_1.id = jobs_1.pipeline_id)
          ->  Nested Loop  (cost=4.60..153.93 rows=1 width=4) (never executed)
                Join Filter: (job_executions_1_1.job_id = jobs_1.id)
                ->  Bitmap Heap Scan on job_executions job_executions_1_1  (cost=4.60..152.04 rows=1 width=4) (never executed)
                      Recheck Cond: ((ci_run_id)::text = (job_executions.ci_run_id)::text)
                      Filter: ((ci_run_attempt = job_executions.ci_run_attempt) AND ((status)::text = 'failed'::text))
                      ->  Bitmap Index Scan on ix_job_executions_ci_run_id  (cost=0.00..4.59 rows=40 width=0) (never executed)
                            Index Cond: ((ci_run_id)::text = (job_executions.ci_run_id)::text)
                ->  Seq Scan on jobs jobs_1  (cost=0.00..1.40 rows=39 width=8) (never executed)
                      Filter: (summary IS FALSE)
          ->  Seq Scan on pipelines pipelines_1  (cost=0.00..1.10 rows=8 width=4) (never executed)
                Filter: (repo_id = pipelines.repo_id)
Planning:
  Buffers: shared hit=63
Planning Time: 0.955 ms
Execution Time: 1.378 ms
```

</details>

## Reproduce

Fixture, after `alembic upgrade head` on an empty database (`score_window` left at its
default of 50):

```sql
INSERT INTO repos (id, name, default_branch, created_at) VALUES (1, 'acme/app', 'main', now());
INSERT INTO projects (id, repo_id, name, root, created_at) VALUES (1, 1, 'default', '', now());
INSERT INTO pipelines (id, repo_id, provider, name, created_at)
SELECT g, 1, 'github', 'ci-' || g, now() FROM generate_series(1, 8) g;
INSERT INTO jobs (id, pipeline_id, name, summary, created_at, last_seen_at)
SELECT g, 1 + ((g - 1) / 5), 'job-' || g, g = 5, now(), now() FROM generate_series(1, 40) g;
INSERT INTO test_cases (id, project_id, fingerprint, name, last_seen_at)
SELECT g, 1, md5('tc' || g), 'test_' || g, now() FROM generate_series(1, 100) g;

-- 5,000 runs per Job, ids interleaved across Jobs like real ingest.
INSERT INTO job_executions (job_id, ci_job_id, ci_run_id, ci_run_attempt, commit_sha, branch, status, created_at)
SELECT j, 'je' || j || '-' || g, 'run-' || g, 1, md5('sha' || g),
       CASE WHEN g % 5 = 0 THEN 'feat/x' ELSE 'main' END,
       CASE WHEN g % 5 = 0 AND j IN (1, 5) THEN 'failed'
            WHEN j % 7 = 0 AND g % 20 = 0 THEN 'failed' ELSE 'passed' END,
       now() - (g::text || ' minutes')::interval
FROM generate_series(1, 5000) g, generate_series(1, 40) j;

INSERT INTO test_runs (id, project_id, commit_sha, branch, ci_run_id, ci_job_id, ci_run_attempt, created_at)
SELECT g, 1, md5('r' || g), 'main', 'noise-run-' || g, 'noise-job-' || g, 1, now()
FROM generate_series(1, 100000) g;
INSERT INTO test_executions (id, test_case_id, test_run_id, status, attempt, failure_category, created_at)
SELECT 4 * (r.id - 1) + k, 1 + ((r.id + k) % 100), r.id,
       CASE WHEN (r.id * 4 + k) % 16 = 0 THEN 'failed' ELSE 'passed' END, 0,
       CASE WHEN (r.id * 4 + k) % 16 = 0 THEN 'assertion' ELSE NULL END, now()
FROM test_runs r, generate_series(1, 4) k;

-- Half of Job 1's failures: a failing Test in a Run with the same ci_job_id.
INSERT INTO test_runs (id, project_id, commit_sha, branch, ci_run_id, ci_job_id, ci_run_attempt, created_at)
SELECT 200000 + je.id, 1, je.commit_sha, je.branch, je.ci_run_id, je.ci_job_id, 1, now()
FROM job_executions je
WHERE je.job_id = 1 AND je.status = 'failed' AND (substring(je.ci_job_id FROM '-([0-9]+)$'))::int % 2 = 0;
INSERT INTO test_executions (id, test_case_id, test_run_id, status, attempt, failure_category, created_at)
SELECT 4 * r.id + 1, 1, r.id, 'failed', 0, 'assertion', now() FROM test_runs r WHERE r.id >= 200000;

SELECT setval('test_runs_id_seq', (SELECT max(id) FROM test_runs));
SELECT setval('test_executions_id_seq', (SELECT max(id) FROM test_executions));
ANALYZE;
```

Then, from each tree (base commit, then the head), capture the statement the app sends and
plan it:

```python
event.listen(engine.sync_engine, "before_cursor_execute", listener)  # keeps the SQL + binds
async with factory() as db:
    await processing._job_history(db, [1])
# EXPLAIN (ANALYZE, BUFFERS) <captured statement> run through asyncpg with the captured binds
```
