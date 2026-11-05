# Follow-on coverage issues

Task 15 drafts one issue per row left in `coverage-gaps.md`, so the gap list does
not become a permanent exception nobody revisits. **The user files these after
the cutover PR merges**, because they must not carry the `agent` label until the
Rust backend is on `main`.

Each draft names the file, the function, why the line or branch is not covered,
and what would cover it. Three groups are not real work and should be filed with
the `coverage-wontfix` label if they are filed at all: the `coverage(off)`
process edges, the test-only `io::Write::flush` bodies, and the `llvm-cov`
measurement artifacts (code that runs, whose inlined body the tool attributes to
its caller). Everything else is real: either a missing seam or an arm that
should be deleted.

Labels for every draft: `coverage`, and `flakeradar` only if you also want them
in the scout's view.

## 1. coverage: cover or remove flakeradar/src/main.rs — main

**File:** `crates/flakeradar/src/main.rs` **Function:** `main` **Kind:** `coverage(off)`

**Why it is not covered:** Process entry point: a test process cannot run it. Task 13 made it parse the command line and dispatch; `Cli`, `Cli::action` and the two subcommands are measured by `crates/flakeradar/tests/cli.rs`, and the `serve`, `health_check` and `runtime` rows below say what else it calls.

**What would cover it:** Nothing a test can do: the process edge (the real runtime, the environment, signal delivery) is not reachable from a test process. It stays excluded unless someone extracts the wiring further behind an injected seam, at which point the exclusion narrows.

**Labels:** coverage

## 2. coverage: cover or remove flakeradar-test-support/src/postgres.rs — lock, ensure_container, container_status, host_port, wait_until_rea…

**File:** `crates/flakeradar-test-support/src/postgres.rs` **Function:** `lock`, `ensure_container`, `container_status`, `host_port`, `wait_until_ready`, `run_docker` **Kind:** unreached

**Why it is not covered:** They run only when `FLAKERADAR_TEST_DATABASE_URL` is unset, and CI sets it for the whole suite, so the coverage job never takes the Docker path. The path works: three cold-start runs (`docker rm -f flakeradar-test-postgres` first) ran the whole suite with no failure.

**What would cover it:** A coverage job without `FLAKERADAR_TEST_DATABASE_URL` (Docker available), which is the configuration this file's container path runs in. Until then the path is exercised by hand.

**Labels:** coverage

## 3. coverage: cover or remove flakeradar-test-support/src/postgres.rs — ensure_postgres

**File:** `crates/flakeradar-test-support/src/postgres.rs` **Function:** `ensure_postgres` **Kind:** unreached branch

**Why it is not covered:** The same reason. The `FLAKERADAR_TEST_DATABASE_URL` branch is the one CI takes, so the container-start branch and its error paths are unmeasured.

**What would cover it:** A coverage job without `FLAKERADAR_TEST_DATABASE_URL` (Docker available), which is the configuration this file's container path runs in. Until then the path is exercised by hand.

**Labels:** coverage

## 4. coverage: cover or remove flakeradar-db/src/migration.rs — table_exists

**File:** `crates/flakeradar-db/src/migration.rs` **Function:** `table_exists` **Kind:** unreached branch

**Why it is not covered:** The `record not found` arm of the `SELECT EXISTS (…)` lookup. That query always returns exactly one row, so the arm cannot run.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 5. coverage: cover or remove flakeradar-test-support/src/db.rs — Drop for TestDb

**File:** `crates/flakeradar-test-support/src/db.rs` **Function:** `Drop for TestDb` **Kind:** unreached branch

**Why it is not covered:** Two error paths of the cleanup: the runtime failing to build, and the cleanup connection failing to open. Both happen only when a process cannot start a thread or reach a server it has just used, and neither can be provoked without breaking the test that runs them.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 6. coverage: cover or remove flakeradar-test-support/src/db.rs — TestDb::new

**File:** `crates/flakeradar-test-support/src/db.rs` **Function:** `TestDb::new` **Kind:** state-dependent branch

**Why it is not covered:** The arm that builds a missing template runs only on a server that has none. The coverage job's `postgres` service starts empty, so CI takes it (measured: db.rs is 98.68% lines and misses only the two `Drop` arms above with a fresh server); a re-run against a server whose template already exists reports the arm as missed. The build itself is covered deterministically, against a throwaway template name, by `a_built_template_is_published_with_the_baseline`.

**What would cover it:** A second coverage run with `FLAKERADAR_TEST_DATABASE_URL` unset (the Docker path), on a runner where the template database does not exist yet. That would measure the arm; the build itself is already covered deterministically by a throwaway template name.

**Labels:** coverage

## 7. coverage: cover or remove flakeradar-core/src/classify.rs — build_rules

**File:** `crates/flakeradar-core/src/classify.rs` **Function:** `build_rules` **Kind:** unreached arm

**Why it is not covered:** The `panic!` for a pattern that does not compile. Every pattern is a literal in the same file and all of them compile, so the arm cannot run; a rule that stopped compiling fails the build's own tests instead.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 8. coverage: cover or remove flakeradar-core/src/parsing.rs — nextest_marker

**File:** `crates/flakeradar-core/src/parsing.rs` **Function:** `nextest_marker` **Kind:** unreached arm

**Why it is not covered:** The `panic!` for the nextest marker pattern that does not compile. The pattern is a literal in the same file, so the arm cannot run.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 9. coverage: cover or remove flakeradar-db/src/identity.rs — get_or_create_repo, get_or_create_project

**File:** `crates/flakeradar-db/src/identity.rs` **Function:** `get_or_create_repo`, `get_or_create_project` **Kind:** unreached arms

**Why it is not covered:** The error arm of each read-back `SELECT` after an insert, and the `ok_or_else` closure for a row that vanished between its insert and its read-back. They need the connection to fail, or a concurrent `DELETE`, between two statements of the same call; the insert and select error arms are covered by `a_blocked_insert_surfaces_as_a_database_error` and `a_closed_connection_surfaces_as_a_database_error`.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 10. coverage: cover or remove flakeradar/src/main.rs — serve (named run until task 13)

