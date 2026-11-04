# Coverage gaps

The goal is as close to 100% line and branch coverage as possible (see `00-shared-context.md`, "The coverage goal"). Log every line or branch that a task could not cover, and every `coverage(off)`.

Task 14 works through this list and closes what it can. Task 15 files a follow-on issue for each gap that remains.

| Task | File | Function | Kind | Why it is not covered | Closed by |
|---|---|---|---|---|---|
| 01 | `crates/flakeradar/src/main.rs` | `main` | `coverage(off)` | Process entry point: a test process cannot run it. The workspace has no other executable Rust yet; the crates hold doc comments only | — |
