# 03 — Baseline schema, entities, test databases, SeaORM spike

Read `00-shared-context.md` first. Tasks 01 and 02 have landed.

Python reference: `backend/app/models.py`, `db.py`, `migrate.py`, `batching.py`, `backend/tests/conftest.py`, `factories.py`, `test_db.py`.

## Goal

The database layer the later tasks build on, plus proof that SeaORM can express the hardest queries. If the spike finds a query the builder cannot express, that query uses a raw `Statement`. A spike result never changes the ORM decision.

## Deliverables

### Migration (`crates/flakeradar-db`)

- `migrations/0001_baseline.sql`, derived from `tests/parity/schema.sql`. Same tables, columns, types, defaults, constraint names and index names, including the partial indexes `ix_test_cases_issue_check` and `ix_jobs_issue_check`.
- A `sea-orm-migration` `Migrator` with that one migration, embedded in the binary with `include_str!`.
- `run_migrations(&db)`: takes `pg_advisory_xact_lock(726300001)` (the same key as Python's `MIGRATION_LOCK_KEY`), runs the check below, then migrates.
- **Old-database check:** if `alembic_version` exists, or any app table exists without `seaql_migrations`, return an error and change nothing. The message must say: this database was not created by FlakeRadar 2, FlakeRadar 2 needs a fresh database, and see the cutover notes in the README.

### Connections

- `connect(url, settings)`:
  - Accepts `postgresql://`, `postgres://` and `postgresql+asyncpg://` (strip the suffix). Anything else is an error using Python's text: `FLAKERADAR_DATABASE_URL must be a PostgreSQL URL`.
  - Sets `statement_timeout` from `db_command_timeout_seconds` on every pooled connection.
  - Checks connections before use, matching `pool_pre_ping`.
- `WORKER_LOCK_KEY = 726300002` and the migration key as constants.
- `chunks` with `CHUNK = 1000`.

### Entities

Generate with a pinned `sea-orm-cli generate entity` into `crates/flakeradar-db/src/entities/`, then commit them. Add a CI step that regenerates against a migrated database and runs `git diff --exit-code`. No ORM relations are needed. Python uses explicit joins, and so do we. Add column-name doc comments only where `CONTEXT.md` uses a different word (`confirmed_flake_count` is a Proven flake count, `test_cases` holds Tests).

### Test support (`crates/flakeradar-test-support`)

Nextest runs each test in its own process, so a per-process container would start hundreds of containers. Instead:

- `ensure_postgres()` returns a server URL. Use `FLAKERADAR_TEST_DATABASE_URL` if it is set. Otherwise start one reused, named `postgres:17-alpine` container using `testcontainers`, guarded by a file lock against concurrent test processes. If `testcontainers` cannot reuse a container reliably, call the `docker` CLI directly.
- `TestDb::new().await`: under an advisory lock, creates a `flakeradar_template_<hash of baseline>` database if it is missing and migrates it. Then runs `CREATE DATABASE t_<uuid> TEMPLATE ...`, returns a pool (at most 5 connections) to it, and drops the database on drop, best effort.
- The env var must also work with a plain superuser server, because Fabro's `fabro-pg-ensure` provides a `test` superuser and no Docker.
- Port `backend/tests/factories.py` as builders with the same defaults: `make_project`, `make_test_case`, `make_run`, `make_execution`, `make_pipeline`, `make_job`, `make_job_execution`, `make_test_score`, `make_job_score`. Port `make_junit` from `conftest.py`.

### SeaORM spike

Prove these against a real database, as tests in `crates/flakeradar-db`. Write a short result for each in `spike-notes.md`: builder or raw `Statement`, and why.

1. `claim_next_report`: `FOR UPDATE SKIP LOCKED`
2. `test_execution_window_ids` and `job_execution_window_ids` in `attribution.py`: `LATERAL ... ORDER BY ... LIMIT`
3. `past_scores`: `row_number()` window
4. `newest_history_before` joined to a filtered, counted, paged leaderboard
5. `upsert_history`: `ON CONFLICT DO UPDATE` with `excluded` and an addition expression
6. A bulk update by primary key with per-row values and optional columns, as one statement per chunk (`UPDATE ... FROM unnest(...)` is acceptable)
7. `_delete_history_batch`: delete with a tuple `IN (SELECT ... LIMIT)`
8. Advisory lock on one dedicated connection that is never returned to the pool

Prefer the builder where it is readable. Use a raw `Statement` for anything else, with bound parameters only. Never build SQL from input strings.

## Tests to port

`test_db.py` (16). `compare_metadata` has no equivalent: replace it with a test that compares `information_schema` and `pg_indexes` of the migrated database against `tests/parity/schema.sql`, and record that in the manifest as the port. Add tests for: the old-database refusal in each case, that refusal changes nothing, a second run migrating nothing, concurrent `run_migrations` calls, each URL prefix, and that `statement_timeout` is applied.

## Done when

- [ ] All verification in `00-shared-context.md` passes, with the `--no-tests=pass` flag removed.
- [ ] `spike-notes.md` covers all 8 queries.
- [ ] Manifest rows for this task are not `pending`, and gaps are logged.
- [ ] `STATUS.md` is updated, and the branch is pushed.