**File:** `crates/flakeradar/src/main.rs` **Function:** `serve` (named `run` until task 13) **Kind:** `coverage(off)`

**Why it is not covered:** Process start-up: reads the environment, gates the token, builds the Tokio runtime and wires Ctrl-C. Everything it calls is `Server::start` and `Server::serve`, which `crates/flakeradar/tests/server.rs` drives over a real socket on a database with no schema; the token gate is `assert_secure_token`, covered by the settings unit tests.

**What would cover it:** Nothing a test can do: the process edge (the real runtime, the environment, signal delivery) is not reachable from a test process. It stays excluded unless someone extracts the wiring further behind an injected seam, at which point the exclusion narrows.

**Labels:** coverage

## 11. coverage: cover or remove flakeradar/src/main.rs — shutdown_signal

**File:** `crates/flakeradar/src/main.rs` **Function:** `shutdown_signal` **Kind:** `coverage(off)`

**Why it is not covered:** Signal wiring: Ctrl-C and SIGTERM delivery is a process-level event a test cannot raise against itself without ending the run. The shutdown it resolves is measured: `crates/flakeradar/tests/server.rs` passes a future of its own to `Server::serve` and asserts that a request in flight finishes, and `scripts/smoke.sh` stops a real binary with SIGTERM and checks a zero exit.

**What would cover it:** Nothing a test can do: the process edge (the real runtime, the environment, signal delivery) is not reachable from a test process. It stays excluded unless someone extracts the wiring further behind an injected seam, at which point the exclusion narrows.

**Labels:** coverage

## 12. coverage: cover or remove flakeradar/src/telemetry.rs — init_tracing

**File:** `crates/flakeradar/src/telemetry.rs` **Function:** `init_tracing` **Kind:** `coverage(off)`

**Why it is not covered:** It installs the process-wide `tracing` subscriber, which a test cannot install twice and another thread cannot observe.

**What would cover it:** Nothing a test can do: the process edge (the real runtime, the environment, signal delivery) is not reachable from a test process. It stays excluded unless someone extracts the wiring further behind an injected seam, at which point the exclusion narrows.

**Labels:** coverage

## 13. coverage: cover or remove flakeradar/src/settings.rs — Settings::from_env

**File:** `crates/flakeradar/src/settings.rs` **Function:** `Settings::from_env` **Kind:** unreached

**Why it is not covered:** It reads the process environment and `.env`, and only the binary calls it. `from_map` and `environment_values` are covered directly, including a malformed `.env` file and the precedence of the environment over the file.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 14. coverage: cover or remove flakeradar/src/settings.rs — environment_values

**File:** `crates/flakeradar/src/settings.rs` **Function:** `environment_values` **Kind:** unreached arm

**Why it is not covered:** `dotenvy::from_path_iter`'s own error mapping. Only a file that cannot be opened at all reaches it (wrong permissions, a directory in its place), which a test process would have to run as another user to stage.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 15. coverage: cover or remove flakeradar/src/settings.rs — test a_bad_value_names_the_variable_it_came_from

**File:** `crates/flakeradar/src/settings.rs` **Function:** test `a_bad_value_names_the_variable_it_came_from` **Kind:** test-only arm

**Why it is not covered:** The `panic!` for an error kind a settings fixture cannot produce: it is an assertion in a test, not production code.

**What would cover it:** None in production: the body is required by `io::Write`, not called. Close it by replacing the helper with a log capture that implements the trait differently (or by a `tracing` test subscriber), and log the removal here.

**Labels:** coverage

## 16. coverage: cover or remove flakeradar/src/extract.rs — json_error_position

**File:** `crates/flakeradar/src/extract.rs` **Function:** `json_error_position` **Kind:** unreached arms

**Why it is not covered:** The `break` for a `serde_json` error claiming a line the input does not hold, and the `map_or` fallback for an offset that is not a character boundary. `serde_json` reports neither for the text it parsed, so no request reaches them; the line, column and multi-byte cases are pinned by `an_error_position_counts_characters_from_the_start`.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 17. coverage: cover or remove flakeradar/src/extract.rs — issue_for

**File:** `crates/flakeradar/src/extract.rs` **Function:** `issue_for` **Kind:** unreached arms

**Why it is not covered:** Two parts of the `loc` mapping: the `Segment::Seq`, `Segment::Enum` and `Segment::Unknown` arms, because no DTO in the suite fails on an element inside a list or on an enum variant, and the `kind == "missing"` arm with no field name, because the same `serde` message that makes the kind `missing` also names the field.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 18. coverage: cover or remove flakeradar/src/extract.rs — path_error

**File:** `crates/flakeradar/src/extract.rs` **Function:** `path_error` **Kind:** unreached arm

**Why it is not covered:** The `let … else` that answers 404 for a `PathRejection` that is not a deserialization failure, which is `MissingPathParams`: Axum supplies the parameters of every route a handler is mounted on, so only a handler run outside its route's match reaches it. Every `ErrorKind` arm is covered, including `ParseError` — a path with exactly one parameter and no key to name — which `tests/extractors.rs::a_single_path_parameter_of_the_wrong_type_names_the_path_only` pins through `loc: ["path"]`, a loc only that arm produces.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 19. coverage: cover or remove flakeradar/src/extract.rs — body_error

**File:** `crates/flakeradar/src/extract.rs` **Function:** `body_error` **Kind:** unreached arm

**Why it is not covered:** The 500 for a body that failed to buffer for a reason other than its length. The 413 arm is covered by `a_json_body_over_the_limit_is_refused`; the 500 arm is now covered too, by task 06's `pipeline_ingest_answers_a_body_that_fails_while_it_is_read`, which sends a body whose stream errors with `Body::from_stream`.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 20. coverage: cover or remove flakeradar/src/routes/ops.rs — queue_facts

