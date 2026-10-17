# 05 — README, end-to-end check, open the PR

## Tracer-Bullet Outcome
The README's GitHub issue automation section tells an external filer how to link its issues. The API table lists the four routes, and the MCP section lists the two write tools and no longer says "read-only". A fresh Docker stack shows the REST routes, the MCP tools and the dashboard link working on simulated data. One PR is open against `main` and closes issue #6.

## User Story
As the maintainer, I want one reviewable PR with docs and evidence that an external filer can link, replace and unlink issues through REST and MCP.

## Description
Documentation, verification, then the PR. Other code changes here are only fixes for problems the checks find.

## Context Pack
- Source decisions: `00-shared-context.md` (all). Issue #6 "Done when": both verbs work for Tests and Jobs with the `409`/`replace` rule (tasks 01, 02); the filer skips a linked entity and the re-arm still fires when a linked issue closes (task 03); the README's GitHub issue automation section says how an external filer links its issues (this task). The user also asked for the MCP tools (task 04).
- Repo facts (`README.md`):
  - `## What it does` has the bullet `- **MCP server.** AI agents can get the worst flaky tests in a repo, with the` / `data they need to find and fix each test.` and the bullet `- **GitHub issues (optional).** When a test or job crosses the flakiness` / `threshold, FlakeRadar opens an issue in the repository of that test or job.`
  - `## MCP server for agents` starts: `FlakeRadar has a read-only [MCP](https://modelcontextprotocol.io) server. It` / `uses the streamable HTTP transport. The endpoint accepts JSON-RPC \`POST\`` / `requests. It is not a web page.`
  - Its `### Tools` table ends with the row for `ci_run_failures(repo, ci_run_id, ci_run_attempt?)`. Then `## GitHub issue automation` starts.
  - `## GitHub issue automation` has subsections `### Filing gate` and `### Duplicate issues`. `### Duplicate issues` ends with the paragraph that starts `The UI shows a link to the issue on the leaderboard row`. Then `## How scoring works` starts.
  - `## Configuration` table row: `| \`GITHUB_TOKEN\` | empty (disabled) | The token for GitHub issue automation. |`
  - `## API` starts with: `The read APIs and the quarantine toggle have no authentication. Put the` / `dashboard on a private network. The CI endpoints and the MCP server need the` / `token.` The table has the row `| \`POST /api/tests/{id}/quarantine\` | none | Set or clear quarantine (body \`{"quarantined": bool}\`). |`.
  - `## Contents` lists only `##` sections. No change is needed there.
- Local stack: `cp .env.example .env`, set `FLAKERADAR_API_TOKEN` (or `FLAKERADAR_ALLOW_INSECURE=1` for a throwaway run), then `docker compose up --build`. App, API and MCP on `http://localhost:8000`. Simulator: `python samples/simulate_ci.py http://localhost:8000 <token>`. It fills Repo `demo/shop` and the github Pipeline `.github/workflows/ci.yml`, and waits until no Report is pending. Leave `FLAKERADAR_GITHUB_TOKEN` empty, so nothing talks to GitHub.
- Non-goals: new features; editing `CONTEXT.md` or ADRs (Step 1 only checks them). If the build had to differ from `00-shared-context.md`, list the difference in the PR body.

## Delivery Strategy
- Shape: Normal tracer bullet (final verification; every earlier task is already green on its own)
- Valid-state scope: `feat/issue-links`, and the PR's CI

