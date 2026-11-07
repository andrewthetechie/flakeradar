# Coverage gaps

The goal is as close to 100% line and branch coverage as possible (see `00-shared-context.md`, "The coverage goal"). Log every line or branch that a task could not cover, and every `coverage(off)`.

Task 14 works through this list and closes what it can. Task 15 files a follow-on issue for each gap that remains.

| Task | File | Function | Kind | Why it is not covered | Closed by |
|---|---|---|---|---|---|
| 01 | `crates/flakeradar/src/main.rs` | `main` | `coverage(off)` | Process entry point: a test process cannot run it. Task 13 made it parse the command line and dispatch; `Cli`, `Cli::action` and the two subcommands are measured by `crates/flakeradar/tests/cli.rs`, and the `serve`, `health_check` and `runtime` rows below say what else it calls | — |
| 03 | `crates/flakeradar-test-support/src/postgres.rs` | `lock`, `ensure_container`, `container_status`, `host_port`, `wait_until_ready`, `run_docker` | unreached | They run only when `FLAKERADAR_TEST_DATABASE_URL` is unset, and CI sets it for the whole suite, so the coverage job never takes the Docker path. The path works: three cold-start runs (`docker rm -f flakeradar-test-postgres` first) ran the whole suite with no failure | — |
| 03 | `crates/flakeradar-test-support/src/postgres.rs` | `ensure_postgres` | unreached branch | The same reason. The `FLAKERADAR_TEST_DATABASE_URL` branch is the one CI takes, so the container-start branch and its error paths are unmeasured | — |
| 03 | `crates/flakeradar-db/src/migration.rs` | `table_exists` | unreached branch | The `record not found` arm of the `SELECT EXISTS (…)` lookup. That query always returns exactly one row, so the arm cannot run | — |
| 03 | `crates/flakeradar-test-support/src/db.rs` | `Drop for TestDb` | unreached branch | Two error paths of the cleanup: the runtime failing to build, and the cleanup connection failing to open. Both happen only when a process cannot start a thread or reach a server it has just used, and neither can be provoked without breaking the test that runs them | — |
| 03 | `crates/flakeradar-test-support/src/db.rs` | `TestDb::new` | state-dependent branch | The arm that builds a missing template runs only on a server that has none. The coverage job's `postgres` service starts empty, so CI takes it (measured: db.rs is 98.68% lines and misses only the two `Drop` arms above with a fresh server); a re-run against a server whose template already exists reports the arm as missed. The build itself is covered deterministically, against a throwaway template name, by `a_built_template_is_published_with_the_baseline` | — |
| 04 | `crates/flakeradar-core/src/classify.rs` | `build_rules` | unreached arm | The `panic!` for a pattern that does not compile. Every pattern is a literal in the same file and all of them compile, so the arm cannot run; a rule that stopped compiling fails the build's own tests instead | — |
| 04 | `crates/flakeradar-core/src/parsing.rs` | `nextest_marker` | unreached arm | The `panic!` for the nextest marker pattern that does not compile. The pattern is a literal in the same file, so the arm cannot run | — |
| 04 | `crates/flakeradar-db/src/identity.rs` | `get_or_create_repo`, `get_or_create_project` | unreached arms | The error arm of each read-back `SELECT` after an insert, and the `ok_or_else` closure for a row that vanished between its insert and its read-back. They need the connection to fail, or a concurrent `DELETE`, between two statements of the same call; the insert and select error arms are covered by `a_blocked_insert_surfaces_as_a_database_error` and `a_closed_connection_surfaces_as_a_database_error` | — |
| 05 | `crates/flakeradar/src/main.rs` | `serve` (named `run` until task 13) | `coverage(off)` | Process start-up: reads the environment, gates the token, builds the Tokio runtime and wires Ctrl-C. Everything it calls is `Server::start` and `Server::serve`, which `crates/flakeradar/tests/server.rs` drives over a real socket on a database with no schema; the token gate is `assert_secure_token`, covered by the settings unit tests | — |
| 05 | `crates/flakeradar/src/main.rs` | `shutdown_signal` | `coverage(off)` | Signal wiring: Ctrl-C and SIGTERM delivery is a process-level event a test cannot raise against itself without ending the run. The shutdown it resolves is measured: `crates/flakeradar/tests/server.rs` passes a future of its own to `Server::serve` and asserts that a request in flight finishes, and `scripts/smoke.sh` stops a real binary with SIGTERM and checks a zero exit | — |
| 05 | `crates/flakeradar/src/telemetry.rs` | `init_tracing` | `coverage(off)` | It installs the process-wide `tracing` subscriber, which a test cannot install twice and another thread cannot observe | — |
| 05 | `crates/flakeradar/src/settings.rs` | `Settings::from_env` | unreached | It reads the process environment and `.env`, and only the binary calls it. `from_map` and `environment_values` are covered directly, including a malformed `.env` file and the precedence of the environment over the file | — |
| 05 | `crates/flakeradar/src/settings.rs` | `dotenv_values` | unreached arm | The `File::open` error mapping, which only a `.env` that exists but cannot be opened reaches (wrong permissions, a directory in its place); staging that means running the test process as another user. The parse-error mapping beside it is covered, including a line with and without a final newline | — |
| 05 | `crates/flakeradar/src/settings.rs` | test `a_bad_value_names_the_variable_it_came_from` | test-only arm | The `panic!` for an error kind a settings fixture cannot produce: it is an assertion in a test, not production code | — |
| 05 | `crates/flakeradar/src/extract.rs` | `json_error_position` | unreached arms | The `break` for a `serde_json` error claiming a line the input does not hold, and the `map_or` fallback for an offset that is not a character boundary. `serde_json` reports neither for the text it parsed, so no request reaches them; the line, column and multi-byte cases are pinned by `an_error_position_counts_characters_from_the_start` | — |
| 05 | `crates/flakeradar/src/extract.rs` | `issue_for` | unreached arms | Two parts of the `loc` mapping: the `Segment::Seq`, `Segment::Enum` and `Segment::Unknown` arms, because no DTO in the suite fails on an element inside a list or on an enum variant, and the `kind == "missing"` arm with no field name, because the same `serde` message that makes the kind `missing` also names the field | — |
| 05 | `crates/flakeradar/src/extract.rs` | `path_error` | unreached arm | The `let … else` that answers 404 for a `PathRejection` that is not a deserialization failure, which is `MissingPathParams`: Axum supplies the parameters of every route a handler is mounted on, so only a handler run outside its route's match reaches it. Every `ErrorKind` arm is covered, including `ParseError` — a path with exactly one parameter and no key to name — which `tests/extractors.rs::a_single_path_parameter_of_the_wrong_type_names_the_path_only` pins through `loc: ["path"]`, a loc only that arm produces | — |
| 05 | `crates/flakeradar/src/extract.rs` | `body_error` | unreached arm | The 500 for a body that failed to buffer for a reason other than its length. The 413 arm is covered by `a_json_body_over_the_limit_is_refused`; the 500 arm is now covered too, by task 06's `pipeline_ingest_answers_a_body_that_fails_while_it_is_read`, which sends a body whose stream errors with `Body::from_stream` | Closed by task 06 |
| 05 | `crates/flakeradar/src/routes/ops.rs` | `queue_facts` | unreached arm | The closure for a queue query that returned no row. The query has no `FROM` clause, so it always returns exactly one | — |
| 05 | `crates/flakeradar/src/routes/ops.rs` | `last_processed` | unreached arm | The `None` arm, for a scrape of a database with no `reports` row at all. The aggregate query still returns its one row, and the `NULL` it holds is the covered `Some` path | — |
| 05 | `crates/flakeradar/src/routes/ops.rs` | `python_float` | unreached branches | Its three guards: `is_finite()`, and the `e` and `E` in the formatted text. Every value the scrape prints is a finite number of seconds below the magnitude where either formatter switches to exponent notation, and Rust's `Display` never prints an uppercase `E` | — |
| 05 | `crates/flakeradar/src/server.rs` | `Server::start`, `Server::serve` | unreached arms | `local_addr` failing on a socket that the same call has just bound, and the warning when closing the pool at shutdown fails | — |
| 06 | `crates/flakeradar/src/routes/reports.rs` | `parse_upload`, `report_body` | unreached arm | The `spawn_blocking` join error of each. Only a panic inside `parse_junit_xml` or inside the multipart scan reaches it, and both run on attacker-controlled CI input, which the code standards forbid from panicking; the arms exist so that a bug there is a 500 rather than a dropped request | — |
| 06 | `crates/flakeradar/src/routes/reports.rs` | `summary` | unreached arm | The `ok_or_else` for a summary query that returned no row. The query is one aggregate with no `GROUP BY`, so PostgreSQL always returns exactly one row; task 05's `queue_facts` row is the same shape | — |
| 07 | `crates/flakeradar-db/src/processing.rs` | `get` | unreached arm | The `.map_err` for a driver error on a column read. Every column this file reads is named by its own statement, so the read can only fail if the schema changed under the query. The same shape as task 04's `identity.rs` read-back arms | — |
| 07 | `crates/flakeradar-db/src/processing.rs` | `process_report` | unreached arm | `DataError: a JUnit Report has no Project`. The baseline schema's `ck_reports_junit_has_project` refuses such a row, so no Report can reach the guard: `a_junit_report_without_a_project_fails` cannot even queue one, and the insert fails with SQLSTATE 23514 | — |
| 07 | `crates/flakeradar-db/src/processing.rs` | `record_test_executions` | unreached arm | `DataError: Test {fingerprint} vanished`, the `ok_or_else` for an id the upsert in the same transaction has just returned. It needs a concurrent `DELETE` between two statements of one transaction, which a single-connection test cannot stage | — |
| 07 | `crates/flakeradar-db/src/processing.rs` | `record_test_executions` | unreached arm (branch) | `DataError: Test bookkeeping is inconsistent`, the `get_mut` for an index `latest_by_id` never wrote. The index and the vector are pushed together two lines apart | — |
| 07 | `crates/flakeradar-db/src/processing.rs` | `apply_default_branch` | unreached arm (branch) | The `else` for a Repo that no row matches. `reports.repo_id` carries a foreign key to `repos` with `ON DELETE CASCADE`, so a Report whose Repo is gone cannot exist, and `a_report_without_a_default_branch_never_clears_one` covers the `None` value arm above it | — |
| 07 | `crates/flakeradar-db/src/processing.rs` | `upsert_pipeline_jobs` | unreached arm | `DataError: the Pipeline vanished`, the read-back after the `ON CONFLICT` upsert in the same transaction. Same shape as the Test arm above | — |
| 07 | `crates/flakeradar-db/src/processing.rs` | `insert_pipeline_job_executions` | unreached arm | `DataError: Job {name} vanished`, the id lookup for a name `upsert_pipeline_jobs` has just inserted. Same shape | — |
| 07 | `crates/flakeradar-db/src/processing.rs` | `insert_pipeline_job_executions` | unreached arm | `DataError: unknown Job status {other}`. The status comes back from rows this function just inserted from [`JobStatus`](flakeradar_core::JobStatus), so only a value the schema accepted and the enum does not know reaches it; `job_history`'s copy of that guard is covered by `a_job_execution_with_an_unknown_status_refuses_to_score`, which writes the row directly | — |
| 07 | `crates/flakeradar-db/src/processing.rs` | `tier_changes` | unreached arm (branch) | The `continue` for a row the naming read returns that the crossing list does not hold. The statement names exactly the ids that list holds, so the arm cannot run; task 27 merged the Test and Job copies of it into this one | — |
| 07 | `crates/flakeradar-db/src/attribution.rs` | `get` | unreached arm | The `.map_err` for a driver error on a column read, the same shape as `processing.rs`'s `get` | — |
| 07 | `crates/flakeradar-db/src/score_history.rs` | `get` | unreached arm | The `.map_err` for a driver error on a column read, the same shape again | — |
| 08 | `crates/flakeradar/src/worker.rs` | `AdvisoryLock::try_lock`, `AdvisoryLock::check` | unreached arm | The `None` for a lock query that answered no row, and for a liveness query that answered no row. `SELECT pg_try_advisory_lock($1) AS locked, pg_backend_pid() AS backend_pid` and `SELECT pg_backend_pid() = $1 AS same_session` have no `FROM` clause, so PostgreSQL always answers exactly one row; the arms mean "not locked" and "cannot prove the lock" rather than a panic, for a driver that ever answers none. They are the file's only missed lines (370-371 after task 16 moved them) and its only missed branch | — |
| 05 | `crates/flakeradar/tests/app.rs` | test helper `SharedWriter`'s `flush` | test-only arm | The same reason as `settings.rs`'s `Sink::flush` below: `tracing_subscriber`'s fmt layer writes each event to a fresh writer from `MakeWriter` and never flushes it, so the body cannot run. `io::Write` requires the method. The `SharedWriter`'s other lines are covered by every log assertion in this file | — |
| 08 | `crates/flakeradar/src/settings.rs` | test helper `captured_log`'s `Sink::flush` | test-only arm | `tracing_subscriber`'s fmt layer writes each event to a fresh writer from `MakeWriter` and never flushes it, so the trait method's body cannot run; it exists because `io::Write` requires it. The `Sink`'s other lines are covered by `the_default_token_is_allowed_when_explicitly_opted_in` | — |
| 08 | `crates/flakeradar/tests/worker.rs` | test helper `SharedWriter`'s `flush` | test-only arm | The same reason as `settings.rs`'s `Sink::flush` above: the fmt layer never flushes a `MakeWriter`'s writer, so the body cannot run. `io::Write` requires the method. The `SharedWriter`'s other lines are covered by every log assertion in this file | — |
| 08 | `crates/flakeradar-db/tests/retention.rs` | test helper `SharedWriter`'s `flush` | test-only arm | The same reason as `settings.rs`'s `Sink::flush` above: the fmt layer never flushes a `MakeWriter`'s writer, so the body cannot run. `io::Write` requires the method. The `SharedWriter`'s other lines are covered by the budget-stop log assertion | — |
| 08 | `crates/flakeradar/src/server.rs` | `Server::serve` | unreached arm | The `warn!` for a pool close that fails at shutdown. `close()` only errors on a connection it cannot end, and the process is on its way out either way; the `local_addr` arm on the same row is task 05's. Re-measured in task 08, unchanged | — |
| 09 | `crates/flakeradar-db/src/reads.rs` | `get` | unreached arm | The `.map_err` for a driver error on a column read. Every column the reads name is selected by the same statement, so a read fails only if the schema changed under the query. The same shape as task 07's `processing.rs` row | — |
| 09 | `crates/flakeradar-db/src/reads.rs` | `Board::run` | unreached arm | The `None` fallback for the total count. `SELECT count(*) FROM (…)` has no `GROUP BY`, so PostgreSQL always answers exactly one row | — |
| 09 | `crates/flakeradar-db/src/reads.rs` | `test_counts`, `new_flaky_count`, `window_sums`, `job_summary` | unreached arms | Each `ok_or_else` for an aggregate that answered no row. Every one of those statements is an aggregate over a table with no `GROUP BY`, so it returns exactly one row; the same shape as task 05's `queue_facts` row | — |
| 09 | `crates/flakeradar-db/src/reads.rs` | `list_tests`, `list_jobs`, `search_tests`, `search_jobs`, `get_job`, `ci_run_failed_jobs` | unreached arm | The `?` on collecting the `id` column of the page or search rows. The column is selected by the same statement and read as the type the schema declares, so only a schema change makes it fail | — |
| 09 | `crates/flakeradar-db/src/reads.rs` | `category_counts` | unreached arm | The `?` on reading the count for a category in `FailureCategory::ALL`; a row's category and count are selected by one statement, so only a schema change breaks the read | — |
| 09 | `crates/flakeradar-db/src/reads.rs` | `explaining_tests` | unreached arms | Two `continue`s for a row the map cannot place: a `ci_job_id` absent from `failed_ci_to_exec` and an execution id absent from `by_exec`. The query filters `ci_job_id IN` the map's keys, and the map is built from the same map's values, so neither can run | — |
| 09 | `crates/flakeradar-db/src/reads.rs` | `ci_run_report_counts` | unreached arm | The `_ => {}` for a Report status outside `pending`, `processed` and `failed`. `ck_reports_status` allows only those three, so no row reaches it | — |
| 09 | `crates/flakeradar-db/src/reads.rs` | `ci_run_test_failures` | unreached arm | The `ok_or_else` for the pair-count aggregate that answered no row, the same shape as the summaries above | — |
| 09 | `crates/flakeradar-db/src/reads.rs` | `ci_run_failure_item` | unreached arms | The two `return Ok(None)` arms for a pair with no Execution, or none failing. The pair was counted as failing in the same listing, so both need a concurrent `DELETE` between the count and the execution read | — |
| 09 | `crates/flakeradar/src/routes/reads.rs` | `From<DbError>`, `From<DbErr>`, `From<ReadError> for ReadRouteError` | measurement artifact | Every read test executes these conversions (a failed query reaches the 500 path, and an unknown CI run reaches the 404 path), but `llvm-cov` attributes the inlined conversion bodies to their callers, so the `impl` lines themselves report zero. The 500 and 404 bodies are covered by `a_failed_read_query_answers_the_plain_text_500` and the unknown-repo/run tests | — |
| 09 | `crates/flakeradar/src/routes/reads.rs` | `category_expected` | unreached arm | The `None` arm of `split_last`. `FailureCategory::ALL` holds five variants, so the list is never empty | — |
| 09 | `crates/flakeradar-core/src/dto.rs` | `timestamp::deserialize`, `optional_timestamp::deserialize`, `lax_bool::deserialize`, `lax_bool::optional` | unreached arm | The `?` that would propagate a failure of `String::deserialize`, `Option::<String>::deserialize` or `Value::deserialize`. Every caller reads a field of a body `serde_json` has already decoded or of a query string, and both sinks answer every one of those methods, so the read cannot fail; a format that refused one (a binary one) would reach the arm, and no route uses one. The decoding and rejecting each reader does afterwards is measured, including `a_bad_timestamp_is_a_deserialization_error` and the lax-bool tables | — |
| 09 | `crates/flakeradar-core/src/dto.rs` | `lax_bool::interpret_number` | unreached arm | The `else` of `let Some(float) = number.as_f64()`. `serde_json::Number` holds a `u64`, an `i64` or an `f64`, and `as_f64` answers all three, so the arm needs a number shape the crate does not build; `serde_json`'s `arbitrary_precision` feature is what adds one, and no member enables it | — |
| 10 | `crates/flakeradar-db/src/deletes.rs` | `From<DbErr> for DeleteError` | measurement artifact | The bare-`DbErr` `?` sites do run this conversion (`transaction.rollback().await?` is one), but `llvm-cov` attributes the inlined body to its caller, so the `impl` lines report zero. The same shape as task 09's row for `routes/reads.rs` | — |
| 10 | `crates/flakeradar-db/src/deletes.rs` | `count` | unreached arm | `return Ok(0)` for a `SELECT count(*)` that answered no row. The statement has no `GROUP BY`, so PostgreSQL always answers exactly one row | — |
| 10 | `crates/flakeradar-db/src/deletes.rs` | `issue_numbers` | unreached arm | The `continue` for `one_many` answering `None`, which it never does: it builds `Some` out of the rows `query_all_raw` returned | — |
| 10 | `crates/flakeradar-db/src/issue_links.rs` | `From<sea_orm::DbErr> for LinkError` | measurement artifact | Inlined into its `?` sites, the same shape as the `deletes.rs` row above | — |
| 10 | `crates/flakeradar-db/src/issue_links.rs` | `link_issue`, `unlink_issue` | measurement artifact | The field-argument lines of the two `tracing::info!` events. `tracing` evaluates those arguments lazily inside the macro, and `llvm-cov` counts them zero although the events fire; each event's own statement line is covered | — |
| 10 | `crates/flakeradar/src/routes/issues.rs` | `From<ApiError>`, `From<DbError>`, `From<DbErr>`, `From<IssueLinkError>`, the `IssueRouteError::Rejected` response arm | dead conversion | No caller: `ApiPath`, `ApiQuery` and `LinkBody` answer their own rejection before a handler returns, and the two engines fail with `LinkError`, whose `From<LinkError>` is the conversion the routes do use. `Rejected` is never constructed; the `LinkError::Database` arm and the 500 body are covered by `a_failed_link_query_answers_the_plain_text_500` | — |
| 10 | `crates/flakeradar/src/routes/issues.rs` | `link_test`, `unlink_test`, `link_job`, `unlink_job` | unreachable arms | The four `LinkedOut` mismatch arms (200-202, 235-237, 282-284, 317-319). `link_issue` and `unlink_issue` build the variant from the target they are handed, and each route passes one target, so a Test route cannot get a `LinkedOut::Job` back (or the reverse) | — |
| 10 | `crates/flakeradar/src/routes/issues.rs` | `IssueRouteError::into_response` | unreachable arm | `StatusCode::from_u16(error.status()).unwrap_or(StatusCode::UNPROCESSABLE_ENTITY)`: `IssueLinkError::status` answers 404, 409 or 422 only, so the fallback cannot be taken (line 78 is covered; the unreachable part is the closure's body) | — |
| 10 | `crates/flakeradar/src/routes/admin.rs` | `From<DbError>`, `From<DbErr> for AdminRouteError` | dead conversion | No caller: every delete fails with `DeleteError`, which has its own conversion (covered by the deadlock and 503 tests), and the extractors answer themselves | — |
| 10 | `crates/flakeradar-db/src/sql.rs` | `get` | unreached arm | The `.map_err` on a column read. Every column is named by the statement that selected it, so only a schema change fails it; the same shape as task 07's `processing.rs` row. The row named `crates/flakeradar/src/github.rs`, which held its own `get` until task 24 moved it here | — |
| 10 | `crates/flakeradar/src/github/filing.rs` | `commit_filed_link` | unreached arms | The savepoint's start and its commit failing, each needing the driver to fail between statements of a filing that is already in progress. Task 18 covered the third arm this row used to name, the link `UPDATE` itself failing: `a_store_failure_leaves_the_issue_open_and_stops_the_batch` raises from a `BEFORE UPDATE` trigger | — |
| 18 | `crates/flakeradar/src/github/client.rs` | `github_client` | unreached arm | The error its `build()?` carries, which is the TLS backend failing to initialize: no test can make a healthy backend fail. The constructor's own failure arm is covered through the injected builder (`Api::with_client_build`), which `a_failed_client_build_is_an_error_at_construction` proves answers `Err` carrying the builder's error; every GitHub test takes the success arm here | — |
| 18 | `crates/flakeradar/src/webhooks.rs` | `webhook_client` | unreached arm | The same `build()?` failure on the Notification client, for the reason the `github_client` row gives; `WebhookWorker::with_client_build` is the covered seam | — |
| 18 | `crates/flakeradar/src/app.rs` | `AppState::new` | unreachable arm | The `?` that carries `Api::production`'s failure, which the two rows above make unreachable in a test | — |
| 18 | `crates/flakeradar/src/server.rs` | `Server::start`, `webhook_task` | unreachable arm | The two `?`s that carry those constructor errors out of the lifespan. Same reason; every other line of both functions is covered by `tests/server.rs` and the `webhook_task` unit test | — |
| 18 | `crates/flakeradar/tests/ops.rs` | test helper `logged_from`'s `Sink` | test-only arm | Its `flush` body, for the reason the `tests/app.rs` sink row gives: `tracing_subscriber`'s fmt layer never flushes a writer from `MakeWriter`. The helper's other lines are covered by `metrics_logs_the_error_it_answers_503_for` | — |
| 10 | `crates/flakeradar/src/github/sync.rs` | `sync_closed_issues`, `record_check` | unreached arm | The write of one row's check failing, which needs the driver to fail on the statement that follows the `GET`. A dropped table reaches the rotation-list read (`a_sync_whose_rotation_list_cannot_be_read_makes_no_request`). Task 17 removed the transaction this row used to name: the sync now reads its list and writes each row with no statement open across a `GET`, so the four per-row writes this list used to name separately (a stamp, a kept `not_planned` link, a re-arm and a cleared flag) are one statement, `issue_links::record_check`, reached through this one call | — |
| 10 | `crates/flakeradar/src/github/notes.rs` | `stamp_note` | unreached arm | The fixed-note stamp write failing, which needs the driver to fail on the `UPDATE` that follows the comment `POST`. The two candidate reads this row used to name are covered now: `a_candidate_read_that_fails_logs_and_reports_an_empty_batch` drops the tables and asserts both note entry points answer an empty `BatchReport`, since task 17 made that path return the batch rather than only logging | — |
| 10 | `crates/flakeradar/src/github/{policy,filing,sync,notes}.rs` | every `tracing::…!` event | measurement artifact | The field-argument and message lines of the events in `BatchReport::log`, `RemovalReport::log`, `file_batch`, `clear_if_closed`, `sync_decision`, `close_issues_after_removal`, `note_batch`, `stamp_note`, `log_note_stamp_failure` and `close_note`. `tracing` evaluates them lazily inside the macro, and only when the callsite's interest lets the event through, so `llvm-cov` counts them zero although the event fires; every event's statement line is covered. Task 29 made this the only thing those lines cost: the tests that read a batch's report instead of its log leave the argument lines zero, measured at `policy.rs` 34 of 44 lines with all ten misses being the two `log` events, and `sync.rs` 137 of 181 with every miss a `tracing` field or message line. The message text is pinned only where a test still captures a log (the ones that say in place why their line is a requirement), because log wording is not a contract. The numbers are not listed per function because every rewrite of these files moves them | — |
| 10 | `crates/flakeradar/src/github_worker.rs` | `with_monotonic` | unreachable arm | The `None` of `Arc::get_mut`: `with_monotonic` takes `self` and nothing clones a `GitHubWorker`, so the worker holds the only reference to its shared state and the arm cannot be taken | — |
| 10 | `crates/flakeradar/src/webhooks.rs` | `post` | measurement artifact | The field-argument lines of its `tracing` events (`152-153`, `161-165`, `172-174`, `180`, `182`, `187`, `189-191` after task 16 added the stop event), for the reason the GitHub events' row gives: the events fire (the retry and stop tests count them) and the argument lines still report zero | — |
| 10 | `crates/flakeradar/src/webhooks.rs` | test `a_delivery_that_raises_is_logged_and_the_task_keeps_serving` | test-only arm | Its `Sink::flush` body, which `tracing_subscriber`'s fmt layer never calls (it writes to a fresh writer from `MakeWriter`); `io::Write` requires the method. Same reason as the `settings.rs` sink row | — |
| 10 | `crates/flakeradar-core/src/notify.rs` | test `a_json_event_omits_the_url_without_a_public_url` | test-only arm | The `return` of its `let Some(payload) = … else`, which cannot be reached because the line above asserts the list holds exactly one event | — |
| 11 | `crates/flakeradar/src/mcp/mod.rs` | `serialize_failure`, and `call_tool`'s arm for it | unreached arms | The error arm of `serde_json::to_value` and `to_string` for the answers this file builds. Every field of every answer is a `String`, an integer, a `bool`, an `Option`, or an `f64` from a `double` column, and `serde_json` writes a non-finite float as `null` instead of failing — measured: `a_non_finite_score_is_answered_as_null_not_as_a_failure` stores a `'NaN'` score and the tool answers a well-formed list whose `flakiness_score` fields are `null`. No value this file serializes can therefore fail, so the arm needs a `Serialize` impl that errors, which none of the DTOs has. `ToolFailure::Protocol` is now produced by that arm alone, so `call_tool`'s `return Err(error)` for it cannot run either | — |
| 11 | `crates/flakeradar/src/mcp/mod.rs` | `object` | unreached arm | The `panic!` for a schema that is not a JSON object. Every input and output schema is an object literal in that file or in `mcp/tools/*.rs`, so the arm cannot run; a schema that stopped being one is a programming error the build's own tests fail on first, the same shape as task 04's two `Regex` panic arms | — |
| 13 | `crates/flakeradar/src/main.rs` | `health_check`, `runtime` | `coverage(off)` | Process-edge wrappers: `health_check` reads `Settings::from_env` and blocks a runtime on `healthcheck`, and `runtime` builds the multi-threaded runtime the server runs on. Both read the process environment and build process-wide state, which a test cannot do twice or observe from another thread; `Settings::from_env`, `healthcheck`, `healthcheck_url` and the URL rules are measured by `crates/flakeradar/tests/cli.rs` and the `settings.rs` unit tests, and the runtime itself is tokio's | — |

## Task 14: what remains, and the figures the floor was set from

Measured with `cargo +nightly-2026-10-25 llvm-cov nextest --branch --all-features --profile ci`
against a PostgreSQL service, with `FLAKERADAR_TEST_DATABASE_URL` set, which is the configuration
CI measures in. Three consecutive runs produced byte-identical summaries and the same totals:
**lines 96.73%** (13853/14321), **branches 94.21%** (1074/1140), regions 92.53% (15278/16511) and
functions 96.03% (1644/1712). `coverage-floor.json` carries `{"lines": 96, "branches": 94}`, both
rounded down from those totals so the gate cannot fail on a figure the same code did not reach.

Nothing in this task's own code is uncovered: `connect.rs` (the pooled-connection wait),
`docs.rs` (every route's `#[utoipa::path]`) and the changed arms of `routes/reports.rs`,
`routes/reads.rs`, `routes/issues.rs` and `routes/admin.rs` are 100% of lines and branches, and the
new suites are `crates/flakeradar/tests/consumers.rs` and the parity harness's own tests, which
nextest measures as test code.

The 468 missed lines and 66 missed branches the totals hold are the rows above and no others. Every
one is either a `coverage(off)` process edge, an arm PostgreSQL cannot produce (an aggregate with no
`GROUP BY` answering no row, a foreign key making a row impossible, a schema change mid-query), a
`tracing` field-argument line llvm-cov counts zero although the event fires, a test-only writer's
`flush`, or the Docker path of `crates/flakeradar-test-support/src/postgres.rs`, which CI never
takes because it sets the database URL.

One measurement caveat, and it is why the figures come from the URL-set configuration: with
`FLAKERADAR_TEST_DATABASE_URL` unset the suite starts its own containers, so that file's Docker path
is covered too and the same tree reports 97.15% of lines and 94.91% of branches. Both figures are
honest; the floor follows the lower one, which is the one CI reaches.

## Task 15: the rows this task leaves, and the drafts it filed

Re-measured after the Python deletion, with the same command and the same
configuration task 14 used (`cargo +nightly-2026-10-25 llvm-cov nextest --branch
--all-features --profile ci` against a PostgreSQL service, `FLAKERADAR_TEST_DATABASE_URL`
set): **lines 96.73%** (13853/14321) and **branches 94.21%** (1074/1140), regions
92.53% (15278/16511), functions 96.03% (1644/1712) — the same figures task 14
recorded, because deleting Python changes no Rust line.

`coverage-floor.json` therefore stays `{"lines": 96, "branches": 94}`, both
rounded down from those totals and both above the {95, 80} minimum. Deleting the
Python service does not lower a floor: the Rust totals are what the gate reads.

The 78 rows above are unchanged and each one has a draft issue in
`coverage-follow-ups.md`, which the user files after this PR merges.

## Task 16: no new row, and the files it touched

Measured with the command and configuration tasks 14 and 15 used
(`cargo +nightly-2026-10-25 llvm-cov nextest --branch --all-features --profile ci` with
`FLAKERADAR_TEST_DATABASE_URL` set): **lines 96.74%** (14403, 470 missed) and **branches 94.24%**
(1146, 66 missed), regions 92.54% and functions 96.06%. `coverage-floor.json` stays
`{"lines": 96, "branches": 94}` and the gate passes at them.

Task 16 adds no row. `LeaderRun` and `until_stopped` in `worker.rs` are covered both ways — a run
started, stopped and started again; a call and a wait each ended by the token and each left to
finish — and so are the four stop arms in `github/` (`file_batch`, `clear_if_closed`,
`note_batch`, `close_note`), the stop arm in `webhooks.rs`'s `post`, and the queue's drop in
`github_worker.rs`. The only misses left in those files are the rows above: `worker.rs`'s two
`None` arms (row 08, whose line numbers moved to 370-371), `github_worker.rs`'s `Arc::get_mut`
(row 10), `webhooks.rs`'s `post` field-argument lines and its test sink's `flush` (rows 10), and
`github/`'s own artifact and schema-change row (row 10).

## Task 18: no new line uncovered, five unreachable client-build arms

Measured with `cargo +nightly-2026-10-25 llvm-cov nextest --branch --all-features --profile ci`:
**lines 97.17%** (14087 of 14497) and **branches 95.38%** (1074 of 1126), regions 93.01% and
functions 96.61%. `coverage-floor.json` stays `{"lines": 96, "branches": 94}` and the gate passes
at them. The 410 missed lines and 52 missed branches are the counts this task measured before its
second leg added the client-build seam, so every line and branch that leg added is covered.

Every line this task adds is covered except the two `commit_filed_link` arms row 10 already names,
and a new test covers the third arm that row used to include. The constructors that return the
client-build error (`Api::new`, `WebhookWorker::new`/`with_retry_delays`, `AppState::new` and
`Server::start`) answer it from an injected builder, and
`a_failed_client_build_is_an_error_at_construction` (once in `crates/flakeradar/tests/github/client.rs`, once in
`tests/webhooks.rs`) fails the build on purpose and asserts the error carries the builder's own,
so what stays uncovered is only the production builders' `build()?` and the two callers a test
cannot make fail: four of the five rows above. The GitHub field-argument artifact row also
covers the three events this task added or moved (`record_filed`'s three outcomes and
`file_job_issues_for`'s unreadable-history warning), all of which a log-capture test proves fires.

## Task 20: three unreachable lax arms and five multipart guards

Measured with `cargo +nightly-2026-10-25 llvm-cov nextest --branch --all-features --profile ci`
and `FLAKERADAR_TEST_DATABASE_URL` set: **lines 96.83%** (15166, 481 missed) and **branches
94.17%** (1286, 75 missed), regions 92.93% and functions 96.29%. `coverage-floor.json` stays
`{"lines": 96, "branches": 94}` and the gate passes at them.

`crates/flakeradar-core/src/lax.rs` is at 98.79% of lines and 88.89% of branches: every accepted
and refused spelling this task reads is covered by `crates/flakeradar-core/tests/lax.rs` (one row
per value Pydantic was measured with) and by the route-level tables in
`crates/flakeradar/tests/validation.rs`. Three arms cannot run, and `multipart.rs`'s guards are
the same shapes its `?`s were before this task gave them a `Result`:

| Task | File | Function | Kind | Why it is not covered | Closed by |
|---|---|---|---|---|---|
| 20 | `crates/flakeradar-core/src/lax.rs` | `integer_from_number` | unreached arms | The `else` of `let Some(float) = number.as_f64()` and the `int_type` it answers for a float that is not finite. `serde_json::Number` holds a `u64`, an `i64` or an `f64` and `as_f64` answers all three, and `serde_json` builds no `Value` from a `NaN` or an infinity (`Number::from_f64` answers `None` for one), so no body carries either. The task 09 row for `lax_bool::interpret_number` is the same arm in the older reader | — |
| 20 | `crates/flakeradar-core/src/lax.rs` | `epoch_instant` | unreached arm | The `if !seconds.is_finite()` guard, for the reason above: a JSON number is always finite | — |
| 20 | `crates/flakeradar/src/multipart.rs` | `file_field` | unreached arms | The two `checked_add` overflows of the delimiter scan and the `body.get(part_start..part_end)` that follows `next_end`, which answers an index inside the same body. A body of 20 MiB cannot reach `usize::MAX`, and `next_end` only returns an end its own scan found. `arithmetic_side_effects` and `indexing_slicing` are in the lint set, so each is an arm rather than an unchecked add or index; before this task they were the same arms written as `?` | — |
| 20 | `crates/flakeradar/src/multipart.rs` | `part_field` | unreached arms | The `let … else` that answers `Ok(None)` for a header block whose end or payload cannot be sliced, which `find(part, HEADER_END)` has just located, and the `continue` for an empty header line, which cannot appear because the first empty line is the end of the block | — |

## Task 21: two unreachable `.env` read arms

Measured with `cargo +nightly-2026-10-25 llvm-cov nextest --branch --all-features --profile ci`:
**lines 97.26%** (15512, 425 missed) and **branches 94.76%** (1316, 69 missed), regions 93.37%
and functions 96.82%, above `coverage-floor.json`'s `{"lines": 96, "branches": 94}`.
`crates/flakeradar/src/settings.rs` is at 98.14% of lines and 96.67% of branches, and the lines it
adds are covered by the tests this task names, including both shapes of a refused `.env` line (a
final newline and none). What stays uncovered is the reading itself failing:

| Task | File | Function | Kind | Why it is not covered | Closed by |
|---|---|---|---|---|---|
| 21 | `crates/flakeradar/src/settings.rs` | `dotenv_values` | unreached arm | The `File::open` mapping (see the task 05 row above, which this task's split of the function moved it into): only a `.env` that exists and cannot be opened reaches it, and staging that needs another user or a directory with a `.env`'s name | — |
| 21 | `crates/flakeradar/src/settings.rs` | `dotenv_error` | unreached arm | The `other` arm, for a `dotenvy` error that is not `LineParse`: the iterator's only other failure is an I/O error between two lines of a file that is already open, which no test can make happen on a healthy file. The `LineParse` arm is covered by `a_dotenv_token_never_reaches_the_log_main_writes` and `a_malformed_dotenv_line_names_its_number_and_never_its_text` | — |

## Task 26: the GitHub rows follow their files, and no line is left uncovered

`crates/flakeradar/src/github.rs` became the directory `crates/flakeradar/src/github/`
(`client`, `policy`, `bodies`, `filing`, `sync`, `notes` and `mod`), and
`crates/flakeradar/tests/github.rs` became `crates/flakeradar/tests/github/` along the same
lines. The rows above whose code moved keep their reason and now name the file that holds it.

Nothing this task adds is uncovered. `bodies.rs`'s `Body` is exercised by the two bodies the
tests render, every method by at least one of them: `nested_bullet` and `filing_gate` by
`a_job_body_lists_its_worst_runner_groups_first`, `code` and `section` by both,
`both_bodies_name_the_executions_the_filing_reads` for the heading `Body::executions` states
once. The three `location` arms, the two `last_failing_commit` arms and both `permalink`
refusals have a test each: `a_field_the_body_prices_differently_is_still_the_reference_sentence`
(no file, hence no Location and no permalink, and the run-hint fallback),
`the_last_failing_commit_has_no_branch_clause_when_no_branch_was_reported` (an empty branch) and
`the_body_matches_the_reference_byte_for_byte` (a file, a line and a branch), which is also the
byte-for-byte comparison against the reference's own output.

One row moved with its code rather than being re-homed here: `github.rs`'s own `get` went to
`flakeradar_db::sql::get` in task 24, so its row now names `crates/flakeradar-db/src/sql.rs`.
The line numbers in the rows this task touched are gone rather than corrected: the split moves
every one of them, and a function name survives a move.

## Task 27: no new row, and the two tier-change rows became one

Task 27 merged the Test and Job copies of the rescore, the bulk write, the Tier change, the
old-score read, the two leaderboard reads and the retention steps. The rows above whose code
moved keep their reason: the two `test_tier_changes`/`job_tier_changes` `continue` rows are the
one `tier_changes` row, because there is one arm now.

Nothing this task adds is uncovered, measured with
`cargo +nightly-2026-10-25 llvm-cov nextest --branch --all-features --profile ci`:
`crates/flakeradar-db/src/retention.rs` is 100% of lines (300) and of branches (14),
`sql.rs` 100% of both (`bigint_array` is the id batch both expiry steps bind), and the new
descriptors are all reached — `Column::value` for all seven columns by the two bulk writes,
`Notices::recovered` by both entities, `Tally`'s two filters by `summary` and `job_summary`,
and `Window::push`/`window_binds` by both window reads. `processing.rs` is 97.86% of lines and
95.59% of branches and `reads.rs` 99.12% and 93.44%; the lines they miss are the ones the rows
above already name, and the suite is 97.67% of lines (364 missed) and 94.71% of branches
(70 missed), so `coverage-floor.json` stays `{96, 94}`.

## Task 28: two join-error arms, and the rows the moves keep

Task 28 moved the Execution build into `spawn_blocking` and the JSON body parse off the async
thread, so two arms now cover a blocking task that did not finish: `parse_report`'s and
`build_executions`' `RuntimeError` closures in `crates/flakeradar-db/src/processing.rs`, and
`ApiJson`'s "The JSON body parser did not finish" closure in `crates/flakeradar/src/extract.rs`.
Only a panic inside the parser, the classifier or the Execution build reaches them, and those
run on attacker-controlled CI input, which the code standards forbid from panicking; the arms
exist so that a bug there is a stored failure or a 500 rather than a dropped request. They are
the same shape as the `report_body`/`parse_report_body` arms row 06 already carries.

Nothing else this task adds is uncovered: `batching.rs` is 100% of lines (24) and branches (0
counted), `sql.rs` 100% of both (136 lines, 2 branches) with `impl Into<String>` reached by both
a borrowed static statement and an owned one, `github/client.rs` 100% of lines and branches
(`Answer::new` parses every answer the tests answer), and `retention.rs` 98.99% of lines and 100%
of branches, whose three missed lines are the `tracing` field arguments of the expiry line that
row 07's artifact note already names. `processing.rs` is 97.66% of lines and 95.59% of branches,
`reads.rs` 99.05% and 92.62%, and `extract.rs` 95.87% and 89.66%; every line they miss is one a
row above already names, plus `extract.rs`'s two join-error lines. The suite is 97.59% of lines
(377 missed) and 94.65% of branches (71 missed), so `coverage-floor.json` stays `{96, 94}`.

The re-measured ingest and processing figures are in `docs/perf/22-rust-vs-python.md`.

## Task 31: the recount at the handoff head

Re-measured three times at `e8563b1` with the command CI runs
(`cargo +nightly-2026-10-25 llvm-cov nextest --branch --all-features --profile ci`, then
`cargo +nightly-2026-10-25 llvm-cov report --branch --json --summary-only`, against a local
`postgres:17-alpine` with `FLAKERADAR_TEST_DATABASE_URL` set): **lines 96.87%** (15445/15944, 499
missed) and **branches 94.62%** (1283/1356, 73 missed), regions 93.38% (16863/18059) and
functions 97.46% (1999/2051). The three runs agree on every one of those figures; only the
`instantiations` count moved, by two (6321 and 6323 in the first two runs against 6323 in the
third), which is a generic-instantiation count and not a line or branch figure.

`coverage-floor.json` is `{"lines": 96, "branches": 93}`. Lines is rounded down from 96.87 and was
not lowered. Branches is 93 by the **owner's decision of 2026-11-06**, because CI's configuration
measures 93.95% (1274/1356) on the merged tree: the 9-branch gap against a run that takes this
file's Docker path is `postgres.rs`'s testcontainers path plus one `db.rs` branch, both logged
above as unreachable when CI sets `FLAKERADAR_TEST_DATABASE_URL`. Both floors stay above the
`{95, 80}` minimum the plan sets.

**The list is 87 rows.** Task 15 left 78; task 18 added five (the client-build arms) and task 20
added four (the lax and multipart guards), and no task removed one. Rows 1–78 of the table are the
rows task 15 drafted an issue for; the nine later rows have no draft in
`coverage-follow-ups.md`, because task 15 was the last task that drafted them. The nine are:
`github_client` and `webhook_client` (`crates/flakeradar/src/github/client.rs` and
`webhooks.rs`), `AppState::new` (`app.rs`), `Server::start`/`webhook_task` (`server.rs`) and the
test helper `logged_from`'s `Sink` (`crates/flakeradar/tests/ops.rs`) from task 18, and
`integer_from_number` and `epoch_instant` (`crates/flakeradar-core/src/lax.rs`) with `file_field`
and `part_field` (`crates/flakeradar/src/multipart.rs`) from task 20.

The 499 missed lines and 73 missed branches are these rows and no others. The gap between the two
counts is the same one the earlier sections describe: a row can be a branch, an arm inside a
covered line, or a group of `tracing` field-argument lines that `llvm-cov` counts zero although
the event fires.

## Task 32: three unreachable arms in the parser's pre-scan

Task 32 added `parsing.rs`'s pre-scan: `refuse_expansion_past_the_budget` with `Expansion`,
`refuse_namespace_entries_past_the_budget` with `NamespaceScope`, and the `DOCTYPE`,
`entity_values`, `start_tag` and `split_name` readers under them. With the tests this task adds,
`parsing.rs` measures 99.47% of lines (754/758) and 97.20% of branches (208/214), and it misses
four lines: the one above, which is `nextest_marker`'s compile-time panic, and the three below.
None is a behaviour a body inside the 20 MiB ingest limit can reach:

- `Expansion::length`'s own `total > self.cap` check, and `refuse_expansion_past_the_budget`'s
  check on the two parts summed. Both are the same budget checked where the loop cannot see it:
  `cap` is the body's own size plus `EXPANSION_BYTES_MAX` (64 MiB), so the initial total — the
  text's length, before any reference is added — can only pass it for a body over 64 MiB, which
  the ingest route refuses before the parser sees it. They stay because `parse_junit_xml` has no
  size limit of its own, so a caller outside the route (`flakeradar-core`'s own tests, a future
  batch importer) still gets the bound.
- `entity_values`'s `body.find(quote)` failure: a declaration whose value has no closing quote.
  `declaration_extent` reads the same quotes and ends the subset at the first `]` outside them, so
  an unterminated quote swallows the rest of the declaration and the subset never holds one. It
  is the guard that keeps the two readers agreeing rather than a case either can see.

One arm that *was* unreachable is gone rather than logged: `Expansion::length` looked an entity's
value up again after checking it was declared, and that second lookup's `None` could not happen.
`entity_length` now answers `None` for an undeclared name itself, which is the case a predefined
entity such as `&lt;` reaches, and a test covers it.

## Task 33: the encoding tables, and the declaration and `DOCTYPE` arms around them

Task 33 replaced the encoding-label lookup with a table generated from the reference's own
codecs (`crates/flakeradar-core/src/encodings.rs`, 462 lines) and taught the parser what a
byte-order mark, a declaration that is not a declaration, and an external subset mean. With the
tests this task adds — 54,309 fixture rows replayed through `parse_junit_xml`, 25 declaration
cases and the 23-row encoding fixture — `parsing.rs` measures **99.32% of lines (876/882) and
96.03% of branches (242/252)** and `encodings.rs` **99.78% of lines (461/462) and 87.5% of
branches (7/8)**. Every miss is one of the rows below or one this file already carries.

Already carried by task 32's section above: `nextest_marker`'s compile-time panic,
`refuse_expansion_past_the_budget`'s own budget check, `Expansion::length`'s, and
`entity_values`'s unterminated-quote guard.

New in this task, all arms that keep a reader total rather than behaviour a body can reach:

- `encodings.rs` `SingleByte::decode`'s `characters.get(..)?`: the table is exactly 256
  characters by construction, so the byte always has an entry. It stays because indexing would
  panic where the `?` cannot.
- `encodings.rs` `table()`'s `panic!`: every name in `RULES` is in `TABLES` in the same generated
  file, so a missing one is a generator bug rather than a body.
- `declared_encoding`'s `std::str::from_utf8(..).ok()?`: a declaration whose text up to `?>` holds
  a byte that is not UTF-8. XML requires the declaration to be ASCII, so such a document is one
  the parser refuses for its declaration anyway.
- `declaration_label`'s `rest.find("?>")?`: reached only from the UTF-16 path, on a document whose
  `<?xml` never closes. The parser refuses that document too.
- `declaration_label`'s `strip_prefix('=')?`: a declaration that says `encoding` and then something
  that is not `=`. Reachable, and deliberately unpinned: `SP4`'s table does not name the shape, so
  no row of `differences.md` may claim it, and pinning it would freeze an answer nobody has
  reviewed. The reference refuses such a declaration as not well formed and this server reads the
  words as no declaration at all; the third review's list of what to attack starts here.
- `doctype`'s `text.get(start..)?`: the offset comes from `find`, so it is always a character
  boundary.
- `standalone_yes`'s two remaining `return false`s: a document whose `<?xml` never closes, and one
  that says `standalone` with no `=`. Both are declarations the parser refuses.
- `start_tag`'s four `?` arms: a `<` that does not start a tag, an attribute with no name, and an
  attribute with no quote or no value. The scan answers "not a tag" and leaves the refusal to the
  parser, which is what the doc comment says it is for.
- `collect_case`'s `)?)?` in both retry loops: the identity-budget error raised while a *retry*
  copy is pushed. `parse_budget.rs`'s `long_identity` shape reaches the budget through cases only,
  because a retry adds no identity a case did not already copy.
