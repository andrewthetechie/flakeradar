# Clean Code review: PR #135, "feat: replace the Python backend with one Rust binary"

Head `f9330fd5`, base `86c98804`. Review only: nothing in the repo changed. Paths are relative to the repo root; `crates/` is omitted where the crate name makes it obvious.

## 1. Verdict

The rewrite is a careful, well-tested transliteration, and the bulk of the code is small functions with good names. Its debt has one main shape: **the Python structure survived where Rust could have removed it.** Test and Job flows stay parallel, statuses stay strings, failures become log lines, and comments keep talking to a server that no longer exists.

- Section 3 (A1 to A4) lists four behavior or standards defects. They are small to fix and should go before merge. A1 and A2 differ from the Python reference and are not recorded in `differences.md`.
- Everything else is follow-up debt. B1 to B6 carry most of the change cost.

## 2. Method and validation

**Routing (SKILL.md).** Read `SKILL.md`, `cheatsheet.md`, `glossary.md`, `patterns.md`, then chapters 02, 03, 04, 06, 07, 08, 09, 10, 12 and 17, plus the first 40 lines of 11 and 14. Chapter 13 did not match: I found no concurrency smell in what I read of the worker. Chapters 15 and 16 did not match either: the Python is gone and the Rust has a characterization suite.

**Ran:**

| Tool | What it does | Output |
|---|---|---|
| `S/fnlen.py` | Brace matcher that ignores strings and comments. Non-test functions, signature to closing brace, attributes and rustdoc excluded. Parameter and bool counts. | `S/fnlen.out` |
| `S/dups.py` | Repeated runs of 8+ normalized lines, with `test`/`job` identifiers collapsed so parallel code matches. | `S/dups-src.out` |
| `S/teststats.py` | Test counts, lengths and assert counts per file. | inline below |
| `grep`, `git show <base>` | Instance counts. Comparison against the deleted Python. | inline below |

`S` = `<S>`.

**Not run:** `cargo` of any kind, including clippy (you said it is green in CI) and the test suite. **A1 to A4 come from reading the code and were not reproduced at runtime.**

**Coverage, stated plainly:**

- Read in full: `github.rs`, `mcp.rs`, `routes/reports.rs`, `routes/reads.rs`.
- Read almost in full: `flakeradar-db/src/reads.rs` (not lines 1398 to 1499).
- Large parts: `processing.rs` (1-130, 420-930, 1100-1360, 1425-1775), `issue_links.rs` (20-419).
- Excerpts: `settings.rs`, `extract.rs`, `dto.rs`, `worker.rs`, `retention.rs`, `routes/issues.rs`, `routes/ops.rs`, `github_worker.rs`, `multipart.rs`, `notify.rs`, `parsing.rs`, `scoring.rs` signatures.
- Measured and grepped only: `webhooks.rs`, `attribution.rs`, `score_history.rs`, `deletes.rs`, `connect.rs`, `app.rs`, `cli.rs`, `server.rs`, `docs.rs`, `middleware.rs`, `identity.rs`, `classify.rs`, `helpers.rs`, entities, migrations.
- Tests: `tests/github.rs` sampled (helpers, 3 long tests, 1 sync test); `tests/parity.rs` header and metrics; `flakeradar-test-support` outline. Instance counts cover the whole file; line-level reading does not.

"Every in-scope instance" therefore holds for the greps quoted, and for the files read in full. It does not hold for rules that need reading in the measured-only files.

## 3. Measurements

**Totals:** 912 non-test functions; 46 over 50 lines; 3 over 100 lines. 59 repeated-block pairs account for about 639 normalized duplicate lines, with some overlap between pairs. Grep counts over `crates/*/src` include `#[cfg(test)]` modules there unless a count says otherwise.

### The 20 longest non-test functions

| # | Function | Location | Lines | Params | Reading |
|---|---|---|---:|---:|---|
| 1 | `process_report` | `flakeradar-db/src/processing.rs:425` | 118 | 3 | Finding B8 |
| 2 | `test_issue_body` | `flakeradar/src/github.rs:604` | 106 | 6 | Finding C1 |
| 3 | `rescore` | `flakeradar-db/src/processing.rs:1131` | 101 | 4 | Finding B1, B8 |
| 4 | `job_summary` | `flakeradar-db/src/reads.rs:1612` | 100 | 3 | Finding B1 |
| 5 | `clear_if_closed` | `flakeradar/src/github.rs:1321` | 99 | 7 | Finding A1, B3 |
| 6 | `upsert_pipeline_jobs` | `flakeradar-db/src/processing.rs:827` | 98 | 4 | Finding B5, B8 |
| 7 | `runner_stats` | `flakeradar-db/src/reads.rs:1509` | 97 | 3 | Finding B9 |
| 8 | `validate` (`PipelineReportIn`) | `flakeradar-core/src/dto.rs:681` | 96 | 0 | Size alone; a flat rule list. Naming point in C3 |
| 9 | `note_batch` | `flakeradar/src/github.rs:1640` | 92 | 5 | Finding B3 |
| 10 | `file_batch` | `flakeradar/src/github.rs:1124` | 90 | 5 | Finding B3 |
| 11 | `job_issue_body` | `flakeradar/src/github.rs:729` | 83 | 5 | Finding C1 |
| 12 | `close_issues_for_deleted_repo` | `flakeradar/src/github.rs:1427` | 80 | 4 | Finding B3, C3 |
| 13 | `list_tests` | `flakeradar-db/src/reads.rs:571` | 80 | 5 | Finding B1, B12 |
| 14 | `get_test` | `flakeradar-db/src/reads.rs:953` | 79 | 4 | Stepdown candidate; low priority |
| 15 | `record_test_executions` | `flakeradar-db/src/processing.rs:557` | 74 | 4 | Finding C3 (name) |
| 16 | `test_tier_changes` | `flakeradar-db/src/processing.rs:1288` | 70 | 4 | Finding B1 |
| 17 | `build_rules` | `flakeradar-core/src/classify.rs:77` | 69 | 0 | Size alone; a declarative table. Leave it |
| 18 | `ci_run_test_failures` | `flakeradar-db/src/reads.rs:1934` | 69 | 7 | Finding B5, C2 |
| 19 | `validate` (`IngestQuery`) | `flakeradar/src/routes/reports.rs:237` | 68 | 0 | Finding B6, C4 |
| 20 | `link_issue` | `flakeradar-db/src/issue_links.rs:285` | 66 | 7 | Finding A2, C2 |

### Parameters and flags (full inventory from `fnlen.py`)

- **7 params:** `github.rs:1321 clear_if_closed`, `issue_links.rs:285 link_issue`, `reads.rs:1934 ci_run_test_failures`.
- **6 params:** `github.rs:139 fixed_comment`, `github.rs:604 test_issue_body`, `github.rs:1757 close_note`, `mcp.rs:1089 tool`, `dto.rs:325 length`, `parsing.rs:354 retry_case`, `issue_links.rs:154 check_link`, `issue_links.rs:607 stamp_fixed_note`, `reads.rs:1081 set_quarantine`, `reads.rs:1812 ci_run_failures`, `reads.rs:2086 ci_run_failed_jobs`.
- **5 params:** 13 more, listed in `S/fnlen.out`.
- **Bool params:** see C2, which gives a verdict per instance.

