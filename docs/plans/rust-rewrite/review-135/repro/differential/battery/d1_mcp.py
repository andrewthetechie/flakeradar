"""`D1`: every MCP tool, with valid, string-typed, missing and out-of-range
arguments, and the write tools' rules (`replace=true`, one target only).

Both servers are driven by `fastmcp`'s own client — the same client and the same
version (4.1.6) the goldens were recorded with — so the comparison is of what a
real MCP client sees, not of a hand-built JSON-RPC frame. The envelope fields
that `differences.md` records as allowed to differ are dropped before comparing;
the tool names, titles, descriptions, schemas, annotations and the result's
`content`/`structuredContent`/`isError` are compared unchanged.
"""

from __future__ import annotations

import asyncio
import json
from typing import Any

from fastmcp import Client

from harness import TOKEN, Battery

from seed import PIPELINE, PROJECT, REPO, job_id_of, test_id_of

# The envelope members `differences.md` records: fastmcp writes explicit nulls and
# its own version, rmcp omits them and adds the SEP-2322/2549 members.
ENVELOPE = {"_meta", "meta", "resultType", "ttlMs", "cacheScope", "icons", "execution", "annotations"}


def _strip(value: Any) -> Any:
    if isinstance(value, dict):
        return {k: _strip(v) for k, v in value.items() if k not in ENVELOPE}
    if isinstance(value, list):
        return [_strip(v) for v in value]
    return value


async def _call(client: Client, name: str, arguments: dict) -> dict:
    try:
        result = await client.call_tool_mcp(name, arguments)
        return result.model_dump(mode="json", by_alias=True)
    except Exception as exc:  # noqa: BLE001 - a protocol-level refusal is compared, not raised
        return {"protocolError": type(exc).__name__, "message": str(exc)}


