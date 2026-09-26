"""JUnit XML parsing: pure functions, no database.

Extracts, per <testcase>, the identity (suite, classname, name), outcome,
Location (file/line attributes, when the runner emits them) and — for
failures — the Failure message plus Failure details (traceback body and
captured stdout/stderr). Sizes are capped here so storage never sees more.
"""

import hashlib
import xml.etree.ElementTree as ET
from dataclasses import dataclass

from junitparser import Error, Failure, JUnitXml, Skipped, TestSuite

MESSAGE_MAX = 2000
DETAILS_MAX = 16384
TRUNCATION_MARKER = "\n…[truncated]"

# Retry elements Playwright (1.59+, includeRetries) and Surefire write as
# children of a <testcase>, which junitparser does not model. `flaky*` ran
# before the final outcome; `rerun*` ran after the first (failed) one.
RETRY_BEFORE = ("flakyFailure", "flakyError")  # attempts before the final outcome
RETRY_AFTER = ("rerunFailure", "rerunError")  # attempts after the first (failed) one


class ParseError(ValueError):
    """The uploaded bytes are not a usable JUnit XML report."""


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


def _load(content: bytes) -> JUnitXml | TestSuite:
    # Bytes first, so the parser honors the declared encoding (e.g. ISO-8859-1).
    # A byte that is invalid in that encoding (binary junk in captured output)
    # falls back to a lenient UTF-8 decode rather than rejecting the report.
    try:
        data = content.decode("utf-8", errors="replace")
        try:
            return JUnitXml.fromstring(content)
        except ET.ParseError:
            return JUnitXml.fromstring(data)
    except Exception as exc:  # junitparser raises xml parse errors
        raise ParseError(f"Not a valid JUnit XML report: {exc}") from exc


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
    status = "failed" if elem.tag.endswith("Failure") else "error"
    stack = (elem.findtext("stackTrace") or "").strip()
    message = (elem.get("message") or "").strip() or _first_line(stack)
    parts = [stack] if stack else []
    out = (elem.findtext("system-out") or "").strip()
    if out:
        parts.append("--- stdout ---\n" + out)
    err = (elem.findtext("system-err") or "").strip()
    if err:
        parts.append("--- stderr ---\n" + err)
    details = "\n\n".join(parts)
    try:
        duration = float(elem.get("time") or 0.0)
    except (TypeError, ValueError):
        duration = 0.0
    return status, _cap(message, MESSAGE_MAX), _cap(details, DETAILS_MAX), duration


def parse_junit_xml(content: bytes) -> list[ParsedCase]:
    xml = _load(content)

    # A file may be a <testsuites> wrapper or a single bare <testsuite>.
    suites = list(xml) if isinstance(xml, JUnitXml) else [xml]
    parsed: list[ParsedCase] = []
    for suite in suites:
        if not isinstance(suite, TestSuite):
            continue
        for case in suite:
            if case.name is None:
                continue
            file, line = _location(case)
            classname = case.classname or ""
            name = case.name
            suite_name = suite.name or ""
            # Retries that ran before the final outcome, then the case's own
            # outcome, then retries that ran after it (document order).
            children = list(case._elem) if case._elem is not None else []
            for c in children:
                if c.tag in RETRY_BEFORE:
                    status, message, details, duration = _retry_outcome(c)
                    parsed.append(
                        ParsedCase(suite_name, classname, name, status, duration, message, details, file, line)
                    )
            status, message, details = _outcome(case)
            parsed.append(
                ParsedCase(suite_name, classname, name, status, float(case.time or 0.0), message, details, file, line)
            )
            for c in children:
                if c.tag in RETRY_AFTER:
                    status, message, details, duration = _retry_outcome(c)
                    parsed.append(
                        ParsedCase(suite_name, classname, name, status, duration, message, details, file, line)
                    )
    if not parsed:
        raise ParseError("Report parsed but contained no test cases.")
    return parsed