## 4. Findings, ranked by change cost

### A. Defects the smells were hiding (correctness and standards rank first)

#### A1. The issue sync no longer stops when GitHub is unreachable. High.

- **Where:** `crates/flakeradar/src/github.rs:1330-1336`, loop at `:1307`; test at `crates/flakeradar/tests/github.rs:3024-3027`.
- **Evidence:**
  ```rust
  Err(error) => {
      tracing::warn!(error = %error, "GitHub unreachable, skipping issue-state sync");
      return true;
  }
  ```
  `true` means "keep going" (`:1307`: `if !clear_if_closed(...) { break; }`). The Python returned `False` here: `# No answer at all: as with a rate limit, this row goes unstamped and the caller stops the run` (`backend/app/github_integration.py:817-820` at base). `docs/plans/rust-rewrite/differences.md` has no matching entry (grepped `unreachable`, `issue-state`, `transport`).
- **Impact:** during an outage the loop tries every row, up to `github_sync_batch` (default 2000) per target, each with a 15 s timeout, inside one open database transaction. If GitHub is black-holed that is up to 4000 x 15 s, about 16.7 hours, against one failed request in Python. The log line says "skipping" while the code continues.
- **Test gap:** the test comment says `// An unreachable GitHub never raises: nothing is stamped, and the batch ends.` It asserts only that the first row is unstamped, against an address that records no requests, so it cannot see the batch end.
- **Rule:** priority 1 (correctness); G20, C2 in the test; ch07 "Catch-all without context"; bool polarity smell, see B3.
- **Move:** return stop on transport error, and assert with two rows and a counting server that one request was made.
- **Effect:** restores the reference behavior. Worst-case transaction length goes from hours to one timeout.

#### A2. Re-linking the same issue number can report `github_issue_closed: false` while the row stores `true`. High.

- **Where:** `crates/flakeradar-db/src/issue_links.rs:321-323`, `:344-349`, `:222`.
- **Evidence:**
  ```rust
  let stored = if current.number == Some(stored_number(number)) { current } else { ... };
  write_link(&transaction, target, target_id, stored).await?;
  ...
  Ok(patched_out(out_for(target, &row, settings)?, &repo, stored.number, false))
  ```
  `stored` keeps `current.closed`, so the database stays `true`. The response hard-codes `false`. Python returns the object's own stored value (`target.to_out(row, threshold)`, `issue_links.py`). `the_same_number_twice_keeps_the_check_stamp` (`tests/issue_links.rs:172`) ignores that tuple slot (`let (_, _, checked, _, _)`).
- **Impact:** a client that re-links a closed-as-not-planned issue is told the flag is clear, though the filer stays quiet.
- **Rule:** F3 (a flag whose only value at both call sites is `false`), G9, T6 (bug neighborhood).
- **Move:** remove the `closed` parameter and pass `stored.closed` through `patched_out`, or rebuild from `LinkState`.
- **Effect:** one source of truth for the response; the flag disappears.

#### A3. `commit_filed_link`'s `bool` merges "row deleted" with "database failed", and the caller acts on the wrong one. Medium.

- **Where:** `crates/flakeradar/src/github.rs:1162-1173`, `:1225-1256`, `:90-92`.
- **Evidence:** any `db.begin()`, store or commit failure returns `false`. The caller then logs `"row deleted (admin delete) while filing; closed the new issue"` and posts `CLOSE_COMMENT`: `"FlakeRadar deleted the tracked flaky test or CI job this issue was filed for via the admin delete API ..."`.
- **Impact:** a transient database error closes a real issue with a comment that states something false, and the log misdirects whoever diagnoses it.
- **Rule:** ch07 "Define exception classes for callers"; G20; N4.
- **Move:** return `enum FiledLink { Stored, RowGone, StoreFailed(DbError) }`. Log each truthfully. Use a neutral comment, or a separate one, for `StoreFailed`.
- **Effect:** the log and the public comment both become accurate. The closing behavior itself can stay.

#### A4. `?page=` can overflow the offset multiply, which `code-standards-rust.md` forbids. Medium.

- **Where:** `crates/flakeradar-db/src/reads.rs:387`; validation at `crates/flakeradar/src/routes/reads.rs:171-175`.
- **Evidence:** `values.push(Value::from((self.page - 1) * self.page_size));`. `validate_paging` calls `check_ge(page, 1, "page", issues)` with no upper bound. `Cargo.toml` sets no `overflow-checks`, and `arithmetic_side_effects` is not in the lint set.
- **Impact:** `page=9223372036854775807` with the default `page_size` of 50 panics in a dev or test build and wraps in release, here to -100, which should make Postgres refuse the offset and the route answer 500. Other values can wrap to a valid offset and return the wrong page. The standard says: "never a panic and never an arithmetic overflow".
- **Rule:** repo standard (priority 3); G3, G33.
- **Move:** `checked_mul` with a 422, or cap `page`. Add a boundary test.
- **Effect:** the rule becomes true on this path.

---

### B. Structural debt, highest change cost first

#### B1. Test and Job flows are parallel copies. High.

A rule shared by Tests and Jobs must be written twice and kept in step, by hand.

| Concern | Test copy | Job copy | Measured overlap |
|---|---|---|---|
| Rescore | `processing.rs:1131 rescore` (101 L) | `:1497 rescore_jobs` (59 L, plus `:1431 job_history`) | 26, 9 and 9 line runs |
| Tier changes | `processing.rs:1288` (70 L) | `:1576` (62 L) | same shape |
| Old scores | `processing.rs:1253` | `:1558` | identical bar the entity |
| Bulk write | `processing.rs:1360` | `:1640` | 9 line run |
| Headline counts | `reads.rs:751 summary` plus 4 helpers | `:1612 job_summary` (100 L, inline) | same SQL shapes |
| List tail | `reads.rs:615-643` | `:681-707` | 20 line run |
| Search | `reads.rs:1188` | `:1729` | 13 line run |
| Filing candidates | `github.rs:894` | `:953` | 9 line runs |
| Note candidates | `github.rs:1521` | `:1558` | 18 line run |
| File entry point | `github.rs:1015` | `:1064` | 16 and 9 line runs |
| Note entry point | `github.rs:1598` | `:1619` | 13 line run |
| Retention | `retention.rs:260`, `:369` | `:295`, `:400` | 18, 13, 12, 9 line runs |
| Link routes | `routes/issues.rs:189`, `:229` | `:274`, `:314` | 22, 11, 10 line runs |

- **Evidence of real change pressure:**
  - The Job copy of `rescore` was factored one way (`job_history` is separate and public) and the Test copy another (reads inline).
  - `summary` was split into helpers and `job_summary` was not.
  - Commit `0a86d3a` fixed a 401 in the filing path only; see B3.
- **Rule:** G5, G11; ch12 rule 2 (no duplication); ch14 "Let repetition reveal concepts"; patterns "Emerging abstraction".
- **Move:** the repo already has the right model twice: `Board` (`reads.rs:278`) and `LinkTarget` (`issue_links.rs:29`). Extend it with one descriptor per entity carrying table, alias, joins and id column, and with Test-only rules passed as data or a predicate (for example the `!is_broken` filter at `processing.rs:1301-1304`). Migrate one pair at a time under the parity replay. Do not reach for SeaORM generics; the raw-SQL descriptor is simpler. Good first pairs: `old_*_scores`, the two `HistoryUpdate` blocks at `processing.rs:1208-1221` and `:1532-1545`, and the retention pairs.
- **Effect:** a shared rule has one site rather than two. Most of the ~639 duplicate lines sit here, so this is the largest single reduction in change cost available.

