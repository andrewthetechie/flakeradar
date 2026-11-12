"""The pytest quarantine plugin, against a server that answers like FlakeRadar.

`flakeradar_quarantine.py` marks every collected test whose (classname, name)
pair is on the quarantine list, and skips nothing when the list cannot be read.
These tests run the plugin in a real pytest run through the `pytester` fixture,
against a local HTTP server bound to `127.0.0.1` on a free port that answers
`GET /api/quarantine` and records every request it saw. The example answers come
from `quarantine.json`, the same fixture
`every_quarantine_sample_fixture_has_the_keys_the_endpoint_answers` checks.

Run it from this directory with pytest 9.2.0 and the pytester plugin:

    python -m pytest -p pytester -q test_flakeradar_quarantine.py

or in Docker with `samples/quarantine/test.sh pytest`.
"""

import http.server
import json
import socket
import threading
import urllib.parse
import xml.etree.ElementTree
from pathlib import Path

import pytest

SAMPLE_DIR = Path(__file__).resolve().parent

FIXTURE_PATH = SAMPLE_DIR / "quarantine.json"

# The answer the server serves by default, and its decoded items.
FIXTURE_BODY = FIXTURE_PATH.read_text(encoding="utf-8")
FIXTURE_ITEMS = json.loads(FIXTURE_BODY)

EXAMPLE_SUITE = """
import pytest


def test_total():
    pass


def test_subtotal():
    pass


class TestDiscount:
    @pytest.mark.parametrize("rate", [10, 20])
    def test_rate(self, rate):
        pass
"""


class QuarantineServer:
    """A localhost HTTP server that answers /api/quarantine and records the requests."""

    def __init__(self, body, status=200, redirect_to=None, truncated=False):
        self.body = body
        self.status = status
        # When set, /api/quarantine answers 302 to this path instead of the body.
        self.redirect_to = redirect_to
        # When set, the answer promises more bytes than it ever sends.
        self.truncated = truncated
        self.requests = []
        self._httpd = None
        self._thread = None

    @property
    def url(self):
        return "http://127.0.0.1:{}".format(self._httpd.server_address[1])

    def start(self):
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                parsed = urllib.parse.urlsplit(self.path)
                # The headers object answers case-insensitively, like HTTP does.
                outer.requests.append((parsed.path, parsed.query, self.headers))
                if outer.redirect_to and parsed.path == "/api/quarantine":
                    # The endpoint redirects to a path that serves a valid body,
                    # so a client that follows the redirect sees a 200.
                    self.send_response(302)
                    self.send_header("Location", outer.redirect_to)
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                payload = outer.body.encode("utf-8")
                promised = len(payload) + 500 if outer.truncated else len(payload)
                self.send_response(outer.status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(promised))
                self.end_headers()
                if outer.truncated:
                    # A body that never arrives: the client sees the connection
                    # close before the promised Content-Length is satisfied, and
                    # http.client raises IncompleteRead out of response.read().
                    self.close_connection = True
                    return
                self.wfile.write(payload)

            def log_message(self, *args):
                pass  # Keep the server out of the test output.

        self._httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._thread = threading.Thread(target=self._httpd.serve_forever, daemon=True)
        self._thread.start()
        return self

    def stop(self):
        self._httpd.shutdown()
        self._thread.join(timeout=10)
        self._httpd.server_close()


class RawAnswerServer:
    """A localhost server that answers every request with raw bytes, then hangs up.

    It stands in for the non-HTTP speaker on the other end of a proxy or a
    captive portal, whose answer ``http.client`` cannot frame.
    """

    def __init__(self, answer):
        self.answer = answer
        self._sock = None
        self._thread = None

    @property
    def url(self):
        return "http://127.0.0.1:{}".format(self._sock.getsockname()[1])

    def start(self):
        self._sock = socket.socket()
        self._sock.bind(("127.0.0.1", 0))
        self._sock.listen(1)

        def serve():
            try:
                while True:
                    conn, _ = self._sock.accept()
                    with conn:
                        conn.recv(65536)
                        conn.sendall(self.answer)
            except OSError:
                pass  # The socket closed at the end of the test.

        self._thread = threading.Thread(target=serve, daemon=True)
        self._thread.start()
        return self

    def stop(self):
        self._sock.close()
        self._thread.join(timeout=10)


def closed_port_url():
    """A URL on a port that was bound and then closed, so connecting fails."""
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    return "http://127.0.0.1:{}".format(port)


def prepare(pytester, monkeypatch, url):
    """The plugin on the path, the environment, and the example test file."""
    monkeypatch.syspath_prepend(str(SAMPLE_DIR))
    monkeypatch.setenv("FLAKERADAR_URL", url)
    monkeypatch.setenv("FLAKERADAR_TOKEN", "secret")
    monkeypatch.setenv("FLAKERADAR_REPO", "acme/app")
    monkeypatch.setenv("FLAKERADAR_PROJECT", "backend")
    monkeypatch.delenv("GITHUB_REPOSITORY", raising=False)
    pytester.makepyfile(test_cart=EXAMPLE_SUITE)


def plugin_lines(result):
    return [
        line for line in result.outlines if line.startswith("flakeradar-quarantine: ")
    ]


@pytest.fixture
def flakeradar():
    """A server that serves the sample's quarantine.json."""
    server = QuarantineServer(FIXTURE_BODY).start()
    try:
        yield server
    finally:
        server.stop()


