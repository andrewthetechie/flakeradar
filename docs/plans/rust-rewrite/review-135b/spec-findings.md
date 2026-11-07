# Spec re-review of PR #135, f9330fd..ac5e6f3

**This review is partial.** The coordinator ended it early on budget, so it reports must-fix and should-fix items only, and nits are left out. Section "Not reviewed" lists what was never attacked. A topic that has no finding here was not cleared unless this report says so.

CI: `checks.json` has no `bucket: fail` check (18 pass, 1 skipped).

No worktree was created, and nothing in the repo was changed or committed. No `cargo nextest` test was run. All measurements come from a scratch harness that links `flakeradar-core` at HEAD (`scratchpad/spec/pc`, release build), run next to the reference's `app.parsing` from `/tmp/python-base/backend` under `/tmp/pyref` (Python 3.12.5, expat 2.7.4, junitparser 5.0.3).

## Rerunning the measurements

From `repro/spec/` (scripts only; rerun them for the outputs):

```
(cd pc && CARGO_TARGET_DIR=/Users/andrew/Documents/code/flakeradar/target CARGO_INCREMENTAL=0 cargo build --release --bins)
python3 genbombs.py bombs                # mkdir bombs first; 20 MiB bodies
python3 genb2.py                         # mkdir bombs2 first; smaller bodies
target/release/bomb <file>               # counting allocator, exits 42 past 3 GiB of live heap
python3 gencorpus.py corpus.tsv && /tmp/pyref/bin/python pyrun.py corpus.tsv > py.jsonl && target/release/cmp corpus.tsv > rs.jsonl && python3 cmp.py py.jsonl rs.jsonl
python3 gencorpus2.py corpus2.tsv        # then the same two runs to py2.jsonl and rs2.jsonl, then python3 cmp2.py
```

The bodies, `corpus2.tsv` and the harness binaries were deleted to save disk. `cmp2.out` (the encoding sweep result), `corpus.tsv`, `py.jsonl` and `rs.jsonl` remain.

---

## SP1. must-fix. CONFIRMED at parser level: a 1 MiB upload drives the parser past 3 GiB of heap

**Where:** `crates/flakeradar-core/src/parsing.rs:395-402`, where `allow_dtd: true` has no expansion bound. The module doc at `:15-16` and `PARSE_BYTES_MAX` at `:76-84` are wrong as a result. The route reaches this at `crates/flakeradar/src/routes/reports.rs:522`.

**Spec:**
- `00-fixes-context.md`: "an entity bomb gets a 4xx, never an abort".
- Task 19: "No JUnit input within the size limit can abort the process" and "Allocation for any body within the ingest limit stays under a documented budget".

**Scenario:** one internal entity, referenced many times at depth 0.

```
<!DOCTYPE t [<!ENTITY e "AAAA...512 KiB...">]>
<testsuite name="s"><testcase name="a" classname="&e;&e;&e;...about 178,000 times..."/></testsuite>
```

That body asks for about 85 GiB.

**Why the limits miss it (read in roxmltree 0.21.2 `parse.rs`):**
- `LoopDetector::inc_references` says "Allow infinite amount of references at zero depth".
- `nodes_limit` counts nodes, not bytes, and an attribute creates no node.
- `IDENTITY_BYTES_MAX` is checked only after `Document::parse_with_options` returns.

**Evidence:**

| Body | Size | Result |
|---|---|---|
| `attr_1mib` | 1,048,575 bytes | `EXCEEDED 3 GiB of live heap`, exit 42 |
| `attr_entity_quadratic` | 20,971,520 bytes | `EXCEEDED 3 GiB of live heap`, exit 42 |
| `text_entity_quadratic` (the same in `<failure>` text) | 20,971,518 bytes | no answer after 120 s CPU, killed by `ulimit -t` (exit 152) |
| `namespace_quadratic` (60,000 nested prefixes, then siblings redeclaring one) | 20,971,513 bytes | no answer after 120 s CPU, killed by `ulimit -t` (exit 152) |
| `time_math_digits` (one 5M-char `time` of U+1D7FF) | 20,971,491 bytes | correct 400 detail after 19.5 s CPU, 75 MiB |
| `many_attrs` | 20,970,912 bytes | Ok, 152 MiB |

**Limits of this evidence:**
- It was not run through the real server, on purpose: it would exhaust this machine. A Rust allocation failure aborts the process, so the abort is inferred from the measured growth and not observed.
- The reference's answer on these bodies was not captured. I stopped that run when told to wrap up, so a further 1 MiB text, a per-case-details and a 1.7 MB namespace measurement also have no result.
- The approved row already fixes the answer as a 4xx, so the reference's answer does not change the verdict.

**Why the existing test misses it:** `an_entity_bomb_is_refused` uses the classic nested bomb, which depth 10 refuses. `every_shape_a_body_at_the_ingest_limit_can_take_stays_within_the_budget` measures three shapes, none with a DOCTYPE. The claims "whatever shape it takes" in `differences.md:108` and in ledger row H3 are false.

