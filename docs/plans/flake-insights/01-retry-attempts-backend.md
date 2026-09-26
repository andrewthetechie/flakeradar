# 01 — Retry attempts become Executions

## Tracer-Bullet Outcome
A JUnit report that holds `<flakyFailure>`, `<flakyError>`, `<rerunFailure>` or `<rerunError>` elements (Playwright 1.59+ with `includeRetries`, or Maven Surefire) produces one Execution per attempt, numbered from 0. A test that failed and then passed on retry shows up as a **Proven flake** after its first Run. The attempt number appears in `GET /api/tests/{id}/history` and in the MCP `get_test` tool, and the Report's `counts` include `retried`.

## User Story
As a maintainer who runs Playwright with retries, I want a test that needed a retry to count as flaky, so that retries stop hiding flakes.

## Description
Four pieces, in this order:
1. **Parser** (`backend/app/parsing.py`): emit extra `ParsedCase` rows for the retry elements, in attempt order.
2. **Schema** (`backend/app/models.py` + new migration `backend/migrations/versions/0003_execution_attempt.py`): add `test_executions.attempt`.
3. **Processing** (`backend/app/processing.py`): number the attempts per Test and count `retried`.
4. **Read surfaces** (`backend/app/schemas.py`, `backend/app/queries.py`, `backend/app/mcp_server.py`): expose `attempt`.

