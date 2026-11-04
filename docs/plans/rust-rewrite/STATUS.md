# Rust rewrite status

Branch: `feat/rust-rewrite`. Python reference: commit `97b933d`. Tasks run strictly in order.

Set `Done` and the last commit SHA when a task lands. If you are blocked, write `BLOCKED: <question>` in the Notes column and stop.

| Task | Title | State | Commit | Notes |
|---|---|---|---|---|
| 01 | Workspace, lints, nextest, CI skeleton, code standards | Done | f95dbcf | `cargo nextest` needs `--no-tests=pass` until task 03 adds the first tests. For the same reason the coverage job keeps its `has-coverage` guard (cargo-llvm-cov cannot export a report when no test has run) and prints a notice instead. Task 03 removes both. Rust coverage of added code: n/a, the crates hold doc comments only and `main` is `coverage(off)` (logged). |
| 02 | Golden recorder and pytest manifest | Pending | | |
| 03 | Baseline schema, entities, test databases, SeaORM spike | Pending | | |
| 04 | Core logic | Pending | | |
| 05 | Server shell | Pending | | |
| 06 | Ingest and Report routes | Pending | | |
| 07 | Processing, attribution, Score history | Pending | | |
| 08 | Worker and retention | Pending | | |
| 09 | Read APIs | Pending | | |
| 10 | Issue links, GitHub, webhooks, repo admin | Pending | | |
| 11 | MCP | Pending | | |
| 12 | Frontend embedding | Pending | | |
| 13 | CLI, Docker, release, five-target smoke test | Pending | | |
| 14 | Convergence | Pending | | |
| 15 | Cutover and the PR | Pending | | |
