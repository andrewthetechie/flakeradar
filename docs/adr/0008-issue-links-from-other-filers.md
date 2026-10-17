---
status: proposed
---

# Other filers can link their issues, and the MCP server can write, but only issue links

Until now only FlakeRadar's own filer set the GitHub issue number on a Test or Job, and the number did two jobs: the dashboard link, and the filer's dedupe (`github_issue_number IS NULL`). An issue filed by another tool (fabro's scout, a triage bot) or by hand was invisible: the dashboard showed no link, and turning FlakeRadar's filing on later filed a duplicate. We now let any authenticated caller set or clear the **Issue link** of a Test or Job, through `PUT`/`DELETE /api/tests/{id}/issue` and `/api/jobs/{id}/issue`, and through two MCP tools, `link_issue` and `unlink_issue`. A linked issue is treated exactly like one FlakeRadar filed: the filer skips the entity, and the closed-issue sync clears the link when GitHub reports the issue closed or gone. These are the MCP server's first write tools. Quarantine stays a human decision and stays out of MCP.

An Issue link stores only the issue number, and the issue must be in the Test's or Job's own Repo. The URL is derived (`https://github.com/{repo}/issues/{number}`), as before. The closed-issue sync asks GitHub about `/repos/{repo}/issues/{number}` in the entity's Repo, so an issue in another repo would be checked against the wrong issue. A caller may send `url`, and FlakeRadar rejects it when it does not match the derived URL. A Job can be linked only when its Pipeline's provider is `github`, the same rule the filer and the dashboard link use.

## Considered Options

- Store the URL (or the issue's repo) in a new column, so an issue can live in any repo: rejected for now. The sync would need the issue's repo too, and the first user (fabro's scout) files in the target repo itself. It can come later as an additive migration.
- Let MCP clients only read, and have agents call REST to write: rejected. The agent that files the issue already talks to FlakeRadar through MCP, and an extra HTTP client with a second header is friction for one integer.
- A separate token or scope for writes: rejected. The MCP server and the CI endpoints already share one token, and the instance runs on a private network. MCP tool annotations (`readOnlyHint`, `destructiveHint`) tell clients which tools write, so a client can ask before it calls one.
- Silently replace an existing link: rejected. A different number already linked answers `409` (MCP: an error) unless the caller passes `replace=true`, so two filers cannot overwrite each other by accident. The same number again is a no-op success.

## Consequences

- When FlakeRadar's GitHub token is set, it must be able to read issues in every Repo where issues are linked. The sync treats a `404` as "issue gone" and clears the link.
- Without a GitHub token, the sync never runs, so a link stays until someone deletes it. The filer does not run either, so no duplicate is filed.
- The filer reads candidates, files on GitHub, then stores the number. A link written between those steps can be overwritten, and a duplicate filed. The window is the length of one GitHub call, so we accept it.
- An agent with the token can now change data through MCP, but only the Issue link. It cannot change scores, quarantine or Reports.