**File:** `crates/flakeradar/src/routes/ops.rs` **Function:** `queue_facts` **Kind:** unreached arm

**Why it is not covered:** The closure for a queue query that returned no row. The query has no `FROM` clause, so it always returns exactly one.

**What would cover it:** A seam that lets a test substitute the statement's answer: these are aggregates with no `GROUP BY` that PostgreSQL always answers with exactly one row. Without one, deleting the arm and stating the invariant is the honest option.

**Labels:** coverage

## 21. coverage: cover or remove flakeradar/src/routes/ops.rs — last_processed

**File:** `crates/flakeradar/src/routes/ops.rs` **Function:** `last_processed` **Kind:** unreached arm

**Why it is not covered:** The `None` arm, for a scrape of a database with no `reports` row at all. The aggregate query still returns its one row, and the `NULL` it holds is the covered `Some` path.

**What would cover it:** A seam that lets a test substitute the statement's answer: these are aggregates with no `GROUP BY` that PostgreSQL always answers with exactly one row. Without one, deleting the arm and stating the invariant is the honest option.

**Labels:** coverage

## 22. coverage: cover or remove flakeradar/src/routes/ops.rs — python_float

**File:** `crates/flakeradar/src/routes/ops.rs` **Function:** `python_float` **Kind:** unreached branches

**Why it is not covered:** Its three guards: `is_finite()`, and the `e` and `E` in the formatted text. Every value the scrape prints is a finite number of seconds below the magnitude where either formatter switches to exponent notation, and Rust's `Display` never prints an uppercase `E`.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 23. coverage: cover or remove flakeradar/src/server.rs — Server::start, Server::serve

**File:** `crates/flakeradar/src/server.rs` **Function:** `Server::start`, `Server::serve` **Kind:** unreached arms

**Why it is not covered:** `local_addr` failing on a socket that the same call has just bound, and the warning when closing the pool at shutdown fails.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 24. coverage: cover or remove flakeradar/src/routes/reports.rs — parse_upload, report_body

**File:** `crates/flakeradar/src/routes/reports.rs` **Function:** `parse_upload`, `report_body` **Kind:** unreached arm

**Why it is not covered:** The `spawn_blocking` join error of each. Only a panic inside `parse_junit_xml` or inside the multipart scan reaches it, and both run on attacker-controlled CI input, which the code standards forbid from panicking; the arms exist so that a bug there is a 500 rather than a dropped request.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 25. coverage: cover or remove flakeradar/src/routes/reports.rs — summary

**File:** `crates/flakeradar/src/routes/reports.rs` **Function:** `summary` **Kind:** unreached arm

**Why it is not covered:** The `ok_or_else` for a summary query that returned no row. The query is one aggregate with no `GROUP BY`, so PostgreSQL always returns exactly one row; task 05's `queue_facts` row is the same shape.

**What would cover it:** A seam that lets a test substitute the statement's answer: these are aggregates with no `GROUP BY` that PostgreSQL always answers with exactly one row. Without one, deleting the arm and stating the invariant is the honest option.

**Labels:** coverage

## 26. coverage: cover or remove flakeradar-db/src/processing.rs — get

**File:** `crates/flakeradar-db/src/processing.rs` **Function:** `get` **Kind:** unreached arm

**Why it is not covered:** The `.map_err` for a driver error on a column read. Every column this file reads is named by its own statement, so the read can only fail if the schema changed under the query. The same shape as task 04's `identity.rs` read-back arms.

**What would cover it:** A driver failure injected mid-query (a proxy that drops the connection between statements, or a `DbConn` seam), which is what these `.map_err` arms need.

**Labels:** coverage

## 27. coverage: cover or remove flakeradar-db/src/processing.rs — process_report

**File:** `crates/flakeradar-db/src/processing.rs` **Function:** `process_report` **Kind:** unreached arm

**Why it is not covered:** `DataError: a JUnit Report has no Project`. The baseline schema's `ck_reports_junit_has_project` refuses such a row, so no Report can reach the guard: `a_junit_report_without_a_project_fails` cannot even queue one, and the insert fails with SQLSTATE 23514.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 28. coverage: cover or remove flakeradar-db/src/processing.rs — record_test_executions

**File:** `crates/flakeradar-db/src/processing.rs` **Function:** `record_test_executions` **Kind:** unreached arm

**Why it is not covered:** `DataError: Test {fingerprint} vanished`, the `ok_or_else` for an id the upsert in the same transaction has just returned. It needs a concurrent `DELETE` between two statements of one transaction, which a single-connection test cannot stage.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 29. coverage: cover or remove flakeradar-db/src/processing.rs — record_test_executions

**File:** `crates/flakeradar-db/src/processing.rs` **Function:** `record_test_executions` **Kind:** unreached arm (branch)

**Why it is not covered:** `DataError: Test bookkeeping is inconsistent`, the `get_mut` for an index `latest_by_id` never wrote. The index and the vector are pushed together two lines apart.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 30. coverage: cover or remove flakeradar-db/src/processing.rs — apply_default_branch

**File:** `crates/flakeradar-db/src/processing.rs` **Function:** `apply_default_branch` **Kind:** unreached arm (branch)

**Why it is not covered:** The `else` for a Repo that no row matches. `reports.repo_id` carries a foreign key to `repos` with `ON DELETE CASCADE`, so a Report whose Repo is gone cannot exist, and `a_report_without_a_default_branch_never_clears_one` covers the `None` value arm above it.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 31. coverage: cover or remove flakeradar-db/src/processing.rs — upsert_pipeline_jobs

**File:** `crates/flakeradar-db/src/processing.rs` **Function:** `upsert_pipeline_jobs` **Kind:** unreached arm