**Fix:** before `Document::parse_with_options`, run a DTD pre-pass that sums declared entity size times references and refuses past the budget as `InvalidXml`. Separately bound namespace redeclaration, for example with a cap on `xmlns` attributes or on prefixes in scope.

**Test that goes red:** add the 1 MiB attribute, text and namespace shapes to `parse_budget.rs`, asserting `Err(InvalidXml)` and `peak <= PARSE_BYTES_MAX`.

**Ledger:** reopen H3 and the entity-bomb half of S10.

---

## SP2. must-fix. CONFIRMED by running: `xmlns=""` reads a failing test as passed (regression from task 19)

**Where:** `crates/flakeradar-core/src/parsing.rs:547`, `node.tag_name().namespace().is_none()`. roxmltree reports an empty default namespace as something other than `None` for these elements. I inferred that from the results below and did not read it in roxmltree.

**Spec:**
- Compatibility rule: "Test fingerprints, byte for byte", and status codes stay identical.
- Task 19: "parsing matches Python".

| Input | Reference | Rust |
|---|---|---|
| `<testsuite><testcase name="a"><failure xmlns="" message="m">b</failure></testcase></testsuite>` | **failed**, `m`, `b` | **passed** |
| `<flakyFailure xmlns="" message="m"/>` under a case | failed Execution, then passed | passed only (the retry is gone) |
| `<system-out xmlns="">o</system-out>` beside a failure | details hold the stdout section | details empty |
| `<testcase xmlns="" name="a"/>` | parsed | `no test cases` |
| `<testsuite xmlns="">` or `<testsuites xmlns="">` at the root | parsed | 422 `Invalid format.` |
| `<testsuites><testsuite xmlns="">...` | parsed | `no test cases` |

Serializers that write an explicit empty default namespace produce this form. By the task's own description of S10, the code at f9330fd matched on the local name alone, so these inputs worked before the fix.

**Fix:** `namespace().is_none_or(str::is_empty)`.

**Test that goes red:** the first row of the table, with the reference's value as expected.

---

## SP3. should-fix. CONFIRMED by running: 258 of 496 encoding labels disagree with Python, most with no log row or approval

**Where:** `parsing.rs:292-363`, which falls through to WHATWG's `Encoding::for_label`.

**Spec:**
- Task 19: "Encoding labels follow Python's codecs".
- Done-when: "`differences.md` has a row for any exception, with the owner's approval".

**Method:** every label in `encodings.aliases` plus WHATWG labels, each crossed with bytes 0x80-0xFF and one ASCII-only body (63,984 documents). Summary per label is in the output of `repro/spec/cmp2.py`.

| Kind of difference | Labels | Examples |
|---|---|---|
| Reference accepts, Rust 422s (even an ASCII-only body) | 224 | `cp437`, `cp850`, `cp852`, `cp037`, `mac-roman`, `windows_1252`, `iso8859_15`, `iso_8859_2`, `latin9`, `us`, `u8`, `utf-8-sig`, `iso-2022-jp` |
| Rust accepts, reference refuses | 11 | `x-cp1252`, `windows-874`, `x-user-defined`, `x-mac-cyrillic`, `unicode-1-1-utf-8`, `logical`, `visual` |
| Both accept, name differs, so the **fingerprint differs** | 23 | `iso-8859-9`/`latin5`/`l5`/`csisolatin5` (25 bytes: 0x80 is U+0080 vs `€`), `iso-8859-11` and `tis-620` (U+0080 vs `€`), `koi8-u` (0xAE `╝` vs `ў`), and `windows-125x`/`cp125x` undefined bytes |

`differences.md:109` logs only the last group's undefined bytes, and calls it "Residual, recorded rather than silent", which is self-approved. It does not mention the `iso-8859-9`, `koi8-u` or `tis-620` mapping differences, or the 235 labels whose status differs.

**Fix:** generate each single-byte codec's 256-entry table and label list from CPython and decode through that table, treating an undefined byte as "fall back to lossy UTF-8". Remove row 109.

**Test that goes red:** a fixture with one row per label and byte.

**Ledger:** S10's encoding half is not closed.

---

## SP4. should-fix. CONFIRMED by running: DOCTYPE and declaration handling diverges, including fingerprints, with no row

**Spec:**
- Approved table: "`DOCTYPE` in JUnit: Restore Python: accepted".
- Task 19: "Run every input in both parsers" and "`differences.md` has a row for any exception".

**Method:** 337 hand-written inputs (`gencorpus.py`). Cases where both refuse, or both agree, are not listed.

