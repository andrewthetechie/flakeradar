# 02 — JUnit parsing with Location and Failure details

## Tracer-Bullet Outcome
Given the raw bytes of a JUnit XML report, `app.parsing.parse_junit_xml` returns one `ParsedCase` per `<testcase>`. Each case carries the test's identity, its status, its **Location** (`file`/`line` when the runner emits them) and, for failures, the **Failure message** plus the full **Failure details** (traceback + captured stdout/stderr), capped in size. This is the evidence that agents will later use to find a test in the codebase.

## User Story
As an agent investigating a flaky test, I want the file, line and full traceback captured from CI reports so that I can find and read the failing code without guessing.

## Description
Create a new pure module, `backend/app/parsing.py` (no database, no I/O), and its unit tests. It replaces the parsing half of the deleted `app/ingest.py`.

What the old parser dropped, and this one must capture:
- the `file` and `line` attributes of `<testcase>`
- the **body text** of `<failure>`/`<error>` (the traceback). The old code kept only the `message` attribute.
- `<system-out>` / `<system-err>`

Nothing calls this module yet. Task 03 (validation at upload) and task 04 (processing) will.

## Context Pack
- Source decisions (from `00-shared-context.md`):
  - Location is stored as reported; no guessing paths from classnames.
  - pytest xunit1 lines are 0-based; store them as-is.
  - Failure message cap is 2,000 chars and Failure details cap is 16,384.
  - The fingerprint must stay byte-identical to the old one.
- Repo facts: this is the old parser from the deleted `backend/app/ingest.py`. Keep its behavior for identity and status mapping:
```python
def _case_status(case) -> tuple[str, str]:
    """Map junitparser results to (status, message)."""
    for result in case.result:
        text = (result.message or "") if hasattr(result, "message") else ""
        if isinstance(result, Failure):
            return "failed", text
        if isinstance(result, Error):
            return "error", text
        if isinstance(result, Skipped):
            return "skipped", text
    return "passed", ""

def parse_junit_xml(content: bytes) -> list[ParsedCase]:
    try:
        xml = JUnitXml.fromstring(content.decode("utf-8", errors="replace"))
    except Exception as exc:  # junitparser raises lxml/xml parse errors
        raise IngestError(f"Not a valid JUnit XML report: {exc}") from exc
    # A file may be a <testsuites> wrapper or a single bare <testsuite>.
    suites = list(xml) if isinstance(xml, JUnitXml) else [xml]
    ...  # skips cases whose name is None; raises when no cases were parsed
```
- Verified external contract (junitparser 5.0.3): see `00-shared-context.md`. `case._elem.get("file")` and `case._elem.get("line")` are the only way to read those attributes. `result.text` is the element body. `case.system_out` and `case.system_err` are `str | None`.
- Non-goals: no DB writes; no HTTP; no per-runner line offsets; no XML-bomb hardening; the fingerprint must not include the file.

## Delivery Strategy
- Shape: Wide refactor: Migrate (integration branch). The module itself is complete and fully tested on its own.
- Valid-state scope: Named integration branch `feat/repo-split`. This task adds no new breakage.

## Implementation Contract
- Expected files: create `backend/app/parsing.py` and `backend/tests/test_parsing.py`. Touch nothing else.
- Interfaces and names (required public surface):
```python
MESSAGE_MAX = 2000
DETAILS_MAX = 16384
TRUNCATION_MARKER = "\n…[truncated]"

class ParseError(ValueError): ...

@dataclass(frozen=True)
class ParsedCase:
    suite: str
    classname: str
    name: str
    status: str          # passed | failed | error | skipped
    duration: float
    message: str         # Failure message ("" when passed)
    details: str         # Failure details ("" unless failed/error)
    file: str | None
    line: int | None

def fingerprint(suite: str, classname: str, name: str) -> str   # sha1 hex of f"{suite}::{classname}::{name}"
def parse_junit_xml(content: bytes) -> list[ParsedCase]           # raises ParseError
```
- Behavior rules:
  - **Status:** the first `Failure` gives `failed`, `Error` gives `error`, `Skipped` gives `skipped`, and no result gives `passed`.
  - **Failure message** (failed/error): the stripped `message` attribute. If that is empty, use the first non-blank line of the body. Cap at `MESSAGE_MAX`.
  - **Failure details** (failed/error only): join these parts with a blank line (`"\n\n"`), skipping empty ones: the stripped body; `"--- stdout ---\n" + system_out.strip()`; `"--- stderr ---\n" + system_err.strip()`. Cap at `DETAILS_MAX`.
  - **Skipped:** `message` is the stripped skip message, and `details` is `""`, even if the test printed output.
  - **Passed:** `message` and `details` are `""`.
  - **Cap rule:** if `len(text) > limit`, the result is `text[:limit - len(TRUNCATION_MARKER)] + TRUNCATION_MARKER`, so it is exactly `limit` chars long.
  - **file:** strip whitespace, then remove any number of leading `./`. An empty result becomes `None`.
  - **line:** an `int` only when the stripped attribute is all digits (so `"0"` becomes `0`); anything else becomes `None`.
  - Duplicate `<testcase>` elements with the same identity are **all** returned, in document order. The processor needs both outcomes for same-SHA proof.
  - Cases whose `name` is `None` are skipped. Zero cases → `ParseError("Report parsed but contained no test cases.")`. Undecodable or non-XML input → `ParseError("Not a valid JUnit XML report: …")`.
