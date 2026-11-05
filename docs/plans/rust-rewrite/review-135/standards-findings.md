# PR #135 standards review: "feat: replace the Python backend with one Rust binary"

Head f9330fd5, base 86c98804. Read-only review; nothing in the repo was edited, and no GitHub comment was made.

## Captured checks

`checks.json` has no `bucket: fail` entry. 18 pass, 1 skipped ("Merge the manifest list"). fmt, clippy `-D warnings`, nextest, doctests and audit passed in CI, so nothing below re-reports a lint unless it is suppressed.

## Sources read

Read in full: `AGENTS.md`, `CONTRIBUTING.md`, `docs/agents/code-standards-rust.md`, `docs/agents/domain.md`, `CONTEXT.md`, `SECURITY.md`, `Cargo.toml`, `clippy.toml`, `smell-baseline.md`, ms-rust `SKILL.md` and guideline files 01, 02, 03, 04, 08, 09, 10 (empty), 11, 12, 13, 14, 15.

Production Rust read in full: every file under `crates/flakeradar/src`, `crates/flakeradar-db/src` and `crates/flakeradar-core/src`, except the two partials below, up to its `#[cfg(test)]` module.

Partials, stated so nobody assumes more:
- `crates/flakeradar-test-support/src/factories.rs`: lines 1-290 read; the other builders scanned by grep only.
- `crates/flakeradar-db/src/entities/*`: two generated files sampled.
- `#[cfg(test)]` modules in `src/` and files under `tests/`: skimmed and grepped, not read line by line.

Not run: the full nextest suite (CI result used), `cargo clippy` (CI result used).

## Commands that settled findings

The proofs live in a scratch crate, `<S>/proof`, that path-depends on the repo's crates. `<S>` is `<S>`.

```
cd <S>/proof
CARGO_TARGET_DIR=<S>/target cargo test --test core_proof -- --nocapture --test-threads=1      # 5 pass
CARGO_TARGET_DIR=<S>/target cargo test --test worker_proof -- --nocapture                     # 2 pass
CARGO_TARGET_DIR=<S>/target cargo test --test settings_proof -- --nocapture --test-threads=1  # 3 pass
CARGO_TARGET_DIR=<S>/target cargo test --test route_proof -- --nocapture --test-threads=1     # page test panics
CARGO_TARGET_DIR=<S>/target cargo test --profile relproof --test route_proof page_past -- --nocapture  # no overflow checks, as release
CARGO_TARGET_DIR=<S>/target cargo test --test abort_proof -- --nocapture                      # process aborts: the proof
cd <S>/proof2; CARGO_TARGET_DIR=<S>/target2 cargo build --release                             # the repo's own release profile, flakeradar-core only
<S>/target2/release/pr135-proof2 suites 5000                                                  # exit 134
cd <repo>; cargo tree --locked -p flakeradar-core -e normal      # no sea-orm, axum, hyper, reqwest, tokio
cd <repo>; cargo doc --locked --no-deps -p flakeradar-db -p flakeradar-core -p flakeradar
```

Each proof test asserts the bad behaviour, so "pass" there means the defect reproduced.

---

# HARD VIOLATIONS

Worst first. H1 breaks no written rule, but it does more damage than the rule violations after it, so it sits second.

## H0. One 115 KB upload aborts the whole server

Severity: critical (token-gated; the standard treats uploads as attacker-controlled). Rule: code-standards "**Untrusted CI input never panics.** ... a malformed JUnit file ... produces a `Result`, a stored failure or a 4xx"; ms-rust 03 M-PANIC-IS-STOP. A stack overflow is worse than a panic: it cannot be caught. VERIFIED BY RUNNING, through the route and in the repo's release profile.

`crates/flakeradar-core/src/parsing.rs:274-287` recurses once per nested `<testsuite>`, and nothing limits the depth:

```rust
fn collect_cases_named(suite: Node<'_, '_>, suite_name: &str, line_base: i64, cases: &mut Vec<ParsedCase>) -> Result<(), ParseError> {
    for case in child_elements(suite, "testcase") {
        collect_case(case, suite_name, line_base, cases)?;
    }
    for nested in child_elements(suite, "testsuite") {
        collect_cases_named(nested, suite_name, line_base, cases)?;
    }
```

`roxmltree` 0.21 limits entity depth and, optionally, node count, but not element depth, and `ParsingOptions::default()` (`parsing.rs:219`) sets neither limit on elements. Tokio's blocking threads have 2 MiB stacks and `main.rs:84` does not change that.

Proof, `POST /api/ingest` with `<testsuite>` nested 5,000 deep, 115,020 bytes:

```
thread 'tokio-rt-worker' has overflowed its stack
fatal runtime error: stack overflow, aborting
```

The same in the repo's exact release profile (opt-level 3, LTO, one codegen unit) on a 2 MiB thread, parser only:

```
suites depth 1000: 23020 bytes -> ok, 1 cases
suites 5000: thread has overflowed its stack, fatal runtime error: stack overflow, aborting, exit=134
other depth 800000: 5600053 bytes -> ok, 1 cases     (equally deep, not in <testsuite>, so the parser itself is safe)
```

The abort happens in `parse_upload` (`routes/reports.rs:493`) before any row is written, so `spawn_blocking` does not contain it. It takes down the API, the Report processor and every in-flight request. A restart recovers, and the next such upload repeats it. `tests/parser_robustness.rs` has no depth case.

Fix: walk the suites with an explicit `Vec` stack, or refuse depth past a documented constant with `ParseError::InvalidXml`. Add the 5,000-depth case to `parser_robustness.rs`.

## H1. GitHub filing and Notifications die permanently after the first leadership loss