| Input | Reference | Rust | Effect |
|---|---|---|---|
| `<!DOCTYPE t [<!ATTLIST testcase classname CDATA "dflt">]>` + case | classname `dflt` | classname `` | fingerprint |
| `<!ATTLIST testcase file CDATA "dflt.py">` | file `dflt.py` | no file | Location |
| `<!ATTLIST testcase name NMTOKEN #IMPLIED>`, `name="  a  b "` | `a b` | `  a  b ` | fingerprint |
| Parameter entity in the internal subset, `<!ENTITY % p "..."> %p;` | accepted | 422 `unknown token` | status |
| `<!DOCTYPE t SYSTEM "x.dtd">` with an undefined `&nope;` in an attribute | accepted | 422 | status |
| UTF-8 BOM + `encoding="iso-8859-1"` or `windows-1252`, bytes `C3 A9` | `Ã©` | `é` | fingerprint |
| `<?xml-stylesheet href="a" encoding="bogus"?>` first | accepted | 422 `unknown encoding` (`declared_encoding`, `parsing.rs:376`, matches any PI starting `<?xml`) | status |
| UTF-16 body with a BOM, declared `bogus` | 422 | accepted | status |
| UTF-16 body with a BOM, declared `utf-8` or `iso-8859-1` | 422 | accepted | status |

Rust also accepts these, which the reference answers 422, and none is logged:
- `<?XML version="1.0"?>`
- `<?xMl x?>`
- `standalone="maybe"`
- `encoding=" utf-8"`
- `xmlns:a=""`
- `xmlns:xmlns="u"`
- `<:a/>`
- `:a="1"`

**Fix:** fix what is cheap: `declared_encoding` must require `<?xml` followed by whitespace, BOM plus declaration must follow the declaration, and the listed status mismatches. Log the rest, with the owner's approval, as a row each. ATTLIST defaults change a fingerprint, so they should be read or the owner should accept the difference explicitly.

**Test that goes red:** the table rows, with the reference's values as expected.

---

## SP5. should-fix. CONFIRMED against the record: differences logged without the approval the spec requires

**Spec:**
- `00-fixes-context.md`: "Tasks 24–30 change structure, not behaviour".
- `00-fixes-context.md`: "Any other status-code change follows 'When you are blocked'".
- `00-shared-context.md` lists "change a status code" under stop-and-ask.

Verdict on all 26 rows added at `differences.md:104-129`:

| Rows | Verdict |
|---|---|
| 16 queue bound, 16 in-flight calls, 17 guarded write | Required by those tasks' contracts |
| 19 depth, 19 non-finite time, 19 node and identity budget | In the approved table |
| 20 page, NUL, multipart garbage header, HEAD/OPTIONS, `GET /api/`, path id | In the approved table |
| 21 setting refusal, `.env` text, database URL | Required by task 21 |
| 23 422 fields, `json_invalid` offset | Allowed by "May differ" for framework 422s |
| 23 `deleted` counts, 204 `Content-Type` | Status and body shape unchanged; no approval needed |
| 20 naive timestamp as UTC, 20 name-less part's `detail` | No status change, but outside the table; the `detail` is a parser message that is not restored. The owner should see them |
| **19 code-page bytes (`:109`)** | **Self-approved**, see SP3. Done-when requires the owner's approval |
| **23 413 past 20 MiB (`:123`)** | **Unapproved status change.** Task 23 is marked Done with "Needs the owner's decision" where the rule says BLOCKED. The limit predates the fixes (`DefaultBodyLimit` at f9330fd `app.rs:139`), so the sweep surfaced it and did not add it. The decision is still open |
| **23 serialization retry (`:122`)** | **Unapproved behaviour difference.** The row itself says the reference failed at once with `retry_count` 0 and Rust retries 3 times. The retry code predates the fixes (f9330fd `processing.rs:175`). Rule: tasks 16-23 change behaviour "only to match the reference or as approved" |
| **25 unknown stored `status`/`kind` (`:128`)** | **Self-approved status change** in a structure-only task: the reference served the word, Rust answers 500. Should have been BLOCKED |
| **25 unknown `failure_category` (`:129`)** | **Self-approved value change** in a structure-only task: stored word becomes `null` |

The two task 25 rows are reachable only through a hand-written row, by their own text. I read those rows and did not run them. Their severity is the broken rule, and they are not a live defect.

**Action:** the owner rules on `:122`, `:123`, `:128` and `:129`. Each row then either cites that ruling or is reverted. No test is needed unless a row is reverted.

---

## SP6. should-fix. CONFIRMED by reading, not run: a store failure stops only the Test batch, so more issues can still be filed

**Where:** `crates/flakeradar/src/github/mod.rs`, `on_report_processed`, which ignores `filed.stopped`.

**Spec:** task 18: "On 'store failed', as in Python, the issue stays open, the batch stops".

**Reference:** in `_file_batch`, a non-`StaleDataError` raised by `nested.commit()` propagates through `file_issues_for` and `on_report_processed` to `GitHubWorker._file`, which logs it. Neither the Job batch nor either note batch runs for that outcome.

**Rust:** `FiledLink::StoreFailed` breaks only `file_batch`, then `file_job_issues_for`, `note_fixed_issues_for` and `note_fixed_job_issues_for` all run.

