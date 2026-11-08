# GitHub issues

FlakeRadar can open a GitHub issue when a test or job becomes flaky. It can
also link issues that other tools or people filed, and it keeps each link
current.

## Turn on issue filing

Set these values in `.env`:

```
FLAKERADAR_GITHUB_TOKEN=<fine-grained PAT with Issues:write on your repos>
FLAKERADAR_GITHUB_ISSUE_LABEL=flakeradar
```

When a test or job passes the [filing gate](#filing-gate), FlakeRadar opens an
issue in the repo of that test or job. For a test, the issue contains:

- the repo, project, file and line.
- a permalink to the test at its last failing commit.
- the score and the threshold that applied.
- the proven flake count.
- the suite and classname.
- the last failing commit and branch.
- the last 10 executions.
- a sample failure traceback.

FlakeRadar opens one issue for each test or job, with the
`FLAKERADAR_GITHUB_ISSUE_LABEL` label. It opens issues after it processes a
report, never while it accepts an upload. When GitHub returns a rate-limit
error, FlakeRadar stops that batch.

To turn off this feature, leave the token empty.

## Filing gate

`FLAKE_THRESHOLD` sets the "flaky" tier in the dashboard and the API. It does
not control issues. Three independent minimums control issues. FlakeRadar
opens an issue only when a test meets each minimum that is not zero:

```
FLAKERADAR_GITHUB_ISSUE_MIN_SCORE=0.30          # minimum flakiness score (0..1)
FLAKERADAR_GITHUB_ISSUE_MIN_PROVEN_FLAKES=0     # minimum same-commit fail+pass count
FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES=0          # minimum failures in the recent window (each failed retry counts)
```

Set a minimum to `0` to ignore that signal. You can use score, proven flakes,
failures, or any combination. With the defaults, only the score counts (score
0.30 or higher).

If you get false positives, increase these minimums. Do not change the tier
threshold for this. If all three minimums are `0` and a token is set,
FlakeRadar opens an issue for each test that it processes. Keep `MIN_SCORE`
above 0.

When `FLAKERADAR_GITHUB_FIXED_STREAK` is set, it adds a fourth signal. A test or
job that already has that clean streak gets no new issue. Its fix holds, so it
is not a current flake. See [When the fix holds](#when-the-fix-holds).
A failure resets the streak and makes filing possible again.

For a job, the failure count includes only unexplained failures.

## Duplicate issues

FlakeRadar stores the number of the issue on the test or job. While the test or
job has that link, FlakeRadar opens no new issue for it.

At each maintenance interval, FlakeRadar asks GitHub for the state of each
stored issue. When an issue is closed as completed, or deleted, FlakeRadar
clears the stored number. A later report that makes the test flaky again then
opens a new issue.

An issue closed as not planned is the exception. The team has said that the
flake is known and accepted. Thus FlakeRadar keeps the link, sets
`github_issue_closed` on the test or job, and files nothing new. When someone
reopens the issue, FlakeRadar clears the flag.

To file a new issue for a link that a not-planned close holds, remove the link
with one of these:

- `DELETE /api/tests/{id}/issue`
- `DELETE /api/jobs/{id}/issue`
- the MCP tool `unlink_issue`

This clears the link and the flag, and makes filing possible again.

The dashboard shows a link to the issue on the leaderboard row and in the
detail panel. It marks a link that a not-planned close holds. The API and the
MCP server also return the link as `github_issue_number`, `github_issue_url`
and `github_issue_closed`, from `/api/tests`, `/api/tests/{id}/history`, and
the `get_test` and `top_flaky_tests` tools.

## When the fix holds

```
FLAKERADAR_GITHUB_FIXED_STREAK=0                # the clean streak that posts the note
FLAKERADAR_GITHUB_CLOSE_FIXED=false             # also close the issue when the note posts
FLAKERADAR_PUBLIC_URL=                          # dashboard URL used for the link in the note
```

`GITHUB_FIXED_STREAK` is `0` by default, which turns this feature off. When it
is set, FlakeRadar watches each test or job with a linked issue. The clean
streak is the number of consecutive passing runs on the default branch. When
the clean streak reaches the setting, FlakeRadar comments one time on the
issue:

> FlakeRadar: N passing runs on `<default branch>` since the last failure
> (score now X). The fix appears to hold.

FlakeRadar posts the comment after it processes a report, like filing.

The comment does not repeat while the fix holds. If the test or job fails
again, the clean streak goes back to `0`, and a later streak makes a new
comment. When you link the test or job to a different issue, a new comment is
also possible.

With `GITHUB_CLOSE_FIXED` set to `true`, FlakeRadar also closes the issue as
completed when it posts the comment. The next maintenance sync then clears the
link as usual, and a later flake can open a new issue. See
[Duplicate issues](#duplicate-issues).

With `PUBLIC_URL` set, the comment also has a link to that test or job in the
dashboard. Leave it empty to post only the sentence.

## Issues filed elsewhere

Another tool or a person can file the issue instead, for example a triage bot,
an agent, or you. Link the issue to the test or job. FlakeRadar then treats it
as an issue that it filed itself:

- The dashboard and the API show the link.
- FlakeRadar files no duplicate.
- The maintenance sync clears the link when the issue is closed.

There is one difference. An [admin delete](api.md#delete-a-repo-or-a-project)
closes only the issues that FlakeRadar filed. It never comments on or closes an
issue that you linked.

```bash
curl -X PUT "https://flakeradar.example.com/api/tests/$TEST_ID/issue" \
  -H "X-API-Key: $FLAKERADAR_API_TOKEN" -H "Content-Type: application/json" \
  -d '{"number": 123, "url": "https://github.com/owner/name/issues/123"}'
```

- For a job, use `/api/jobs/{id}/issue`. Get the id from `/api/tests`,
  `/api/jobs` or the MCP tools.
- The issue must be in the repo of the test or job. `url` is optional. If you
  send it, it must be the URL of that issue. Otherwise the request fails with
  `422`.
- You can link an issue to a job only when its pipeline runs on GitHub Actions.
- If the test or job already has a link to a different issue, the request fails
  with `409`. Add `?replace=true` to replace the link. The same number again
  succeeds and changes nothing.
- `DELETE` on the same path removes the link.
- Agents can use the MCP tools `link_issue` and `unlink_issue`.

The sync needs `FLAKERADAR_GITHUB_TOKEN`. Without it, a link stays until you
delete it. With it, the token must be able to read the issues of each repo
where you link issues. When GitHub answers `404`, FlakeRadar treats the issue
as deleted and clears the link.