## Context Pack
- Source decisions: `00-shared-context.md`, "Retries". ADR 0005.
- Repo facts:
  - junitparser 5.0.3 (`backend/requirements.txt`: `junitparser>=5.0`) ignores the retry elements. For a `<testcase>` whose only child is `<flakyFailure>`, `case.result == []` and the case parses as `passed`. The raw element is `case._elem` (the parser already uses it in `_location`).
  - Verified element shape (from Playwright `packages/playwright/src/reporters/junit.ts`, `_buildRetryEntry`, and the Surefire format):
    ```xml
    <testcase name="…" classname="…" time="1.2">
      <flakyFailure message="Timeout 30000ms exceeded" type="FAILURE" time="30.1">
        <stackTrace>Error: boom
        at x</stackTrace>
        <system-out>out of that attempt</system-out>
        <system-err>err of that attempt</system-err>
      </flakyFailure>
      <system-out>output of the final (passing) attempt</system-out>
    </testcase>
    ```
    Playwright writes `flaky*` for a test that passed on retry (no `<failure>`, so the test counts as passed), and `rerun*` after the `<failure>` of a test that failed every attempt. `ElementTree.Element.find("stackTrace")` and `.findtext("system-out")` read the children (verified in the repo venv).
  - Current parser code you extend (copied from `backend/app/parsing.py`):
    ```python
    MESSAGE_MAX = 2000
    DETAILS_MAX = 16384

    @dataclass(frozen=True)
    class ParsedCase:
        suite: str
        classname: str
        name: str
        status: str  # passed | failed | error | skipped
        duration: float
        message: str  # Failure message ("" when passed)
        details: str  # Failure details ("" unless failed/error)
        file: str | None  # Location file as reported, "./" stripped
        line: int | None  # Location line as reported (pytest xunit1 is 0-based)

    def _cap(text: str, limit: int) -> str: ...
    def _first_line(text: str) -> str: ...
    def _location(case) -> tuple[str | None, int | None]: ...

    def _outcome(case) -> tuple[str, str, str]:
        """Return (status, message, details) for one testcase."""
        for result in case.result:
            if isinstance(result, (Failure, Error)):
                status = "failed" if isinstance(result, Failure) else "error"
                body = (result.text or "").strip()
                message = (result.message or "").strip() or _first_line(body)
                parts = [body] if body else []
                if case.system_out and case.system_out.strip():
                    parts.append("--- stdout ---\n" + case.system_out.strip())
                if case.system_err and case.system_err.strip():
                    parts.append("--- stderr ---\n" + case.system_err.strip())
                details = "\n\n".join(parts)
                return status, _cap(message, MESSAGE_MAX), _cap(details, DETAILS_MAX)
            if isinstance(result, Skipped):
                return "skipped", _cap((result.message or "").strip(), MESSAGE_MAX), ""
        return "passed", "", ""

    def parse_junit_xml(content: bytes) -> list[ParsedCase]:
        xml = _load(content)
        suites = list(xml) if isinstance(xml, JUnitXml) else [xml]
        parsed: list[ParsedCase] = []
        for suite in suites:
            if not isinstance(suite, TestSuite):
                continue
            for case in suite:
                if case.name is None:
                    continue
                status, message, details = _outcome(case)
                file, line = _location(case)
                parsed.append(
                    ParsedCase(
                        suite=suite.name or "",
                        classname=case.classname or "",
                        name=case.name,
                        status=status,
                        duration=float(case.time or 0.0),
                        message=message,
                        details=details,
                        file=file,
                        line=line,
                    )
                )
        if not parsed:
            raise ParseError("Report parsed but contained no test cases.")
        return parsed
    ```
  - Current processing loop you change (from `process_report` in `backend/app/processing.py`):
    ```python
    counts = {"passed": 0, "failed": 0, "error": 0, "skipped": 0}
    executions: list[dict[str, Any]] = []
    latest: dict[int, dict[str, Any]] = {}  # per Test: last occurrence in the report wins
    for pc in parsed:
        tc_id = ids[fingerprint(pc.suite, pc.classname, pc.name)]
        counts[pc.status] += 1
        executions.append(
            {
                "test_case_id": tc_id,
                "test_run_id": run.id,
                "status": pc.status,
                "duration": pc.duration,
                "message": pc.message,
                "details": pc.details,
                "created_at": now,
            }
        )
        row = latest.setdefault(tc_id, {"id": tc_id})
        row["last_status"] = pc.status
        row["last_seen_at"] = now
        if pc.file is not None:  # a report without Location never erases one
            row["file"] = pc.file
            row["line"] = pc.line
    ```
    Later in the same function: `report.counts = counts` and `ProcessOutcome(..., counts=counts, ...)`.
  - Current `TestExecution` model (`backend/app/models.py`):
    ```python
    class TestExecution(Base):
        __tablename__ = "test_executions"
        __test__ = False
        __table_args__ = (Index("ix_exec_case_id", "test_case_id", "id"),)

        id: Mapped[int] = mapped_column(BigInteger, primary_key=True)
        test_case_id: Mapped[int] = mapped_column(ForeignKey("test_cases.id", ondelete="CASCADE"))
        test_run_id: Mapped[int] = mapped_column(ForeignKey("test_runs.id", ondelete="CASCADE"), index=True)
        status: Mapped[str] = mapped_column(String(16))  # passed | failed | error | skipped
        duration: Mapped[float] = mapped_column(Float, default=0.0, server_default="0")
        message: Mapped[str] = mapped_column(Text, default="", server_default="")
        details: Mapped[str] = mapped_column(Text, default="", server_default="")
        created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=utcnow, index=True)
    ```
  - Current `ExecutionOut` (`backend/app/schemas.py`) and the two places in `queries.py` that build it (`get_test` and `latest_failure`), both of this form:
    ```python
    class ExecutionOut(BaseModel):
        id: int
        status: str
        duration: float
        message: str  # Failure message
        details: str  # Failure details (traceback + captured output)
        created_at: datetime
        commit_sha: str
        branch: str
        ci_run_id: str

    schemas.ExecutionOut(
        id=e.id, status=e.status, duration=e.duration, message=e.message, details=e.details,
        created_at=e.created_at, commit_sha=r.commit_sha, branch=r.branch, ci_run_id=r.ci_run_id,
    )
    ```
  - Current MCP `get_test` builds each execution as this dict (in `backend/app/mcp_server.py`):
    ```python
    {
        "status": e.status,
        "commit_sha": e.commit_sha,
        "branch": e.branch,
        "ci_run_id": e.ci_run_id,
        "created_at": e.created_at.isoformat(),
        "duration": e.duration,
        "message": e.message,
    }
    ```
  - Migration pattern to copy (header of `0002_ci_jobs.py`):
    ```python
    import sqlalchemy as sa
    from alembic import op

    revision = "0002"
    down_revision = "0001"
    branch_labels = None
    depends_on = None
    ```
- Non-goals: the UI and README (task 02); Failure categories (task 03); changing scoring. The same-SHA rule already turns fail + pass on one SHA into a Proven flake, so `scoring.py` does not change. Changing the GitHub `min_failures` code is also a non-goal: it keeps counting Executions.

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `feat/flake-insights` after this draft