**Scenario:** a `BEFORE UPDATE` trigger or lock timeout on `test_cases` only, with one Test and one Job both over the gate. The reference makes 1 `POST`. Rust makes 2, plus any comments due. `a_store_failure_leaves_the_issue_open_and_stops_the_batch` touches Tests only, so it cannot see this.

**Note on other stops:** a rate limit is different. The reference's `_file_batch` returns there and the next batch still runs, so only `StoreFailed` needs this change.

**Fix:** return from `on_report_processed` when `filed.stopped` or `filed_jobs.stopped` is `Some(BatchStop::StoreFailed)`.

**Test that goes red:** the scenario above, asserting exactly one `POST` on the counting mock and no comment.

---

## SP7. should-fix. CONFIRMED by reading: the record makes false or stale claims

| Lead | Finding | Needs a task? |
|---|---|---|
| RE-REVIEW §5 floor | `RE-REVIEW.md:159-160` says the floor "stays `{"lines": 96, "branches": 94}`"; its own `:19` and `coverage-floor.json` say 93 | Yes, a one-line doc fix |
| Budget claim | `differences.md:108`, `parsing.rs:15-16,76-84` and ledger H3: "whatever shape it takes" is false (SP1) | With SP1 |
| Open 20 MiB row | `differences.md:123`, RE-REVIEW §3 and the PR body all still ask the owner | Owner decision, not a code task (SP5) |
| E5 negative controls | RE-REVIEW:206-211 admits they do not exist. `a_missing_path_method_parameter_or_field_fails_the_openapi_contract` exists, but going by its name and §7's list it covers names. Nothing shows the type and `required` comparison can fail | Yes, small: without a control that goes red, the row does not meet this review's closure standard |
| `record.py --verify` gone | `differences.md:50-53` still says in the present tense that it "fails" and "both checks pass", and `:87-95` says it is deleted. RE-REVIEW:216-219 calls reconciliation "manual", yet `tests/common/allowances.rs:173` `reconcile` and `an_allowance_that_mismatches_the_record_is_reported` exist. I did not read their bodies. An allowance that covers nothing is, by the record's own statement, undetected | Yes, small: fix the text and add a used-allowance check |
| "No golden or allowance change across tasks 24-31" | **True.** `git diff --stat 64d42e9..e8563b1 -- tests/parity` is empty. The only later change is `tests/parity/README.md`, in `0a97cd1` | No |
| D7, D8, J14a | Their evidence is restated in RE-REVIEW §4. I did not check it independently | Unknown |

---

## Ledger rows

**Measured and consistent with the fix:**
- **H19b:** `NaN`, `inf`, `1e999` on a case and on a retry all answer `InvalidTime`.
- **J18:** `𝟗`, `٣`, `१२`, `１２` in `time`, no difference from the reference.

**Partly measured:**
- **S10, character references:** every reference in the corpus (`&#0;` through `&#xFFFFFFFF;`) gets the reference's status, apart from one literal `&#xD800;` in a `SYSTEM` id, which Rust refuses.
- **S10, namespaces:** prefixed elements and attributes match. The `xmlns=""` form does not (SP2).
- **Time grammar:** the `time` corpus differs only where the approved table says it should.

**Reopen:** H3 (SP1), S10 (SP1 to SP4), and A3/S13c's "as in Python" half (SP6).

**Read but not confirmed closed:** S1/H1, S5, J3, S2/J1, S4/A1, S6 and B3, plus the 401 table. I read the code and the reference, found no counterexample, and the policy table matches `_FILING_SKIP_REPO_KINDS`, `_NOTE_SKIP_REPO_KINDS` and `_SYNC_SKIP_REPO_KINDS`. I did not read or run their tests, so by the closure standard I cannot vouch for these rows.

## Not reviewed

Stopped on the coordinator's instruction before these were reached:
- Lax Pydantic validation and multi-error ordering, multipart, query-int forms.
- Settings refusal, secret redaction, the serialization-retry path at HEAD, and the 20 MiB limit's behaviour at the boundary.
- Whether the `github` and `failed-report` goldens came from the reference, and whether an allowance could mask them.
- Tasks 16-18 tests, the HANDOFF body, and the PR body beyond a grep.
- Real-server runs. No route-level test was run, so every parser finding is at `parse_junit_xml`.
- Sync guarded writes under real concurrency.
- Task 19's depth boundary on the Rust side. Only the reference was measured: it parses depth 994 in a bare script and depth 972 with 20 caller frames, so `SUITE_DEPTH_MAX` "990 is the deepest" depends on the caller's stack.

---

## Second pass

This pass covers what section "Not reviewed" left for the Spec axis, at `ac5e6f3`. All runs used real servers: the debug binary of HEAD and the reference at `86c98804` under uvicorn (one worker), each on its own database in a throwaway `postgres:17-alpine`. No source change remains: `git status --short` shows only `docs/plans`.

The scripts are in `repro/spec/second-pass/`. Create the container and the databases `py` and `rs`, run `start.sh`, then run `/tmp/pyref/bin/python -u t<n>.py`. The ports are 18181 (reference) and 18182 (Rust). `lib.py` compares status, `type`, `loc` and `msg`.