#### B2. `github.rs` has five reasons to change, and raw SQL crosses into the binary crate. High.

- **Where:** `crates/flakeradar/src/github.rs` (1843 lines).
- **Responsibilities:**
  1. REST client (`Api`, `Answer`, `:196-439`)
  2. SQL and row mapping (`:446-874`, `:894-1009`, `:1521-1593`)
  3. Issue body rendering (`:95-165`, `:604-844`)
  4. Filing and note batch policy (`:1015-1213`, `:1598-1809`)
  5. Issue-state sync (`:1264-1419`)
- **Boundary evidence:**
  - `use flakeradar_db::attribution::{int_array, test_window_ids_sql};` (`:52`). The database crate publishes SQL fragments with a positional bind contract ("Binds: `$1` ids, `$2` window, `$3` ids, `$4` window", `attribution.rs:41-42`), and the binary rebuilds binds to match (`:860-865`).
  - `pub headers: HeaderMap` (`:201`) exposes a `reqwest` type on a public struct.
  - `get` and `rows` are copied here (`:447`, `:453`), because the private database helpers are out of reach.
- **Rule:** ch10 SRP; ch08 "Keep external types out of application public APIs"; G8, G17, G27.
- **Move:**
  - Move candidate selection into `flakeradar_db` as a `filing` module returning typed rows.
  - Split the rest into `github/{client,bodies,filing,sync,notes}.rs`.
  - Replace `HeaderMap` with the parsed facts `Answer` needs.
- **Effect:** body text, SQL and retry policy each change in one small file. `flakeradar` loses its SQL dependency for this feature. Rendering becomes testable without a database.

#### B3. GitHub status policy is scattered, and every failure disappears into a log line. High.

**Inconsistent status sets at six decision points:**

| Site | "skip Repo" | Other handling |
|---|---|---|
| `file_batch` `github.rs:1191` | `401, 403, 404, 410` | stops on rate limit |
| `clear_if_closed` `:1345` | `401, 403, 410` | 404 re-arms |
| `close_issues_for_deleted_repo` `:1457`, `:1492` | none | `404, 410` is "already gone" |
| `note_batch` `:1710` | `403, 404, 410` | **no 401** |
| `close_note` `:1785` | `403, 404, 410` | **no 401** |

The module doc (`:28`) says "A 401 (bad credentials) ... skip that Repo for the rest of the batch". The note paths do not do that, because commit `0a86d3a` fixed only the filing copy. This is what duplicated policy costs. Some differences are intended (404 means something different in a sync); the code cannot tell a reader which.

**Three `bool` returns, two polarities:**

- `clear_if_closed`: `false` means stop (`:1318`).
- `close_note`: `true` means stop (`:1755`).
- `commit_filed_link`: `true` means stored (`:1215`).

The caller sites `if !clear_if_closed(...) { break; }` and `&& close_note(...).await { return; }` read opposite ways.

**Cluster:** the module's stated policy is "this module never raises", so these public functions return `()`: `sync_closed_issues`, `note_fixed_issues_for`, `note_fixed_job_issues_for`, `close_issues_for_deleted_repo`. A test can then only observe outcomes through log text; see T3. Also `clear_if_closed` and `close_note` thread `skipped: &mut BTreeSet<String>` (F2).

- **Rule:** G5, G11, F2, G30; ch07 "Exception boundary"; patterns "Smell-cluster review".
- **Move:**
  - `Answer::classify() -> GitHubVerdict { Success, RateLimited, RepoRefused, Gone, Other }`, with the few intended per-operation differences stated once in a table.
  - `std::ops::ControlFlow<BatchStop>` in place of the three bools.
  - Return a `BatchReport { filed, skipped_repos, stopped }` and log it once at the edge. "Never raises" stays true: the error is a value the caller ignores if it wants to.
- **Effect:** one policy table replaces five `matches!` lists, and the 401 bug class disappears. Tests assert values, not logs. `file_batch`, `note_batch` and `clear_if_closed` shrink, because each status arm becomes one table row.

#### B4. Stringly-typed domain values, although the enums exist. High.

| Concept | Evidence | Count |
|---|---|---|
| Execution or Job status | `status: String` | 29 sites; `"failed"` 24, `"passed"` 17, `"skipped"` 15, `"error"` 15 |
| "Is failing" | `github.rs:442 is_failing(status: &str)`, `reads.rs:2047`, SQL `IN ('failed', 'error')` | 1 + 1 + 7, while `ExecutionStatus::is_failing` exists at `model.rs:37` |
| Failure category | `failure_category: Option<String>` | 10 sites; `FailureCategory` exists |
| Category parse | `category_from_wire` lists five by hand (`processing.rs:1240`); `mcp.rs:742`; `routes/reads.rs:211`, `:343` | 4 copies, no `from_wire` on the enum although `ExecutionStatus` has one |
| Report status | `ProcessOutcome.status: String`; `outcome.status != REPORT_PROCESSED` (`github.rs:1824`); `outcome.status == REPORT_PENDING` (`worker.rs`, in `deliver`); `reads.rs:1923-1927` matches literals | constants instead of an enum |
| Provider | `provider == "github"` at `reads.rs:518`, `issue_links.rs:232`, `:308`; SQL at `github.rs:970`, `:1569` | `issue_links.rs:232` duplicates `reads.rs:517 job_issue_url` outright |
| Test or Job | `Kind` (`notify.rs:21`), `HistoryKind` (`score_history.rs:22`), `LinkTarget` (`issue_links.rs:29`), and `entity: &str` (`github.rs:140`, `:1127`, `:1643`, `:1760`) | 3 enums plus strings |

- **Sharpest instances:**
  - `fixed_comment`: `if entity == "job" { ... } else { ... }` (`:159`). A typo silently builds a Test link.
  - `note_batch(db, candidates, "test", ...)` where every candidate already holds `target: LinkTarget::Test`: the same fact stated twice.
- **Standards gap:** `code-standards-rust.md` asks for "a newtype for a fingerprint, a score, a CI run ID". There are none. The only tuple structs are `Secret` and the three extractor wrappers. `ci_run_id` is a bare string at 28 sites, and scores are bare `f64` at 40. `i64::from(row.entity_id)` appears 8 times in `github.rs`, because ids are `i32` in storage and `i64` in reads. Yet all 56 files carry the compliance marker; see B7.
- **Rule:** repo standard (priority 3); J3, G23, G26, G27; glossary "primitive obsession".
- **Move:**
  - One `EntityKind` in core, replacing the three enums and `entity`.
  - `#[serde(rename_all = "lowercase")]` enums in the DTOs, which keeps the wire identical; the parity replay proves it.
  - `FailureCategory::from_wire`, `ReportStatus`, `Provider`.
  - `Fingerprint`, `CiRunId`, `FlakinessScore` per the standard, gradually.
- **Effect:** exhaustive `match` replaces string comparison, and a new status is a compile error at each site rather than a silent fallthrough. Removes the 4 category parsers.