## Implementation Contract
- Expected files:
  - `backend/app/parsing.py` (edit)
  - `backend/app/models.py` (edit)
  - `backend/migrations/versions/0003_execution_attempt.py` (new)
  - `backend/app/processing.py` (edit)
  - `backend/app/schemas.py`, `backend/app/queries.py`, `backend/app/mcp_server.py` (edit)
  - `backend/tests/factories.py` (edit: `make_execution` gains `attempt: int = 0`)
  - `backend/tests/test_parsing.py`, `backend/tests/test_processing.py`, `backend/tests/test_test_detail_api.py`, `backend/tests/test_mcp.py` (edit)
- Interfaces and names:
  - `parsing.py`, add:
    ```python
    RETRY_BEFORE = ("flakyFailure", "flakyError")   # attempts that ran before the final outcome
    RETRY_AFTER = ("rerunFailure", "rerunError")    # attempts that ran after the first (failed) one

    def _retry_outcome(elem) -> tuple[str, str, str, float]:
        """(status, message, details, duration) of one retry element.

        *Failure -> "failed", *Error -> "error". message = the `message`
        attribute, stripped, else the first non-blank line of <stackTrace>.
        details = <stackTrace> text, then "--- stdout ---\n…" and
        "--- stderr ---\n…" from the element's own <system-out>/<system-err>,
        joined by blank lines (same layout as _outcome). Both are capped with
        _cap(…, MESSAGE_MAX / DETAILS_MAX). duration = float(time attr), or 0.0
        when it is missing or not a number.
        """
    ```
    `parse_junit_xml` keeps its signature and return type. For each `<testcase>` it appends, in this order: one `ParsedCase` per child whose tag is in `RETRY_BEFORE` (document order), then the case built today from `_outcome(case)`, then one per child whose tag is in `RETRY_AFTER` (document order). Retry rows copy `suite`, `classname`, `name`, `file` and `line` from the `<testcase>`. **`ParsedCase` does not change.**
  - `models.py`, add to `TestExecution`:
    ```python
    attempt: Mapped[int] = mapped_column(Integer, default=0, server_default="0")  # 0 = first try in its Run
    ```
  - Migration `0003_execution_attempt.py`: `revision = "0003"`, `down_revision = "0002"`. `upgrade()` runs `op.add_column("test_executions", sa.Column("attempt", sa.Integer(), nullable=False, server_default="0"))`. `downgrade()` drops it. Docstring: `"""execution attempt: number retries of a Test within one Run (ADR 0005)"""` plus `Revision ID: 0003`, `Revises: 0002`, `Create Date: 2026-09-26`.
  - `processing.py`, inside `process_report`:
    ```python
    counts = {"passed": 0, "failed": 0, "error": 0, "skipped": 0}
    attempts: dict[int, int] = {}  # per Test: Executions seen so far in this report
    for pc in parsed:
        tc_id = ids[fingerprint(pc.suite, pc.classname, pc.name)]
        attempt = attempts.get(tc_id, 0)
        attempts[tc_id] = attempt + 1
        counts[pc.status] += 1
        executions.append({... existing keys ..., "attempt": attempt})
        ... (latest/last_status logic unchanged: the last attempt wins)
    counts["retried"] = sum(1 for n in attempts.values() if n > 1)
    ```
  - `schemas.py`: `ExecutionOut` gains `attempt: int` (after `ci_run_id`). Both builders in `queries.py` pass `attempt=e.attempt`.
  - `mcp_server.py`: the `get_test` execution dict gains `"attempt": e.attempt`.
  - `tests/factories.py`: `make_execution(..., attempt: int = 0)` passes `attempt=attempt` to `TestExecution(...)`.
- Verified external contracts: the Playwright JUnit reporter's `_buildRetryEntry` writes `name = prefix + ("Error" | "Failure")`, attributes `message`, `type`, `time` (seconds), a `<stackTrace>` child, and `<system-out>`/`<system-err>` children. The option is `includeRetries` or env `PLAYWRIGHT_JUNIT_INCLUDE_RETRIES`, present from v1.59.0 and absent in v1.58.0 (checked against the tagged sources on 2026-09-26).
- Behavior rules:
  - A `<testcase>` with no retry elements parses exactly as today (same single `ParsedCase`).
  - A `<testcase>` whose own outcome is `skipped` still emits its retry rows (rare; keep the order rule).
  - Attempt numbering is per Test **within one Report**, in the order `parse_junit_xml` returns rows. Two separate `<testcase>` entries for one identity are numbered too (0, 1).
  - `counts["retried"]` is always present on processed JUnit Reports (0 when there are no retries). Pipeline Report counts do not change.
  - `last_status` stays "last row wins". For a `flaky*` test, that is the final pass. For a `rerun*` test, it is the last rerun failure.
