# 11 — MCP

Read `00-shared-context.md` first. Tasks 01–10 have landed.

Python reference: `backend/app/mcp_server.py`, `backend/tests/test_mcp.py`, the MCP golden files from task 02.

## Goal

Agents get the same 11 tools, through the official `rmcp` SDK.

## Contracts

### Transport

- `StreamableHttpService` with `NeverSessionManager` (stateless), mounted at `/mcp/`. Check `rmcp`'s current config docs for the JSON-response and stateless settings.
- `POST /mcp` returns 307 to `/mcp/`. README documents `https://<host>/mcp/`, and some clients drop the slash.
- `Authorization: Bearer <FLAKERADAR_API_TOKEN>`, compared in constant time, in a Tower layer in front of the service. A missing or wrong token is 401 with `WWW-Authenticate: Bearer`, and matches the recorded body where practical. Log any difference in `differences.md`.

Confirm no tool uses session state or server-initiated messages: `mcp_server.py` has no `Context` use.

### Server info

Name `FlakeRadar`. `instructions` copied **verbatim** from `INSTRUCTIONS`. Add a test against the golden text.

### Tools

`list_repos`, `list_projects`, `top_flaky_tests`, `search_tests`, `get_test`, `top_flaky_jobs`, `search_jobs`, `get_job`, `ci_run_failures`, `link_issue`, `unlink_issue`.

For each tool, keep the same as the Python and the golden `tools/list`:

- name and description (the Python docstring)
- argument names, types, defaults and required list
- annotations: `_READ_ONLY`, `_LINK`, `_UNLINK`

Check the input schemas against the golden semantically, since `schemars` may arrange them differently. Do not add `outputSchema` unless the golden has it; if it does, keep it where the SDK makes this practical, and log any gap.

### Behaviour

Port each tool body. Keep:

- `MAX_LIMIT = 100`, `MAX_NEW_WITHIN_DAYS = 90`, MCP `DETAILS_MAX = 4096` with `\n…[truncated]` appended after the limit (unlike `_cap`)
- `_EXPLAINED_BY_MAX = 5` and `_JOB_EXECUTION_FIELDS`, which trims Job execution fields to save tokens
- `_only_id`'s lookups by name and its ambiguity text
- all `ToolError` text, returned as `isError: true` results with the same message
- `_require_one_target`: exactly one of `test_id` or `job_id`
- link rules: reuse task 10's engine and its error text. Quarantine stays out of MCP

Results: match the golden's content shape, with JSON text content and `structuredContent` where the golden has it, including the `{"result": [...]}` wrapper for list-returning tools.

Tools call the query functions from task 09 and never make HTTP calls to the app.

## Tests to port

`test_mcp` (24). Use `rmcp`'s client over a real local HTTP listener, so the whole transport is tested.

Add tests for:

- no token, wrong token, right token
- `POST /mcp` redirect
- two requests with no session header both succeeding (the stateless proof)
- each error text

Enable all MCP parity scenarios.

## Done when

- [ ] All verification in `00-shared-context.md` passes.
- [ ] All MCP golden scenarios pass.
- [ ] A real client works: point a local Claude Code MCP config at `http://localhost:8000/mcp/`, list tools and call `list_repos` and `top_flaky_tests`.
- [ ] Manifest rows for this task are not `pending`, and gaps are logged.
- [ ] `STATUS.md` is updated, and the branch is pushed.