**Why it is not covered:** `DataError: the Pipeline vanished`, the read-back after the `ON CONFLICT` upsert in the same transaction. Same shape as the Test arm above.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 32. coverage: cover or remove flakeradar-db/src/processing.rs — insert_pipeline_job_executions

**File:** `crates/flakeradar-db/src/processing.rs` **Function:** `insert_pipeline_job_executions` **Kind:** unreached arm

**Why it is not covered:** `DataError: Job {name} vanished`, the id lookup for a name `upsert_pipeline_jobs` has just inserted. Same shape.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 33. coverage: cover or remove flakeradar-db/src/processing.rs — insert_pipeline_job_executions

**File:** `crates/flakeradar-db/src/processing.rs` **Function:** `insert_pipeline_job_executions` **Kind:** unreached arm

**Why it is not covered:** `DataError: unknown Job status {other}`. The status comes back from rows this function just inserted from [`JobStatus`](flakeradar_core::JobStatus), so only a value the schema accepted and the enum does not know reaches it; `job_history`'s copy of that guard is covered by `a_job_execution_with_an_unknown_status_refuses_to_score`, which writes the row directly.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 34. coverage: cover or remove flakeradar-db/src/processing.rs — test_tier_changes

**File:** `crates/flakeradar-db/src/processing.rs` **Function:** `test_tier_changes` **Kind:** unreached arm (branch)

**Why it is not covered:** The `continue` for a Test deleted between the score write and the naming read in the same transaction. A single-connection test cannot delete between them both.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 35. coverage: cover or remove flakeradar-db/src/processing.rs — job_tier_changes

**File:** `crates/flakeradar-db/src/processing.rs` **Function:** `job_tier_changes` **Kind:** unreached arm (branch)

**Why it is not covered:** The same `continue` for a Job, with the same reason.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 36. coverage: cover or remove flakeradar-db/src/attribution.rs — get

**File:** `crates/flakeradar-db/src/attribution.rs` **Function:** `get` **Kind:** unreached arm

**Why it is not covered:** The `.map_err` for a driver error on a column read, the same shape as `processing.rs`'s `get`.

**What would cover it:** A driver failure injected mid-query (a proxy that drops the connection between statements, or a `DbConn` seam), which is what these `.map_err` arms need.

**Labels:** coverage

## 37. coverage: cover or remove flakeradar-db/src/score_history.rs — get

**File:** `crates/flakeradar-db/src/score_history.rs` **Function:** `get` **Kind:** unreached arm

**Why it is not covered:** The `.map_err` for a driver error on a column read, the same shape again.

**What would cover it:** A driver failure injected mid-query (a proxy that drops the connection between statements, or a `DbConn` seam), which is what these `.map_err` arms need.

**Labels:** coverage

## 38. coverage: cover or remove flakeradar/src/worker.rs — AdvisoryLock::try_lock, AdvisoryLock::check

**File:** `crates/flakeradar/src/worker.rs` **Function:** `AdvisoryLock::try_lock`, `AdvisoryLock::check` **Kind:** unreached arm

**Why it is not covered:** The `None` for a lock query that answered no row, and for a liveness query that answered no row. `SELECT pg_try_advisory_lock($1) AS locked, pg_backend_pid() AS backend_pid` and `SELECT pg_backend_pid() = $1 AS same_session` have no `FROM` clause, so PostgreSQL always answers exactly one row; the arms mean "not locked" and "cannot prove the lock" rather than a panic, for a driver that ever answers none. They are the file's only missed lines (279-280) and its only missed branch.

**What would cover it:** A seam that lets a test substitute the statement's answer: these are aggregates with no `GROUP BY` that PostgreSQL always answers with exactly one row. Without one, deleting the arm and stating the invariant is the honest option.

**Labels:** coverage

## 39. coverage: cover or remove flakeradar/tests/app.rs — test helper SharedWriter's flush

**File:** `crates/flakeradar/tests/app.rs` **Function:** test helper `SharedWriter`'s `flush` **Kind:** test-only arm

**Why it is not covered:** The same reason as `settings.rs`'s `Sink::flush` below: `tracing_subscriber`'s fmt layer writes each event to a fresh writer from `MakeWriter` and never flushes it, so the body cannot run. `io::Write` requires the method. The `SharedWriter`'s other lines are covered by every log assertion in this file.

**What would cover it:** None in production: the body is required by `io::Write`, not called. Close it by replacing the helper with a log capture that implements the trait differently (or by a `tracing` test subscriber), and log the removal here.

**Labels:** coverage

## 40. coverage: cover or remove flakeradar/src/settings.rs — test helper captured_log's Sink::flush

**File:** `crates/flakeradar/src/settings.rs` **Function:** test helper `captured_log`'s `Sink::flush` **Kind:** test-only arm

**Why it is not covered:** `tracing_subscriber`'s fmt layer writes each event to a fresh writer from `MakeWriter` and never flushes it, so the trait method's body cannot run; it exists because `io::Write` requires it. The `Sink`'s other lines are covered by `the_default_token_is_allowed_when_explicitly_opted_in`.

**What would cover it:** None in production: the body is required by `io::Write`, not called. Close it by replacing the helper with a log capture that implements the trait differently (or by a `tracing` test subscriber), and log the removal here.

**Labels:** coverage

## 41. coverage: cover or remove flakeradar/tests/worker.rs — test helper SharedWriter's flush

**File:** `crates/flakeradar/tests/worker.rs` **Function:** test helper `SharedWriter`'s `flush` **Kind:** test-only arm

**Why it is not covered:** The same reason as `settings.rs`'s `Sink::flush` above: the fmt layer never flushes a `MakeWriter`'s writer, so the body cannot run. `io::Write` requires the method. The `SharedWriter`'s other lines are covered by every log assertion in this file.

**What would cover it:** None in production: the body is required by `io::Write`, not called. Close it by replacing the helper with a log capture that implements the trait differently (or by a `tracing` test subscriber), and log the removal here.

**Labels:** coverage