Severity: high. Rule: none directly. It breaks the contract in `worker.rs:1-14` ("take over when the leader stops or its connection dies") and `LeaderTask` ("starts with leadership and stops before the lock is released"). VERIFIED BY RUNNING for `WebhookWorker`. `GitHubWorker` has the same code by reading.

`crates/flakeradar/src/webhooks.rs:278-298`, `crates/flakeradar/src/github_worker.rs:175-196`:

```rust
let shared = Arc::clone(&self.shared);
let cancel = self.cancel.clone();
*task = Some(tokio::spawn(async move { shared.run(&cancel).await }));
...
fn stop(&self) -> StopFuture<'_> {
    Box::pin(async move {
        self.cancel.cancel();
```

`cancel` is a field that is never replaced, and a `CancellationToken` cannot be reset. `run` opens with `while !cancel.is_cancelled()`, so a task started after any `stop` exits at once.

Scenario: the leader's Report processing returns one DB error, or `lock.check()` fails. `worker.rs:573-577` runs `drop_leadership`, which runs `release`, which calls `task.stop()`. The next loop wins the lock again and `take_leadership` calls `task.start()`. Reports keep processing, but no GitHub issue or Notification goes out until the process restarts, and nothing is logged. `GitHubWorker.queue` has no bound (`github_worker.rs:199`), so it also grows forever. `WebhookShared` caps at `QUEUE_MAX`, then drops the oldest batch.

Proof output (`worker_proof::a_webhook_worker_never_works_again_after_one_stop`):

```
first term: queue drained = true
second term: queue drained = false, queued = 1
```

No test enqueues after a restart and checks delivery. The re-election tests near `tests/worker.rs:902` use `RecordingTask`, not a real worker, and `tests/github_worker.rs:804-812` only pins that an enqueue after stop is not filed.

Fix: create a fresh `CancellationToken` in `start` (hold it in the mutex with the handle). Add a start, stop, start, enqueue test.

## H2. `page` overflows an `i64` on an unauthenticated route

Severity: high. Rule: code-standards "Errors and panics", "never a panic and never an arithmetic overflow". VERIFIED BY RUNNING.

`crates/flakeradar-db/src/reads.rs:387`:

```rust
values.push(Value::from((self.page - 1) * self.page_size));
```

`crates/flakeradar/src/routes/reads.rs:171-175` gives `page` no upper bound:

```rust
check_ge(page, 1, "page", issues);
check_ge(page_size, 1, "page_size", issues);
check_le(page_size, 100, "page_size", issues);
```

Results:
- `GET /api/tests?page=9223372036854775807&page_size=100`, dev or test build: `panicked at crates/flakeradar-db/src/reads.rs:387:33: attempt to multiply with overflow`.
- The same request with overflow checks off, which is what `[profile.release]` gives: the offset wraps negative, PostgreSQL refuses it, and the answer is 500. `/api/jobs` is the same.
- `GET /api/tests?page=4611686018427387905&page_size=4`: the offset wraps to 0 and the answer is 200 with `"page":4611686018427387905`, carrying page one's rows under that page number.

Fix: bound `page` so the product fits (`check_le` in `validate_paging`, 422), and use `checked_mul` in `Board::run`.

## H3. Allocation grows quadratically from one upload

Severity: high (token-gated; the standard still calls uploads attacker-controlled). Rule: code-standards "Untrusted CI input never panics"; ms-rust 14 M-MOCKABLE-SYSCALLS, "code handling external input should provide bounded and / or chunking operations". The module doc is also wrong: `parsing.rs:6-7` says "Sizes are capped here so storage never sees more." Names are not capped. VERIFIED BY RUNNING at small scale.

`crates/flakeradar-core/src/parsing.rs:326-336` and `:363-373`:

```rust
cases.push(ParsedCase {
    suite: suite_name.to_owned(),
    classname: classname.to_owned(),
    name: name.to_owned(),
```

One attribute is copied once per case. There are two routes to it: a long `<testsuite name>` with many `<testcase>` children, or a long `<testcase name>` with many `<rerunFailure/>` children.

```
body 168921 bytes -> 5000 cases -> 250000000 bytes of suite names (1479x)
body 125061 bytes -> 5001 cases -> 250050000 bytes of names (1999x)
```

Scaled to the 20 MiB limit, a 10 MiB name plus about 500,000 short cases asks for about 5 TB. An allocation failure aborts the process, so `spawn_blocking` cannot contain it, and the abort takes out the API and the processor together. It happens in `parse_upload` (`routes/reports.rs:493`), before any row exists. `test_cases.suite`, `classname` and `name` are `text` columns, so storage does not refuse a long name either.

Fix: refuse a suite, classname, name or file over a documented length as `ParseError`, and hold repeated strings as `Arc<str>` rather than copying per case. Then correct the module doc.

## H4. A malformed `.env` line writes the token to the log

Severity: medium-high. Rule: code-standards checklist "No secrets or Failure details in logs"; `SECURITY.md:31-33` "it never logs a token or a Webhook URL". VERIFIED BY RUNNING.

`crates/flakeradar/src/settings.rs:546-550`, reaching `main.rs:48` (`tracing::error!(error = ?error, ...)`):

```rust
let (key, value) = entry.map_err(|source| SettingsError::EnvFile {
    path: path.display().to_string(),
    source: std::io::Error::other(source.to_string()),
})?;
```

`dotenvy`'s parse error prints the whole raw line. With `.env` holding `FLAKERADAR_API_TOKEN="s3cret-token` and no closing quote:

```
reading .env: Error parsing line: 'FLAKERADAR_API_TOKEN="s3cret-token\n', error at line index: 35
```

Fix: report the path and the line number only, and add a test that the error text omits the value.

## H5. The database password prints through `Debug`

Severity: medium. Rule: code-standards "Types holding a secret ... implement Debug manually with redaction, plus a unit test"; ms-rust 11 M-PUBLIC-DEBUG. VERIFIED BY RUNNING. No log line prints these today, so this is exposure rather than a live leak.

