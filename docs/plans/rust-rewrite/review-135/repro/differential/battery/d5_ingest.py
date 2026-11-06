"""`D5`: ingest at and past every boundary — the writers-app `curl`, the
multipart shapes `t8.py` sent, gzip, the content types, the query parameters and
the 20 MiB size limit.

The earlier run of `t8.py` compared two databases whose ids had drifted, so most
of its answers were noise. Each case here is a fresh identity (`commit_sha`
counted up), on two databases built by the same requests, so a difference is the
answer's, not the fixture's.
"""

from __future__ import annotations

import base64
import gzip

from harness import API_KEY, Battery

from seed import PROJECT, REPO

XML = b'<testsuite name="s"><testcase name="t" classname="c"/></testsuite>'
BOUNDARY = "BOUND"
FILE_PART = b'Content-Disposition: form-data; name="report"; filename="r.xml"'


def multipart(parts: list[tuple[bytes, bytes]], *, boundary: str = BOUNDARY, close: bool = True,
              content_type: str | None = None, preamble: bytes = b"") -> tuple[bytes, str]:
    body = preamble
    for header, value in parts:
        body += b"--" + boundary.encode() + b"\r\n" + header + b"\r\n\r\n" + value + b"\r\n"
    if close:
        body += b"--" + boundary.encode() + b"--\r\n"
    return body, content_type or f"multipart/form-data; boundary={boundary}"


def _cases() -> dict[str, tuple[bytes, str | None]]:
    file_part = FILE_PART
    return {
        "plain": multipart([(file_part, XML)]),
        "with content type": multipart([(file_part + b"\r\nContent-Type: application/xml", XML)]),
        "no filename": multipart([(b'Content-Disposition: form-data; name="report"', XML)]),
        "empty filename": multipart([(b'Content-Disposition: form-data; name="report"; filename=""', XML)]),
        "filename star": multipart([(b"Content-Disposition: form-data; name=\"report\"; filename*=UTF-8''r.xml", XML)]),
        "wrong field": multipart([(b'Content-Disposition: form-data; name="file"; filename="r.xml"', XML)]),
        "two fields first bad": multipart([
            (b'Content-Disposition: form-data; name="report"; filename="a.xml"', b"not xml"),
            (file_part, XML),
        ]),
        "two fields last bad": multipart([
            (file_part, XML),
            (b'Content-Disposition: form-data; name="report"; filename="a.xml"', b"not xml"),
        ]),
        "text then file": multipart([
            (b'Content-Disposition: form-data; name="report"', b"x"),
            (file_part, XML),
        ]),
        "file then text": multipart([
            (file_part, XML),
            (b'Content-Disposition: form-data; name="report"', b"x"),
        ]),
        "other then report": multipart([
            (b'Content-Disposition: form-data; name="a"', b"x"),
            (file_part, XML),
        ]),
        "quoted boundary": multipart([(file_part, XML)], content_type='multipart/form-data; boundary="BOUND"'),
        "no boundary": multipart([(file_part, XML)], content_type="multipart/form-data"),
        "boundary upper param": multipart([(file_part, XML)], content_type="multipart/form-data; BOUNDARY=BOUND"),
        "wrong boundary": multipart([(file_part, XML)], content_type="multipart/form-data; boundary=OTHER"),
        "content type charset": multipart([(file_part, XML)], content_type="multipart/form-data; charset=utf-8; boundary=BOUND"),
        "content type upper": multipart([(file_part, XML)], content_type="Multipart/Form-Data; boundary=BOUND"),
        "unclosed": multipart([(file_part, XML)], close=False),
        "preamble crlf": multipart([(file_part, XML)], preamble=b"\r\n"),
        "preamble text": multipart([(file_part, XML)], preamble=b"hello\r\n"),
        "lf only": (b"--BOUND\n" + file_part + b"\n\n" + XML + b"\n--BOUND--\n", "multipart/form-data; boundary=BOUND"),
        "empty file": multipart([(file_part, b"")]),
        "header case": multipart([(file_part.replace(b"Content-Disposition", b"CONTENT-DISPOSITION"), XML)]),
        "unquoted name": multipart([(b"Content-Disposition: form-data; name=report; filename=r.xml", XML)]),
        "single quoted": multipart([(b"Content-Disposition: form-data; name='report'; filename='r.xml'", XML)]),
        "filename first": multipart([(b'Content-Disposition: form-data; filename="r.xml"; name="report"', XML)]),
        "non form-data disposition": multipart([
            (b'Content-Disposition: attachment; name="report"; filename="r.xml"', XML),
        ]),
        "no disposition": multipart([(b"Content-Type: text/plain", XML)]),
        "empty body": (b"", "multipart/form-data; boundary=BOUND"),
        "name with semicolon": multipart([
            (b'Content-Disposition: form-data; name="x;name=report"; filename="r.xml"', XML),
        ]),
        "body holds boundary text": multipart([(file_part, XML + b"\n--BOUNDX")]),
        "trailing whitespace boundary": (
            b"--BOUND  \r\n" + file_part + b"\r\n\r\n" + XML + b"\r\n--BOUND--\r\n",
            "multipart/form-data; boundary=BOUND",
        ),
        "transfer encoding base64": multipart([
            (file_part + b"\r\nContent-Transfer-Encoding: base64", base64.b64encode(XML)),
        ]),
        "header folding": multipart([
            (b'Content-Disposition: form-data;\r\n name="report"; filename="r.xml"', XML),
        ]),
        "bad header line": multipart([(b"garbage\r\n" + file_part, XML)]),
    }