## 42. coverage: cover or remove flakeradar-db/tests/retention.rs — test helper SharedWriter's flush

**File:** `crates/flakeradar-db/tests/retention.rs` **Function:** test helper `SharedWriter`'s `flush` **Kind:** test-only arm

**Why it is not covered:** The same reason as `settings.rs`'s `Sink::flush` above: the fmt layer never flushes a `MakeWriter`'s writer, so the body cannot run. `io::Write` requires the method. The `SharedWriter`'s other lines are covered by the budget-stop log assertion.

**What would cover it:** None in production: the body is required by `io::Write`, not called. Close it by replacing the helper with a log capture that implements the trait differently (or by a `tracing` test subscriber), and log the removal here.

**Labels:** coverage

## 43. coverage: cover or remove flakeradar/src/server.rs — Server::serve

**File:** `crates/flakeradar/src/server.rs` **Function:** `Server::serve` **Kind:** unreached arm

**Why it is not covered:** The `warn!` for a pool close that fails at shutdown. `close()` only errors on a connection it cannot end, and the process is on its way out either way; the `local_addr` arm on the same row is task 05's. Re-measured in task 08, unchanged.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 44. coverage: cover or remove flakeradar-db/src/reads.rs — get

**File:** `crates/flakeradar-db/src/reads.rs` **Function:** `get` **Kind:** unreached arm

**Why it is not covered:** The `.map_err` for a driver error on a column read. Every column the reads name is selected by the same statement, so a read fails only if the schema changed under the query. The same shape as task 07's `processing.rs` row.

**What would cover it:** A driver failure injected mid-query (a proxy that drops the connection between statements, or a `DbConn` seam), which is what these `.map_err` arms need.

**Labels:** coverage

## 45. coverage: cover or remove flakeradar-db/src/reads.rs — Board::run

**File:** `crates/flakeradar-db/src/reads.rs` **Function:** `Board::run` **Kind:** unreached arm

**Why it is not covered:** The `None` fallback for the total count. `SELECT count(*) FROM (…)` has no `GROUP BY`, so PostgreSQL always answers exactly one row.

**What would cover it:** A seam that lets a test substitute the statement's answer: these are aggregates with no `GROUP BY` that PostgreSQL always answers with exactly one row. Without one, deleting the arm and stating the invariant is the honest option.

**Labels:** coverage

## 46. coverage: cover or remove flakeradar-db/src/reads.rs — test_counts, new_flaky_count, window_sums, job_summary

**File:** `crates/flakeradar-db/src/reads.rs` **Function:** `test_counts`, `new_flaky_count`, `window_sums`, `job_summary` **Kind:** unreached arms

**Why it is not covered:** Each `ok_or_else` for an aggregate that answered no row. Every one of those statements is an aggregate over a table with no `GROUP BY`, so it returns exactly one row; the same shape as task 05's `queue_facts` row.

**What would cover it:** A seam that lets a test substitute the statement's answer: these are aggregates with no `GROUP BY` that PostgreSQL always answers with exactly one row. Without one, deleting the arm and stating the invariant is the honest option.

**Labels:** coverage

## 47. coverage: cover or remove flakeradar-db/src/reads.rs — list_tests, list_jobs, search_tests, search_jobs, get_job, ci_run_f…

**File:** `crates/flakeradar-db/src/reads.rs` **Function:** `list_tests`, `list_jobs`, `search_tests`, `search_jobs`, `get_job`, `ci_run_failed_jobs` **Kind:** unreached arm

**Why it is not covered:** The `?` on collecting the `id` column of the page or search rows. The column is selected by the same statement and read as the type the schema declares, so only a schema change makes it fail.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 48. coverage: cover or remove flakeradar-db/src/reads.rs — category_counts

**File:** `crates/flakeradar-db/src/reads.rs` **Function:** `category_counts` **Kind:** unreached arm

**Why it is not covered:** The `?` on reading the count for a category in `FailureCategory::ALL`; a row's category and count are selected by one statement, so only a schema change breaks the read.

**What would cover it:** A seam that lets a test substitute the statement's answer: these are aggregates with no `GROUP BY` that PostgreSQL always answers with exactly one row. Without one, deleting the arm and stating the invariant is the honest option.

**Labels:** coverage

## 49. coverage: cover or remove flakeradar-db/src/reads.rs — explaining_tests

**File:** `crates/flakeradar-db/src/reads.rs` **Function:** `explaining_tests` **Kind:** unreached arms

**Why it is not covered:** Two `continue`s for a row the map cannot place: a `ci_job_id` absent from `failed_ci_to_exec` and an execution id absent from `by_exec`. The query filters `ci_job_id IN` the map's keys, and the map is built from the same map's values, so neither can run.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 50. coverage: cover or remove flakeradar-db/src/reads.rs — ci_run_report_counts

**File:** `crates/flakeradar-db/src/reads.rs` **Function:** `ci_run_report_counts` **Kind:** unreached arm

**Why it is not covered:** The `_ => {}` for a Report status outside `pending`, `processed` and `failed`. `ck_reports_status` allows only those three, so no row reaches it.

**What would cover it:** A seam that lets a test substitute the statement's answer: these are aggregates with no `GROUP BY` that PostgreSQL always answers with exactly one row. Without one, deleting the arm and stating the invariant is the honest option.

**Labels:** coverage

## 51. coverage: cover or remove flakeradar-db/src/reads.rs — ci_run_test_failures

**File:** `crates/flakeradar-db/src/reads.rs` **Function:** `ci_run_test_failures` **Kind:** unreached arm

**Why it is not covered:** The `ok_or_else` for the pair-count aggregate that answered no row, the same shape as the summaries above.

**What would cover it:** A seam that lets a test substitute the statement's answer: these are aggregates with no `GROUP BY` that PostgreSQL always answers with exactly one row. Without one, deleting the arm and stating the invariant is the honest option.

**Labels:** coverage