### SP8. must-fix. CONFIRMED by running: a numeric timestamp past 2e10 is read as seconds, where the reference reads milliseconds

**Where:** `crates/flakeradar-core/src/lax.rs:260` and `epoch_instant` at `:361`. Row `differences.md:113` is false: it says "the accepted and refused spellings are the reference's".

**Command:** `t3.py`, then a read of `jobs[0].started_at` from `reports.body`. Each input is `started_at` on `POST /api/ingest/pipeline`.

| Input | Reference | Rust |
|---|---|---|
| `20000000000` | 2603-10-11T11:33:20, processed | same |
| `20000000001` | 1970-08-20T11:33:20.001, processed | 2603-10-11T11:33:21, processed |
| `-20000000001` | 1969-05-14T12:26:39.999, processed | 1336-03-23T12:26:39, processed |
| `1767225600000` (`Date.now()` shape) | 2026-01-01T00:00:00, processed | **202, then Report `failed`**; body holds `+57971-02-25T00:00:00.000000Z` |
| `1767225600123` | 2026-01-01T00:00:00.123, processed | 202, then `failed` |
| `253402300799` | 1978-01-11T21:31:40.799, processed | 9999-12-31T23:59:59, processed |
| `253402300800` | 1978-01-11T21:31:40.8, processed | 202, then `failed` |
| `1e14` | 5138-11-16T09:46:40, processed | 422 `int_parsing_size` |
| `1e15` | 422 `datetime_parsing`, "dates after 9999 are not supported as unix timestamps" | 422 `int_parsing_size` |

The failed Reports store `ValidationError: the stored Pipeline Report body is not the Report it was built from: input contains invalid characters at line 1 column 282`. Every failed row has a year past 9999, and the year-9999 row processed. So the `+YYYYY` form failing its re-read is PLAUSIBLE, not traced. `completed_at` goes through the same reader (`dto.rs:899`, CONFIRMED by reading).

**Exposure:** I found no `started_at` in `actions/`, `docs/*.md` or `README.md`, so this grep shows no first-party client that sends a number. It is must-fix because a third-party client loses or corrupts data.

**Fix:** port the reference's rule: `abs(value) > 2e10` is milliseconds, and past the year-9999 limit is `datetime_parsing`. I measured that limit only as lying between `1e14` and `1e15`, so measure the exact bound before writing it as a literal. Correct row 113, and cite the test's current name: it was renamed to `the_pipeline_body_reads_a_timestamp_the_way_the_wire_does` (`f628c28`).

**Red test:** add to that test (`crates/flakeradar/tests/validation.rs:328`): `1767225600000` gives `2026-01-01T00:00:00.000000Z`, `20000000001` gives `1970-08-20T11:33:20.001000Z`, and `1e15` is refused with `datetime_parsing`.

### SP9. must-fix. CONFIRMED by running: a numeric string timestamp is accepted by the reference and refused by Rust

**Where:** `lax.rs:267` (`parse_timestamp`). The test comment at `validation.rs:331`, "Every spelling the reference accepted", is false, and so is row 113.

**Command:** `t2.py` and `t3.py`.

| `started_at` | Reference | Rust |
|---|---|---|
| `"1767225600"` | 202, 2026-01-01T00:00:00 | 422 `datetime_from_date_parsing` |
| `"1767225600.5"` | 202, 2026-01-01T00:00:00.5 | 422 |
| `"1767225600000"` | 202, 2026-01-01T00:00:00 | 422 |
| `"1700000000"` | 202 | 422 |
| `"0"` | 202, 1970-01-01 | 422, "input is too short" |
| `"-5"` | 202, 1969-12-31T23:59:55 | 422, "input is too short" |

Both sides refuse `"1e3"`, `"1_000"` and `" 1767225600"`.

**Exposure:** a shell reporter that writes `"started_at":"$(date +%s)"` meets this. I do not know of a client that does. It is must-fix under "refuses valid CI input". The owner may instead record it as a difference.

**Fix:** in `parse_timestamp`, read a string that is a plain decimal number through the rule from SP8.

**Red test:** the same test, with the rows above as accepted spellings.

### SP10. should-fix. CONFIRMED by running: a query reports only one bad value, chosen by URL order

**Where:** `crates/flakeradar/src/extract.rs:189-193` (`parse_query`). `serde_path_to_error` stops at the first failure, and only then does `validate` run. Row `differences.md:124` approves "one issue per parse" for a **missing** parameter only, and it says type, loc and msg match. No row covers bad values.

**Command:** `t2.py`.