## Implementation Contract
- Expected files: `README.md`. Also any file that needs a fix found by the checks (each fix in its own commit that names the task it belongs to).
- Interfaces and names: None new.
- Verified external contracts: `gh pr create --base main --head feat/issue-links --title … --body-file …` (GitHub CLI).
- Behavior rules (exact text):
  - `## What it does`, the MCP bullet becomes:
    ```markdown
    - **MCP server.** AI agents can get the worst flaky tests in a repo, with the
      data they need to find and fix each test, and link the issues they file.
    ```
    The GitHub issues bullet becomes:
    ```markdown
    - **GitHub issues (optional).** When a test or job crosses the flakiness
      threshold, FlakeRadar opens an issue in the repository of that test or job.
      Issues filed by other tools or by hand can be linked instead.
    ```
  - `## MCP server for agents`, first paragraph becomes:
    ```markdown
    FlakeRadar has an [MCP](https://modelcontextprotocol.io) server. It uses the
    streamable HTTP transport. The endpoint accepts JSON-RPC `POST` requests. It
    is not a web page. Its tools only read, except `link_issue` and
    `unlink_issue`, which link a GitHub issue to a test or job (see
    [Issues filed elsewhere](#issues-filed-elsewhere)).
    ```
  - `### Tools` table: add two rows after the `ci_run_failures` row:
    ```markdown
    | `link_issue(number, test_id \| job_id, replace=false)` | **Writes.** Links GitHub issue `number`, in the repo of the test or job, so FlakeRadar shows it and files no issue of its own. A different linked issue is an error unless `replace` is true. Returns the test or job. |
    | `unlink_issue(test_id \| job_id)` | **Writes.** Removes the issue link of a test or job. Returns the test or job. |
    ```
    And this paragraph right after the table:
    ```markdown
    The read tools have the MCP annotation `readOnlyHint: true`. The two write
    tools have `readOnlyHint: false`, so your client can ask you before it calls
    them.
    ```
  - New subsection `### Issues filed elsewhere` at the end of `## GitHub issue automation` (after `### Duplicate issues`, before `## How scoring works`):
    ````markdown
    ### Issues filed elsewhere

    Another tool or a person can file the issue instead: a triage bot, an
    agent, or you by hand. Link the issue to the test or job. FlakeRadar then
    treats it as an issue that it filed itself. The UI and API show the link,
    FlakeRadar files no duplicate, and the maintenance sync clears the link when
    the issue is closed.

    ```bash
    curl -X PUT "https://flakeradar.example.com/api/tests/$TEST_ID/issue" \
      -H "X-API-Key: $FLAKERADAR_API_TOKEN" -H "Content-Type: application/json" \
      -d '{"number": 123, "url": "https://github.com/owner/name/issues/123"}'
    ```

    - For a job, use `/api/jobs/{id}/issue`. Get the id from `/api/tests`,
      `/api/jobs` or the MCP tools.
    - The issue must be in the repo of the test or job. `url` is optional. If
      you send it, it must be the URL of that issue, or the request fails with
      `422`.
    - You can link an issue to a job only when its pipeline runs on GitHub
      Actions.
    - If the test or job already has a link to a different issue, the request
      fails with `409`. Add `?replace=true` to replace the link. The same number
      again succeeds and changes nothing.
    - `DELETE` on the same path removes the link.
    - Agents can use the MCP tools `link_issue` and `unlink_issue`.

    The sync needs `FLAKERADAR_GITHUB_TOKEN`. Without it, a link stays until you
    delete it. With it, the token must be able to read the issues of each repo
    where you link issues: when GitHub answers `404`, FlakeRadar treats the
    issue as deleted and clears the link.
    ````
  - `## Configuration`, the `GITHUB_TOKEN` row's meaning becomes: `The token for GitHub issue automation. It also checks linked issues, so it needs read access to the issues of those repos.`
  - `## API`, the first sentence block becomes:
    ```markdown
    The read APIs and the quarantine toggle have no authentication. Put the
    dashboard on a private network. The CI endpoints, the issue link endpoints
    and the MCP server need the token.
    ```
  - `## API` table: add three rows right after the `POST /api/tests/{id}/quarantine` row:
    ```markdown
    | `PUT /api/tests/{id}/issue?replace=` | `X-API-Key` | Link a GitHub issue filed elsewhere (body `{"number": int, "url"?: str}`). `409` when a different issue is linked and `replace` is not `true`. See [Issues filed elsewhere](#issues-filed-elsewhere). |
    | `DELETE /api/tests/{id}/issue` | `X-API-Key` | Remove the issue link of a test. |
    | `PUT /api/jobs/{id}/issue?replace=`, `DELETE /api/jobs/{id}/issue` | `X-API-Key` | The same for a job. GitHub pipelines only. |
    ```
- Error and security rules: None.