## 52. coverage: cover or remove flakeradar-db/src/reads.rs — ci_run_failure_item

**File:** `crates/flakeradar-db/src/reads.rs` **Function:** `ci_run_failure_item` **Kind:** unreached arms

**Why it is not covered:** The two `return Ok(None)` arms for a pair with no Execution, or none failing. The pair was counted as failing in the same listing, so both need a concurrent `DELETE` between the count and the execution read.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 53. coverage: cover or remove flakeradar/src/routes/reads.rs — From<DbError>, From<DbErr>, From<ReadError> for ReadRouteError

**File:** `crates/flakeradar/src/routes/reads.rs` **Function:** `From<DbError>`, `From<DbErr>`, `From<ReadError> for ReadRouteError` **Kind:** measurement artifact

**Why it is not covered:** Every read test executes these conversions (a failed query reaches the 500 path, and an unknown CI run reaches the 404 path), but `llvm-cov` attributes the inlined conversion bodies to their callers, so the `impl` lines themselves report zero. The 500 and 404 bodies are covered by `a_failed_read_query_answers_the_plain_text_500` and the unknown-repo/run tests.

**What would cover it:** Nothing: the code runs and `llvm-cov` attributes the inlined body to its caller. The row is here so the figure is explained; the option is to inline the wrapper by hand, which buys a nicer number and no behaviour.

**Labels:** coverage

## 54. coverage: cover or remove flakeradar/src/routes/reads.rs — category_expected

**File:** `crates/flakeradar/src/routes/reads.rs` **Function:** `category_expected` **Kind:** unreached arm

**Why it is not covered:** The `None` arm of `split_last`. `FailureCategory::ALL` holds five variants, so the list is never empty.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 55. coverage: cover or remove flakeradar-core/src/dto.rs — timestamp::deserialize, optional_timestamp::deserialize, lax_bool::…

**File:** `crates/flakeradar-core/src/dto.rs` **Function:** `timestamp::deserialize`, `optional_timestamp::deserialize`, `lax_bool::deserialize`, `lax_bool::optional` **Kind:** unreached arm

**Why it is not covered:** The `?` that would propagate a failure of `String::deserialize`, `Option::<String>::deserialize` or `Value::deserialize`. Every caller reads a field of a body `serde_json` has already decoded or of a query string, and both sinks answer every one of those methods, so the read cannot fail; a format that refused one (a binary one) would reach the arm, and no route uses one. The decoding and rejecting each reader does afterwards is measured, including `a_bad_timestamp_is_a_deserialization_error` and the lax-bool tables.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 56. coverage: cover or remove flakeradar-core/src/dto.rs — lax_bool::interpret_number

**File:** `crates/flakeradar-core/src/dto.rs` **Function:** `lax_bool::interpret_number` **Kind:** unreached arm

**Why it is not covered:** The `else` of `let Some(float) = number.as_f64()`. `serde_json::Number` holds a `u64`, an `i64` or an `f64`, and `as_f64` answers all three, so the arm needs a number shape the crate does not build; `serde_json`'s `arbitrary_precision` feature is what adds one, and no member enables it.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 57. coverage: cover or remove flakeradar-db/src/deletes.rs — From<DbErr> for DeleteError

**File:** `crates/flakeradar-db/src/deletes.rs` **Function:** `From<DbErr> for DeleteError` **Kind:** measurement artifact

**Why it is not covered:** The bare-`DbErr` `?` sites do run this conversion (`transaction.rollback().await?` is one), but `llvm-cov` attributes the inlined body to its caller, so the `impl` lines report zero. The same shape as task 09's row for `routes/reads.rs`.

**What would cover it:** Nothing: the code runs and `llvm-cov` attributes the inlined body to its caller. The row is here so the figure is explained; the option is to inline the wrapper by hand, which buys a nicer number and no behaviour.

**Labels:** coverage

## 58. coverage: cover or remove flakeradar-db/src/deletes.rs — count

**File:** `crates/flakeradar-db/src/deletes.rs` **Function:** `count` **Kind:** unreached arm

**Why it is not covered:** `return Ok(0)` for a `SELECT count(*)` that answered no row. The statement has no `GROUP BY`, so PostgreSQL always answers exactly one row.

**What would cover it:** A seam that lets a test substitute the statement's answer: these are aggregates with no `GROUP BY` that PostgreSQL always answers with exactly one row. Without one, deleting the arm and stating the invariant is the honest option.

**Labels:** coverage

## 59. coverage: cover or remove flakeradar-db/src/deletes.rs — issue_numbers

**File:** `crates/flakeradar-db/src/deletes.rs` **Function:** `issue_numbers` **Kind:** unreached arm

**Why it is not covered:** The `continue` for `one_many` answering `None`, which it never does: it builds `Some` out of the rows `query_all_raw` returned.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 60. coverage: cover or remove flakeradar-db/src/issue_links.rs — From<sea_orm::DbErr> for LinkError

**File:** `crates/flakeradar-db/src/issue_links.rs` **Function:** `From<sea_orm::DbErr> for LinkError` **Kind:** measurement artifact

**Why it is not covered:** Inlined into its `?` sites, the same shape as the `deletes.rs` row above.

**What would cover it:** Nothing: the code runs and `llvm-cov` attributes the inlined body to its caller. The row is here so the figure is explained; the option is to inline the wrapper by hand, which buys a nicer number and no behaviour.

**Labels:** coverage

## 61. coverage: cover or remove flakeradar-db/src/issue_links.rs — link_issue, unlink_issue

**File:** `crates/flakeradar-db/src/issue_links.rs` **Function:** `link_issue`, `unlink_issue` **Kind:** measurement artifact

**Why it is not covered:** The field-argument lines of the two `tracing::info!` events. `tracing` evaluates those arguments lazily inside the macro, and `llvm-cov` counts them zero although the events fire; each event's own statement line is covered.