#### B5. Three hand-rolled SQL builders and five copies of the row helpers. Medium-high.

- **Row helpers:** `get` and `rows` are defined at `reads.rs:170/176` (`pub(crate)`), `score_history.rs:80/86`, `attribution.rs:154/160`, `processing.rs:203/209`, `github.rs:447/453`. `processing.rs:1107 rows_of` is a sixth `rows` with a different error type. Three of the database-crate copies are needless because `reads.rs` already exports the helpers to the crate.
- **Placeholder builders:**
  - `Where` (`reads.rs:206`).
  - `scope_filters`, `repo_filters`, `where_suffix` (`reads.rs:718`, `:1714`, `:738`), which do the same as `scope_conditions` and `Where::suffix`.
  - Hand arithmetic: `format!("(${}, ${}, ...)", base + 1, ...)` at `processing.rs:872`, `:1649`, near-duplicate at `:718`, `score_history.rs:219`.
- **Missing method:** the sequence `next()`, `raw()`, `values.push()` is spelled out at `reads.rs:331-333`, `:340-344`, `:357-361`, `:581-585`, `:598-600`. `Where` lacks a `bind` operation.
- **SQL text rewriting:** `.replace("$1::DATE", &format!("DATE '{}'", ...))` at `reads.rs:253`, `:828`, `:1644` depends on the literal placeholder inside another module's string (G22). `.replace("$P", ...)` at `reads.rs:341`, `:1197`, `:1739` is a private template language.
- **Bind-order coupling:** `pair_executions(db, values: Vec<Value>)` (`reads.rs:2005`) makes the caller know the SQL's bind order. `ci_run_test_failures` repeats the same 8-line `pairs` subquery twice (`:1948-1956`, `:1969-1979`).
- **Hand-wrapped not-found:** `DbError::Database(DbErr::RecordNotFound(...))` at 9 sites; `status_of` reports an unknown status as `DbErr::Custom` (`processing.rs:1236`), which is not a database error.
- **Rule:** G5, G11, G22; ch12 "Reuse in the small"; ch07.
- **Move:** a `flakeradar_db::sql` module owning `get`, `rows`, `one`, `required_one(what)`, one `Binds` builder with `bind`, `eq`, `tuple_rows`, and a real bind for the cut-off date.
- **Effect:** removes four `get`/`rows` copies and two of the three placeholder schemes, and placeholder off-by-one errors become impossible in callers.

#### B6. The route layer has repeated its plumbing in each module. Medium-high.

- **Five near-identical error types:** `ReportError` (`routes/reports.rs:129`), `ReadRouteError` (`routes/reads.rs:41`), `AdminRouteError` (`routes/admin.rs:31`), `IssueRouteError` (`routes/issues.rs:31`), plus `database_failure` (`routes/ops.rs:343`). The plain-text 500 is written out 5 times.
- **Three copies of the body extractor while the generic one serves no route:** `QuarantineBody` (`routes/reads.rs:513`), `LinkBody` (`routes/issues.rs:139`), `PipelineBody` (`routes/reports.rs:414`) copy `ApiJson` (`extract.rs:105`). `ApiJson` is re-exported and only `tests/extractors.rs` uses it (G9). The cause is an API mismatch: `Validate::validate(&self) -> ApiResult<()>` against `PipelineReportIn::validate(self) -> Result<Self, Vec<ValidationIssue>>`.
- **Repeated rule boilerplate:** `if issues.is_empty() { Ok(()) } else { Err(ApiError::unprocessable(issues)) }` at 10 sites. Two `check_max` with different signatures (`routes/reads.rs:132`, `routes/reports.rs:340`). `identity_error` (`routes/reads.rs:122`, `routes/admin.rs:125`) and `named_error` (`routes/reports.rs:644`) are one function under two names.
- **Hard-coded limits:** across routes, MCP and DTOs, `255` at 22 sites, `100` at 6, `1024` at 4, `512` at 4, `90` at 5, `64` at 3, `200` at 3. `mcp.rs:70-74` documents `MAX_NEW_WITHIN_DAYS` as "The same bound as the REST `?new_within_days=` parameter", which is the literal `90` at `routes/reads.rs:180`. A comment holds the two together.
- **Redundant types and code:**
  - `SummaryQuery` is a copy of `ScopeQuery` and validates by cloning itself into one (`routes/reads.rs:267-285`).
  - `QuarantinePath` copies `TestPath`. `TestPath`'s doc still says it covers the quarantine route and "The Test's or Job's id" (`:435-447`), though `QuarantinePath` and `JobPath` exist (C2).
  - `ci_run` builds an `issues` vector for one check and hard-codes `loc` (`:871-885`).
  - `jobs()` inlines `ListOptions` where `TestsQuery::parts()` does not (`:750-763`, G11).
  - Repeated `match raw { Some(raw) => Some(normalize_repo(&raw).map_err(...)?), None => None }` instead of `transpose` (`:255-262`, `:746-749`, `:795-798`).
- **Impossible arms:** `LinkedOut::Job(_) => Err(IssueRouteError::Refused(IssueLinkError::NotFound("Test not found".to_owned())))`, 4 times in `routes/issues.rs`. `link_issue(target, ...)` returns a sum type whose variant the argument already decided. The arm invents a 404 that cannot happen (G27).
- **Layering is inconsistent:** the module doc of `routes/reads.rs` promises "calls one function in `flakeradar_db::reads`". `routes/reports.rs` instead holds raw SQL and row mapping (`:78-96`, `:724-730`, `:771-783`, `:945-955`), including two hand-written copies of the `ReportOut` mapping (`:920`, `:958`). `retry_failed_report(&mut stored.report)` mutates in memory, and then the route hand-copies four fields into an `ActiveModel` (`:850-858`); that is hidden temporal coupling, and it will silently drop a field if the function grows one.
- **Rule:** G5, G9, G11, G17, G25, G27, G31; ch11 "keep policy apart from infrastructure"; ch03.
- **Move:**
  - One `RouteError` in `error.rs`.
  - Make `Validate` return the validated value, then use `ValidatedJson<T>` everywhere and delete the three copies.
  - A `Rules` collector with `finish() -> ApiResult<()>`.
  - Named constants in one `limits` module of core.
  - A `flakeradar_db::report_queue` module.
  - `LinkedOut: Serialize` (untagged) so the handlers share one body.
- **Effect:** several hundred lines go, and a change to the 500 body, a column width or the JSON rejection has one site.

#### B7. 226 comment lines talk to a server that no longer exists. Medium-high (reader cost across the whole tree).

- **Counts, comment lines only, in `crates/*/src`:** 226 distinct lines match any of `Python|python`, `frozen`, `FastAPI|Pydantic|Starlette|fastmcp` or `.py` (88, 83, 69 and 25 per pattern, overlapping). A further 16 say "raises" or "raised", in a language without raising. Top files: `dto.rs` 34, `mcp.rs` 21, `routes/reports.rs` 19, `settings.rs` 18, `extract.rs` 18.
- **Stale pointers:** `git ls-files | grep -c '^backend/'` gives 0, so these name deleted files:
  - `backend/app/routers/reports.py is the reference` (`routes/reports.rs:7`).
  - `routers/tests.py`, `routers/jobs.py`, `routers/ci_runs.py` "are the reference" (`routes/reads.rs:6-7`).
  - `Scope.__post_init__` (`reads.rs:119`), `_list_leaderboard` (`reads.rs:10`), `parsing._cap` (`mcp.rs:78`), `_rows_by_id` (`processing.rs:1309`, `:1591`).
  - The `#[expect(clippy::needless_raw_strings)]` reason at `mcp.rs:19-23` justifies itself by a "diff against" Python strings that are gone.