def ingest(battery: Battery) -> None:
    battery.phase = "d5"
    counter = 0

    def query(**extra: str) -> list[tuple[str, str]]:
        nonlocal counter
        counter += 1
        return [("repo", "m/p"), ("commit_sha", f"a{counter}"), *[(k, v) for k, v in extra.items()]]

    # Every multipart shape `t8.py` sent.
    for label, (body, content_type) in _cases().items():
        battery.call(
            "POST",
            "/api/ingest",
            params=query(),
            headers={**API_KEY, "Content-Type": content_type},
            content=body,
            known="K-multipart-detail",
            label=f"multipart {label}",
        )
    # The raw body under each content type: not multipart, so the body is the report.
    raw = [
        ("raw xml", XML, {"Content-Type": "application/xml"}),
        ("urlencoded", XML, {"Content-Type": "application/x-www-form-urlencoded"}),
        ("no content type", XML, {}),
        ("json content type", XML, {"Content-Type": "application/json"}),
        ("gzip body and header", gzip.compress(XML), {"Content-Type": "application/xml", "Content-Encoding": "gzip"}),
        ("gzip body no header", gzip.compress(XML), {"Content-Type": "application/xml"}),
        ("empty", b"", {"Content-Type": "application/xml"}),
        ("whitespace", b" \n", {"Content-Type": "application/xml"}),
    ]
    for label, body, headers in raw:
        battery.call(
            "POST",
            "/api/ingest",
            params=query(),
            headers={**API_KEY, **headers},
            content=body,
            known="K-parse-suffix",
            label=f"raw {label}",
        )
    # The writers-app `curl`: `Content-Type: application/xml`, a raw body and
    # every query parameter `actions/report/report.sh` sends.
    battery.call(
        "POST",
        "/api/ingest",
        params=[
            ("root", "services/api"),
            ("pipeline", ".github/workflows/ci.yml"),
            ("ci_job_id", "job-9"),
            ("ci_run_attempt", "2"),
            ("default_branch", "main"),
            ("repo", "acme/app"),
            ("commit_sha", "writers-app-1"),
        ],
        headers={**API_KEY, "Content-Type": "application/xml"},
        content=b'<testsuite name="writers"><testcase name="t" classname="c"/></testsuite>',
        label="the writers-app curl",
    )
    # The order the route checks things in.
    for label, params, body in [
        ("empty body and bad repo", [("repo", "bad"), ("commit_sha", "x")], b""),
        ("junk body and bad repo", [("repo", "bad"), ("commit_sha", "x")], b"junk"),
        ("bad project", [("repo", "o/r"), ("project", "Bad Name!"), ("commit_sha", "x")], b"junk"),
        ("bad root", [("repo", "o/r"), ("root", "../x"), ("commit_sha", "x")], XML),
        ("absolute root", [("repo", "o/r"), ("root", " /abs/ "), ("commit_sha", "x")], XML),
        ("upper repo", [("repo", "O/R "), ("commit_sha", "x"), ("branch", ""), ("ci_run_id", " x ")], XML),
        ("blank attribution", [("repo", "o/r"), ("commit_sha", "blank"), ("default_branch", "  "),
                               ("ci_job_id", "  "), ("pipeline", "   ")], XML),
        ("blank attribution again", [("repo", "o/r"), ("commit_sha", "blank"), ("default_branch", ""),
                                     ("ci_job_id", ""), ("pipeline", "")], XML),
        ("line_base 0", [("repo", "o/r"), ("commit_sha", "lb"), ("line_base", "0")], XML),
        ("line_base 1", [("repo", "o/r"), ("commit_sha", "lb"), ("line_base", "1")], XML),
        ("line_base 1 duplicate", [("repo", "o/r"), ("commit_sha", "lb"), ("line_base", "1")], XML),
    ]:
        battery.call("POST", "/api/ingest", params=params, headers=API_KEY, content=body, label=f"order {label}")
    for value in ["0", "2", "-1", "x", "", "1.0"]:
        battery.call(
            "POST",
            "/api/ingest",
            params=[("repo", "o/r"), ("commit_sha", "z"), ("line_base", value)],
            headers=API_KEY,
            content=XML,
            label=f"line_base {value}",
        )
    for value in ["0", "-1", "x", "", "1.0", " 1", "3000000000", "99999999999999999999999"]:
        battery.call(
            "POST",
            "/api/ingest",
            params=[("repo", "o/r"), ("commit_sha", "z"), ("ci_run_attempt", value)],
            headers=API_KEY,
            content=XML,
            known="K-attempt-past-int4",
            label=f"attempt {value}",
        )
    # The size limit, at the byte and one past it, on both ingest routes.
    twenty = 20 * 1024 * 1024
    padded = b'<testsuite name="big"><testcase name="t" classname="c"/></testsuite><!--' + b"x" * twenty + b"-->"
    battery.call(
        "POST", "/api/ingest", params=query(), headers={**API_KEY, "Content-Type": "application/xml"},
        content=padded[:twenty], known="K-parse-suffix", label="20 MiB exactly",
    )
    battery.call(
        "POST", "/api/ingest", params=query(), headers={**API_KEY, "Content-Type": "application/xml"},
        content=padded[: twenty + 1], label="20 MiB plus one byte",
    )
    battery.call(
        "POST", "/api/ingest", params=query(), headers={**API_KEY, "Content-Type": "application/xml"},
        content=padded[: twenty + 70_000], label="20 MiB plus 70k",
    )
    huge_json = b'{"a":"' + b"x" * (21 * 1024 * 1024) + b'"}'
    battery.call(
        "POST", "/api/ingest/pipeline", headers={**API_KEY, "Content-Type": "application/json"},
        content=huge_json, known="K-body-limit", label="21 MiB of JSON to the Pipeline route",
    )
    battery.call(
        "POST", "/api/tests/1/quarantine", headers={"Content-Type": "application/json"},
        content=huge_json, known="K-body-limit", label="21 MiB of JSON to the quarantine route",
    )
    # Authentication, which ingest shares with every write route.
    battery.call(
        "POST", "/api/ingest", params=query(), headers={"X-API-Key": "wrong"}, content=XML,
        label="ingest with a wrong key",
    )
    battery.call(
        "POST", "/api/ingest", params=[], headers={"X-API-Key": "wrong"}, content=XML,
        label="ingest with a wrong key and no query",
    )
    battery.call(
        "POST", "/api/ingest", params=[], headers=API_KEY, content=XML,
        known="K-framework-422-fields", label="ingest with no query",
    )
    battery.call(
        "POST", "/api/ingest", params=query(), headers={"x-api-key": f"{API_KEY['X-API-Key']} "}, content=XML,
        label="ingest with a padded key",
    )
    battery.call("GET", "/api/quarantine", headers={"X-API-Key": ""}, label="quarantine list with an empty key")
    battery.call("GET", "/api/quarantine", label="quarantine list with no key")
    # The Pipeline route's own boundary: 1001 Jobs, and a body that is not JSON.
    battery.call(
        "POST", "/api/ingest/pipeline", headers=API_KEY,
        json_body={"repo": "acme/app", "provider": "github", "pipeline": "ci.yml", "commit_sha": "x",
                   "branch": "main", "ci_run_id": "big",
                   "jobs": [{"ci_job_id": str(i), "name": "n", "status": "passed"} for i in range(1001)]},
        known="K-framework-422-fields", label="a Pipeline report with 1001 Jobs",
    )
    battery.call(
        "POST", "/api/ingest/pipeline", headers={**API_KEY, "Content-Type": "application/json"},
        content=b"not json", known="K-json-invalid-offset", label="a Pipeline body that is not JSON",
    )
    battery.drain()
    battery.dump(["reports"], label="d5: the Reports every ingest case left")
    battery.note(
        f"{counter} accepted-or-refused ingest uploads across {len(_cases())} multipart shapes, "
        "eight raw content types, the writers-app curl, both size boundaries and both Pipeline boundaries"
    )