def cases(battery: Battery) -> list[tuple[str, dict, str | None]]:
    """(tool, arguments, known-difference id) for every case the sweep sends."""
    flaky = test_id_of(battery, "test_timing_flake")
    linked = test_id_of(battery, "test_network_flake")
    stable = test_id_of(battery, "test_stable")
    job = job_id_of(battery, "lint")
    ok_job = job_id_of(battery, "frontend")
    rows: list[tuple[str, dict, str | None]] = [
        # list_repos and list_projects: valid, missing, unknown.
        ("list_repos", {}, None),
        ("list_repos", {"extra": 1}, "K-mcp-error-text"),
        ("list_projects", {"repo": REPO}, None),
        ("list_projects", {"repo": REPO.upper()}, None),
        ("list_projects", {"repo": "nope/nope"}, None),
        ("list_projects", {"repo": "bad"}, None),
        ("list_projects", {}, "K-mcp-error-text"),
        ("list_projects", {"repo": 5}, "K-mcp-error-text"),
        # top_flaky_tests: every argument, at both ends and refused.
        ("top_flaky_tests", {"repo": REPO}, None),
        ("top_flaky_tests", {"repo": REPO, "project": PROJECT}, None),
        ("top_flaky_tests", {"repo": REPO, "limit": 1}, None),
        ("top_flaky_tests", {"repo": REPO, "limit": 20}, None),
        ("top_flaky_tests", {"repo": REPO, "include_suspect": False}, None),
        ("top_flaky_tests", {"repo": REPO, "include_suspect": "yes"}, None),
        ("top_flaky_tests", {"repo": REPO, "file": "test_cron"}, None),
        ("top_flaky_tests", {"repo": REPO, "category": "timing"}, None),
        ("top_flaky_tests", {"repo": REPO, "new_within_days": 7}, None),
        ("top_flaky_tests", {"repo": REPO, "limit": 0}, None),
        ("top_flaky_tests", {"repo": REPO, "limit": 101}, None),
        ("top_flaky_tests", {"repo": REPO, "limit": -1}, None),
        ("top_flaky_tests", {"repo": REPO, "limit": "many"}, "K-mcp-error-text"),
        ("top_flaky_tests", {"repo": REPO, "category": "bogus"}, None),
        ("top_flaky_tests", {"repo": REPO, "new_within_days": 0}, None),
        ("top_flaky_tests", {"repo": REPO, "new_within_days": 91}, None),
        ("top_flaky_tests", {}, "K-mcp-error-text"),
        # search_tests.
        ("search_tests", {"repo": REPO, "query": "flake"}, None),
        ("search_tests", {"repo": REPO, "query": "test_cron", "limit": 1}, None),
        ("search_tests", {"repo": REPO, "query": " ", "limit": 1}, None),
        ("search_tests", {"repo": REPO, "query": "flake", "limit": 0}, None),
        ("search_tests", {"repo": REPO}, "K-mcp-error-text"),
        # get_test: by id, by name, ambiguous name, bad id, both ends of the limit.
        ("get_test", {"test_id": flaky}, None),
        ("get_test", {"test_id": flaky, "executions_limit": 1}, None),
        ("get_test", {"repo": REPO, "project": PROJECT, "name": "test_timing_flake"}, None),
        ("get_test", {"repo": REPO, "project": PROJECT, "name": "dup_name"}, None),
        ("get_test", {"repo": REPO, "project": PROJECT, "name": "dup_name", "classname": "first"}, None),
        ("get_test", {"repo": REPO, "project": PROJECT, "name": "nope"}, None),
        ("get_test", {"repo": REPO, "name": "test_stable"}, None),
        ("get_test", {"test_id": 999_999}, None),
        ("get_test", {"test_id": "many"}, "K-mcp-error-text"),
        ("get_test", {"test_id": flaky, "executions_limit": 0}, None),
        ("get_test", {"test_id": flaky, "executions_limit": 501}, None),
        ("get_test", {"name": "test_stable"}, None),
        # top_flaky_jobs and search_jobs.
        ("top_flaky_jobs", {"repo": REPO}, None),
        ("top_flaky_jobs", {"repo": REPO, "limit": 1}, None),
        ("top_flaky_jobs", {"repo": REPO, "include_suspect": False}, None),
        ("top_flaky_jobs", {"repo": REPO, "new_within_days": 7}, None),
        ("top_flaky_jobs", {"repo": REPO, "limit": 0}, None),
        ("search_jobs", {"repo": REPO, "query": "unit"}, None),
        ("search_jobs", {"repo": REPO, "query": ""}, None),
        ("search_jobs", {"repo": REPO, "query": "unit", "limit": 101}, None),
        # get_job: by id, by name, by name and pipeline, bad id.
        ("get_job", {"job_id": job}, None),
        ("get_job", {"job_id": job, "executions_limit": 1}, None),
        ("get_job", {"repo": REPO, "name": "lint"}, None),
        ("get_job", {"repo": REPO, "name": "unit", "pipeline": PIPELINE}, None),
        ("get_job", {"repo": REPO, "name": "nope"}, None),
        ("get_job", {"job_id": 999_999}, None),
        ("get_job", {"job_id": job, "executions_limit": 501}, None),
        ("get_job", {"name": "lint"}, None),
        # ci_run_failures: every CI run the fixture has, and the refused arguments.
        ("ci_run_failures", {"repo": REPO, "ci_run_id": "run-explain"}, None),
        ("ci_run_failures", {"repo": REPO, "ci_run_id": "run-unexplained"}, None),
        ("ci_run_failures", {"repo": REPO, "ci_run_id": "run-summary"}, None),
        ("ci_run_failures", {"repo": REPO, "ci_run_id": "run-cancel"}, None),
        ("ci_run_failures", {"repo": REPO, "ci_run_id": "run-explain", "ci_run_attempt": 1}, None),
        ("ci_run_failures", {"repo": REPO, "ci_run_id": "run-explain", "ci_run_attempt": 0}, None),
        ("ci_run_failures", {"repo": REPO, "ci_run_id": " "}, None),
        ("ci_run_failures", {"repo": REPO, "ci_run_id": "nope"}, None),
        ("ci_run_failures", {"repo": REPO}, "K-mcp-error-text"),
        # link_issue: one target, both, neither, the already-linked rules, replace.
        ("link_issue", {"number": 77, "test_id": stable}, None),
        ("link_issue", {"number": 78, "job_id": ok_job}, None),
        ("link_issue", {"number": 79, "test_id": stable, "job_id": ok_job}, None),
        ("link_issue", {"number": 79}, None),
        ("link_issue", {"number": 0, "test_id": stable}, None),
        ("link_issue", {"number": -1, "test_id": stable}, None),
        ("link_issue", {"number": "many", "test_id": stable}, "K-mcp-error-text"),
        ("link_issue", {"number": 80, "test_id": 999_999}, None),
        # A different issue on an already-linked Test: refused, then replaced.
        ("link_issue", {"number": 4243, "test_id": linked}, None),
        ("link_issue", {"number": 4243, "test_id": linked, "replace": True}, None),
        # The same number again is not a replacement.
        ("link_issue", {"number": 4243, "test_id": linked}, None),
        # unlink_issue: a link, no link, both, neither, unknown.
        ("unlink_issue", {"job_id": ok_job}, None),
        ("unlink_issue", {"job_id": ok_job}, None),
        ("unlink_issue", {"test_id": linked, "job_id": ok_job}, None),
        ("unlink_issue", {}, None),
        ("unlink_issue", {"test_id": 999_999}, None),
    ]
    return rows