## Steps
1. **Docs check.** Read `CONTEXT.md` (**Issue link**) and `docs/adr/0008-issue-links-from-other-filers.md`. If what you built differs from them, do not edit them: list the difference in the PR body for the reviewer.
2. Make the README changes.
3. Run every check in the Validator and fix anything that fails.
4. **End-to-end checks** on a fresh stack (`docker compose down -v`, then `docker compose up --build -d`, then run the simulator). Set `TOKEN` to your API token and `BASE=http://localhost:8000`.
   - Pick a Test: `curl -s "$BASE/api/tests?repo=demo/shop" | python -m json.tool`. Take the `id` of one with `github_issue_number: null` as `TID`.
   - `curl -si -X PUT "$BASE/api/tests/$TID/issue" -H 'Content-Type: application/json' -d '{"number": 101}'` → `401`.
   - The same with `-H "X-API-Key: $TOKEN"` → `200`, `github_issue_number: 101`, `github_issue_url: "https://github.com/demo/shop/issues/101"`.
   - `{"number": 102}` → `409` with `Test already links issue #101; pass replace=true to replace it`. With `?replace=true` → `200`, `102`.
   - `{"number": 103, "url": "https://github.com/other/repo/issues/103"}` with `?replace=true` → `422`.
   - `curl -s "$BASE/api/tests/$TID/history" | python -m json.tool` → `test.github_issue_number: 102`.
   - Open `http://localhost:8000/?repo=demo/shop` in a browser (or Playwright). The Test's leaderboard row shows `#102`, and the link points at `https://github.com/demo/shop/issues/102`. Take a screenshot for the PR.
   - Pick a Job: `curl -s "$BASE/api/jobs?repo=demo/shop&include_stable=true"`, take an `id` as `JID`. `PUT /api/jobs/$JID/issue` `{"number": 201}` with the key → `200`, then `DELETE` → `200`, `github_issue_number: null`.
   - `DELETE /api/tests/$TID/issue` with the key → `200`, `null`.
   - **MCP**, with a fastmcp `Client` against `$BASE/mcp/` and `auth=TOKEN` (or `claude mcp add --transport http flakeradar http://localhost:8000/mcp/ --header "Authorization: Bearer <token>"`): `list_tools` shows `link_issue` and `unlink_issue` with `readOnlyHint: false` and every other tool with `readOnlyHint: true`. `link_issue(number=104, test_id=TID)` returns the Test with `github_issue_number: 104`. `unlink_issue(test_id=TID)` clears it.
   - `docker compose logs flakeradar | grep -i "issue"` shows the `Linked issue …` / `Unlinked issue …` log lines.
5. **Open the PR:** `gh pr create --base main --head feat/issue-links --title "API and MCP: link an issue filed elsewhere to a test or job" --body-file <file>`. The body has:
   - a summary: the four REST routes, the two MCP tools (the first MCP write tools), and the `409`/`replace` rule;
   - the decisions beyond the issue text: no new column, so `url` is optional and is only checked against the derived URL, and the issue must be in the entity's own Repo (ADR 0008); a Job can be linked only on a `github` Pipeline; the same number is a no-op `200`; `DELETE` is idempotent; the read tools now carry `readOnlyHint: true`;
   - the known gap from ADR 0008 (the filer can overwrite a link written during its GitHub call), and the token consequence (a token that cannot read a repo's issues makes the sync clear links there);
   - the CONTEXT.md and ADR 0008 changes (they came with the plan commit);
   - the task list (01–04) with commit ranges;
   - the end-to-end checks from step 4 and what you saw, with the screenshot;
   - `Closes #6`;
   - and it ends with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.

## Acceptance Criteria
- [ ] Every command in the Validator passes locally.
- [ ] The end-to-end checks in step 4 are done, and the results are in the PR body.
- [ ] The README has the new subsection, both table changes, the MCP text change and the Configuration change, and it no longer calls the MCP server "read-only".
- [ ] The PR is open against `main`, its body contains `Closes #6`, and its CI is green.

## Test Expectations
- No new automated tests unless a check finds a bug. A bug fix comes with a regression test in `backend/tests/test_issue_links_api.py` (REST), `backend/tests/test_github.py` (filer or sync) or `backend/tests/test_mcp.py` (MCP).

## Validator
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
grep -n "read-only \[MCP\]" README.md && exit 1 || true
```

## Dependencies
- Blocked by: 01, 02, 03, 04.
- Blocks: None.

## Labels
`chore`, `docs`, `priority:high`