**What would cover it:** Nothing: the code runs and `llvm-cov` attributes the inlined body to its caller. The row is here so the figure is explained; the option is to inline the wrapper by hand, which buys a nicer number and no behaviour.

**Labels:** coverage

## 62. coverage: cover or remove flakeradar/src/routes/issues.rs — From<ApiError>, From<DbError>, From<DbErr>, From<IssueLinkError>, t…

**File:** `crates/flakeradar/src/routes/issues.rs` **Function:** `From<ApiError>`, `From<DbError>`, `From<DbErr>`, `From<IssueLinkError>`, the `IssueRouteError::Rejected` response arm **Kind:** dead conversion

**Why it is not covered:** No caller: `ApiPath`, `ApiQuery` and `LinkBody` answer their own rejection before a handler returns, and the two engines fail with `LinkError`, whose `From<LinkError>` is the conversion the routes do use. `Rejected` is never constructed; the `LinkError::Database` arm and the 500 body are covered by `a_failed_link_query_answers_the_plain_text_500`.

**What would cover it:** Delete the conversion: no caller exists, so removing it takes the unreachable code out of the tree instead of measuring it. It was kept to keep the error type uniform.

**Labels:** coverage

## 63. coverage: cover or remove flakeradar/src/routes/issues.rs — link_test, unlink_test, link_job, unlink_job

**File:** `crates/flakeradar/src/routes/issues.rs` **Function:** `link_test`, `unlink_test`, `link_job`, `unlink_job` **Kind:** unreachable arms

**Why it is not covered:** The four `LinkedOut` mismatch arms (200-202, 235-237, 282-284, 317-319). `link_issue` and `unlink_issue` build the variant from the target they are handed, and each route passes one target, so a Test route cannot get a `LinkedOut::Job` back (or the reverse).

**What would cover it:** Delete the arm, or prove the invariant with a comment and a debug assertion instead of a branch: the invariant (a schema literal, a variant built from its own target) is what makes it unreachable.

**Labels:** coverage

## 64. coverage: cover or remove flakeradar/src/routes/issues.rs — IssueRouteError::into_response

**File:** `crates/flakeradar/src/routes/issues.rs` **Function:** `IssueRouteError::into_response` **Kind:** unreachable arm