`settings.rs:212-217`, `worker.rs:214-216`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub api_token: Secret,
    pub database_url: String,
...
#[derive(Debug)]
pub struct AdvisoryLock {
    url: String,
```

```
AdvisoryLock { url: "postgresql://flakeradar:s3cret-password@db:5432/flakeradar", settings: ..., session: ... }
```

`AppState` derives `Debug` over `Arc<Settings>`, so it prints the password too. The existing redaction test (`settings.rs:835-850`) checks the token, the GitHub token and the Webhook URL, and not this field. `Secret`'s own doc claims a derived `Debug` "prints the wrapper's redacted form, not the secret".

Fix: hold the URL in `Secret`, and extend the test.

## H6. No newtype for a fingerprint, a score or a CI run ID, which the standard names

Severity: medium, wide. Rule: code-standards "Strong types over primitive obsession (M-DESIGN-FOR-AI): a newtype for a fingerprint, a score, a CI run ID"; baseline Primitive Obsession.

`grep 'struct \w+\('` over `crates/*/src` finds `Secret`, three request-body wrappers and two test sinks. None of them is a domain newtype.

- `parsing.rs:119`: `pub fn fingerprint(suite: &str, classname: &str, name: &str) -> String`
- `dto.rs:964`: `pub fingerprint: String`; `dto.rs:976`: `pub flakiness_score: f64`; `dto.rs:661`, `:858`: `pub ci_run_id: String`
- `scoring.rs:136-141`: `combined_score(...) -> (f64, usize)`

Enums exist and the DTOs still hold strings: `dto.rs:1059` `status: String` beside `ExecutionStatus`; `dto.rs:982`, `:1078` `failure_category: Option<String>` beside `FailureCategory`; `dto.rs:508`, `:848`, `:860` Report `status` and `kind`, with `report_intake.rs:20-35` as bare string constants. Each conversion is hand-written again (`processing.rs:1240-1250`, `reads.rs:1356-1369`, `github.rs:442`, `ops.rs:28-31`).

Consequence you can reach: `scoring::branch_scoped_score(history: &[(&str, &str, ExecutionStatus)], ...)` takes `(commit_sha, branch, status)`, and swapping the first two compiles and silently scores wrong.

Fix: `Score` with a fallible constructor that refuses NaN and values outside 0-1 (this also closes J2's scoring half), `Fingerprint`, `CiRunId`, a typed Report status and kind. Serde `transparent` keeps the wire unchanged.

## H7. Tests wait on the real clock

Severity: medium. Rule: code-standards "**No test depends on real time.** ... never `tokio::time::sleep` against the real clock". `grep start_paused|time::pause|time::advance` finds one test (`tests/parity.rs:2025`).

Negative assertions after a fixed sleep: `tests/webhooks.rs:200`, `:271`; `tests/github_worker.rs:214`, `:808`, `:858`, `:889`, `:1043`, `:1108`.

```rust
LeaderTask::enqueue(&worker, &processed(&[])).expect("the queue takes the outcome");
tokio::time::sleep(Duration::from_millis(50)).await;
LeaderTask::stop(&worker).await.expect("the task stops");
assert!(server.received_requests().await.expect("a record").is_empty());
```

On a loaded runner the task may not run inside 50 ms, so these pass without testing anything, and a regression goes unseen.

Polling against wall-clock deadlines: `tests/worker.rs:107-113`, `tests/server.rs:220-238`, `tests/webhooks.rs:92`, `:151`, `:453`, `tests/github_worker.rs:63`, `:327-333`, `tests/parity.rs:214-216`, `:1530-1551`. The comment at `tests/worker.rs:100` argues the deadline "is a bound on a failure". The doc's rule has no such exception, so either change the rule in the standard or change the tests.

Also `test-support/src/factories.rs:26-28`: `now()` stamps rows with `Utc::now()`, so a row's age depends on the wall clock unless the test sets it.

Fix for the negative cases: have the task signal a drained batch through a seam, then assert. Real sockets conflict with a paused clock, so wiremock-driven tests need the seam rather than `advance`.

## H8. Production reads the real clock past the seam

Severity: medium. Rule: code-standards "The clock ... injected"; checklist "New I/O reached through an injected seam, not a real clock"; ms-rust 14 M-MOCKABLE-SYSCALLS.

- `routes/reports.rs:498`, `:684`: `let now = Utc::now().fixed_offset();`
- `github.rs:1294`: `let now = Utc::now();`; `github.rs:1676`: `Utc::now(),`
- `settings.rs:323`, `:337`: `clock: std::sync::Arc::new(flakeradar_db::SystemClock),` hard-coded, so a request-level test cannot inject a clock, and `reads()` allocates an `Arc` per request.

`AppState` carries no clock. `ProcessingSettings` and `ReadSettings` show the seam exists and was simply not extended.

## H9. Lint overrides that break the override rules

Severity: medium for the first, low for the rest. Rule: code-standards "Case-by-case opt-outs happen in the config with a comment, never per-file"; "`#[expect]` with a reason for every override"; "Unit tests are exempt through a crate-level `cfg_attr(test, expect(...))` in `lib.rs`".

- `crates/flakeradar/src/mcp.rs:19-23` is a per-file opt-out:
  ```rust
  #![expect(
      clippy::needless_raw_strings,
      reason = "the texts are verbatim copies of the Python strings, so a raw literal is \
                what makes a diff against them readable"
  )]
  ```
  The Python source it stays diffable against is deleted by this PR. None of the file's 12 raw literals contains a double quote or a backslash (checked by script), so a plain string literal would read and diff the same, and the opt-out can simply go.
- `crates/flakeradar/tests/common/mod.rs:13-16`: `#![allow(dead_code, reason = "...")]`. It is the only `#[allow` in the workspace. It exists because helpers live outside `flakeradar-test-support`; see H14.
- `crates/flakeradar-db/src/reads.rs:2156`, `deletes.rs:367`, `crates/flakeradar-test-support/src/db.rs:363`: `#![expect]` at module level inside `mod tests`. `flakeradar-db/src/lib.rs` and `flakeradar-test-support/src/lib.rs` carry no crate-level `cfg_attr(test, ...)`, as the standard describes and as the other two crates do.

## H10. Rustdoc describes the design journey and a deleted codebase

Severity: medium, very wide. Rule: code-standards "No design-journey essays in doc comments (M-NO-META-DESIGN-DOCUMENTATION) ... rustdoc describes end-state behavior"; ms-rust 01.

191 `///` or `//!` lines across 39 files name the Python server, FastAPI, Pydantic, Starlette, asyncpg, fastmcp or "the reference". `git ls-files backend` is 0 at head, so a reader cannot follow them. Worst files: `dto.rs` 33, `mcp.rs` 16, `extract.rs` 16, `routes/reports.rs` 14, `settings.rs` 13.

```
processing.rs:4   //! This is the frozen Python server's `processing.py`, statement by statement:
parsing.rs:7      //! here. `backend/app/parsing.py` is the reference.
routes/reports.rs:7 //! `backend/app/routers/reports.py` is the reference.
factories.rs:4    //! `backend/tests/factories.py`
routes/ops.rs:338 /// The frozen server ran that query outside its `try`, so the unhandled
                  /// `SQLAlchemyError` reached Starlette's error handler ...
mcp.rs:1437       /// the frozen server's transport (fastmcp 4.1.6, measured over a loopback port)
```

Task numbers, which go stale the day the plan closes: `worker.rs:10`, `:63` ("Task 10 implements this"), `middleware.rs:23` ("task 11"), `routes/issues.rs:4`, `routes/mod.rs:1`, `:4` ("from task 06 on"), `app.rs:4`.

Plan files in rustdoc: `reads.rs:6` (`spike-notes.md`), `main.rs:27`, `mcp.rs:688` (`differences.md`).

Fix: say what the code does. Move the parity notes to `docs/plans/rust-rewrite/`.

## H11. The compliance marker is on files that do not comply

Severity: medium. Rule: code-standards "Add it only to files you have actually checked — a stale marker is worse than none."

57 files carry `// Rust guideline compliant 2026-07-21`. I picked the files below and each fails a rule cited here:

| File | Fails |
|---|---|
| `flakeradar/src/mcp.rs` | per-file lint opt-out (H9), M-NO-META (H10) |
| `flakeradar-db/src/reads.rs` | arithmetic overflow (H2), `Result<_, String>` (H13), M-DOCUMENTED-MAGIC (H12) |
| `flakeradar/src/worker.rs` | task numbers in rustdoc, inline `5`/`30` second values, formatted log lines (H16), `Debug` secret (H5) |
| `flakeradar/src/settings.rs` | secret in an error (H4), `Debug` secret (H5), magic defaults (H12) |
| `flakeradar-core/src/parsing.rs` | unbounded recursion (H0), unbounded allocation (H3), false module claim, deleted-file reference |
| `flakeradar-core/src/dto.rs` | wrong doc (H18), primitives (H6), inline limits (H12) |
| `flakeradar-db/src/processing.rs` | "statement by statement" module doc, formatted log, magic `Default` |
| `flakeradar-db/src/issue_links.rs` | broken doc link (H18) |

The 39 files in H10 fail on that rule alone, which is most of the 57. The marker tells the next reader these files were checked, so it does damage here. Fix: remove the markers, then add them back file by file as each is fixed.

## H12. Magic values without the required comment, or without a name

Severity: medium. Rule: code-standards "Magic values are named constants with a doc comment saying why the value, what breaks if it changes, and what external system it interacts with"; ms-rust 11 M-DOCUMENTED-MAGIC.

Inline with no name (validation limits, all repeated by hand):
- `dto.rs:555-558`: `check_string(&self.ci_job_id, 1, 255, ...)`, `512`, `2048`; `:571` `50`; `:684-695`; `:738` `1000`; `:1333` `500`; `:1348` `365`; `:1548` `2048`
- `routes/reports.rs:239-290`: `255`, `100`, `1024`, `64`, `512`
- `routes/reads.rs:174` `100`; `:180` `90`; `:321-327`, `:359` `50`; `:873` `255`
- `worker.rs:386`, `:388`: `Duration::from_secs(5)`, `Duration::from_secs(30)`
- `github.rs:572`: `sha.chars().take(10)`

The same default written in four places, none commented:
- `settings.rs:281-306` (`Default`) and `settings.rs:357-382` (`from_map`)
- `processing.rs:72-76` and `reads.rs:87-89`: `flake_threshold: 0.30`, `score_decay: 0.85`, `score_window: 50`
- `retention.rs:68-73`

Scenario: change the default threshold in `Settings` and `Settings::default()`, `ProcessingSettings::default()` and `ReadSettings::default()` disagree silently, and only tests use the last two.

Named but saying only what, not why: `parsing.rs:29` `DETAILS_MAX`; `helpers.rs:16`, `:19` `TREND_DAYS`, `TREND_DELTA`; `reads.rs:38-59` (seven constants); `processing.rs:41` `ERROR_MAX`; `webhooks.rs:26`, `:36`, `:39`; `github.rs:75-87`; `mcp.rs:84-90`; `score_history.rs:18`; `attribution.rs:20`; `routes/reports.rs:68`, `:71`.

Done right, for comparison: `connect.rs:16-23` `ACQUIRE_TIMEOUT`, `batching.rs:8-14` `CHUNK`, `cli.rs:18-22`.

## H13. Stringly error in the DB crate, and a public error type that is not an error

Severity: low-medium. Rule: code-standards "`flakeradar-core` and `flakeradar-db` carry canonical error structs, as `thiserror` enums"; ms-rust 15 M-ERRORS-CANONICAL-STRUCTS.

- `flakeradar-db/src/reads.rs:121`: `pub fn new(repo: Option<String>, project: Option<String>) -> Result<Self, String>`
- `flakeradar-db/src/processing.rs:118-124`: `pub struct ProcessError { pub transient: bool, pub message: String }`, exported at `lib.rs:69`, with no `Display` or `Error`, and used nowhere outside the module. Make it private, or make it a real error.

## H14. Test helpers outside `flakeradar-test-support`, and test hooks in the release binary

Severity: medium for `urls()`, low for the rest. Rule: code-standards "Test-only helpers — test databases, factories, mocks — live in `flakeradar-test-support` ... so no test helper reaches a release binary (M-TEST-UTIL)"; ms-rust 14 M-TEST-UTIL lists "the ability to inspect sensitive data".

In release code:
- `webhooks.rs:266-270`: `/// The Webhook URLs the task posts to, for a test.` `pub fn urls(&self) -> Vec<String>`. This exposes a credential, and its only caller is `server.rs:191`, in a unit test.
- `webhooks.rs:254-264`: `queued()`, `dropped()`, documented "for a test".
- `settings.rs:405-410`: `with_api_token`, "for a test"; its only caller is `settings.rs:907`.
- `flakeradar-db/src/clock.rs:38-56`: `FixedClock`, "for tests". The only users are under `tests/`. `ScriptedMonotonic` sits correctly in test-support, so this is inconsistent.

In tests:
- `crates/flakeradar/tests/common/mod.rs` (217 lines), including the `MemoryAssets` mock at `:51`.
- Fakes per file: `tests/worker.rs:286-530` (`RecordingTask`, `CountingLock`, `FailingLock`, `FailingUnlockLock`, `ScriptedProcessor`); `tests/github_worker.rs:586`.

## H15. Items reachable through two paths, versions outside the workspace

Severity: low-medium.

- ms-rust 01 M-SINGLE-ITEM-PATH: `flakeradar-core/src/lib.rs:20-59` declares `pub mod classify;` and then `pub use classify::{FailureCategory, ...}`, which is the guideline's own "Not OK" example. `flakeradar-db/src/lib.rs:15-28` does the same for `attribution`, `issue_links`, `processing`, `reads`, `retention`, `score_history`, as does `flakeradar/src/lib.rs:25-79`. Callers already mix them: `mcp.rs:43` uses `flakeradar_db::reads::{...}` and `routes/reads.rs:22` uses `flakeradar_db::{...}`.
- Code-standards "Versions live in `[workspace.dependencies]`"; ms-rust 09 M-CARGO-WORKSPACE; `Cargo.toml:19` itself says "Versions live here and nowhere else": `crates/flakeradar/Cargo.toml:40` `mimalloc = { version = "0.1", ... }`, and the `futures-util = { version = "0.3", ... }` dev-dependency.
- ms-rust 09 M-CRATES-IN-WORKSPACE: siblings link by path, which the guideline marks "Bad": `flakeradar-core = { path = "../flakeradar-core" }` in `crates/flakeradar/Cargo.toml` and `crates/flakeradar-db/Cargo.toml`, plus `flakeradar-db` and `flakeradar-test-support` entries.
- `Cargo.toml:35` `schemars` and `:54` `testcontainers` are declared and no member uses them. `CONTRIBUTING.md:43`, `:50` say testcontainers starts Postgres; `flakeradar-test-support/src/postgres.rs:131` shells out to `docker`.

## H16. Logging that is not structured

Severity: low-medium. Rule: ms-rust 11 M-LOG-STRUCTURED ("Avoid String Formatting", "Name Your Events").

Grep finds 79 `tracing` call sites and 0 with `name:`. Five format the message first, one of them deliberately to get around the check:

```rust
worker.rs:585-588     let message = format!("the {name} leader task failed to take processed Report {report_id}: {error}");
                      tracing::error!("{message}");
worker.rs:637-640     same shape
retention.rs:517-528  let summary = format!("Pruned {} reports, ..."); info!("{summary}");
retention.rs:512      info!("Retention prune hit its {budget}s budget; ...");
processing.rs:1755    tracing::info!("Rescore of Repo {repo_id} dropped {dropped} Notification(s)");
```

Fix: named properties, as `webhooks.rs:149-155` already does for most of the call.

## H17. Glossary violations in identifiers

Severity: low-medium. Rule: code-standards "Identifiers use `CONTEXT.md` vocabulary ... never the glossary's avoided synonyms".

- **Report**, _Avoid: Upload, payload_: `routes/reports.rs:622` `parse_upload`, `:603` `upload_error`; `report_intake.rs:229` `stored_pipeline_payload`; `error.rs:39` `DbError::StoredPayload`; `dto.rs:796` parameter `payload`, and the local throughout `processing.rs`.
- **Proven flake**, _Avoid: Confirmed flake, same-SHA flip (in user-facing text)_: `scoring.rs:102` `count_same_sha_flips`, `:20` `SAME_SHA_FLOOR`, `:143` `confirmed`, `notify.rs:70`. The JSON key `confirmed_flake_count` is a wire and column name, which `flakeradar-db/src/lib.rs:32-35` acknowledges; the Rust-only names are not bound by that.
- **CI run attempt**, _Avoid: Attempt (alone)_: `report_intake.rs:177` `stored_attempt`, `routes/reports.rs:309` `AttemptValue`, `:320` `attempt_value`. An Execution's `attempt` means something else, so this is the exact confusion the glossary guards against. It also misleads at `routes/reports.rs:512`: `line_base: Some(stored_attempt(line_base))`, baseline Mysterious Name.

## H18. Docs that are wrong, and one validation `loc` that points at the wrong Job

Severity: medium for the `loc`, low for the docs. Rule: M-CANONICAL-DOCS; the doc claim itself at `dto.rs:545-546` ("each issue's `loc` points at the field a client has to fix"). VERIFIED BY RUNNING for the `loc` and the links.

`dto.rs:560-567`, then `:597-599`:

```rust
for (label_index, label) in self.runner_labels.into_iter().enumerate() {
    ...
    issues.push(ValidationIssue::string_too_long(
        job_loc_with(label_index, "runner_labels"),
...
fn job_loc(index: usize, field: &str) -> Vec<LocSegment> {
    job_loc_with(index, field)
}
```

`job_loc_with`'s first argument is the Job index. For `jobs[2].runner_labels[0]`:

```
loc: ["body","jobs",0,"runner_labels"]
```

Wrong Job, and no label position. `job_loc` is a pure pass-through (baseline Middle Man), which is how the slip got through. Fix: `[body, jobs, index, runner_labels, label_index]`.

`cargo doc` warnings (CI runs doctests, not `cargo doc`):

```
warning: unresolved link to `LinkTarget::data`            --> crates/flakeradar-db/src/issue_links.rs:11:32
warning: `connect` is both a function and a module        --> crates/flakeradar-db/src/lib.rs:8:7   (--document-private-items)
warning: public documentation for `stop` links to private item `ReportWorker::release`  --> crates/flakeradar/src/worker.rs:513:48
```

Other wrong or garbled docs:
- `settings.rs:314-329`: the `reads()` doc opens with "The processor's view of these settings, with the system clock", and `processing()` gets the text about the database layer. The two were interleaved.
- `dto.rs:808`: `normalized_name` "Records a maximum-length failure, returning whether the value fits." It returns a `String`.
- `webhooks.rs:254`: "The home of the batches queued and dropped so far"; `routes/reads.rs:98`: "`GO /api/quarantine`"; `dto.rs:1037`, `:1296`: "How many flew at least one Proven flake"; `processing.rs:1685`: "Rescales every Job"; `app.rs:30`: "lifts the limit entirely", while `routes/reports.rs:106` sets `DefaultBodyLimit::max(INGEST_BODY_LIMIT)`.
- `docs/agents/code-standards-rust.md:117`: "The `0.5` default Flake threshold". The default is `0.30` (`settings.rs:293`, and base `backend/app/config.py:71`).

## H19. NUL and non-finite values do not produce a 4xx

Severity: low-medium. Rule: code-standards "a malformed JUnit file, a 10 MB timestamp or a NaN score produces a `Result`, a stored failure or a 4xx". VERIFIED BY RUNNING.

NUL in text, which PostgreSQL `text` cannot hold:

```
POST /api/ingest commit_sha=abc%00                -> 500 Internal Server Error
POST /api/ingest/pipeline branch=m\u0000n         -> 500 Internal Server Error
GET  /api/tests?q=a%00b                           -> 500 Internal Server Error (unauthenticated, logs at error level)
```

`app.rs:239` already refuses NUL for asset paths. Fix: refuse it in `check_string` and the query validators.

Non-finite `time`: `parsing.rs:515-523` `python_float` accepts it, and `parse_junit_xml` returns it:

```
time=NaN -> NaN   time=1e999 -> inf   time=inf -> inf   time=-5 -> -5.0
```

The value is stored as is. `ExecutionOut.duration: f64` then serializes as JSON `null`, while the OpenAPI schema says number. The rule's own example is a NaN score rather than a NaN duration, so this one is a close reading. Fix: `is_finite() && >= 0`, else `ParseError::InvalidTime`.

## H20. Unit tests that belong in `tests/`, and tests that restate the code

Severity: low. Rule: code-standards "Tests that only touch a public API are integration tests and live in `tests/`, not `mod tests {}`" (M-INTEGRATION-TESTS); "never restate ground truth from the implementation" (M-TAUTOLOGICAL-TESTS).

About 3,170 lines of `#[cfg(test)]` sit in `src/`. Modules that use only public items: `flakeradar-core/src/helpers.rs:148`, `identity.rs:178`, `model.rs:190`, `py.rs:128`, `classify.rs:203`, `flakeradar-db/src/batching.rs:28`. `multipart.rs:363-878` is 515 lines of tests of `pub fn file_field`, with only two tests touching private items (`Needle`, `comparisons`).

Restating: `settings.rs:613-649` asserts every default against the same literal; `model.rs:195-214`, `:217-236`; `classify.rs:261-268`.

---

# JUDGEMENT CALLS

## J1. One transaction held across up to 4,000 GitHub calls

`github.rs:1273-1313` opens a transaction and then, for each row, does an HTTP GET (15 s timeout) and `stamp_checked`, an `UPDATE` that takes the row lock until the commit. `Settings::github_sync_batch` is 2000 per target.

From reading, not run: a Report touching a stamped Test blocks in `write_test_updates`, hits the 30 s statement timeout (SQLSTATE 57014), `transient_sqlstate` treats class `57` as transient, and the Report spends its retries. It also holds a pooled connection throughout. M-THROUGHPUT.

**Correction after the side-by-side run (spec finding 2): this is a regression, not inherited.** Python only sets ORM attributes during the loop and flushes them in the final commit, so it holds no row lock during HTTP. Treat J1 as must-fix. Commit per row or per small batch, as `retention.rs` does.

## J2. Settings ranges are not checked

Ms-rust 14 M-STRONG-TYPES-GUARD applied to configuration. VERIFIED BY RUNNING:

```
FLAKERADAR_SCORE_DECAY=NaN, FLAKE_THRESHOLD=-3, SCORE_WINDOW=-1, WORKER_POLL_SECONDS=-1
-> accepted: decay=NaN threshold=-3 window=0 poll=0ns
all-passing history with decay NaN scores 1
```

`score.min(1.0)` turns NaN into 1.0 (`scoring.rs:150`), so a typo marks every Test with three or more runs flaky. `poll=0` makes the worker loop query with no wait. Operators are trusted, so this is not the untrusted-input rule. `DbSettings::new` shows the right pattern (`connect.rs:46-52`).

## J3. Unbounded GitHub queue

`github_worker.rs:199` `guard(&self.shared.queue).push_back(outcome.clone())` has no bound. The Webhook queue has `QUEUE_MAX`. With H1 it never drains.

## J4. CPU-heavy work on an async worker

`processing.rs:485` `record_test_executions` and `:476` `upsert_test_cases` run on the async thread and hash every case (`flakeradar_core::fingerprint` at `:570`, `:651`, and again at `:663`), so three SHA-1s per case and BTreeMap work, up to about a million cases, with no yield. The parse beside it is deliberately moved off the thread (`:435`). M-YIELD-POINTS.

Similarly `routes/reports.rs:429` parses up to 20 MiB of JSON inline, and `mcp.rs:1452` parses the body once only to validate it.

## J5. Needless copies of large data

`processing.rs:434` (`body.to_vec()`), `:438` (clones every `ParsedCase`, including 16 KB of details, so a function can read names), `:488` (`chunk.to_vec()`), `routes/reports.rs:493` (`content.clone()`), `:560`. M-MEM-REUSE.

## J6. N+1 update

`retention.rs:456-473`: `clear_expired_quarantines` loads full rows and issues one UPDATE each. One `UPDATE ... RETURNING` does it.

## J7. Duplicated code (baseline)

- `get` and `rows`: five copies (`processing.rs:203`, `:209`, plus `:1107` `rows_of`; `reads.rs:170`, `:176`; `attribution.rs:154`, `:160`; `score_history.rs:80`, `:86`; `github.rs:447`, `:453`). `processing.rs:1107` exists only for its error type.
- `.map_err(ProcessError::from)?` 24 times, where `?` already applies `From` (M-FROM-ERROR).
- `rescore`/`rescore_jobs`, `test_tier_changes`/`job_tier_changes`, `old_test_scores`/`old_job_scores`, `write_test_updates`/`write_job_updates`.
- The GitHub permalink: `github.rs:633` against `helpers.rs:123`. `category_from_wire` (`processing.rs:1240`) against `FailureCategory::ALL`. `values_of` (`score_history.rs:137`) against `int_array` (`attribution.rs:28`). `ops.rs:31` `REPORT_STATUSES` against `flakeradar_db::REPORT_STATUSES`. `scope_conditions` against `scope_filters` (`reads.rs:241`, `:718`).
- Route errors: four near-identical enums with the same plain-text 500 (`routes/reports.rs:129-171`, `routes/reads.rs:41-94`, `routes/issues.rs:31-93`, `routes/admin.rs:31-90`).
- Three identical `FromRequest` bodies (`PipelineBody`, `QuarantineBody`, `LinkBody`) next to the generic `extract::ApiJson`. `TestPath` and `JobPath` exist twice; `ScopeQuery` and `SummaryQuery` are the same.

## J8. Python shapes carried into Rust (M-RUST-SHAPED)

The wire parity is the spec. These go beyond the wire or read as ports:
- Python exception names as stored error prefixes: `processing.rs:143-145` `"OperationalError"`, `"DBAPIError"`; `:152` `"DataError"`; `:786` `"ValidationError"`; `:437` `"RuntimeError"`. Check whether a golden actually needs these.
- `extract.rs:269-293` infers Pydantic error kinds by substring match on serde's English messages, which any serde release can change. Tests pin it, but it is fragile.
- `multipart.rs` is a hand-written 330-line parser with its own KMP search, written to mimic python-multipart's quirks. Its comments say why; the reader should still weigh this against `multer`. The cost is that the "reference parser" it follows is now gone from the repo.
- `#[cfg(test)] comparisons::record()` inside production `Needle::find` (`multipart.rs:310`, `:323`).

## J9. Layering

Raw SQL in the server crate: `github.rs` (`:550`, `:854`, `:907`, `:966`, `:1528`, `:1565`), `routes/reports.rs:78-96`, `:724`, `routes/ops.rs:40-72`. `CONTRIBUTING.md:146` says `reads.rs` holds "every read". `flakeradar-db` exports SQL text as public API (`attribution.rs:44`, `:111`, `:128`), taking table aliases as `&str`. `IssueLinkError::status()` (`issue_links.rs:99`) returns HTTP codes from the DB crate.

## J10. `Scope` guards an invariant it does not enforce

`reads.rs:106-127` has public fields, `Default`, and a fallible `new`, and `mcp.rs:714` builds it with a struct literal. Ms-rust 14 M-STRONG-TYPES-GUARD.

## J11. `unwrap_or_default` on the HTTP client builder

`webhooks.rs:240-244`, `github.rs:321-325`: a failed build silently yields `Client::default()` with no timeout, no user agent, and a panic if TLS cannot initialize. The timeout is the point of the module docs ("a slow or unreachable endpoint"). Return an error at construction.

## J12. Attempt past `i32`: refused on one route, rewritten on the other

`routes/reports.rs:295-302` says "refused with 400 ... rather than rewritten into a value CI never sent". `report_intake.rs:172-179` saturates for the Pipeline route, and `PipelineReportIn::validate` sets no ceiling. The stored body keeps the original number, so the column and the body disagree.

## J13. Silent failures against the stated policy

`github.rs:23-24` says "Every failure is logged". `github.rs:1085-1093` `.ok().flatten()` drops a `get_job` error unlogged, and `routes/ops.rs:157` `Err(_) =>` drops the scrape's error.

## J14. Dead parameters

`github.rs:1417`, `:1436` `let _ = settings;`; `github.rs:1765` `let _ = (db, settings);`; `issue_links.rs:410` `repo_of(_target, ...)`; `issue_links.rs:256-259` `match target { Test | Job => ... }` where both arms are one; `processing.rs:1719-1724` `rescore_repo` returns ids no caller reads and takes `report_tier_changes` only to count it. Baseline Speculative Generality.

## J15. Data clumps and argument order

`test_counts -> (i64, i64, i64, i64, i64)` read as `counts.0..4` (`reads.rs:780`, `:762-767`). `BTreeMap<i32, (i32, i32)>` for day counts. `permalink(repo, sha, root, file, line)`, four `&str` in a row (`helpers.rs:123`). M-PARAMETER-CONSISTENCY: `ci_run_test_failures(db, repo_id, &repo_name, ci_run_id, attempt, ...)` against `ci_run_failed_jobs(db, repo_id, ci_run_id, attempt, &repo_name, ...)` (`reads.rs:1846`, `:1848`).

## J16. Dependency injection through `dyn`

`worker.rs:405-410` has four `Arc<dyn ...>` seams. M-DI-HIERARCHY prefers an enum when the second implementation is only for tests. The seams work and are documented; this is a preference.

## J17. Guideline items not applied, each defensible

- M-MIMALLOC-APPS: musl targets only (`main.rs:23-30`, documented).
- M-TARGET-CPU: no `.cargo/config.toml`; a multi-architecture release makes this reasonable.
- M-STATIC-VERIFICATION: no `cargo-udeps` (it would catch `schemars` and `testcontainers`); no `cargo doc` gate (it would catch H18's links); no `cargo-hack` (no features) and no miri (no `unsafe`), which are fine.
- M-MSRV: no `rust-version`, though `rust-toolchain.toml` pins 1.99.0.
- M-PUBLIC-DISPLAY: `ReportError`, `ReadRouteError`, `IssueRouteError`, `AdminRouteError` have no `Display` or `Error`.

## J18. A small parser bug

`parsing.rs:539-564` assumes `Nd` digits sit in blocks of ten. U+1D7CE..U+1D7FF is a run of 50, so `time="\u{1D7D9}"` (double-struck one) reads 9.0. Python gives 1.0. VERIFIED BY RUNNING. Rare input; flagged because the function's comment states the false assumption.

---

# CHECKED AND CLEAN

- No `unsafe` anywhere; `10_safety.md` not needed.
- `tracked in` sites: none in code.
- Lint set matches the standard's claims: every category including `nursery`; 20 of 21 M-STATIC-VERIFICATION restriction lints, with `string_to_string` absent as documented; `indexing_slicing`, `string_slice`, `iter_over_hash_type`; the rustc lints named. All four crates opt in with `lints.workspace = true`.
- Production panics: exactly five sites (`parsing.rs:53`, `:504`, `identity.rs:25`, `classify.rs:140`, `mcp.rs:1381`) plus `dto.rs:797`. Each is on a compile-time literal or a type that cannot fail to serialize, and each has `#[expect(..., reason)]`. No `unwrap`/`expect` on input or I/O.
- `as` casts: only `scoring.rs:37` and `reads.rs:1576`, `:1581`, each with a reason and each bounded.
- No byte slicing of strings; truncation goes through `.chars().take`, `cap` and `str::get`.
- `Duration::days` inputs: `new_within_days` bounded 1-90 on both REST and MCP, `expires_in_days` 1-365, retention clamped by `MAX_RETENTION_DAYS`. `backoff` cannot pass NaN to `from_secs_f64`.
- `println!`/`dbg!`: none. `build.rs` writes to stdout through `writeln!` for cargo instructions, with a comment.
- `Secret` redaction tested for the token, GitHub token and Webhook URLs (`settings.rs:835-850`); `Api` (`tests/github.rs:1415`) and `WebhookWorker` (`tests/webhooks.rs:416`) `Debug` tested. Webhook logs name the host only and strip the URL from `reqwest` errors. The request span records the path, never the query.
- GitHub base URL is injected through `Api::new` and `AppState::with_github`. `flakeradar_core::issue_url` and `permalink` hard-code `github.com`, but they build display strings rather than doing I/O.
- `flakeradar-core`: `cargo tree` shows no SeaORM, Axum, Tokio, Hyper or Reqwest. `flakeradar-test-support` is not in any member's normal dependency tree.
- `thiserror` and `anyhow` placement matches the crate rules, apart from H13. `#[doc(inline)]` on every crate re-export; no glob re-exports; `unreachable_pub` passes.
- Crates are flat under `crates/`; default-features off in `[workspace.dependencies]`.
- `coverage(off)`: six sites (`main.rs` five, `telemetry.rs` one), all at the process edge and all logged in `coverage-gaps.md`.
- Locks: no `std::sync::Mutex` guard across an `.await`. The `tokio::sync::Mutex` in `AdvisoryLock` is held across queries on purpose. `stop` releases its guard before awaiting the handle.
- `parse_upload`, `parse_and_classify` and the multipart scan run on `spawn_blocking`. The multipart scan is linear, with a test counting comparisons. `serde_json` has its recursion limit. XML nesting is not limited: see H0.
- Bulk statements chunk at 1000 rows and stay under the bind limit. `rescore` is not N+1.
- CI: `fmt`, `clippy -D warnings`, `nextest --profile ci`, doctests and `cargo audit` all run; entities are regenerated and diffed; `.cargo/audit.toml` ignores nothing. `Dockerfile` copies `clippy.toml` and the toolchain file and builds `--locked`; the final image is pinned by digest.
- `assets.rs:101` is a module-level `#![expect]`, but its module holds only the one derive, so it acts as item-level.
