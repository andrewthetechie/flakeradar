"""Pure JUnit parsing: identity, outcome, Location, Failure message/details."""

from pathlib import Path

import pytest
from app.parsing import (
    DETAILS_MAX,
    MESSAGE_MAX,
    ParseError,
    fingerprint,
    parse_junit_xml,
)

FIXTURES = Path(__file__).parent / "fixtures"


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
    c = _one(
        '<testsuites><testsuite name="s"><testcase classname="c" name="t" '
        'file="./tests/test_a.py" line="0"/></testsuite></testsuites>'
    )
    assert c.file == "tests/test_a.py"
    assert c.line == 0  # stored as reported; pytest xunit1 is 0-based


def test_bad_line_is_ignored():
    c = _one('<testsuite name="s"><testcase classname="c" name="t" file="a.py" line="abc"/></testsuite>')
    assert (c.file, c.line) == ("a.py", None)


def test_line_base_zero_shifts_lines_to_one_based():
    xml = '<testsuite name="s"><testcase classname="c" name="t" file="tests/a.py" line="11"/></testsuite>'
    assert parse_junit_xml(xml.encode(), 0)[0].line == 12
    # Default and explicit 1 store the reported line unchanged.
    assert parse_junit_xml(xml.encode())[0].line == 11
    assert parse_junit_xml(xml.encode(), 1)[0].line == 11


def test_line_base_zero_shifts_line_zero_to_one():
    # The case that previously lost its `#L` anchor entirely.
    xml = '<testsuite name="s"><testcase classname="c" name="t" file="tests/a.py" line="0"/></testsuite>'
    assert parse_junit_xml(xml.encode(), 0)[0].line == 1


def test_line_base_zero_leaves_non_digit_lines_as_none():
    xml = '<testsuite name="s"><testcase classname="c" name="t" file="a.py" line="abc"/></testsuite>'
    case = parse_junit_xml(xml.encode(), 0)[0]
    assert (case.file, case.line) == ("a.py", None)


def test_failure_message_and_details_include_traceback_and_output():
    c = _one(
        '<testsuite name="s"><testcase classname="c" name="t">'
        '<failure message="assert 1 == 2" type="AssertionError">Traceback...\n'
        "tests/test_a.py:14: AssertionError</failure>"
        "<system-out>hello out</system-out><system-err>err here</system-err>"
        "</testcase></testsuite>"
    )
    assert c.status == "failed"
    assert c.message == "assert 1 == 2"
    assert c.details == (
        "Traceback...\ntests/test_a.py:14: AssertionError\n\n--- stdout ---\nhello out\n\n--- stderr ---\nerr here"
    )


def test_error_without_message_attr_uses_first_body_line():
    c = _one(
        '<testsuite name="s"><testcase classname="c" name="t">'
        "<error>\n  panic: boom\ngoroutine 1</error></testcase></testsuite>"
    )
    assert c.status == "error"
    assert c.message == "panic: boom"
    assert c.details == "panic: boom\ngoroutine 1"


def test_skipped_keeps_message_but_no_details():
    c = _one(
        '<testsuite name="s"><testcase classname="c" name="t">'
        '<skipped message="not on windows"/><system-out>x</system-out></testcase></testsuite>'
    )
    assert (c.status, c.message, c.details) == ("skipped", "not on windows", "")


def test_caps_are_applied():
    long_msg = "m" * (MESSAGE_MAX + 50)
    long_body = "d" * (DETAILS_MAX + 50)
    c = _one(
        f'<testsuite name="s"><testcase classname="c" name="t">'
        f'<failure message="{long_msg}">{long_body}</failure></testcase></testsuite>'
    )
    assert len(c.message) == MESSAGE_MAX and c.message.endswith("…[truncated]")
    assert len(c.details) == DETAILS_MAX and c.details.endswith("…[truncated]")


def test_duplicate_testcases_are_all_returned():
    cases = parse_junit_xml(
        b'<testsuite name="s"><testcase classname="c" name="t"><failure message="x"/></testcase>'
        b'<testcase classname="c" name="t"/></testsuite>'
    )
    assert [c.status for c in cases] == ["failed", "passed"]


def test_flaky_failure_becomes_a_failed_attempt_before_the_pass():
    xml = (
        '<testsuite name="s"><testcase classname="c" name="t" time="1.2">'
        '<flakyFailure message="Timeout 30000ms exceeded" type="FAILURE" time="30.1">'
        "<stackTrace>Error: boom\n at x</stackTrace><system-out>out1</system-out></flakyFailure>"
        "<system-out>final</system-out></testcase></testsuite>"
    )
    cases = parse_junit_xml(xml.encode())
    assert [c.status for c in cases] == ["failed", "passed"]
    assert cases[0].message == "Timeout 30000ms exceeded"
    assert cases[0].duration == 30.1
    assert cases[0].details == "Error: boom\n at x\n\n--- stdout ---\nout1"