- **Journal voice:**
  - `// Python crashed on it after creating rows this Request would have to undo` (`routes/reports.rs:296-297`).
  - `as Python's single `commit` site never reached them` (`:520-521`).
  - `The first ten characters of a commit SHA, as Python's slice did` (`github.rs:570`).
  - The module doc of `processing.rs`: "This is the frozen Python server's `processing.py`, statement by statement".
- **Wrong or obsolete comments, independent of Python:**
  - `normalized_name`: "Records a maximum-length failure, returning whether the value fits" (`dto.rs:808`); it returns a `String`.
  - `Settings::reads` and `Settings::processing` have their doc sentences interleaved (`settings.rs:314-329`).
  - "`GO /api/quarantine`" (`routes/reads.rs:98`).
  - "Rescales every Job" for a rescore (`processing.rs:1685`).
  - The doc at `routes/reports.rs:494-495` says the attempt is "Checked here"; it was already refused in the extractor; see C4.
  - `code-standards-rust.md:117` still gives "the `0.5` default Flake threshold" while the code default is `0.30` (`settings.rs:293`).
- **Marker:** 56 of 56 non-entity source files carry `// Rust guideline compliant 2026-07-21`. The standard says "Add it only to files you have actually checked — a stale marker is worse than none", and M-NO-META-DESIGN-DOCUMENTATION says "rustdoc describes end-state behavior". This comment set and the missing newtypes (B4) make the blanket marker inaccurate.
- **Keep, rephrased:** many of these comments hold a real reason, such as a wire contract, an ordering dependency, or a surprising 422. Those are the book's "warning of consequences". State the end-state fact and point at what enforces it.
  - Before: "`Python rescored sorted(latest)`..."
  - After: "Ids are ordered by id: `touched_test_ids` and Tier change order depend on it; `tests/parity` pins it."
- **Rule:** repo standard; C1, C2, C4; ch04 "Truth locality" and "Delete recoverable history".
- **Move:** one mechanical pass over the tree: delete what `git log` can recover, move rationale to `docs/plans/rust-rewrite/differences.md`, rephrase the rest. Re-check the marker afterward. Rename `ToolFailure::Raised` and "Never raises" (`Failed`, "Never returns an error").
- **Effect:** the docs stop depending on knowledge of deleted code. Newcomers do not need the old repo to read this one.

#### B8. `process_report` mixes four levels, with error-mapping noise. Medium.

- **Where:** `crates/flakeradar-db/src/processing.rs:425-542`; pipeline twin at `:780-823`.
- **Evidence:** one function holds the blocking parse, a raw `UPDATE projects` (`:450-455`), a SeaORM run insert (`:461-474`), chunked execution inserts, rescoring and report completion.
- **Noise:** `.map_err(ProcessError::from)?` appears 24 times in the file, though `From<DbError>` and `From<DbErr>` exist (`:136-160`) and `:455` already uses plain `?`. `?` suffices at each site where the error type already matches.
- **Dead values:**
  - `rescore_repo` returns `(test_ids, job_ids)`, and both callers discard it (`:506`, `:802`).
  - Its `report_tier_changes` parameter exists only to add `.len()` to a log count (`:1722`, `:1749`).
  - The branch-changed drop is expressed two ways: `tier_changes = Vec::new()` at `:805` and `if branch_changed { Vec::new() } else { tier_changes }` at `:535-539`.
- **Error classification:** `ProcessError { transient: bool, message }` with a hand-typed Python exception name in each message: `"DataError: ..."` 8 times, plus `RuntimeError`, `ValidationError`, `ParseError`, `OperationalError`, `DBAPIError`. Stored text is a contract only if the parity goldens record it; I did not check that.
- **Rule:** G30, G34, G12, G26; ch03 Stepdown, ch07 "caller-oriented classification".
- **Move:**
  - Narrate: `parse`, `store_run`, `apply_scores`, `complete`, with a shared `finish_report` for the two kinds.
  - Delete the `map_err`s.
  - `rescore_repo -> Result<usize>` returning the dropped count.
  - `enum ProcessFailure { Transient(..), Data(..), Parse(..) }` whose `Display` writes the prefix.
- **Effect:** the function reads as five named steps, 24 noise lines go, and the classification decides by type.

#### B9. Positional tuples and index constants where names belong. Medium.

| Where | Evidence | Move |
|---|---|---|
| `flakeradar-core/src/scoring.rs:174`, `:220` | `history: &[(&str, &str, ExecutionStatus)]`: two `&str` in a public API, so swapping sha and branch compiles | `struct ScoredExecution<'a> { sha, branch, status }` |
| `processing.rs:1137`, `:1436`, `:1178`, `:1508` | `Vec<(String, String, ExecutionStatus)>` plus a second conversion to refs, twice | same |
| `processing.rs:1134`, `:1500`, `:549`, `:577` | `BTreeMap<i32, (i32, i32)>` in a public signature; `entry.0 += 1`; `.1` | `struct DayCounts { executions, failures }` |
| `reads.rs:780`, `:762-767` | `Result<(i64, i64, i64, i64, i64), DbError>`, read as `counts.0` to `counts.4` | `struct TestCounts` |
| `reads.rs:1528`, `:1538-1539` | `BTreeMap<Vec<String>, [i64; 2]>`; `counts[0] += 1; counts[1] += unexplained;` | `struct RunnerCounts { executions, unexplained }`. `runner_stats` also splits into aggregate, rank and map |
| `processing.rs:1313`, `:1338` | `BTreeMap<i32, (String, String, String, String)>` | `struct NamedTest` |
| `github.rs:1734` | `state: Option<(i32, Option<i32>)>` | named struct on `stamp_fixed_note`'s result |
| `mcp.rs:515-520`, `:531-533` | `target.0, target.1` | `let (kind, id) = ...` destructure |

- **Rule:** G19, G16, G26; ch02 "mental mapping"; ch06.
- **Effect:** reads directly; two of these remove a possible silent swap.

#### B10. In `mcp.rs` one tool lives in five places, and list-ness is derived three times. Medium.

- **Tool spread:** adding a tool touches the name const (`:141`), description const (`:169`), definition fn (`:1136`) and `tools()` list (`:1120`), the dispatch arm (`:540`), the handler (`:303`), and an args struct (`:1516`), spread over 1500 lines. The schema literal and the serde struct are two hand-kept copies of the argument contract.
- **Deliberate, and fine:** literal schemas instead of `schemars` are justified at `:9-12` and `:1115-1117`, and pinned by the replay. I am not recommending derivation.
- **Three statements of "this tool returns a list":** `wraps_result(name)` matching six names (`:642`), `wrapped_output_schema()` at each definition, and `list_answer` in each handler. `success` then re-discovers emptiness by digging `"result"` out of the JSON it just built (`:628-633`), though `list_answer` had the slice in hand.
- **Repetition:**
  - `.map_err(|error| ToolFailure::Tool(error.to_string()))` 7 times (`:309`, `:423`, `:445`, `:463`, `:500`, `:707`, `:710`); a `From<IdentityError>` removes them.
  - `ListOptions` literal twice (`:337`, `:425`).
  - `let reads = self.reads();` makes a local share its name with the `reads` module two lines from `reads::find_test_ids` (`:364`, `:374`, `:453`).
  - `scope.repo.as_deref().unwrap_or_default()` twice, on a value that was just built with `Some` (`:376-377`, `:388-389`), because `Scope` cannot say "has a Repo".