**Why it is not covered:** `StatusCode::from_u16(error.status()).unwrap_or(StatusCode::UNPROCESSABLE_ENTITY)`: `IssueLinkError::status` answers 404, 409 or 422 only, so the fallback cannot be taken (line 78 is covered; the unreachable part is the closure's body).

**What would cover it:** Delete the arm, or prove the invariant with a comment and a debug assertion instead of a branch: the invariant (a schema literal, a variant built from its own target) is what makes it unreachable.

**Labels:** coverage

## 65. coverage: cover or remove flakeradar/src/routes/admin.rs — From<DbError>, From<DbErr> for AdminRouteError

**File:** `crates/flakeradar/src/routes/admin.rs` **Function:** `From<DbError>`, `From<DbErr> for AdminRouteError` **Kind:** dead conversion

**Why it is not covered:** No caller: every delete fails with `DeleteError`, which has its own conversion (covered by the deadlock and 503 tests), and the extractors answer themselves.

**What would cover it:** Delete the conversion: no caller exists, so removing it takes the unreachable code out of the tree instead of measuring it. It was kept to keep the error type uniform.

**Labels:** coverage

## 66. coverage: cover or remove flakeradar/src/github.rs — get

**File:** `crates/flakeradar/src/github.rs` **Function:** `get` **Kind:** unreached arm

**Why it is not covered:** The `.map_err` on a column read. Every column is named by the statement that selected it, so only a schema change fails it; the same shape as task 07's `processing.rs` row.

**What would cover it:** A driver failure injected mid-query (a proxy that drops the connection between statements, or a `DbConn` seam), which is what these `.map_err` arms need.

**Labels:** coverage

## 67. coverage: cover or remove flakeradar/src/github.rs — store_link

**File:** `crates/flakeradar/src/github.rs` **Function:** `store_link` **Kind:** unreached arms

**Why it is not covered:** The savepoint's start, its link `UPDATE` and its commit each failing (1227-1228, 1243-1244, 1252-1253). Each needs the driver to fail between statements of a filing that is already in progress.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 68. coverage: cover or remove flakeradar/src/github.rs — sync_closed_issues

**File:** `crates/flakeradar/src/github.rs` **Function:** `sync_closed_issues` **Kind:** unreached arms

**Why it is not covered:** Failing to open its transaction, to stamp a row and to commit (1275-1276, 1302, 1311). A dropped table reaches the rotation-list read (`a_sync_whose_rotation_list_cannot_be_read_makes_no_request`), but these need the connection to fail after it.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 69. coverage: cover or remove flakeradar/src/github.rs — clear_if_closed

**File:** `crates/flakeradar/src/github.rs` **Function:** `clear_if_closed` **Kind:** unreached arms

**Why it is not covered:** The four row writes of the sync failing: the stamp inside its loop, keeping a `not_planned` link, re-arming one and clearing the flag (1342, 1377, 1391, 1399, 1413). Each needs a driver failure inside the sync's transaction.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 70. coverage: cover or remove flakeradar/src/github.rs — note_fixed_issues_for, note_fixed_job_issues_for

**File:** `crates/flakeradar/src/github.rs` **Function:** `note_fixed_issues_for`, `note_fixed_job_issues_for` **Kind:** unreached arms

**Why it is not covered:** The two candidate reads and the stamp write failing (1612, 1633, 1681-1682). Each read is one statement over a table the fixture has just written, so only a schema change fails it.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 71. coverage: cover or remove flakeradar/src/github.rs — every tracing::…! event

**File:** `crates/flakeradar/src/github.rs` **Function:** every `tracing::…!` event **Kind:** measurement artifact

**Why it is not covered:** The field-argument lines of the events in this file (1155-1157, 1167-1170, 1176-1180, 1185-1186, 1192-1195, 1202-1206, 1346-1349, 1357-1360, 1368-1372, 1381-1386, 1406-1410, 1441-1444, 1451-1452, 1462-1466, 1474-1477, 1484-1487, 1497-1501, 1690-1694, 1704-1705, 1711-1714, 1721-1725, 1737-1747, 1777-1780, 1786-1789, 1794-1796, 1800-1804). `tracing` evaluates them lazily inside the macro, so `llvm-cov` counts them zero although the event fires; every event's statement line is covered, and the log-capture tests in `tests/github.rs` pin the message text.

**What would cover it:** Nothing: the code runs and `llvm-cov` attributes the inlined body to its caller. The row is here so the figure is explained; the option is to inline the wrapper by hand, which buys a nicer number and no behaviour.

**Labels:** coverage

## 72. coverage: cover or remove flakeradar/src/github_worker.rs — with_monotonic

**File:** `crates/flakeradar/src/github_worker.rs` **Function:** `with_monotonic` **Kind:** unreachable arm

**Why it is not covered:** The `None` of `Arc::get_mut`: `with_monotonic` takes `self` and nothing clones a `GitHubWorker`, so the worker holds the only reference to its shared state and the arm cannot be taken.

**What would cover it:** Delete the arm, or prove the invariant with a comment and a debug assertion instead of a branch: the invariant (a schema literal, a variant built from its own target) is what makes it unreachable.

**Labels:** coverage

## 73. coverage: cover or remove flakeradar/src/webhooks.rs — post

**File:** `crates/flakeradar/src/webhooks.rs` **Function:** `post` **Kind:** measurement artifact

**Why it is not covered:** The field-argument lines of its three `tracing` events, for the reason the `github.rs` row gives: the warnings fire (the retry tests count them) and the argument lines still report zero.

**What would cover it:** Nothing: the code runs and `llvm-cov` attributes the inlined body to its caller. The row is here so the figure is explained; the option is to inline the wrapper by hand, which buys a nicer number and no behaviour.

**Labels:** coverage

## 74. coverage: cover or remove flakeradar/src/webhooks.rs — test a_delivery_that_raises_is_logged_and_the_task_keeps_serving

**File:** `crates/flakeradar/src/webhooks.rs` **Function:** test `a_delivery_that_raises_is_logged_and_the_task_keeps_serving` **Kind:** test-only arm

**Why it is not covered:** Its `Sink::flush` body, which `tracing_subscriber`'s fmt layer never calls (it writes to a fresh writer from `MakeWriter`); `io::Write` requires the method. Same reason as the `settings.rs` sink row.

**What would cover it:** None in production: the body is required by `io::Write`, not called. Close it by replacing the helper with a log capture that implements the trait differently (or by a `tracing` test subscriber), and log the removal here.

**Labels:** coverage

## 75. coverage: cover or remove flakeradar-core/src/notify.rs — test a_json_event_omits_the_url_without_a_public_url

**File:** `crates/flakeradar-core/src/notify.rs` **Function:** test `a_json_event_omits_the_url_without_a_public_url` **Kind:** test-only arm

**Why it is not covered:** The `return` of its `let Some(payload) = … else`, which cannot be reached because the line above asserts the list holds exactly one event.

**What would cover it:** None in production: the body is required by `io::Write`, not called. Close it by replacing the helper with a log capture that implements the trait differently (or by a `tracing` test subscriber), and log the removal here.

**Labels:** coverage

## 76. coverage: cover or remove flakeradar/src/mcp.rs — serialize_failure, and call_tool's arm for it

**File:** `crates/flakeradar/src/mcp.rs` **Function:** `serialize_failure`, and `call_tool`'s arm for it **Kind:** unreached arms

**Why it is not covered:** The error arm of `serde_json::to_value` and `to_string` for the answers this file builds. Every field of every answer is a `String`, an integer, a `bool`, an `Option`, or an `f64` from a `double` column, and `serde_json` writes a non-finite float as `null` instead of failing — measured: `a_non_finite_score_is_answered_as_null_not_as_a_failure` stores a `'NaN'` score and the tool answers a well-formed list whose `flakiness_score` fields are `null`. No value this file serializes can therefore fail, so the arm needs a `Serialize` impl that errors, which none of the DTOs has. `ToolFailure::Protocol` is now produced by that arm alone, so `call_tool`'s `return Err(error)` for it cannot run either.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 77. coverage: cover or remove flakeradar/src/mcp.rs — object

**File:** `crates/flakeradar/src/mcp.rs` **Function:** `object` **Kind:** unreached arm

**Why it is not covered:** The `panic!` for a schema that is not a JSON object. Every input and output schema is an object literal in the same file, so the arm cannot run; a schema that stopped being one is a programming error the build's own tests fail on first, the same shape as task 04's two `Regex` panic arms.

**What would cover it:** A statement-level seam (a connection whose next statement fails, or a query whose rows a test can shape) so the arm runs; otherwise delete it and state the invariant it guards.

**Labels:** coverage

## 78. coverage: cover or remove flakeradar/src/main.rs — health_check, runtime

**File:** `crates/flakeradar/src/main.rs` **Function:** `health_check`, `runtime` **Kind:** `coverage(off)`

**Why it is not covered:** Process-edge wrappers: `health_check` reads `Settings::from_env` and blocks a runtime on `healthcheck`, and `runtime` builds the multi-threaded runtime the server runs on. Both read the process environment and build process-wide state, which a test cannot do twice or observe from another thread; `Settings::from_env`, `healthcheck`, `healthcheck_url` and the URL rules are measured by `crates/flakeradar/tests/cli.rs` and the `settings.rs` unit tests, and the runtime itself is tokio's.

**What would cover it:** Nothing a test can do: the process edge (the real runtime, the environment, signal delivery) is not reachable from a test process. It stays excluded unless someone extracts the wiring further behind an injected seam, at which point the exclusion narrows.

**Labels:** coverage
