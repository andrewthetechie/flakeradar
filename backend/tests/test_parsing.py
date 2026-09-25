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