- **Rule:** G5, G10, G16, G27; ch05 "vertical separation" via G10.
- **Move:**
  - Make `Answer` an enum, `List { text, items_empty }` / `Object`, so `success` needs no name.
  - One file per tool, or one table row per tool, under `mcp/`, keeping literals. Move transport and auth (`:1391-1513`) to `mcp/transport.rs`.
  - A `RepoScope { repo, project: Option }` type.
- **Effect:** a new tool is one file. List-ness has one source.

#### B11. Every default is written up to four times; `Settings` holds unvalidated primitives. Medium.

- **Evidence:**
  - `impl Default` (`settings.rs:276-311`) and `from_map` (`:354-385`) each state all 30 defaults.
  - `ReadSettings::default` (`reads.rs:84-93`) and `ProcessingSettings::default` (`processing.rs:69-80`) restate some again: `0.30` and `score_window: 50` four times, `broken_streak: 5` four times.
  - `Raw::int`, `float` and `port` are one function typed three times (`settings.rs:133`, `:145`, `:178`).
- **Consumers re-derive meaning:**
  - `usize::try_from(settings.score_window.max(1)).unwrap_or(usize::MAX)` (`github.rs:940`, `:999`) makes a bad window 1. `usize::try_from(self.score_window).unwrap_or_default()` (`settings.rs:322`, `:332`) makes it 0.
  - `github_fixed_streak <= 0` means disabled (`github.rs:1604`, `:1625`), `> 0` again in `FilingGate` (`:185`), then `i32::try_from(...)` at `:1535`, `:1573`, `:1669`.
  - `.unwrap_or(i32::MAX)` appears 21 times across the crates.
- **Whole-struct tramp:** `test_issue_body` takes `&Settings` to read `github_issue_min_score`. `reads()`, `processing()`, `retention()` and `worker()` views exist, but GitHub has none.
- **Rule:** G5, G33, G35, G8; F1.
- **Move:**
  - Named default constants, with `from_map` starting from `Self::default()`.
  - A `parsed<T: FromStr>` helper.
  - Parse into validated types at startup (`ScoreWindow`, `Option<NonZero>` for the streak).
  - A `GitHubSettings` view.
- **Effect:** one default site per setting, one clamp, and narrower signatures.

#### B12. The leaderboard's tier rule is three booleans, and `Board` copies `ListOptions`. Medium.

- **Evidence:** `reads.rs:330-337`:
  ```rust
  if self.flaky_only || self.new_within_days.is_some() { ... }
  else if !self.include_stable && self.q.is_none() && !self.keeps_stable_rows { ... }
  ```
  `Board` has 17 fields (`:278-315`). Eight are copied one by one from `ListOptions` at both `:615-624` and `:681-690`, which is the 20-line duplicate in B1. `ListOptions` literals are spelled out four times (`mcp.rs:337`, `:425`; `routes/reads.rs:350`, `:750`). Comment at `:335`: "Kept as descriptor data by the caller", which adds nothing.
- **Rule:** F3, G15, G28; glossary "flag argument".
- **Move:** `enum TierFilter { FlakyOnly, HideStable, All }`, decided once. `Board { entity: &EntityBoard, options: &ListOptions, threshold, today, filter }`. Constructors `ListOptions::leaderboard(...)`.
- **Effect:** the rule reads as a decision table. The `flaky_only` and `include_stable` impossible combination disappears.

---

### C. Smaller, contained

#### C1. Issue bodies are built by pushing lines. Low-medium.

- **Where:** `github.rs:604-709`, `:729-811`.
- **Evidence:**
  - A `Vec<String>` plus `.join("\n")` is Python's `"\n".join(lines)` transliterated.
  - `test_issue_body` computes a permalink by hand (`:631-642`), though `flakeradar_core::permalink` (`helpers.rs:122`) implements the same rule and `reads.rs:1016` uses it.
  - It clones `commit_sha` and `branch` out of `NewestFailure` to rebuild the same `Option`s (`:627-630`).
  - `"### Last 10 executions"` is a literal at `:687` and `:800`, beside `BODY_EXECUTIONS` (`:78`); `short_sha` uses a bare `10` (`:572`).
  - The score and Proven flake lines are identical at `:650-656` and `:746-752`.
- **Rule:** G5, G25, G34.
- **Move:** small section functions (`location_lines`, `evidence_table`, `sample_failure`) composed under `test_issue_body`; use the core `permalink`; format the heading from the constant.
- **Effect:** the template reads top to bottom, and the permalink has one rule.

#### C2. Flag arguments: verdict per instance (full list from `fnlen.py`).

| Instance | Verdict |
|---|---|
| `issue_links.rs:222 patched_out(closed)` | **Bug**: see A2. Delete |
| `issue_links.rs:567 set_closed(bool)` | Split. Both call sites pass a literal (`github.rs:1376`, `:1398`). `flag_not_planned` / `clear_not_planned` |
| `reads.rs:1081 set_quarantine(quarantined, reason, expires_in_days)` | Split. It branches on the bool three times (`:1095-1100`), and two parameters are meaningful only when true. `enum QuarantineDecision { Quarantine { reason, expires_at }, Release }` |
| `issue_links.rs:285 link_issue(replace)`, `:154 check_link(replace)` | Acceptable as a real user choice, but 7 and 6 parameters. Group `number, url, replace` as `LinkRequest` |
| `settings.rs:162 boolean(default)` | Not a flag; it is a default value |
| `factories.rs:693 summary(bool)` | Not a flag; a builder setter |
| `Board`, `ListOptions` struct bools | B12 |

Unused parameters kept alive with `let _`, which silences the compiler warning (G4, F1):

- `github.rs:1417` (`settings` in `clear_if_closed`)
- `github.rs:1436` (`settings` in `close_issues_for_deleted_repo`, a public function)
- `github.rs:1765` (`db, settings` in `close_note`)
- `issue_links.rs:410` `repo_of(_target, row)`, which is `get(row, "repo_name")` under a name

Removing them takes `clear_if_closed` from 7 to 6 and `close_note` from 6 to 4 before any other change. Other parameter counts: `ci_run_failures`, `ci_run_test_failures`, `ci_run_failed_jobs` pass `repo_id, repo_name, ci_run_id, attempt` together in differing orders (`reads.rs:1846-1848`: `repo_id, ci_run_id, attempt, &repo_name` against `repo_id, &repo_name, ci_run_id, attempt`), so a `CiRunAttempt` struct is the right data-clump move. `retry_case` (`parsing.rs:354`) takes the case identity in four fields, for the same reason.

#### C3. Names that mislead. Low-medium (each is cheap to fix).