- Reference implementation (verified: the tests below pass against it):
```python
"""JUnit XML parsing: pure functions, no database.

Extracts, per <testcase>, the identity (suite, classname, name), outcome,
Location (file/line attributes, when the runner emits them) and — for
failures — the Failure message plus Failure details (traceback body and
captured stdout/stderr). Sizes are capped here so storage never sees more.
"""
import hashlib
from dataclasses import dataclass

from junitparser import Error, Failure, JUnitXml, Skipped, TestSuite

MESSAGE_MAX = 2000
DETAILS_MAX = 16384
TRUNCATION_MARKER = "\n…[truncated]"


class ParseError(ValueError):
    """The uploaded bytes are not a usable JUnit XML report."""


@dataclass(frozen=True)
class ParsedCase:
    suite: str
    classname: str
    name: str
    status: str          # passed | failed | error | skipped
    duration: float
    message: str         # Failure message ("" when passed)
    details: str         # Failure details ("" unless failed/error)
    file: str | None     # Location file as reported, "./" stripped
    line: int | None     # Location line as reported (pytest xunit1 is 0-based)


def fingerprint(suite: str, classname: str, name: str) -> str:
    raw = f"{suite}::{classname}::{name}".encode()
    return hashlib.sha1(raw).hexdigest()


def _cap(text: str, limit: int) -> str:
    if len(text) <= limit:
        return text
    return text[: limit - len(TRUNCATION_MARKER)] + TRUNCATION_MARKER


def _first_line(text: str) -> str:
    for line in text.splitlines():
        if line.strip():
            return line.strip()
    return ""


def _location(case) -> tuple[str | None, int | None]:
    raw_file = (case._elem.get("file") or "").strip()  # no public accessor in junitparser
    while raw_file.startswith("./"):
        raw_file = raw_file[2:]
    file = raw_file or None
    raw_line = (case._elem.get("line") or "").strip()
    line = int(raw_line) if raw_line.isdigit() else None
    return file, line


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
    try:
        xml = JUnitXml.fromstring(content.decode("utf-8", errors="replace"))
    except Exception as exc:  # junitparser raises lxml/xml parse errors
        raise ParseError(f"Not a valid JUnit XML report: {exc}") from exc

    # A file may be a <testsuites> wrapper or a single bare <testsuite>.
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
- Error and security rules: none beyond `ParseError`. Never log the report body.

## Acceptance Criteria
- [ ] `fingerprint("unit", "tests.test_mod", "t1") == "2b8823c6c55be0a19a34111f08b687028f5273df"`.
- [ ] `file="./tests/test_a.py" line="0"` → `file == "tests/test_a.py"`, `line == 0`.
- [ ] A failure with a body and system-out/err produces `details` exactly as in the test below.
- [ ] Messages and details longer than the caps are exactly `MESSAGE_MAX`/`DETAILS_MAX` long and end with `…[truncated]`.
- [ ] `b"not xml at all"` and `b"<testsuites></testsuites>"` raise `ParseError`.

## Test Expectations
- Framework: pytest (plain sync tests; no DB fixtures needed).
- File `backend/tests/test_parsing.py` (verified, 11 passing):
```python
"""Pure JUnit parsing: identity, outcome, Location, Failure message/details."""
import pytest

from app.parsing import (
    DETAILS_MAX, MESSAGE_MAX, ParseError, fingerprint, parse_junit_xml,
)


