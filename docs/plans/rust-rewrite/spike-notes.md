# SeaORM spike: can the builder express the hard queries?

Written by task 03. Every query below is proved against a real PostgreSQL 17 in
`crates/flakeradar-db/tests/spike.rs`, and the test that proves it is named here.
The question for each is the same: can SeaORM 2 build it readably, or does it need
a raw `Statement`? A raw `Statement` binds parameters and never builds SQL from an
input string, so the answer changes readability, not safety.

The spike never changes the ORM decision (`00-shared-context.md`, "Settled
decisions"): the answer for a query is "builder" or "raw `Statement`", and either
way the query is written with SeaORM in the crate.

| # | Query | Python reference | Verdict | Proved by |
|---|---|---|---|---|
| 1 | `claim_next_report` | `processing.py:83` | Builder | `claim_next_report_locks_one_pending_report_and_skips_locked_ones` |
| 2 | `test_execution_window_ids`, `job_execution_window_ids` | `attribution.py:227`, `attribution.py:264` | Raw `Statement` | `the_newest_window_ids_per_test_need_a_lateral_join`, `the_newest_window_ids_per_job_need_a_lateral_join` |
| 3 | `past_scores` | `score_history.py:53` | Builder | `past_scores_use_a_row_number_window` |
| 4 | `newest_history_before` on a filtered, counted, paged leaderboard | `score_history.py`, `queries.py:132` | Raw `Statement` | `newest_history_before_feeds_a_filtered_paged_leaderboard` |
| 5 | `upsert_history` | `score_history.py:139` | Builder | `upsert_history_accumulates_through_on_conflict` |
| 6 | Bulk update by primary key, per-row values, optional columns, one statement per chunk | `processing.py:311`, `processing.py:433` | Raw `Statement` | `a_bulk_update_by_primary_key_sets_optional_columns_per_row` |
| 7 | `_delete_history_batch` | `retention.py:93` | Builder | `deleting_a_history_batch_uses_a_tuple_in_a_limited_subquery` |
| 8 | Advisory lock on one dedicated connection that never returns to the pool | `db.py` worker lock | Raw `Statement` plus the pool seam | `one_dedicated_connection_holds_the_worker_lock` |

## 1. `claim_next_report`: `FOR UPDATE SKIP LOCKED` — builder

The builder has both halves: `QuerySelect::lock_with_behavior(LockType::Update,
LockBehavior::SkipLocked)` emits `FOR UPDATE SKIP LOCKED` inside a transaction,
and a second claim on another connection takes the next Report instead of waiting
for the locked one. Nothing raw is needed.

## 2. `test_execution_window_ids` / `job_execution_window_ids`: `LATERAL ... ORDER BY ... LIMIT` — raw

The query is `FROM unnest($1::int[]) AS w(id) CROSS JOIN LATERAL (SELECT ...
WHERE ... = w.id ORDER BY id DESC LIMIT $2)`. sea-query 1.0 has no lateral-join
construct: `Query::select().join_as` takes a table, not a subquery, and nothing
attaches `LATERAL` to a subquery. Writing it as a raw `Statement` keeps the
shape — and the plan, which is the point of the query: each lateral stops on the
`(owner, id)` index after `window` rows.

The spike binds the id array as `Value::Array(ArrayType::Int, ...)`, because the
driver rows come from a bound array rather than a table scan. That binding is the
part the later tasks depend on, and it works.

## 3. `past_scores`: `row_number()` window — builder

`WindowStatement::partition_by(..)` with `order_by(..)` feeds
`Expr::cust("ROW_NUMBER()").over(window)`, and the result goes into
`Query::select().from_subquery(..).and_where(rn = 1)`. The builder expresses the
window and the outer filter, so this stays a builder query.

## 4. `newest_history_before` on a filtered, counted, paged leaderboard — raw

The leaderboard joins a `LEFT JOIN LATERAL ... ORDER BY day DESC LIMIT 1` for the
history row a page needs, then filters on it (`h.score IS NULL OR h.score < $`),
orders and pages, and the count is the same query with `count(*)` instead of the
selected columns. The lateral is the blocker from #2, and the count reuses the
same SQL, so one raw `Statement` and one counted variant of it are clearer than
a half-built query.

## 5. `upsert_history`: `ON CONFLICT DO UPDATE` with `EXCLUDED` and an addition — builder

`OnConflict::columns([..]).value(col, Expr::cust("EXCLUDED.flakiness_score"))
.value(col, Expr::col((Entity, Column)).add(Expr::cust("EXCLUDED.executions")))`
emits the whole upsert, and two runs leave `executions` summed and the score from
the newer row. The stored column has to be qualified with the entity: an
unqualified name in `DO UPDATE` is ambiguous between the stored row and
`EXCLUDED`, which PostgreSQL rejects (42702).

## 6. Bulk update by primary key, per-row values and optional columns — raw

`UPDATE test_cases AS t SET file = v.file, ... FROM (VALUES ($1, ..), ($2, ..))
AS v(id, file, line, score) WHERE t.id = v.id` sets a different value per row in
one statement, and a row whose `file` is a null parameter stores NULL while a row
the statement does not name is untouched. `UpdateMany` has no per-row `SET`, and
`UPDATE ... FROM (VALUES ...)` in a raw `Statement` binds every value, so this is
the one statement per chunk the plan allows. Chunks are `CHUNK` (1000) rows, from
`flakeradar_db::chunks`.

## 7. `_delete_history_batch`: delete with a tuple `IN (SELECT ... LIMIT)` — builder

`Expr::tuple([..]).in_subquery(select(..).limit(batch))` inside
`Entity::delete_many().filter(..)` emits the tuple `IN` and returns the affected
row count, which is what the retention loop pages on.

## 8. Advisory lock on one dedicated connection — raw, plus the pool seam

`SELECT pg_try_advisory_lock($1)` is a raw `Statement` because it is a
PostgreSQL function, and a session lock lives and dies with the connection that
took it. The connection must therefore never be handed back to the shared pool:
the spike proves that a separate `connect(url, settings)` pool holds the lock
while a second connection cannot take it, and that the lock is free again once
the holder releases it. `connect` is the seam tasks 08 and 10 take the lock
through, so a pool of one connection serves as the dedicated session.

## 9. The two rescore window reads keep the measured plan (task 07)

Task 07's "Done when" asks for `EXPLAIN` on the two rescore reads against the
plans in `docs/perf/20-test-rescore-explain.md` and
`21-job-rescore-explain.md`: the same indexes, and a per-row `LATERAL ... LIMIT`
rather than a rank over the whole retained history.

`attribution::test_window_ids_sql` and `attribution::job_window_ids_sql` are the
committed statements, and `rescore` / `rescore_jobs` send them unchanged. Run
with `EXPLAIN (ANALYZE, BUFFERS)` through the driver, one chunk of 500 ids and a
window of 50, against a fixture seeded in the test database (500 Tests x 200
Runs x 100,000 Test executions, and 100 Jobs x 200 Job executions in one
Pipeline, `ANALYZE`d):

```
tests: HashAggregate -> Append -> Nested Loop x2
         Function Scan on unnest u (500 rows)
         ->  Limit  (loops=500)                      <- one per id, the lateral
               ->  Index Only Scan Backward using ix_exec_case_id on test_executions
                     Index Cond: (test_case_id = u.test_case_id)
       Execution Time: 46.1 ms, Buffers: shared hit=53120

jobs:  HashAggregate -> Append -> Nested Loop x2
         Function Scan on unnest u (500 rows)
         ->  Limit  (loops=500)                      <- one per id, the lateral
               ->  Index Only Scan Backward using ix_job_exec_job_id on job_executions
                     Index Cond: (job_id = u.job_id)
       Execution Time: 5.2 ms, Buffers: shared hit=2768
```

Both shapes match `docs/perf/`: one `Limit` per id (the lateral arm), an index
scan on `ix_exec_case_id` / `ix_job_exec_job_id` inside it, and no sort of the
retained history. The branch-scoped arm adds a join to `test_runs`/`jobs` whose
rows are memoized per run in the plan, which is the `Nested Loop` under the
second `Limit`. The figures are from a laptop fixture, not the 2,000,000-row
seed the perf note measured, so only the shape and the index names are the
claim; the note's own timings are the performance record.