| Location | Name | Problem | Rule |
|---|---|---|---|
| `routes/reports.rs:512` | `stored_attempt(line_base)` | Converts a line base with a function named for CI run attempts and documented only for them | N4, G26 |
| `processing.rs:557` | `record_test_executions` | Writes nothing; it builds models. `build_test_executions` | G20, N4 |
| `github.rs:1427` | `close_issues_for_deleted_repo` | Also used for a deleted Test or Job (doc at `:1421`) and for a row that vanished mid-filing (`:1165`) | N4 |
| `dto.rs:681` | `PipelineReportIn::validate(self)` | Strips, normalizes and returns a new value, while `QuarantineIn::validate(&self)` only checks. `into_normalized` | N7, G11 |
| `report_intake.rs` | `retry_failed_report(&mut model)` | Resets in memory only; the name reads as an action on the stored Report | N7 |
| `parsing.rs:515`, `routes/ops.rs:328` | `python_float` twice | One parses, the other formats; same name, opposite direction, both named for a language that is gone | N4 |
| `notify.rs:137`, `py.rs:27` | `python_quote`, `py_strip` | Named for the origin, not the purpose: `percent_encode`, `strip_wire_whitespace` | N1, N2 |
| `github.rs:197`, `mcp.rs:271` | `Answer` | Two unrelated types, same word | N4 |
| `github.rs`, `issue_links.rs`, `notify.rs` | `entity`, `target`, `kind` | Three words for Test-or-Job. `LinkTarget` has both `kind()` and `entity()`, differing only in case (`:42`, `:51`) | ch02 "one word per concept" |
| `TestRow`, `JobRow`, etc. | `confirmed_flake_count` | `CONTEXT.md` avoids "Confirmed flake". The column and wire are the frozen contract, so leave those; a private field could use `proven_flakes`. Low | standards, N3 |

Also `issue_links.rs:256-259`:
```rust
let filed = match target {
    LinkTarget::Test | LinkTarget::Job => state.number.map(|_| false),
};
```
This matches on a variable and then does the same thing for every variant (G12, G16). `LinkTarget::github_provider_only()` is a descriptor field transliterated from Python's dataclass, called once (`:66`, `:306`). Low.

#### C4. `ci_run_attempt` is parsed twice, and the second parse's error is dead. Low-medium.

- **Where:** `routes/reports.rs:270-288`, `:300-302`, `:494-496`, `:586-596`.
- **Evidence:** `ApiQuery` runs `validate` during extraction (`extract.rs:65-68`), and `validate` already returns `too_large_attempt()`. So `stored_ci_run_attempt`'s `ok_or_else(too_large_attempt)` cannot fire. The comment `// Checked here, where Python's insert failed on the same value, so every refusal before this one answers exactly as it did` is false about ordering: the extractor refuses before the body is read.
- **Rule:** G9, C2, G22; "parse, don't validate".
- **Move:** parse into `Option<CiRunAttempt>` in one step, inside the query type.
- **Effect:** one parse, no dead branch, no false comment.

#### C5. Needless clones and repeated work. Low-medium.

177 `.clone()` calls in `crates/*/src`; most are justified. Those with cost:

- `routes/reports.rs:493`: `parse_upload(content.clone(), line_base)` copies up to 20 MiB per upload. Hold `Bytes`, which clones cheaply. `:560`/`:565` also copy out of `Bytes`.
- `processing.rs:438`: clones every `ParsedCase`, Failure details included, only so `upsert_test_cases` can read identity fields.
- `processing.rs:570`, `:651`, `:663`: the SHA fingerprint is computed three times per case.
- `reads.rs:2039`: `by_pair.get(...).cloned()` copies a list only to read it.
- `processing.rs:1506`: `history.get(id).cloned()` copies each Job's whole window; `remove` works.
- `github.rs:218`: `Answer::json_object` reparses the body per accessor. `issue_state` then `state_reason` parse the same text twice on one path (`:1356`, `:1366`). Parse once in `Api::send`.
- `rows(db, sql: String, ...)` forces `sql.to_owned()` on static strings at most call sites; `impl Into<String>`.

**Rule:** ch12; LLM-translation smell. The `body` and `ParsedCase` clones are the two with real runtime cost, on the ingest and processing hot paths.

#### C6. Swallowed errors and inconsistent handling. Low-medium.

- `github.rs:1085-1093`: `get_job(...).await.ok().flatten()`. A database failure silently produces an issue body with no evidence. The Test twin at `:1036-1042` at least logs it (G11; ch07).
- `github.rs:321-325`: `.build().unwrap_or_default()` silently drops the timeout and user agent if the builder fails. It is an edge case, but a default client has no timeout, which is what this module's error policy relies on.
- `Scope::new` returns `Result<Self, String>` (`reads.rs:121`), and `Scope`'s public fields let `mcp.rs:714` skip it (G8).
- `IssueLinkError::status() -> u16` puts an HTTP status in the database crate (`issue_links.rs:99`; G6). Move the mapping to the route.
- `Utc::now()` in `github.rs:1294`, `:1676` and `routes/reports.rs:498`, `:684`, while `ReadSettings` and `ProcessingSettings` inject a clock. `Settings::reads()` hard-codes `SystemClock` (`settings.rs:323`, `:337`), so a route test cannot inject one. This is a repo-standard gap ("the clock ... injected").

---

### D. Tests

Figures: `tests/github.rs` has 103 tests and 41 helpers, a mean of 41 lines per test, 45 tests over 40 lines. `tests/parity.rs` has 20 tests and 63 helpers, mean 31 lines, and only 19 duplicate lines. `tests/worker.rs` has 23 tests and 69 helpers.

**F.I.R.S.T.:**

| Rule | Reading |
|---|---|
| Fast | Not measured. Each test builds a real database through `TestDb`, so this is the expected trade for integration tests |
| Independent | Good. A `TestDb` per test, no `#[ignore]`, no env reads in tests |
| Repeatable | **Weak**, see T1 |
| Self-validating | Good overall; T3 is the qualification |
| Timely | Not assessable from the code |

#### T1. Real-clock sleeps at 18 sites, against an explicit standard. High for this section.

- **Standard:** "No test depends on real time ... a wait uses `tokio::time::pause` and `tokio::time::advance`, never `tokio::time::sleep` against the real clock."
- **Count:** paused time at 1 test (`tests/parity.rs:2025`, `start_paused = true`), `advance` at 0 sites; `tokio::time::sleep` at 18 sites in `crates/*/tests`:
  - `tests/github_worker.rs:63, 214, 333, 808, 858, 889, 1043, 1108`
  - `tests/webhooks.rs:92, 151, 200, 271, 453`
  - `tests/worker.rs:113, 694`
  - `tests/parity.rs:216, 1551`
  - `tests/server.rs:238`
- **Worst shape:** the negative wait. `tests/github_worker.rs:214`: `tokio::time::sleep(Duration::from_millis(50)).await;`, then an assertion that nothing was sent. On a slow CI box this passes without proving anything. Some of the others are polling loops, which at least converge; I read only `:214` in context and classified the others by duration and indentation.
- **Rule:** repo standard; ch09 Repeatable; coverage gate rule ("A path reached only by timing makes CI fail at random").
- **Move:** paused time where no real I/O is in the wait. For negative assertions, wait on a positive signal that the worker handled the item (a `Notify` or a processed counter) and only then assert. Stopping is not enough here: `stop` cancels, so it could end the task before the item was handled and the assertion would still pass. `wait_until`-style polling can stay as a bounded helper if the standard is amended to allow it.
- **Effect:** negative checks become real, and CI time drops.

#### T2. One test, several concepts; Test and Job merged through a string selector. Medium-high.

- **Count:** 19 lines in `tests/github.rs` are `for entity in [...]` or `if entity == "test"`.
- **Example:** `a_gone_issue_rearms_its_link_but_any_other_answer_keeps_it` (`:3273`). The "but" in the name gives it away: two scenarios and two entities, 16 asserts, and four `if entity == "test"` branches.
- **Example:** `a_permission_403_costs_only_the_denied_repo` (`:1928`, 92 lines, 16 asserts) runs the whole scenario for Tests, then repeats it for Jobs inside the same function. It also asserts 300-character log truncation, a third concept. `a_repo_whose_issues_are_disabled_is_skipped_for_the_rest_of_the_batch` (`:2022`, 101 lines) has the same shape. `a_sync_rate_limit_stops_the_batch_and_keeps_the_stamps_it_made` (`:3017`) covers transport failure too, which is how A1 slipped by.
- **Rule:** ch09 "Single Concept per Test"; G15 in test form.
- **Move:** a scenario helper plus thin `#[tokio::test]` per entity and per concept: `async fn denied_repo_scenario(kind)`. A failure then names the entity and the rule.
- **Effect:** a failing test states which rule broke, and mutation of one branch cannot hide behind another.