def test_quarantined_tests_are_skipped_and_the_rest_run(pytester, monkeypatch, flakeradar):
    prepare(pytester, monkeypatch, flakeradar.url)

    result = pytester.runpytest("-p", "flakeradar_quarantine", "-rs", "--junitxml=junit.xml")

    result.assert_outcomes(passed=2, skipped=2)
    result.stdout.fnmatch_lines(["*quarantined in FlakeRadar*"])
    assert len(flakeradar.requests) == 1, flakeradar.requests
    path, query, headers = flakeradar.requests[0]
    assert path == "/api/quarantine"
    assert urllib.parse.parse_qs(query) == {
        "repo": ["acme/app"],
        "project": ["backend"],
    }
    assert headers["X-API-Key"] == "secret"


def test_fixture_names_are_what_junitxml_writes(pytester, monkeypatch, flakeradar):
    prepare(pytester, monkeypatch, flakeradar.url)

    result = pytester.runpytest("-p", "flakeradar_quarantine", "--junitxml=junit.xml")

    assert result.ret == 0
    written = {
        (case.get("classname"), case.get("name"))
        for case in xml.etree.ElementTree.parse(pytester.path / "junit.xml").iter("testcase")
    }
    for item in FIXTURE_ITEMS:
        assert (item["classname"], item["name"]) in written


def test_junitprefix_is_honoured(pytester, monkeypatch, flakeradar):
    items = json.loads(flakeradar.body)
    for item in items:
        item["classname"] = "ci." + item["classname"]
    flakeradar.body = json.dumps(items)
    prepare(pytester, monkeypatch, flakeradar.url)

    result = pytester.runpytest(
        "-p", "flakeradar_quarantine", "--junitprefix=ci", "--junitxml=junit.xml"
    )

    result.assert_outcomes(passed=2, skipped=2)


def test_unreachable_flakeradar_skips_nothing(pytester, monkeypatch):
    prepare(pytester, monkeypatch, closed_port_url())

    result = pytester.runpytest("-p", "flakeradar_quarantine")

    result.assert_outcomes(passed=4)
    assert result.ret == 0
    result.stdout.fnmatch_lines(["flakeradar-quarantine: *; skipping nothing"])
    assert len(plugin_lines(result)) == 1


def test_error_status_skips_nothing(pytester, monkeypatch, flakeradar):
    flakeradar.status = 401
    prepare(pytester, monkeypatch, flakeradar.url)

    result = pytester.runpytest("-p", "flakeradar_quarantine")

    result.assert_outcomes(passed=4)
    assert len(plugin_lines(result)) == 1
    result.stdout.fnmatch_lines(["flakeradar-quarantine: *; skipping nothing"])


def test_redirect_skips_nothing(pytester, monkeypatch, flakeradar):
    """A 302 to a valid body is a non-2xx answer, so the run skips nothing."""
    flakeradar.redirect_to = "/quarantine.json"
    prepare(pytester, monkeypatch, flakeradar.url)

    result = pytester.runpytest("-p", "flakeradar_quarantine")

    result.assert_outcomes(passed=4)
    assert result.ret == 0
    assert len(plugin_lines(result)) == 1
    result.stdout.fnmatch_lines(["flakeradar-quarantine: *; skipping nothing"])
    # The plugin never fetches the redirect target.
    assert [request[0] for request in flakeradar.requests] == ["/api/quarantine"]


def test_truncated_body_skips_nothing(pytester, monkeypatch, flakeradar):
    """A body that never arrives in full raises IncompleteRead, not a failed run."""
    flakeradar.truncated = True
    prepare(pytester, monkeypatch, flakeradar.url)

    result = pytester.runpytest("-p", "flakeradar_quarantine")

    result.assert_outcomes(passed=4)
    assert result.ret == 0
    assert len(plugin_lines(result)) == 1
    result.stdout.fnmatch_lines(["flakeradar-quarantine: *; skipping nothing"])


def test_answer_that_is_not_http_skips_nothing(pytester, monkeypatch):
    """A proxy or captive portal answer raises BadStatusLine, not a failed run."""
    server = RawAnswerServer(b"gone now\r\n\r\n").start()
    try:
        prepare(pytester, monkeypatch, server.url)

        result = pytester.runpytest("-p", "flakeradar_quarantine")
    finally:
        server.stop()

    result.assert_outcomes(passed=4)
    assert result.ret == 0
    assert len(plugin_lines(result)) == 1
    result.stdout.fnmatch_lines(["flakeradar-quarantine: *; skipping nothing"])


def test_body_that_is_not_a_list_skips_nothing(pytester, monkeypatch, flakeradar):
    flakeradar.body = '{"detail": "x"}'
    prepare(pytester, monkeypatch, flakeradar.url)

    result = pytester.runpytest("-p", "flakeradar_quarantine")

    result.assert_outcomes(passed=4)
    assert len(plugin_lines(result)) == 1
    result.stdout.fnmatch_lines(["flakeradar-quarantine: *; skipping nothing"])


def test_without_url_does_nothing(pytester, monkeypatch, flakeradar):
    prepare(pytester, monkeypatch, flakeradar.url)
    monkeypatch.delenv("FLAKERADAR_URL")

    result = pytester.runpytest("-p", "flakeradar_quarantine")

    result.assert_outcomes(passed=4)
    assert flakeradar.requests == []
    assert plugin_lines(result) == []


def test_warnings_as_errors_still_fail_open(pytester, monkeypatch):
    prepare(pytester, monkeypatch, closed_port_url())

    result = pytester.runpytest("-p", "flakeradar_quarantine", "-W", "error")

    result.assert_outcomes(passed=4)
    assert result.ret == 0
