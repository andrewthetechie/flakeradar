# MCP server for agents

FlakeRadar has an [MCP](https://modelcontextprotocol.io) server. AI agents can
use it to get the worst flaky tests in a repo. Each test comes with the data
that an agent needs to find and fix it. Agents can also link the issues that
they file.

The server uses the streamable HTTP transport. The endpoint accepts JSON-RPC
`POST` requests. It is not a web page. Its tools only read, except
`link_issue` and `unlink_issue`, which link a GitHub issue to a test or job.
See [Issues filed elsewhere](github-issues.md#issues-filed-elsewhere).

## Endpoint

```
https://<your-host>/mcp/
```

Include the trailing slash. The MCP app is mounted at `/mcp`, and `/mcp/` is
the endpoint. A request to `/mcp` without the slash does not get to the MCP
server.

## Authentication

Each request must send the API token as a bearer token:

```
Authorization: Bearer <FLAKERADAR_API_TOKEN>
```

Without a valid token, the server answers `401 Unauthorized` with
`WWW-Authenticate: Bearer`. `FLAKERADAR_API_TOKEN` is the token in `.env`. CI
systems send the same token in the `X-API-Key` header.

## Connect an MCP client

Give your client the `/mcp/` URL, the streamable HTTP transport and the bearer
token. For example, with the Claude Code CLI:

```bash
claude mcp add --transport http flakeradar https://flakeradar.example.com/mcp/ \
  --header "Authorization: Bearer $FLAKERADAR_API_TOKEN"
```

Other clients, such as Cursor, VS Code and custom agents, need the same three
values:

- URL: `https://<your-host>/mcp/`
- Transport: streamable HTTP
- Credential: the bearer token

In production, always use HTTPS, so that the token is not sent as clear text.

## Tools

| Tool | Returns |
|---|---|
| `list_repos` | All repos, with their projects. |
| `list_projects(repo)` | The projects of one repo. |
| `top_flaky_tests(repo, project?, limit=20, include_suspect=true, file?, category?, new_within_days?)` | The worst tests first. `category` (network, environment, timing, assertion or other) filters by likely cause. `new_within_days=N` (1 to 90) lists only what became flaky in the last N days. |
| `search_tests(repo, query, project?, limit=20)` | Tests whose name, classname or file contains `query`. |
| `get_test(test_id \| repo+project+name[+classname], executions_limit=20)` | The location and GitHub permalink, the last failing commit, the latest failure message and traceback (4 KB maximum), recent executions, and up to 30 days of score history. |
| `top_flaky_jobs(repo, limit=20, include_suspect=true, new_within_days?)` | The worst CI jobs first. Scores count only unexplained failures. `new_within_days=N` (1 to 90) lists only what became flaky in the last N days. |
| `search_jobs(repo, query, limit=20)` | Jobs whose name or pipeline contains `query`. |
| `get_job(job_id \| repo+name[+pipeline], executions_limit=20)` | Unexplained and explained failures, recent executions with the tests that explain them, and up to 30 days of score history. |
| `ci_run_failures(repo, ci_run_id, ci_run_attempt?)` | The failing tests of one CI run attempt (the latest by default) with their current tier and score, its failed jobs (explained or unexplained), and its report counts. |
| `link_issue(number, test_id \| job_id, replace=false)` | Writes. Links GitHub issue `number`, in the repo of the test or job, so FlakeRadar shows it and files no issue of its own. A different linked issue is an error unless `replace` is true. Returns the test or job. |
| `unlink_issue(test_id \| job_id)` | Writes. Removes the issue link of a test or job. Returns the test or job. |

The read tools have the MCP annotation `readOnlyHint: true`. The two write
tools have `readOnlyHint: false`, so your client can ask you before it calls
them.

## Prompts

The server has one prompt, `investigate_flaky_test`. It walks an agent through
investigating one flaky test with the tools above, so every investigation
reads the same evidence.

| Argument | Required | Meaning |
|---|---|---|
| `test_id` | yes | The test's id, from `top_flaky_tests` or `search_tests`, as a positive integer in a string. |
| `repo` | no | The repo's `owner/name`. |

The prompt text tells the agent to:

1. call `get_test` with that `test_id`;
2. read the latest failure message and the Failure details, and open the
   Location: the file, the line, and the GitHub permalink at the last failing
   commit;
3. check `score_history` and `clean_streak` to see whether a past fix held;
4. treat `failure_category` as a hint, not a diagnosis;
5. when it files a GitHub issue, call `link_issue` with that issue number and
   the `test_id`.

It also says that quarantine stays a human decision: the prompt tells the
agent to recommend it, and no tool quarantines anything.

The prompt is a fixed template: the server builds it from the arguments only,
so it reads no database row and calls no model. An unknown prompt name, a
missing `test_id`, or a `test_id` that is not a positive integer answers a
JSON-RPC error with code `-32602` (invalid params).
