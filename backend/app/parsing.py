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