- Error and security rules: an unknown or malformed retry element never fails the Report. A missing `message`, `time` or `<stackTrace>` gives `""` / `0.0` / `""`. Do not log Failure details.

## Acceptance Criteria
- [ ] `parse_junit_xml` on a `<testcase>` holding one `<flakyFailure message="Timeout 30000ms exceeded" time="30.1">` (with `<stackTrace>Error: boom</stackTrace>`) returns 2 cases: `("failed", "Timeout 30000ms exceeded", duration 30.1)`, then `("passed", …)`.
- [ ] A `<testcase>` with `<failure message="first">` and two `<rerunError message="again">` returns 3 cases with statuses `["failed", "error", "error"]`.
- [ ] Processing that report stores Executions with `attempt` `[0, 1]` (flaky) and `[0, 1, 2]` (rerun). The flaky Test ends with `confirmed_flake_count == 1` and `flakiness_score >= 0.6` after this single Report.
- [ ] `outcome.counts` for a report with one retried and one plain Test is `{"passed": 2, "failed": 1, "error": 0, "skipped": 0, "retried": 1}`.
- [ ] `GET /api/tests/{id}/history` executions include `"attempt"`, and MCP `get_test` executions include `"attempt"`.
- [ ] The migration upgrades an existing database: existing rows get `attempt = 0`.

## Test Expectations
- Framework: pytest + pytest-asyncio, real Postgres via testcontainers. Run: `cd backend && .venv/bin/python -m pytest -q`.
- `tests/test_parsing.py`: add
  - `test_flaky_failure_becomes_a_failed_attempt_before_the_pass`. XML: `'<testsuite name="s"><testcase classname="c" name="t" time="1.2"><flakyFailure message="Timeout 30000ms exceeded" type="FAILURE" time="30.1"><stackTrace>Error: boom\n at x</stackTrace><system-out>out1</system-out></flakyFailure><system-out>final</system-out></testcase></testsuite>'`. Expect `[c.status for c in cases] == ["failed", "passed"]`, `cases[0].message == "Timeout 30000ms exceeded"`, `cases[0].duration == 30.1` and `cases[0].details == "Error: boom\n at x\n\n--- stdout ---\nout1"`.
  - `test_rerun_elements_follow_the_failure`: `<failure message="first">t</failure><rerunError message="again"/><rerunError message="again"/>` gives statuses `["failed", "error", "error"]`.
  - `test_retry_without_message_uses_stacktrace_first_line`: `<flakyError><stackTrace>\n  TypeError: x\n  at y</stackTrace></flakyError>` gives `message == "TypeError: x"`, status `"error"` and duration `0.0`.
- `tests/test_processing.py`:
  - Update `test_processes_report_into_run`: the expected dict becomes `{"passed": 1, "failed": 1, "error": 0, "skipped": 0, "retried": 0}`.
  - `test_same_test_twice_in_one_report` keeps passing, and its two Executions now have `attempt` `[0, 1]` (add that assertion, ordered by id).
  - Add `test_flaky_retry_is_proven_flake_from_one_report`: queue one report built from the flaky XML above plus `<testcase classname="c" name="plain"/>`, then `process_next`. Assert `outcome.counts == {"passed": 2, "failed": 1, "error": 0, "skipped": 0, "retried": 1}`, the `t` Test has `confirmed_flake_count == 1` and `flakiness_score >= 0.6`, and its Executions ordered by id have `(status, attempt)` `[("failed", 0), ("passed", 1)]`.
- `tests/test_test_detail_api.py`: seed a second Execution with `make_execution(..., attempt=1)` in an existing test, or add a new one, and assert `body["executions"][0]["attempt"] == 1`.
- `tests/test_mcp.py`: in the existing `get_test` test, assert every item in `executions` has an `"attempt"` key.

## Dependencies
- Blocked by: None.
- Why blocked: N/A.
- Blocks: 02 (UI shows `attempt`), 03 (migration `0004` revises `0003`).

## Labels
`feature`, `backend`, `priority:high`

## Estimate
Medium

## Risk
2 - Additive column with a default. The parser change only adds rows for elements it used to drop.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```
