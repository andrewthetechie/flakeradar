"""Skip the tests FlakeRadar quarantines, at collection time.

Copy this file into your project, then load it with ``-p flakeradar_quarantine``
or with ``pytest_plugins = ["flakeradar_quarantine"]`` in the root
``conftest.py``. It needs pytest and the standard library only, and Python 3.9
or later.

The plugin reads the quarantine list once per session, in
``pytest_collection_modifyitems``, and marks each quarantined test with
``pytest.mark.skip``. The pair it matches is the one pytest's own junitxml
report writes, so a test quarantined from a FlakeRadar report is the same test
here. When FlakeRadar cannot be reached it writes one line and skips nothing, so
a FlakeRadar outage never fails or hides a test run.

Set ``FLAKERADAR_URL`` to the server, ``FLAKERADAR_REPO`` to the repo as
``owner/name`` (``GITHUB_REPOSITORY`` is used when it is empty),
``FLAKERADAR_TOKEN`` to the API token, and ``FLAKERADAR_PROJECT`` to the
project. When the URL or the repo is empty the plugin does nothing and prints
nothing. Under pytest-xdist there is one session per worker, so each worker
fetches the list on its own.
"""

import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

import pytest
from _pytest.junitxml import mangle_test_address


def _setting(name):
    """The environment variable, or "" when it is unset or empty."""
    return os.environ.get(name) or ""


def _quarantined_pairs(payload):
    """The (classname, name) pairs of a decoded answer, or None when it is invalid.

    A valid answer is a JSON array whose every element is an object with a
    string classname and a non-empty string name. Every other key is ignored,
    and one invalid element invalidates the whole answer.
    """
    if not isinstance(payload, list):
        return None
    pairs = set()
    for item in payload:
        if not isinstance(item, dict):
            return None
        classname = item.get("classname")
        name = item.get("name")
        if not isinstance(classname, str) or not isinstance(name, str) or not name:
            return None
        pairs.add((classname, name))
    return pairs


class _RejectRedirects(urllib.request.HTTPRedirectHandler):
    """Answer a redirect with an error instead of following it.

    urllib follows a 3xx to its target and reports only the final status, so a
    redirect to a valid quarantine body would look like a 200 answer and the
    plugin would skip tests on an answer it cannot trust. Returning no new
    request makes urllib raise HTTPError for the original 3xx status, which is
    fail-open like any other non-2xx status.
    """

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def _fetch(url, repo, project, token):
    """The set of quarantined pairs, or (None, the reason the list is unusable)."""
    query = {"repo": repo}
    if project:
        query["project"] = project
    headers = {"X-API-Key": token} if token else {}
    request = urllib.request.Request(
        "{}/api/quarantine?{}".format(url, urllib.parse.urlencode(query)),
        headers=headers,
    )
    opener = urllib.request.build_opener(_RejectRedirects)
    try:
        with opener.open(request, timeout=10) as response:
            if not 200 <= response.status < 300:
                return None, "FlakeRadar answered {}".format(response.status)
            body = response.read()
    except urllib.error.HTTPError as error:
        return None, "FlakeRadar answered {}".format(error.code)
    except urllib.error.URLError as error:
        return None, "FlakeRadar is unreachable: {}".format(error.reason)
    except (OSError, ValueError) as error:
        # A timed-out socket is an OSError, and a URL urllib cannot build or an
        # answer http.client cannot frame is a ValueError.
        return None, "FlakeRadar cannot be read: {}".format(error)
    try:
        payload = json.loads(body)
    except ValueError as error:
        return None, "FlakeRadar did not answer with JSON: {}".format(error)
    pairs = _quarantined_pairs(payload)
    if pairs is None:
        return None, "FlakeRadar's answer is not a list of items with a name"
    return pairs, None


def _report(config, reason):
    """One fail-open line, through the terminal reporter when there is one."""
    line = "flakeradar-quarantine: {}; skipping nothing".format(reason)
    reporter = config.pluginmanager.get_plugin("terminalreporter")
    if reporter is not None:
        reporter.write_line(line)
    else:
        print(line, file=sys.stderr)


def pytest_collection_modifyitems(session, config, items):
    url = _setting("FLAKERADAR_URL")
    if url.endswith("/"):
        url = url[:-1]
    repo = _setting("FLAKERADAR_REPO") or _setting("GITHUB_REPOSITORY")
    if not url or not repo:
        return
    pairs, reason = _fetch(
        url, repo, _setting("FLAKERADAR_PROJECT"), _setting("FLAKERADAR_TOKEN")
    )
    if pairs is None:
        _report(config, reason)
        return
    # junitxml writes the classname with config.option.junitprefix as its first
    # part when the option is set; None and "" both mean no prefix.
    prefix = getattr(config.option, "junitprefix", None)
    for item in items:
        names = mangle_test_address(item.nodeid)
        classnames = names[:-1]
        if prefix:
            classnames.insert(0, prefix)
        if (".".join(classnames), names[-1]) in pairs:
            item.add_marker(pytest.mark.skip(reason="quarantined in FlakeRadar"))