def test_nextest_flaky_marker_is_a_passed_attempt():
    cases = parse_junit_xml((FIXTURES / "nextest_flaky_fail.xml").read_bytes())
    flaky = [c for c in cases if c.name.endswith("invalidate_priority_readiness_drops_from_each_driver")]
    assert [c.status for c in flaky] == ["failed", "passed"]
    assert flaky[0].message.startswith("thread 'embedding::reindexer")
    assert flaky[0].message.endswith("worker.rs:2706:17")
    assert flaky[0].duration == 0.398
    assert (flaky[1].message, flaky[1].details, flaky[1].duration) == ("", "", 0.412)
    other = [c for c in cases if c not in flaky]
    assert [(c.name, c.status) for c in other] == [("embedding::chunker::tests::splits_on_headings", "passed")]


def test_nextest_flaky_marker_in_error_is_a_passed_attempt():
    xml = (
        '<testsuite name="s"><testcase classname="c" name="t">'
        '<flakyError message="boom"/><flakyError message="boom"/>'
        '<error message="test passed on attempt 3/3 but is configured to fail when flaky"/>'
        "</testcase></testsuite>"
    )
    cases = parse_junit_xml(xml.encode())
    assert [c.status for c in cases] == ["error", "error", "passed"]


def test_nextest_flaky_marker_without_earlier_tries_stays_failed():
    xml = (
        '<testsuite name="s"><testcase classname="c" name="t">'
        '<failure message="test passed on attempt 2/2 but is configured to fail when flaky"/>'
        "</testcase></testsuite>"
    )
    cases = parse_junit_xml(xml.encode())
    assert [c.status for c in cases] == ["failed"]
    assert cases[0].message == "test passed on attempt 2/2 but is configured to fail when flaky"


def test_message_that_only_starts_like_the_marker_stays_failed():
    xml = (
        '<testsuite name="s"><testcase classname="c" name="t">'
        '<flakyFailure message="x"/>'
        '<failure message="test passed on attempt 2/2 but is configured to fail when flaky, and more"/>'
        "</testcase></testsuite>"
    )
    cases = parse_junit_xml(xml.encode())
    assert [c.status for c in cases] == ["failed", "failed"]


def test_rerun_elements_follow_the_failure():
    xml = (
        '<testsuite name="s"><testcase classname="c" name="t">'
        '<failure message="first">t</failure>'
        '<rerunError message="again"/><rerunError message="again"/></testcase></testsuite>'
    )
    cases = parse_junit_xml(xml.encode())
    assert [c.status for c in cases] == ["failed", "error", "error"]


def test_retry_without_message_uses_stacktrace_first_line():
    xml = (
        '<testsuite name="s"><testcase classname="c" name="t">'
        "<flakyError><stackTrace>\n  TypeError: x\n  at y</stackTrace></flakyError></testcase></testsuite>"
    )
    c = parse_junit_xml(xml.encode())[0]
    assert c.message == "TypeError: x"
    assert c.status == "error"
    assert c.duration == 0.0


def test_declared_encoding_is_honored():
    body = (
        '<?xml version="1.0" encoding="ISO-8859-1"?>'
        '<testsuite name="s"><testcase classname="c" name="café"/></testsuite>'
    )
    assert parse_junit_xml(body.encode("latin-1"))[0].name == "café"


def test_invalid_utf8_bytes_are_replaced_not_rejected():
    c = parse_junit_xml(b'<testsuite name="s"><testcase classname="c" name="bad\xff"/></testsuite>')[0]
    assert c.name == "bad�"


@pytest.mark.parametrize("body", [b"not xml at all", b"<testsuites></testsuites>"])
def test_invalid_or_empty_reports_raise(body):
    with pytest.raises(ParseError):
        parse_junit_xml(body)


def test_nameless_testcase_is_dropped():
    # Lines 149-150: `if case.name is None: continue`.
    cases = parse_junit_xml(
        b'<testsuite name="s"><testcase classname="c"/><testcase classname="c" name="n"/></testsuite>'
    )
    assert [(c.name, c.status) for c in cases] == [("n", "passed")]


def test_retry_time_that_is_not_a_number_is_zero():
    # Lines 134-135: `except (TypeError, ValueError): duration = 0.0`.
    cases = parse_junit_xml(
        b'<testsuite name="s"><testcase classname="c" name="r">'
        b'<flakyFailure message="m" time="abc"/></testcase></testsuite>'
    )
    assert [(c.status, c.duration) for c in cases] == [("failed", 0.0), ("passed", 0.0)]


def test_failure_without_message_or_body_is_empty_strings():
    # _first_line("") with no message attribute: the `else ""` arm.
    c = _one('<testsuite name="s"><testcase classname="c" name="n"><failure/></testcase></testsuite>')
    assert (c.status, c.message, c.details) == ("failed", "", "")


def test_properties_child_of_testsuites_is_not_yielded():
    # Pins why the isinstance guard is dead: JUnitXml.__iter__ yields only
    # TestSuite children, so <properties> never reaches it.
    cases = parse_junit_xml(
        b'<testsuites><properties/><testsuite name="s"><testcase classname="c" name="n"/></testsuite></testsuites>'
    )
    assert [(c.name, c.status) for c in cases] == [("n", "passed")]