def _one(xml: str):
    cases = parse_junit_xml(xml.encode())
    assert len(cases) == 1
    return cases[0]


def test_fingerprint_is_stable():
    # Byte-identical to the pre-fork fingerprint, so identities never drift.
    assert fingerprint("unit", "tests.test_mod", "t1") == "2b8823c6c55be0a19a34111f08b687028f5273df"
    assert fingerprint("s", "c", "n") == fingerprint("s", "c", "n")
    assert fingerprint("s", "c", "n") != fingerprint("s", "c", "m")


def test_passed_case_without_location():
    c = _one('<testsuite name="unit"><testcase classname="c" name="t" time="0.5"/></testsuite>')
    assert (c.suite, c.classname, c.name, c.status) == ("unit", "c", "t", "passed")
    assert c.duration == 0.5
    assert (c.message, c.details, c.file, c.line) == ("", "", None, None)


def test_location_attributes_are_read_and_normalized():
    c = _one('<testsuites><testsuite name="s"><testcase classname="c" name="t" '
             'file="./tests/test_a.py" line="0"/></testsuite></testsuites>')
    assert c.file == "tests/test_a.py"
    assert c.line == 0  # stored as reported; pytest xunit1 is 0-based


def test_bad_line_is_ignored():
    c = _one('<testsuite name="s"><testcase classname="c" name="t" file="a.py" line="abc"/></testsuite>')
    assert (c.file, c.line) == ("a.py", None)


def test_failure_message_and_details_include_traceback_and_output():
    c = _one(
        '<testsuite name="s"><testcase classname="c" name="t">'
        '<failure message="assert 1 == 2" type="AssertionError">Traceback...\n'
        'tests/test_a.py:14: AssertionError</failure>'
        '<system-out>hello out</system-out><system-err>err here</system-err>'
        '</testcase></testsuite>'
    )
    assert c.status == "failed"
    assert c.message == "assert 1 == 2"
    assert c.details == (
        "Traceback...\ntests/test_a.py:14: AssertionError"
        "\n\n--- stdout ---\nhello out"
        "\n\n--- stderr ---\nerr here"
    )


def test_error_without_message_attr_uses_first_body_line():
    c = _one('<testsuite name="s"><testcase classname="c" name="t">'
             '<error>\n  panic: boom\ngoroutine 1</error></testcase></testsuite>')
    assert c.status == "error"
    assert c.message == "panic: boom"
    assert c.details == "panic: boom\ngoroutine 1"


def test_skipped_keeps_message_but_no_details():
    c = _one('<testsuite name="s"><testcase classname="c" name="t">'
             '<skipped message="not on windows"/><system-out>x</system-out></testcase></testsuite>')
    assert (c.status, c.message, c.details) == ("skipped", "not on windows", "")


def test_caps_are_applied():
    long_msg = "m" * (MESSAGE_MAX + 50)
    long_body = "d" * (DETAILS_MAX + 50)
    c = _one(f'<testsuite name="s"><testcase classname="c" name="t">'
             f'<failure message="{long_msg}">{long_body}</failure></testcase></testsuite>')
    assert len(c.message) == MESSAGE_MAX and c.message.endswith("…[truncated]")
    assert len(c.details) == DETAILS_MAX and c.details.endswith("…[truncated]")


def test_duplicate_testcases_are_all_returned():
    cases = parse_junit_xml(
        b'<testsuite name="s"><testcase classname="c" name="t"><failure message="x"/></testcase>'
        b'<testcase classname="c" name="t"/></testsuite>'
    )
    assert [c.status for c in cases] == ["failed", "passed"]


@pytest.mark.parametrize("body", [b"not xml at all", b"<testsuites></testsuites>"])
def test_invalid_or_empty_reports_raise(body):
    with pytest.raises(ParseError):
        parse_junit_xml(body)
```

## Dependencies
- Blocked by: 01 — Postgres + async foundation
- Why blocked: 01 sets up the new `requirements.txt` (`junitparser>=5.0`) and `pytest.ini`, and deletes `app/ingest.py`, the old home of this code.
- Blocks: 03 (upload validation), 04 (processor), and `tests/factories.py::make_test_case` (imports `fingerprint`)

## Labels
`feature`, `backend`, `ingest`, `priority:high`

## Estimate
Small

## Risk
1 - Pure function with full unit tests.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q tests/test_parsing.py tests/test_db.py tests/test_scoring.py   # expect: 33 passed
```