| Request | Reference | Rust |
|---|---|---|
| `/api/tests?sort=z&page=x&page_size=y&include_stable=q&new_within_days=0` | `include_stable`, `sort`, `page`, `page_size` and later issues, in declared order | one issue: `page` |
| `/api/tests?page=x&page_size=0` | `int_parsing` page, `greater_than_equal` page_size | `int_parsing` page |
| `/api/tests?page=0&page_size=x` | `greater_than_equal` page, `int_parsing` page_size | `int_parsing` page_size |
| `/api/tests/abc/history?limit=x` | path issue and query issue | path issue |
| `/api/ingest?line_base=9` | `repo`, `line_base`, `commit_sha` | `repo` |
| `/api/ci-runs/1/failures?ci_run_attempt=0` | `repo`, `ci_run_attempt` | `repo` |

These agree: two range failures (`?page=0&page_size=0`, ingest's `commit_sha=&line_base=9&ci_run_attempt=0`), which `Validate` collects, and a body's issues in every shape tried.

**Fix:** the status is the same, so this needs the owner's decision. Either read the query field by field through `lax`, as the body does, or widen row 124 to say so. The row is the cheaper option.

**Red test (if restored):** a new `a_query_lists_every_bad_value_in_declared_order` in `validation.rs`, using the first row above.

### SP11. should-fix. CONFIRMED by running: a database password reaches the process output when it holds `/`, or when it is in the URL's query

**Where:** `crates/flakeradar-db/src/connect.rs:116-131` (`redact`). It cuts the authority at the first `/`, so the `@` that follows is never seen. An unescaped `/` in a password is also what makes the URL unparseable, so the leak fires on the case the error exists for. Base64 passwords can hold `/`. Row `differences.md:120` ("with only the password replaced") is false for these inputs. The reference also quotes the URL (per that row), so this is a false record claim and a missed fix for `H5`, not a regression.

**Command:** `t8.py` and `t11.py`, running the binary with `FLAKERADAR_DATABASE_URL` set:

```
postgresql://u:s3cretpw/x@db:5432/d              is not a URL: postgresql://u:s3cretpw/x@db:5432/d
postgresql://u:ab+c/s3cretpw==@db:5432/d         is not a URL: postgresql://u:ab+c/s3cretpw==@db:5432/d
postgresql://db:notaport/d?password=s3cretpw     is not a URL: postgresql://db:notaport/d?password=s3cretpw
postgresql://u@db:notaport/d?password=s3cretpw   is not a URL: postgresql://u:***@db:notaport/d?password=s3cretpw
```

These stayed clean: passwords with `@`, `#` or `?`, an empty user, an `asyncpg` scheme, a bad scheme, a bad `sslmode`, an unreachable host, and `%2F`.

**Fix:** take the userinfo up to the last `@` in the whole string, and print nothing of the query.

**Red test:** add those inputs to `a_database_url_that_cannot_be_parsed_never_names_its_password` (`connect.rs:231`).

### SP12. should-fix. CONFIRMED by running: settings spellings the reference starts with are refused, with no row

**Where:** `crates/flakeradar/src/settings.rs:208-255` (`parsed`, `boolean`). Row `differences.md:118` covers a value "its field could not mean", not these spellings.

**Command:** `t8.py`, against `Settings()` plus `assert_secure_token` on the reference.

| Variable and value | Reference | Rust |
|---|---|---|
| `SCORE_WINDOW=5.0` | starts | refused, "expected a whole number" |
| `SCORE_WINDOW=1_000` | starts | refused |
| `SCORE_DECAY=1_0` | parses | refused as "a number"; 10 would fail the range rule anyway |
| `GITHUB_CLOSE_FIXED=t` | starts | refused, "expected a boolean" |
| `GITHUB_CLOSE_FIXED=Y` | starts | refused |

The refusal is loud and names the variable, so no data is at risk. An operator upgrading with `=y` or `=t` fails to start.

**Fix:** read whole numbers with `lax::text_int` and booleans with the body's `lax_bool`, or add a row. The first is a few lines and also removes a second definition of these spellings.

**Red test:** extend `a_boolean_takes_every_spelling_it_documents` (`settings.rs:1058`) with `t`, `f`, `y`, `n`, and add `5.0` and `1_000` to a whole-number case.

### SP13. should-fix. CONFIRMED by running: row 115's "the status is the same 400" is false, and multipart's limits have no row

**Where:** `crates/flakeradar/src/multipart.rs:176-182` and `:88-94`, `crates/flakeradar/src/routes/reports.rs:60` and `:471`, and `differences.md:115`. Rows 2 and 3 below contradict the status claim directly. No CI client sends these, so this is a record fix and not a behaviour demand.

**Command:** `t4.py` and `t5.py`.

| # | Body | Reference | Rust |
|---|---|---|---|
| 1 | Valid `report` plus a part with no `Content-Disposition` | 400 "...`name` must be provided." | 202 |
| 2 | Valid `report` plus a part with a filename and no name, after it | 400 | 202 |
| 3 | The same part, before `report` | 400 | 202 |
| 4 | Valid `report` plus a form value over 1 MiB | 400 "Part exceeded maximum size of 1024KB." | 202 |
| 5 | Valid `report` plus 1001 fields | 400 "Too many fields..." | 202 |
| 6 | Valid `report` plus 1001 files | 400 "Too many files..." | 202 |
| 7 | Valid `report` with a 9 KB header | 400 "Invalid multipart data." | 202 |
| 8 | `Content-Type: multipart/form-dataX; boundary=BB` | 400 "Empty report body" | 202 |
| 9 | A 20 MiB `report` plus a 100 KB value | 202 | 413 (`MULTIPART_ALLOWANCE`) |
| 10 | A 1 KB `report` plus a 21 MiB other file | 202 | 413 |

**Fix:** correct row 115, and add one row for rows 4-10 for the owner to approve. Restoring them is not worth the code.

**Red test:** none is needed for the record. If the owner approves the row, pin rows 2 and 9 in `crates/flakeradar/tests/multipart.rs` and the route tests.

### SP14. should-fix. CONFIRMED by running: a refused timestamp's `msg` differs, and row 124 says `msg` matches

**Where:** `lax.rs:344` (`timestamp_detail`) has two sentences; the reference has several. `differences.md:124`.

| `started_at` | Reference `msg` suffix | Rust `msg` suffix |
|---|---|---|
| `"2026-13-01"` | month value is outside expected range of 1-12 | invalid date |
| `"20260101T000000Z"` | invalid date separator, expected `-` | invalid date |
| `"2026-01-01T24:00:00Z"` | unexpected extra characters at the end of the input | invalid date |
| `"  2026-01-01T00:00:00Z "` | invalid character in year | invalid date |

Status, `type` and `loc` match. No consumer I found reads this text.

**Fix:** add the exception to row 124. Porting the sentences is not worth it.

**Red test:** none.

### SP15. should-fix. CONFIRMED by reading: the parity README names the wrong reference for two scenarios

**Where:** `tests/parity/README.md:6`, "recorded from the Python server at commit `97b933d`". Commits `8cea33f` and `449b29f` say `failed-report` and `github` were recorded at `86c98804`, and `git diff --stat 97b933d 86c98804 -- backend` shows 2 files changed (900+, 256-).

**Fix:** one sentence in the README. Also say how those two were recorded: `tools/parity-record` was deleted in `fd259c1`, and it is absent from the tree at `8cea33f`, the commit whose message cites `record.py`.

**Red test:** none.

### Checked and clean

**Query integers (`t1.py`).** 33 of 35 forms match on status and `type` (the other two are the past-`i64` values below the bar): `1_0`, ` 7 `, `+3`, `٣`, `٧٧`, `７`, `1e3`, `7.0`, `7.00`, `7.`, `7.5`, `0x10`, the empty string, `-0`, `007`, `1__0`, `_1`, `1_`, tab and newline padding, NBSP, U+2007 and U+3000 padding, `Infinity`, `NaN` and `true`. A repeated parameter takes the last value on both.

**Body integers and booleans (`t1.py`, `t2.py`).** `ci_run_attempt` matches for `1e3`, `1.0`, `1.5`, `true`, `false`, the string forms, `null`, `[]`, `{}` and `9.0e15`. `summary` stores the same value for all 16 spellings accepted. `expires_in_days` and `number` match in-range.

**Multi-error order in a body (`t2.py`).** Type, loc and msg match, in order, for: `{}`, a non-object, every field the wrong type, reversed keys, empties, a bad `repo` with a bad `provider`, every length limit, a Job with every field wrong, label lists, 51 labels, mixed bad Jobs, duplicate ids, 1001 Jobs and blank names.

**Multipart (`t4.py`).** Status matches for a missing part, no parts, an empty body, a filename-less part, `filename=""`, `filename*`, mixed-case parameters, unquoted values, `;` in a filename, a part whose only `name=report` is inside its filename, `attachment`, a wrong part `Content-Type`, base64 (not decoded on either side), no closing delimiter, a closing delimiter with no CRLF, an epilogue, a leading CRLF, the boundary in the data, duplicate file parts (last wins), a file then a value (no upload), a value then a file, quoted and upper-case `boundary`, `charset` on either side of it, and `Multipart/Form-Data`, which is read as raw on both. `detail` differences are row 06's.

**Secrets (`t8.py`, reading).** `Secret` redacts in `Debug`. `webhooks.rs:178` drops the URL from a delivery error. `.env` errors carry only a line number. Token refusal matches for `changeme`, ` changeme`, `changeme `, `CHANGEME` and the empty string.

**Serialization retry (`t7.py`).** A trigger on `test_runs` raised SQLSTATE 40001.

| Fires | Reference | Rust |
|---|---|---|
| Always | `failed`, `retry_count` 0, 1 attempt | `failed`, `retry_count` 3, 4 attempts, no `test_cases` row left |
| Once | `failed`, 0 | `processed`, `retry_count` 1, row written |
| Twice | `failed`, 0 | `processed`, `retry_count` 2 |

Row 122 is true as written. JUnit Reports only.

**The 20 MiB limit (`t5.py`, `t6.py`).** L is 20,971,520 bytes.

| Body | Reference | Rust |
|---|---|---|
| Raw, or a multipart file, of L | 202 | 202 |
| Raw, or a multipart file, of L+1, and up to L+10 MiB | 413 | 413, same detail |
| Chunked raw L | 202 | 202 |
| Chunked raw L+1, L+64 KiB, L+64 KiB+1, L+1 MiB | 413 | 413 |
| JSON L, plain and chunked | 202 | 202 |
| JSON L+1, plain and chunked | 202 | 413 (approved, row 123) |
| Content-Length larger than the body, client stays open | hangs until the client gives up | hangs until the client gives up |

**The goldens (`replay.py`).** I replayed the scenarios on a fresh reference server, with `fake_gh.py` as GitHub and the webhook receiver. `failed-report` matched 3 of 3 and `github` 13 of 13, on status and body with timestamps normalized. Their values are the reference's. The Report's stored error confirms it: `DBAPIError ... invalid input for query argument $2: 3000...`. `allowed-differences.json` names files only under `ingest-junit`, `junit`, `mcp` and `trailing-slash`, and `for_step` (`allowances.rs:103`) matches an exact file, so no allowance can mask these two. I did not run the Rust replay; it passed per `checks.json`.

**`SUITE_DEPTH_MAX` (`t9.py`, `t10.py`).** 990 holds through a real request. On both document shapes (bare `<testsuite>` and a `<testsuites>` wrapper):

| Depth | Reference | Rust |
|---|---|---|
| Up to 990 | 202, `processed` | 202, `processed`, 1 case |
| 991 | 202, then `failed` with `RecursionError` | 422 |
| 992 and deeper, up to 100,000 (bare root: 992 to 995 only) | 500, or a reset connection on some requests in my harness (not separated out) | 422 |

So nothing the reference processed is refused. Row 106's "depth 995" understates the reference's failure, which starts at 991 on this path. That is harmless. The earlier 994 and 972 figures were measured outside the server.

**Tests of tasks 16-18.** Each mutation was run, then reverted with `git checkout -- <file>`. Failure text was not captured (`failure-output = "never"`), so the rows below show red or green only, not the assertion message. The baseline was all green.

| Row | Mutation | Result |
|---|---|---|
| S1 / H1 | One shared token for every run in `LeaderRun` | All 4 named tests red |
| S5 | `until_stopped` ignores the stop | 4 red and `stopping_ends_the_wait_before_a_retry_at_once` timed out; `stopping_waits_for_a_filing_post_and_keeps_its_link` stayed green, as it should |
| S2 / J1 | One transaction across the sync's `GET`s, at the caller | `a_sync_holds_no_row_lock_across_its_get` red |
| S2 / J1 | `AND github_issue_number = $2` removed from `record_check` | `a_link_replaced_during_the_sync_get_survives` red |
| S4 / A1 | `Continue` on a transport error in `clear_if_closed` | `a_sync_stops_at_a_transport_error_with_one_request` red |
| S6 | `Continue` on a transport error in `close_note` | `a_transport_error_on_a_fixed_note_close_ends_the_batch` red |

A first S1 mutation, reusing the token held in `running`, stayed green. It changed nothing, because `stop` takes that value out, so it says nothing about the tests. The S4, S6 and guard mutations ran together in one build. Each named test covers a separate path, and the row-lock test stayed green in that build.

**Measured and left below the bar:**
- `INTEGER_RANGE` (`lax.rs:22`) is about 9.22e30 (a 31-digit literal), while its comment says `2^63`. So `1e19` up to that bound saturate to `i64::MAX`, then fail a later range check. `ci_run_attempt` answers 400 where the reference answers 422, and `number` and `expires_in_days` answer `less_than_equal` where it answers `int_parsing_size`. Nothing was stored. Fix it with SP8, which touches the same lines. The two MCP callers (`mcp/mod.rs:706`, `:715`) were not measured.
- A query integer past `i64` answers `int_parsing`, where the reference answers the range issue or `int_parsing_size`. The status is 422 on both.
- A truncated or badly chunked body: Rust answers 500 with the reader's text (`reports.rs:509`). The reference sends nothing for a truncated body and 400 for bad chunks. Nothing is stored in Rust's path, by reading.
- Both `Content-Length` and `Transfer-Encoding`: the reference answers 400 and Rust closes with no reply.

### Not reached

- The MCP tools' integer arguments.
- The other settings fields. One int, one float and one boolean field were sampled, plus every token form.
- A `.env` file on the reference side. The probe passed `_env_file=None`.
- GitHub token handling in logs on a real failing call. I read the webhook path only.
- A real (not staged) serialization conflict, and the retry path for Pipeline Reports.
- `completed_at`, measured only by reading. Any build other than debug for the Rust depth runs. Uvicorn with `--workers 2`.
- Whether `job_executions` holds the same instant as `reports.body` for SP8: my query for those rows returned nothing on both sides, so SP8's values come from `reports.body`.
- Still open from section "Not reviewed", and outside this pass: the HANDOFF body and the PR body.
