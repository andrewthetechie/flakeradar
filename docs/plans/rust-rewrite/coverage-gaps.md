# Coverage gaps

The goal is as close to 100% line and branch coverage as possible (see `00-shared-context.md`, "The coverage goal"). Log every line or branch that a task could not cover, and every `coverage(off)`.

Task 14 works through this list and closes what it can. Task 15 files a follow-on issue for each gap that remains.

| Task | File | Function | Kind | Why it is not covered | Closed by |
|---|---|---|---|---|---|
| 01 | `crates/flakeradar/src/main.rs` | `main` | `coverage(off)` | Process entry point: a test process cannot run it. The workspace has no other executable Rust yet; the crates hold doc comments only | — |
| 03 | `crates/flakeradar-test-support/src/postgres.rs` | `lock`, `ensure_container`, `container_status`, `host_port`, `wait_until_ready`, `run_docker` | unreached | They run only when `FLAKERADAR_TEST_DATABASE_URL` is unset, and CI sets it for the whole suite, so the coverage job never takes the Docker path. The path works: three cold-start runs (`docker rm -f flakeradar-test-postgres` first) ran the whole suite with no failure | — |
| 03 | `crates/flakeradar-test-support/src/postgres.rs` | `ensure_postgres` | unreached branch | The same reason. The `FLAKERADAR_TEST_DATABASE_URL` branch is the one CI takes, so the container-start branch and its error paths are unmeasured | — |
| 03 | `crates/flakeradar-db/src/migration.rs` | `table_exists` | unreached branch | The `record not found` arm of the `SELECT EXISTS (…)` lookup. That query always returns exactly one row, so the arm cannot run | — |
| 03 | `crates/flakeradar-test-support/src/db.rs` | `Drop for TestDb` | unreached branch | Two error paths of the cleanup: the runtime failing to build, and the cleanup connection failing to open. Both happen only when a process cannot start a thread or reach a server it has just used, and neither can be provoked without breaking the test that runs them | — |