#### T3. Log scraping stands in for outcomes. Medium.

- **Count:** `logged_from` 27 and `lines_with` 29 uses in `tests/github.rs`. Assertions like `assert!(only[0].contains("repo=acme/app"))` pin `tracing` field formatting.
- **Cause:** B3. These public functions return nothing.
- **Move:** return a report and assert on it. Keep one or two log tests for the secrets rule.
- **Effect:** log wording stops being a compatibility surface.

#### T4. Positional tuples and bare flags in the test language. Medium.

- **Count:** 82 `.await.N` accesses such as `test_link(&db, id).await.2`, where `.2` is "checked". 63 calls of the form `stamp_link(&db, id, 11, false, false)`. `tests/issue_links.rs` has its own `test_link` returning a five-tuple, a different shape under the same name, destructured as `let (_, _, checked, _, _)`. That is how the closed flag went unchecked in the test nearest A2.
- **Rule:** ch09 "Domain-Specific Testing Language"; F3; G19.
- **Move:** `struct IssueLink { number, filed, checked, closed }` and a builder in `flakeradar-test-support`: `stamp(&db, id).number(11).checked().await`. The existing factory builders set the pattern.
- **Effect:** the assertion states what it checks.

#### T5. Size and helper placement. Medium.

- `tests/github.rs` at 5154 lines holds five concerns, the same split as B2. 23 repeated block pairs, 239 lines: `flaky_test` against `flaky_test_in` (`:81-101`, `:1597-1617`), `:3081-3104` against `:3129-3152`.
- **Standard:** "Test-only helpers — test databases, factories, mocks — live in `flakeradar-test-support`". `tests/github.rs` has 41 local helpers, including mock mounts; `tests/worker.rs` 69; `tests/parity.rs` 63; `tests/common/mod.rs` 217 lines. Domain fixtures such as `flaky_test`, `mount_issue` and `logged_from` belong in the support crate; assertions specific to one file can stay.
- `tests/parity.rs` mixes a comparison library (`compare_value`, `replay_http`, `replay_mcp`, `compare_openapi_*`) with that library's own tests. A `tests/parity/` module tree separates them. Duplication there is low, and the design is strong: no way to switch a scenario off, and a missing golden fails first. Only the file layout needs work.
- `factories.rs:26` uses `Utc::now()` as a default, and `tests/github.rs:1233`, `:1276`, `:1510`, `:1535` take it directly, but these look like fixture timestamps, so low risk. Factories still take stringly-typed `.status("failed")`; it follows B4.

---

## 5. Not findings

- **Size alone, on purpose:** `dto.rs:681` (flat rule list), `classify.rs:77 build_rules` (a table), `mcp.rs` tool literals, the `#[utoipa::path]` blocks, the `Settings` struct's field count (one concept).
- **`ValidationIssue` and `extract.rs`'s Pydantic emulation:** a large module, but the 422 shape is a recorded wire contract. `kind_for` (`extract.rs:269`) classifies by substring-matching serde's English messages, which is a real boundary risk on upgrade; each mapping is pinned by a test, which is exactly the book's learning test. Leave it.
- **`let _ = write!`, `let _ = map.insert`:** idiomatic with this lint set.
- **`impl ConnectionTrait` in `flakeradar-db`'s public API:** a sensible seam; the wrapper the book suggests would add little here.
- **Worker traits (`LeaderTask`, `ReportProcessor`, `Pruner`, `Waiter`, `LockSession`):** each serves an injected test seam the standard asks for. Not speculative.
- **Custom multipart parser:** justified by its module doc (reference-parser behavior, linear scan against a crafted boundary), with functions of sensible size. I read its header and outline only.

## 6. What is good

- **Dependency direction holds.** `flakeradar-core`'s manifest has no SeaORM, Axum or network crate, as the standard requires. `rmcp` stays inside `mcp.rs`.
- **REST and MCP share `flakeradar_db::reads`.** Logic does not fork between front doors; only validation bounds repeat (B6).
- **Two genuine abstractions:** `Board` and the `LinkTarget` link engine show the right fix for B1.
- **Function size is good overall:** 3 of 912 functions over 100 lines, and the long ones are mostly flat.
- **`scoring.rs`:** small pure functions with thorough tests. The domain's heart is the cleanest code in the PR, apart from B9's tuple.
- **Consequence comments in `worker.rs`** (`release` ordering, the lock session check) are the book's "warning of consequences" done right. The module doc on `github.rs`'s error policy is excellent, even though B3 argues against the policy's return types.
- **`Secret`:** a redacting `Debug`, tests proving it, and URLs stripped from `reqwest` errors.
- **The parity harness:** no opt-out, every scenario auto-enrolled. It makes every refactor above safe to attempt.
- **Clear test names:** `a_permission_403_costs_only_the_denied_repo` reads as a specification, even where the body does too much.

## 7. Suggested order

1. **Before merge:** A1 to A4, each with a test, and T2's split of the sync test that hid A1.
2. **Cheap, high yield, no design risk:** `let _` parameters and dead values (C2, B8), C3 renames, the `map_err` noise, one comment pass (B7).
3. **Unblocks the rest:** `flakeradar_db::sql` (B5), then `RouteError` and `ValidatedJson` (B6), then `GitHubVerdict` and `BatchReport` (B3, T3).
4. **Types:** B4's `EntityKind` and status enums, under the parity replay.
5. **The large move:** B1 and B2, one Test/Job pair per small commit, re-running the replay each time (ch14).
6. **Tests alongside:** T1, T4, T5.

Book advice deliberately not applied: wildcard imports (J1; the standard forbids glob re-exports), "functions under 20 lines" as a rule, polymorphism over `match` (an exhaustive `match` on a closed enum is the Rust answer to G23), and returning exceptions (`Result` is the idiom, and the standards fix the error types per crate).
