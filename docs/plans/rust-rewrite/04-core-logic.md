# 04 — Core logic

Read `00-shared-context.md` first. Tasks 01–03 have landed.

Python reference: `backend/app/parsing.py`, `scoring.py`, `classify.py`, `identity.py`, `notify.py`, `schemas.py`, the pure parts of `score_history.py` (`trend_for`), `report_intake.py` (digest and body helpers), `queries.py` (`tier_for`, `is_broken`, `escape_like`, `issue_url`, `repo_path`, `permalink`).

## Goal

Everything that needs no database or network, in `crates/flakeradar-core`. These modules decide scores and identity, so they get the strictest tests in the project.

## Deliverables

### `py` helpers

`py_strip`, `py_splitlines`, `cap(text, limit)` (by code points, with `\n…[truncated]` inside the limit, as `_cap` does) and `round4`. See the trap table in `00-shared-context.md`.

### Parsing

`parse_junit_xml(bytes, line_base) -> Result<Vec<ParsedCase>, ParseError>`, with identical results to `parsing.py`:

- **Encoding:** read the declaration, transcode with `encoding_rs`, and parse with `roxmltree`. If that fails, retry on a lossy UTF-8 decode, as `_load` does. Handle the BOM. Enable `roxmltree`'s DTD option only if the corpus requires it, and never fetch external entities. Bound entity expansion.
- **Structure:** both `<testsuites>` and a bare `<testsuite>`. Skip a case with no `name` attribute. Follow what the corpus recorded for nested suites.
- **Outcome:** the first `failure`, `error` or `skipped` child in document order wins, as `case.result` does. `message` is the stripped attribute, or else the first line of the stripped body. `details` joins the stripped body, `--- stdout ---\n...` and `--- stderr ---\n...` with blank lines.
- **Retries:** `flakyFailure` and `flakyError` come before the case's own row, and `rerunFailure` and `rerunError` come after it, each with its own message, details and duration, using `stackTrace`, `system-out` and `system-err`.
- **Nextest marker:** a failing outcome whose message **fully** matches `test passed on attempt \d+/\d+ but is configured to fail when flaky`, on a case that has `flaky*` children, becomes `passed` with empty message and details.
- **Location:** strip `file`, remove repeated leading `./`. `line` is ASCII digits only, plus 1 when `line_base` is 0.
- **Errors:** keep the messages `Not a valid JUnit XML report: ...` and `Report parsed but contained no test cases.` Parser detail after the colon may differ. If the corpus recorded a Python crash on a bad `time` value, treat the value as `0.0` and log it in `differences.md`.
- `fingerprint(suite, classname, name)`: lowercase hex SHA-1 of `{suite}::{classname}::{name}` as UTF-8.
- Parsing must never panic on any input. Add a `proptest` or byte-mutation test over the corpus.

### Scoring

Port `scoring.py` exactly: `flip_score`, `count_same_sha_flips`, `combined_score`, `branch_scoped_score`, `clean_streak`, `fail_streak` and the branch-scoped forms. Constants `SAME_SHA_FLOOR = 0.6` and `FULL_CONFIDENCE_SAMPLES = 7`. Compute weights with `decay.powf(i as f64)`. Parameterize statuses with an enum at the boundary, keeping the strings `passed`, `failed`, `error` and `skipped` on the wire.

### Classification

`classify_failure` and `dominant_category`, with the same patterns in the same order. Compile once with `(?i)` in a `LazyLock`. Message first, then details, then `other`. `dominant_category`: the most common non-null value, the newest wins a tie.

### Identity

`normalize_repo`, `normalize_project`, `normalize_root`, `normalize_provider`, with the same regexes and error text, and `DEFAULT_PROJECT`.

### Notifications

`TierChange`, `event`, `parse_urls`, `render_json`, `render_slack`, `render`, the dashboard deep links (`/?view=jobs&repo=...&job=...` and `/?repo=...&project=...&test=...`, with `quote(safe="")`-style encoding), and the Slack line with `{:.2}` and the em dash exactly as in `notify.py`.

### DTOs

Every model in `schemas.py` as a `serde` and `utoipa::ToSchema` struct, with the same field names and optionality.

- A `None` that Python serializes as `null` stays `null`. Do not use `skip_serializing_if` unless Python omits the field.
- Timestamps use one serde helper that follows the timestamp rule.
- `JobResultIn` and `PipelineReportIn` get a `validate()` that matches Pydantic's rules: strip whitespace first (`str_strip_whitespace`), then length and range checks, then `normalize_repo` and `normalize_provider`, a blank `default_branch` becomes `None`, duplicate `ci_job_id` is an error. Failures return a list of `{type, loc, msg}`.

### Other pure helpers

`tier_for`, `is_broken`, `trend_for` (`TREND_DAYS = 14`, `TREND_DELTA = 0.05`), `escape_like`, `issue_url`, `repo_path`, `permalink`, `body_digest` (SHA-256 hex).

## Tests to port

`test_parsing` (26), `test_scoring` (24), `test_classify` (5), `test_identity` (6).

Also add:

- A test that replays **every** file in `tests/parity/golden/junit/` and compares all fields, including fingerprints and categories.
- The `round4` table from `00-shared-context.md`.
- Each row of the trap table that applies here.

`flakeradar-core` tests run on all five targets in task 13, so they must not need a database, a network or Unix-only behaviour.

Target **100%** line and branch coverage for this crate. It has no I/O, so there is no reason for gaps.

## Done when

- [ ] All verification in `00-shared-context.md` passes.
- [ ] Every JUnit golden file replays with no differences, apart from logged ones.
- [ ] Manifest rows for this task are not `pending`, and gaps are logged.
- [ ] `STATUS.md` is updated, and the branch is pushed.
