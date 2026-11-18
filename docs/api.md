# API reference

This page lists each HTTP endpoint of FlakeRadar. The server also serves the
OpenAPI specification at `/openapi.json`, and a read-only reference at `/docs`.
The `/docs` page renders the specification with Redoc and sends requests only
to this server. It has no interactive "try it" console.

## Authentication

The read APIs and the quarantine toggle have no authentication. Put the
dashboard on a private network.

The other endpoints need the API token (`FLAKERADAR_API_TOKEN`):

- An endpoint with `X-API-Key` in the Auth column needs the token in the
  `X-API-Key` header. These are the CI, admin, report retry, quarantine list
  and issue link endpoints.
- The [MCP server](mcp.md) at `/mcp/` needs the token as
  `Authorization: Bearer <token>`.

## Ingest

| Endpoint | Auth | Purpose |
|---|---|---|
| `POST /api/ingest?repo=&project=&root=&line_base=&commit_sha=&branch=&ci_run_id=&ci_run_attempt=&ci_job_id=&pipeline=&default_branch=` | `X-API-Key` | Put a JUnit XML report in the queue. The body is the raw XML or a multipart `report` field, 20 MB maximum. Returns `202` with `{report_id, status, duplicate}`. `duplicate` is `true` when an identical upload with the same identity was already stored. The response then gives that report, and FlakeRadar stores no new one. See [Retried uploads](ci-integration.md#retried-uploads). |
| `POST /api/ingest/pipeline` | `X-API-Key` | Put a pipeline report (JSON) with `jobs` in the queue. Returns `202`. See [Other CI providers](ci-integration.md#other-ci-providers). |

[CI integration](ci-integration.md#other-ci-systems) describes each ingest
parameter.

## Reports

| Endpoint | Auth | Purpose |
|---|---|---|
| `GET /api/reports/{id}` | none | The status of one report (`pending`, `processed` or `failed`), its counts, and its error. |
| `GET /api/reports?status=&limit=` | none | Recent reports, newest first. |
| `GET /api/reports/summary` | none | `{pending, failed}` |
| `POST /api/reports/{id}/retry` | `X-API-Key` | Put a failed report back in the queue with a new automatic-retry budget. `retry_count` goes back to `0`, whatever the earlier retries were. `409` for a report that has not failed. `404` when it does not exist. |
| `DELETE /api/reports/{id}` | `X-API-Key` | Dismiss a failed report by deleting it. Returns `204`. `409` for a report that has not failed. `404` when it does not exist. |

## Tests

| Endpoint | Auth | Purpose |
|---|---|---|
| `GET /api/tests?repo=&project=&include_stable=&sort=&page=&page_size=&file=&category=&q=&quarantined=&acknowledged=&has_issue=&new_within_days=&new=` | none | The paginated leaderboard. See [Leaderboard filters](#leaderboard-filters). |
| `GET /api/tests/{id}/history?limit=` | none | One test: the location and permalink, the last failing commit, executions with failure details, the jobs that its runs came from, `score_history` (90 days), `clean_streak` and `trend`. |
| `GET /api/summary?repo=&project=` | none | Dashboard tiles for tests. `executions_30d` and `failures_30d` are the sums of score history over the last 30 days. `total_runs` counts each CI run ingested. `category_counts` gives the number of flaky and suspect tests for each likely cause. |
| `POST /api/tests/{id}/quarantine` | none | Set or clear quarantine. The body is `{"quarantined": bool, "reason"?: str (max 500), "expires_in_days"?: int (1 to 365)}`. The response is the test, with `quarantine_reason` and `quarantine_expires_at`. Removing the quarantine clears the reason and the expiry. |
| `POST /api/tests/{id}/acknowledge` | none | Set or clear an acknowledgement. The body is `{"acknowledged": bool, "reason"?: str (max 500), "days"?: int (1 to 30)}`; `days` is required when `acknowledged` is `true`, and `reason` and `days` are ignored when it is `false`. The response is the test, with `acknowledged_until` and `acknowledge_reason`; `acknowledged` `false` clears both. `404` for a test that does not exist. |
| `GET /api/quarantine?repo=&project=` | `X-API-Key` | The quarantined tests of one repo and project, for the test runner. Each item has `reason` and `expires_at`. See [Quarantine](concepts.md#quarantine). |
| `PUT /api/tests/{id}/issue?replace=` | `X-API-Key` | Link a GitHub issue filed elsewhere. The body is `{"number": int, "url"?: str}`. `409` when a different issue is linked and `replace` is not `true`. See [Issues filed elsewhere](github-issues.md#issues-filed-elsewhere). |
| `DELETE /api/tests/{id}/issue` | `X-API-Key` | Remove the issue link of a test. |

Each test execution has `attempt` and `failure_category`. `attempt` is 0 for
the first try in its run.

### Leaderboard filters

These parameters of `GET /api/tests` filter and sort the leaderboard:

- `sort` is `score` (the default), `last_seen`, `proven` or `impact`. `impact`
  lists the tests that broke the most CI runs first.
- `category` filters by likely cause.
- `q` (200 characters maximum) filters by a literal substring of the name,
  classname or file. It includes stable tests.
- `quarantined=true` lists each quarantined test, including stable tests.
- `acknowledged=true` lists each acknowledged test, including stable tests.
- `has_issue` filters by linked GitHub issue.
- `new_within_days` (1 to 90) lists only new flaky tests, which are tests that
  became flaky in the last N days. It shows only flaky tests.
- `new=true` lists each test first seen within `NEW_TEST_DAYS` days. Listed tests
  carry `first_seen_at`, and report `new` when that timestamp is within the
  window. It includes stable tests. This is not `new_within_days`, which lists
  tests that became flaky recently and shows only flaky ones.

A `project` filter needs `repo`.

### Impact fields

Every answer that returns a test carries them: the leaderboard, a search hit, the
history, and the test of a quarantine, acknowledgement or issue-link response. All
three count the last 30 days only.

- `impacted_runs` is the number of distinct CI runs with a `failed` or `error`
  execution of the test in the window. Runs with no CI run id do not add to it.
- `impacted_branches` is the number of those runs' branches other than the repo's
  default branch. It is `0` while the default branch is unknown.
- `time_lost_seconds` is the total duration of those failing executions, in seconds.

An execution in a run flagged as
[suspected infrastructure](concepts.md#suspected-infrastructure) counts in none of
the three, because such a run is not the test's cost. `sort=impact` ranks by
`impacted_runs`, and then by score.

## Jobs

| Endpoint | Auth | Purpose |
|---|---|---|
| `GET /api/jobs?repo=&include_stable=&sort=&page=&page_size=&q=&pipeline=&has_issue=&acknowledged=&new_within_days=` | none | The jobs leaderboard for a repo, worst first. `q` (200 characters maximum) filters by a literal substring of the job or pipeline name, and includes stable jobs. `pipeline` filters by the exact pipeline name. `has_issue` filters by linked GitHub issue. `acknowledged=true` lists each acknowledged job, including stable jobs; `acknowledged=false` lists jobs with no live acknowledgement. `new_within_days` (1 to 90) lists only new flaky jobs, which are jobs that became flaky in the last N days. It shows only flaky jobs. `sort` is `score` (the default), `last_seen`, `proven` or `impact`, which lists the jobs that broke the most CI runs first. Every job answer also carries `impacted_runs`, `impacted_branches` and `time_lost_seconds`, defined as [for a test](#impact-fields) and counted over the last 30 days, but from the job's unexplained failed executions only: the same rule the job score uses, so a failure explained by a failing test costs the job nothing. Its `time_lost_seconds` sums `completed_at - started_at` over those executions that have both timestamps. |
| `GET /api/jobs/summary?repo=` | none | Job summary tiles, and `pipelines`: the sorted, distinct pipeline names of the scope. The scope is that repo, or all repos without `repo`. `job_executions_30d` and `job_failures_30d` are the sums of job score history over the last 30 days. |
| `GET /api/jobs/{id}/history?limit=` | none | One job: recent job executions, each marked `passed`, `failed`, `explained` or `skipped`, with the tests that explain them, `score_history` (90 days), `clean_streak` and `trend`. |
| `POST /api/jobs/{id}/acknowledge` | none | Set or clear an acknowledgement. The body is `{"acknowledged": bool, "reason"?: str (max 500), "days"?: int (1 to 30)}`; `days` is required when `acknowledged` is `true`, and `reason` and `days` are ignored when it is `false`. The response is the job, with `acknowledged_until` and `acknowledge_reason`; `acknowledged` `false` clears both. `404` for a job that does not exist. |
| `PUT /api/jobs/{id}/issue?replace=`, `DELETE /api/jobs/{id}/issue` | `X-API-Key` | The same as for a test. GitHub pipelines only. |

## CI runs

| Endpoint | Auth | Purpose |
|---|---|---|
| `GET /api/ci-runs/{ci_run_id}/failures?repo=&ci_run_attempt=` | none | The failing tests and failed jobs of one CI run attempt, with the current tier of each and the report counts of the attempt. See [Failures of one CI run](ci-integration.md#failures-of-one-ci-run). |

## Repos and admin

| Endpoint | Auth | Purpose |
|---|---|---|
| `GET /api/repos` | none | Repos with their projects. |
| `DELETE /api/repos/{owner}/{name}` | `X-API-Key` | Delete a repo and all its data. See [Delete a repo or a project](#delete-a-repo-or-a-project). |
| `DELETE /api/repos/{owner}/{name}/projects/{project}` | `X-API-Key` | Delete one project of a repo and its data. |

### Delete a repo or a project

FlakeRadar creates a repo or a project on its first upload, and removes one
only through these admin endpoints:

```bash
curl -X DELETE -H "X-API-Key: $FLAKERADAR_API_TOKEN" \
  https://flakeradar.example.com/api/repos/acme/app/projects/backend-tests
curl -X DELETE -H "X-API-Key: $FLAKERADAR_API_TOKEN" \
  https://flakeradar.example.com/api/repos/demo/shop
```

A repo delete removes these rows in one transaction:

- all its projects, tests, runs, executions and score history.
- its pipelines, jobs and job executions.
- its reports in any status, `pending` and `failed` included.

FlakeRadar normalizes the names, so `ACME/App` deletes `acme/app`. An unknown
repo gives `404`. The response is
`{repo, deleted: {tests, executions, jobs, reports}, github_issue_numbers}`.

A project delete removes the tests, runs, executions, score history and
reports of that project, in any status. The repo, its other projects, and its
pipelines and jobs stay. Thus `jobs` is always `0`, and
`github_issue_numbers` lists only the issues of the deleted tests, never the
issue of a job. An unknown project gives `404`. The response is
`{repo, project, deleted: {tests, executions, jobs, reports}, github_issue_numbers}`.

`github_issue_numbers` lists each issue that the deleted tests and jobs point
at. It is empty when the GitHub integration was never used. FlakeRadar treats
the two kinds of link differently:

- An issue that FlakeRadar filed would otherwise stay open, with permalinks
  that now give 404. When the GitHub integration is configured, FlakeRadar
  comments on each of these issues and then closes it, after the delete
  commits. This is best effort. FlakeRadar logs a GitHub failure, does not
  report it, and does not change the response.
- An issue that a person linked is never changed. FlakeRadar does not comment
  on it or close it. It only lists the issue, so that you can close it.

Without a GitHub token, FlakeRadar makes no GitHub calls. You must then close
all the issues yourself, also the issues that FlakeRadar filed.

The dashboard has no button for these deletes, and there is no MCP tool for
them. Deleting data is an admin task, and the MCP server must not delete data.

## Health and metrics

| Endpoint | Auth | Purpose |
|---|---|---|
| `GET /api/health` | none | Liveness check. |
| `GET /api/health/ready` | none | Readiness. `200` with `{database, processor_leader, pending_reports, oldest_pending_age_seconds}` when `SELECT 1` succeeds. `503` with `{"database": false}` when the database is not available. A missing report processor leader shows in the body, not as `503`. |
| `GET /metrics` | none | Prometheus gauges for the report queue, processor leadership, test and job tiers, and the last processed report. See [Monitoring](monitoring.md). |

## MCP

| Endpoint | Auth | Purpose |
|---|---|---|
| `/mcp/` | Bearer token | The MCP server. See [MCP server for agents](mcp.md). |