async def _drive(battery: Battery, rows: list[tuple[str, dict, str | None]]) -> None:
    async with Client(f"{battery.py}/mcp/", auth=TOKEN, timeout=60, mode="legacy") as python, Client(
        f"{battery.rs}/mcp/", auth=TOKEN, timeout=60, mode="legacy"
    ) as rust:
        # tools/list first: the tool contract itself (names, titles, descriptions,
        # input and output schemas, annotations) is compared unchanged.
        py_tools = [tool.model_dump(mode="json", by_alias=True) for tool in await python.list_tools()]
        rs_tools = [tool.model_dump(mode="json", by_alias=True) for tool in await rust.list_tools()]
        battery.compare("tools/list", _strip(py_tools), _strip(rs_tools))

        for name, arguments, known in rows:
            left = await _call(python, name, arguments)
            right = await _call(rust, name, arguments)
            label = f"{name} {json.dumps(arguments, sort_keys=True)}"
            if known == "K-mcp-error-text":
                # The failure text is a recorded difference; the rest of the
                # answer (isError, the envelope, the structured content) is not.
                battery.compare(
                    label,
                    {k: v for k, v in _strip(left).items() if k != "content"},
                    {k: v for k, v in _strip(right).items() if k != "content"},
                    known="K-mcp-error-text",
                )
                battery.compare(
                    label + " isError",
                    left.get("isError"),
                    right.get("isError"),
                )
            else:
                battery.compare(label, _strip(left), _strip(right), known=known)


def mcp_tools(battery: Battery) -> None:
    battery.phase = "d1"
    rows = cases(battery)
    asyncio.run(_drive(battery, rows))
    battery.note(f"{len(rows)} tool calls plus tools/list, driven by fastmcp's own client")

    # The write tools must have left the same rows behind, not just answered alike.
    battery.rows(
        "SELECT id, name, github_issue_number, github_issue_closed, quarantined "
        "FROM test_cases WHERE github_issue_number IS NOT NULL ORDER BY id",
        label="d1: Test links after the write tools",
    )
    battery.rows(
        "SELECT id, name, github_issue_number, github_issue_closed FROM jobs ORDER BY id",
        label="d1: Job links after the write tools",
    )

