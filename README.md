# FlakeRadar

![License](https://img.shields.io/badge/license-Apache--2.0-blue)
![Python](https://img.shields.io/badge/python-3.12-blue)
![Self-hosted](https://img.shields.io/badge/self--hosted-yes-green)

This project is a fork of [manijose1919/flakeradar](https://github.com/manijose1919/flakeradar).

FlakeRadar is a self-hosted service that finds flaky tests and flaky CI jobs.
It is an open-source alternative to BuildPulse and Datadog CI Visibility for
small engineering teams.

When a developer re-runs a failed CI job and it passes, the evidence of a flaky
test is usually lost. FlakeRadar keeps that evidence. It ingests JUnit XML
reports from any CI system and any test runner that writes JUnit XML (pytest,
Jest, Vitest, Go, JUnit). It records the outcome of each test in each run,
together with the commit SHA. Then it calculates a flakiness score for each
test. When a test passes and fails on the same commit, that is proof that the
test is nondeterministic.

<img width="1180" height="463" alt="FlakeRadar dashboard" src="https://github.com/user-attachments/assets/f2f14878-1ad7-4b08-9e2b-cbe6fd5441c7" />

## Contents

- [Who it is for](#who-it-is-for)
- [What it does](#what-it-does)
- [Quick start with Docker](#quick-start-with-docker)
- [Quick start for local development](#quick-start-for-local-development)
- [CI integration](#ci-integration)
- [CI jobs](#ci-jobs)
- [Repos and projects](#repos-and-projects)
- [Test locations and failure details](#test-locations-and-failure-details)
- [Retries](#retries)
- [Failure categories](#failure-categories)
- [Quarantine](#quarantine)
- [MCP server for agents](#mcp-server-for-agents)
- [GitHub issue automation](#github-issue-automation)
- [How scoring works](#how-scoring-works)
- [Score history and trends](#score-history-and-trends)
- [Configuration](#configuration)
- [API](#api)
- [Development](#development)

## Who it is for

FlakeRadar is for teams of approximately 2 to 50 developers who:

- run tests in CI (GitHub Actions, GitLab CI, Jenkins, or any CI that can
  write JUnit XML) and no longer trust a red build.
- re-run failed jobs by habit, without knowing which tests are unreliable.
- cannot justify the cost of a paid CI analytics platform. They also do not
  want a real regression to hide behind a test that "is always flaky".

A solo developer with a 30-second test suite does not need FlakeRadar.

## What it does

- **CI integration in one step.** After each test run, including failed runs,
  send `junit.xml` to `/api/ingest` with `curl`. The server stores the report
  and answers `202` before it processes the report.
- **A score that separates flaky tests from broken tests.** A test that fails
  every time scores 0, because it is broken. A test that changes between pass
  and fail scores high. Recent changes have more weight. A small number of
  executions gives a lower score. A pass and a fail on the same commit set the
  score to 0.6 or higher.
- **Repos and projects.** One instance keeps data for many repositories. Each
  repository can have several test suites, for example `frontend`, `backend`
  and `e2e`.
- **Evidence for each flaky test.** FlakeRadar shows the file and line of the
  test when your runner reports them. It also shows a GitHub permalink at the
  last failing commit. FlakeRadar keeps the full traceback and captured output
  of each failure.
- **A likely cause for each failure.** Fixed rules put each failure into one
  category: network, environment, timing, assertion or other. You can then fix
  one type of flaky test at a time.
- **Trends.** FlakeRadar keeps a daily score history for each test and job.
  The leaderboard shows if a test becomes worse or better. A clean streak
  shows if a fix continues to work.
- **Flaky CI jobs.** FlakeRadar also scores CI jobs. A job failure that a
  failing test explains does not count against the job.
- **Dashboard.** The dashboard has a repo and project picker and a slide-over
  detail panel. Its paginated leaderboard shows flaky and suspect tests and
  jobs. Each view has a URL that you can share.
- **MCP server.** AI agents can get the worst flaky tests in a repo, with the
  data they need to find and fix each test.
- **Quarantine.** Mark a test as quarantined in the dashboard. Your test runner
  then gets the list of tests to skip from the API.
- **GitHub issues (optional).** When a test or job crosses the flakiness
  threshold, FlakeRadar opens an issue in the repository of that test or job.

## Quick start with Docker

```bash
cp .env.example .env   # set FLAKERADAR_API_TOKEN (and POSTGRES_PASSWORD)
docker compose up --build
# The app, API and MCP server are on http://localhost:8000.
# Postgres data is in a named volume.
```

Compose runs two services:

- `db`: PostgreSQL 17.
- `flakeradar`: the API, the report processor and the built dashboard, in two
  uvicorn workers.

Migrations run automatically when the app starts.

To load demo data, run:

```bash
python samples/simulate_ci.py http://localhost:8000 "$FLAKERADAR_API_TOKEN"
```

## Quick start for local development

FlakeRadar needs PostgreSQL. To start a local PostgreSQL in a container:

```bash
docker run -d --name flakeradar-pg -p 5432:5432 \
  -e POSTGRES_USER=flakeradar -e POSTGRES_PASSWORD=flakeradar -e POSTGRES_DB=flakeradar \
  postgres:17-alpine

# Backend (Python 3.12)
cd backend
python3.12 -m venv .venv
.venv/bin/pip install -r requirements.txt
FLAKERADAR_ALLOW_INSECURE=1 .venv/bin/python -m uvicorn app.main:app --port 8000
# The default FLAKERADAR_DATABASE_URL points at the container above.

# Frontend (dev server with hot reload; it sends /api requests to :8000)
cd frontend
npm install
npm run dev            # http://localhost:5173
```

## CI integration

Add one step after your tests. The same step, with comments, is in
`samples/github-actions-snippet.yml`.

```yaml
- name: Report to FlakeRadar
  if: always()   # always report: the failed runs are the evidence
  run: |
    PIPELINE="${GITHUB_WORKFLOW_REF#${GITHUB_REPOSITORY}/}"; PIPELINE="${PIPELINE%@*}"
    QUERY=$(jq -rn --arg repo "${{ github.repository }}" --arg project "backend" \
      --arg root "backend" --arg sha "${{ github.sha }}" --arg branch "${{ github.ref_name }}" \
      --arg run "${{ github.run_id }}" --arg attempt "${{ github.run_attempt }}" \
      --arg job "${{ job.check_run_id }}" --arg pipeline "$PIPELINE" \
      --arg default_branch "${{ github.event.repository.default_branch }}" \
      '"repo="+($repo|@uri)+"&project="+($project|@uri)+"&root="+($root|@uri)+"&commit_sha="+($sha|@uri)+"&branch="+($branch|@uri)+"&ci_run_id="+($run|@uri)+"&ci_run_attempt="+($attempt|@uri)+"&ci_job_id="+($job|@uri)+"&pipeline="+($pipeline|@uri)+"&default_branch="+($default_branch|@uri)')
    curl --fail-with-body -sS --retry 5 --retry-all-errors --retry-delay 2 \
      -X POST "${{ secrets.FLAKERADAR_URL }}/api/ingest?${QUERY}" \
      -H "X-API-Key: ${{ secrets.FLAKERADAR_TOKEN }}" \
      -H "Content-Type: application/xml" \
      --data-binary @junit.xml
```

| Parameter | Required | Meaning |
|---|---|---|
| `repo` | yes | `owner/name`, converted to lowercase. On GitHub Actions, use `${{ github.repository }}`. |
| `project` | no (`default`) | The test suite in the repo, for example `frontend`, `backend` or `e2e`. |
| `root` | no | The directory of the project in the repo, for example `frontend`. FlakeRadar uses it to make file paths and permalinks. |
| `commit_sha` | yes | The commit that the tests ran on. |
| `branch` | no (`main`) | The branch that the tests ran on. |
| `ci_run_id` | no | The workflow run ID (`${{ github.run_id }}`). |
| `ci_run_attempt` | no | The run attempt number (`${{ github.run_attempt }}`). A re-run is a new attempt on the same SHA. |
| `ci_job_id` | no | The ID of this job (`${{ job.check_run_id }}`). It links the run to a job execution, so that failing tests can explain a failed job. |
| `pipeline` | no | The workflow file path, for example `.github/workflows/ci.yml`. The snippet gets it from `GITHUB_WORKFLOW_REF`. |
| `default_branch` | no | The default branch of the repo. Flips on other branches do not count. |

The server answers `202 {"report_id": 7, "status": "pending"}` when it has
stored the report. `GET /api/reports/7` shows when processing is complete. The
`--retry` flags help when the server is not available for a short time, for
example during a redeploy.

## CI jobs

FlakeRadar also scores CI jobs for flakiness, with the same rules as tests. A
job does not need to write JUnit XML.

To set this up on GitHub:

1. Copy `samples/flakeradar-jobs.yml` to `.github/workflows/flakeradar-jobs.yml`
   on your default branch. The `workflow_run` trigger works only from the
   default branch.
2. In that file, list the workflows to track by their exact `name:`. Globs do
   not work.
3. Add the `FLAKERADAR_URL` and `FLAKERADAR_TOKEN` secrets. The workflow needs
   the `actions: read` permission.

After each attempt of a listed workflow, this workflow gets the job results
for that attempt and sends them to `POST /api/ingest/pipeline`. FlakeRadar
never calls GitHub for job results. Your workflow sends the results to
FlakeRadar. Each job execution needs a branch. Thus the workflow skips an
attempt that has no branch (for example, a run that a tag push started) and
writes a notice.

The JUnit snippet above sends `ci_job_id`, `ci_run_attempt` and `pipeline`.
These values link each test run to the job execution that produced it.

### Terms

- A **pipeline** is a named CI workflow in a repo. On GitHub, its workflow file
  path identifies it, for example `.github/workflows/ci.yml`. A pipeline
  contains jobs. FlakeRadar does not score pipelines.
- A **job** is one job in a pipeline. Its display name identifies it. Each
  matrix leg is a separate job, for example `test (ubuntu-latest, 3.12)`. A job
  belongs to a repo through its pipeline. It does not belong to a project.
- A **job execution** is the outcome of one job (`passed`, `failed` or
  `skipped`) in one attempt on one commit. When a job fails and a re-run on the
  same SHA passes, the job is a **proven flake**. This rule is the same as for
  tests.

### Explained and unexplained job failures

A failed test often causes a failed job. If FlakeRadar counted that failure
against the job too, it would count one flaky test two times. Thus a failed
job execution is **explained** when a JUnit report has the same `ci_job_id`
and the same repo and contains a failed or errored test. FlakeRadar scores an
explained failure as `skipped`. Only **unexplained** failures change the score
of a job. Examples are setup failures, network failures, runner failures, and
crashes before the tests start.

For this to work, each JUnit report must send `ci_job_id`. The snippet sends
`ci_job_id=${{ job.check_run_id }}` for this reason. Without it, all failures
of that job are unexplained. The job can then look flaky when a test caused
the failure.

### Other CI providers

Any CI can send the same JSON to `POST /api/ingest/pipeline`:

```json
{
  "repo": "acme/app", "provider": "gitlab", "pipeline": ".gitlab-ci.yml",
  "commit_sha": "deadbeef", "branch": "main", "default_branch": "main",
  "ci_run_id": "123", "ci_run_attempt": 1,
  "jobs": [
    {"ci_job_id": "456", "name": "test", "status": "failed",
     "url": "https://gitlab.com/acme/app/-/jobs/456",
     "runner_name": "runner-1", "runner_labels": ["linux"]}
  ]
}
```

Your reporter must convert each job status to `passed`, `failed` or `skipped`.
FlakeRadar does not interpret provider-specific job results. For GitLab, you
can map `CI_JOB_ID`, `CI_JOB_NAME`, `CI_PIPELINE_ID`, `CI_DEFAULT_BRANCH` and
`CI_COMMIT_SHA` to the fields above. This project has no GitLab example, and
this mapping is not tested.

## Repos and projects

A **repo** is a repository (`owner/name`). A **project** is a test suite in a
repo. For example, `andrewthetechie/writers-app` can have the projects
`frontend`, `backend` and `e2e`. Another repo can have projects with the same
names. FlakeRadar keeps them separate. A repo with one suite can omit
`project`, and FlakeRadar then uses `default`. FlakeRadar creates a repo or
project when it gets the first upload for it.

In the dashboard, select a repo. Then you can select one of its projects. In
the "all repos" view, each row shows `repo · project`.

## Test locations and failure details

When a JUnit report includes the file and line of a test, FlakeRadar stores
them. It links to the code on GitHub at the last failing commit. Most runners
need a setting to write the file and line:

| Runner | Setting |
|---|---|
| pytest | `pytest --junitxml=junit.xml -o junit_family=xunit1` (line numbers start at 0) |
| Vitest | `reporters: [["junit", { addFileAttribute: true }]]` |
| Jest (jest-junit) | `JEST_JUNIT_ADD_FILE_ATTRIBUTE=true` |

Runners that do not write a file attribute (Go, cargo-nextest) also work.
FlakeRadar identifies the test by its classname and name. It keeps the full
traceback of each failure, and the traceback usually contains `file:line`.

## Retries

When a runner retries a test in one run, FlakeRadar stores each attempt. A
test that fails and then passes on retry has a fail and a pass on the same
commit. Thus it is a **proven flake** from its first run.

| Runner | Setting |
|---|---|
| Playwright 1.59 or later | `reporter: [["junit", { outputFile: "junit.xml", includeRetries: true }]]`, or `PLAYWRIGHT_JUNIT_INCLUDE_RETRIES=1` |
| Maven Surefire / Failsafe | `-Dsurefire.rerunFailingTestsCount=2` (retries are written as `<flakyFailure>` or `<rerunFailure>`) |

Runners that write each attempt as a separate `<testcase>` with the same name
also work. Without these settings, a retried test that passes at the end looks
like a clean pass.

Each failed retry counts as one failure. This includes the count for
`FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES`. With 2 retries, one run can add 3
failures.

## Failure categories

Each failing test execution gets a **likely cause**: `network`, `environment`,
`timing`, `assertion` or `other`. Fixed rules set the category. FlakeRadar
tries the rules in this order, from most to least specific:

1. network
2. environment
3. timing
4. assertion

A failure that matches no rule is `other`. FlakeRadar checks the failure
message first. It checks the failure details only when the message matches no
rule.

For example, `Timed out 5000ms waiting for expect(...)` is `timing`, and
`connect ECONNREFUSED` is `network`. The category of a test is the most common
category in its last `SCORE_WINDOW` executions. The category is a likely
cause, not a diagnosis. Read the failure details before you decide on the
cause.

Failures that FlakeRadar stored before categories existed have no category
(`null`).

## Quarantine

In the dashboard, mark an unreliable test as quarantined. Before a run, your
test runner gets the quarantine list for its repo and project:

```
curl -sS "$FLAKERADAR_URL/api/quarantine?repo=$GITHUB_REPOSITORY&project=backend" \
  -H "X-API-Key: $FLAKERADAR_TOKEN"
# -> [{"suite","classname","name","fingerprint","file","line","quarantined_at"}, ...]
```

The runner matches each item on `classname` and `name` to find the tests to
skip. Only a person can quarantine a test, and a person can undo it.
FlakeRadar never skips a test by itself, and the MCP server cannot quarantine
a test.

## MCP server for agents

FlakeRadar has a read-only [MCP](https://modelcontextprotocol.io) server. It
uses the streamable HTTP transport. The endpoint accepts JSON-RPC `POST`
requests. It is not a web page.

### Endpoint

```
https://<your-host>/mcp/
```

Include the trailing slash. The MCP app is mounted at `/mcp`, and `/mcp/` is
the endpoint. A request to `/mcp` without the slash does not get to the MCP
server.

### Authentication

Each request must send the API token as a bearer token:

```
Authorization: Bearer <FLAKERADAR_API_TOKEN>
```

Without a valid token, the server answers `401 Unauthorized` with
`WWW-Authenticate: Bearer`. `FLAKERADAR_API_TOKEN` is the token in `.env`. CI
systems send the same token in the `X-API-Key` header.

### Connect an MCP client

Give your client the `/mcp/` URL, the streamable HTTP transport and the bearer
token. For example, with the Claude Code CLI:

```bash
claude mcp add --transport http flakeradar https://flakeradar.example.com/mcp/ \
  --header "Authorization: Bearer $FLAKERADAR_API_TOKEN"
```

Other clients (Cursor, VS Code, custom agents) need the same three values:

- URL: `https://<your-host>/mcp/`
- Transport: streamable HTTP
- Credential: the bearer token

In production, always use HTTPS, so that the token is not sent as clear text.

### Tools

| Tool | Returns |
|---|---|
| `list_repos` | All repos, with their projects. |
| `list_projects(repo)` | The projects of one repo. |
| `top_flaky_tests(repo, project?, limit=20, include_suspect=true, file?, category?)` | The worst tests first. `category` (network, environment, timing, assertion or other) filters by likely cause. |
| `search_tests(repo, query, project?, limit=20)` | Tests whose name, classname or file contains `query`. |
| `get_test(test_id \| repo+project+name[+classname], executions_limit=20)` | The location and GitHub permalink, the last failing commit, the latest failure message and traceback (4 KB maximum), recent executions, and up to 30 days of score history. |
| `top_flaky_jobs(repo, limit=20, include_suspect=true)` | The worst CI jobs first. Scores count only unexplained failures. |
| `search_jobs(repo, query, limit=20)` | Jobs whose name or pipeline contains `query`. |
| `get_job(job_id \| repo+name[+pipeline], executions_limit=20)` | Unexplained and explained failures, recent executions with the tests that explain them, and up to 30 days of score history. |

## GitHub issue automation

Set these values in `.env`:

```
FLAKERADAR_GITHUB_TOKEN=<fine-grained PAT with Issues:write on your repos>
FLAKERADAR_GITHUB_ISSUE_LABEL=flakeradar
```

When a test or job passes the filing gate, FlakeRadar opens an issue in the
repo of that test or job. For a test, the issue contains:

- the repo, project, file and line.
- a permalink to the test at its last failing commit.
- the score and the threshold that applied.
- the proven-flake count.
- the suite and classname.
- the last failing commit and branch.
- the last 10 executions.
- a sample failure traceback.

FlakeRadar opens one issue for each test or job, with the
`FLAKERADAR_GITHUB_ISSUE_LABEL` label. It opens issues after it processes a
report, never while it accepts an upload. When GitHub returns a rate-limit
error, FlakeRadar stops that batch. To disable this feature, leave the token
empty.

### Filing gate

`FLAKE_THRESHOLD` sets the "flaky" tier in the UI and API. It does not control
issues. Three independent minimums control issues. FlakeRadar opens an issue
only when a test meets each minimum that is not zero:

```
FLAKERADAR_GITHUB_ISSUE_MIN_SCORE=0.30          # minimum flakiness score (0..1)
FLAKERADAR_GITHUB_ISSUE_MIN_PROVEN_FLAKES=0     # minimum same-commit fail+pass count
FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES=0          # minimum failures in the recent window (each failed retry counts)
```

Set a minimum to `0` to ignore that signal. You can use score, proven flakes,
failures, or any combination. With the defaults, only the score counts (score
0.30 or higher). If you get false positives, increase these minimums. Do not
change the tier threshold for this. If all three minimums are `0` and a token
is set, FlakeRadar opens an issue for each test that it processes. Keep
`MIN_SCORE` above 0.

For a job, the failure count includes only unexplained failures.

### Duplicate issues

FlakeRadar stores the number of the issue on the test or job. While the issue
is open, FlakeRadar opens no new issue for that test or job. At each
maintenance interval, FlakeRadar asks GitHub for the state of each stored
issue. When an issue is closed or deleted, FlakeRadar clears the stored
number. A later report that makes the test flaky again then opens a new
issue.

The UI shows a link to the issue on the leaderboard row and in the detail
panel. The API and MCP server also return it, as `github_issue_number` and
`github_issue_url`, from `/api/tests`, `/api/tests/{id}/history`, and the
`get_test` and `top_flaky_tests` tools.

## How scoring works

For each test, FlakeRadar uses the last 50 executions. You can change this
number.

1. **Flip score.** FlakeRadar calculates the rate of changes between pass and
   fail in consecutive executions. Recent changes have more weight. A test
   that alternates every time scores 1.0. A test that always fails or always
   passes scores 0.
2. **Sample damping.** One flip in two runs is a 100% flip rate, but it is weak
   evidence. FlakeRadar reduces the score until it has 7 executions.
3. **Same-SHA floor.** A commit with both a pass and a fail is a **proven
   flake**. The score is then 0.6 or higher. Each additional proven flake adds
   0.1, to a maximum of 1.0.

FlakeRadar ignores `skipped` executions. It counts `error` as a failure. Tests
are in three tiers:

- **flaky**: the score is at or above the threshold.
- **suspect**: the score is above 0 and below the threshold.
- **stable**: the score is 0.

The leaderboard does not show stable tests unless you ask for them. The same
rules and tiers apply to CI jobs.

### The default-branch rule

This rule applies to tests and jobs. Flips count only on the default branch
of the repo. A test or job that fails on a PR branch and then gets a fix must
not look flaky. Thus the flip score uses only the newest executions on the
default branch. Proven flakes count on all branches, because a pass and a
fail on the same commit show nondeterminism on the same code. Until a report
gives the default branch, all branches count.

## Score history and trends

FlakeRadar keeps a daily score history. Each test and each job gets one row
for each UTC day. The row contains the flakiness score and proven-flake count
at the end of the day, and the number of executions and failures on that day.

Each test and job also has:

- a **trend** (`worsening`, `improving` or `steady`). FlakeRadar compares the
  current score with the newest history row that is at least 14 days old.
- a **clean streak**. This is the number of non-skipped executions on the
  default branch since the last failure. An explained job failure does not
  stop the clean streak of a job.

The detail panel shows the score for the last 90 days as a sparkline.
FlakeRadar deletes history after 365 days (`SCORE_HISTORY_RETENTION_DAYS`).

## Configuration

FlakeRadar reads environment variables with the prefix `FLAKERADAR_`. It also
reads a `.env` file.

| Variable | Default | Meaning |
|---|---|---|
| `API_TOKEN` | `changeme` (the app does not start with it unless `FLAKERADAR_ALLOW_INSECURE=1`) | The token for CI uploads (`X-API-Key`) and MCP (`Authorization: Bearer`). |
| `DATABASE_URL` | `postgresql+asyncpg://flakeradar:flakeradar@localhost:5432/flakeradar` | PostgreSQL only. `postgresql://` URLs also work. |
| `FLAKE_THRESHOLD` | `0.30` | The score at which a test is flaky. |
| `SCORE_WINDOW` / `SCORE_DECAY` | `50` / `0.85` | The number of executions to score, and the weight for recent executions. |
| `GITHUB_TOKEN` | empty (disabled) | The token for GitHub issue automation. |
| `GITHUB_ISSUE_LABEL` | `flakeradar` | The label on each issue that FlakeRadar opens. |
| `GITHUB_ISSUE_MIN_SCORE` / `GITHUB_ISSUE_MIN_PROVEN_FLAKES` / `GITHUB_ISSUE_MIN_FAILURES` | `0.30` / `0` / `0` | The filing gate. |
| `WORKER_POLL_SECONDS` | `1.0` | How often the report processor checks an empty queue. |
| `REPORT_RETENTION_DAYS` | `7` | FlakeRadar deletes processed raw reports after this time. It keeps failed reports. |
| `EXECUTION_RETENTION_DAYS` | `90` | FlakeRadar deletes older executions. |
| `SCORE_HISTORY_RETENTION_DAYS` | `365` | FlakeRadar deletes older daily score history rows. |
| `PRUNE_INTERVAL_SECONDS` | `3600` | How often retention runs. |
| `CORS_ORIGINS` | `http://localhost:5173` | The origin of the dev server. |

## API

The read APIs and the quarantine toggle have no authentication. Put the
dashboard on a private network. The CI endpoints and the MCP server need the
token.

| Endpoint | Auth | Purpose |
|---|---|---|
| `POST /api/ingest?repo=&project=&root=&commit_sha=&branch=&ci_run_id=&ci_run_attempt=&ci_job_id=&pipeline=&default_branch=` | `X-API-Key` | Queue a JUnit XML report (raw body or multipart `report` field, 20 MB maximum). Returns `202`. |
| `POST /api/ingest/pipeline` | `X-API-Key` | Queue a pipeline report (JSON) with `jobs`. Returns `202`. |
| `GET /api/reports/{id}` | none | The status of one report (`pending`, `processed` or `failed`), its counts, and its error. |
| `GET /api/reports?status=&limit=` | none | Recent reports, newest first. |
| `GET /api/reports/summary` | none | `{pending, failed}` |
| `POST /api/reports/{id}/retry` | `X-API-Key` | Put a failed report back in the queue. |
| `GET /api/repos` | none | Repos with their projects. |
| `GET /api/tests?repo=&project=&include_stable=&sort=&page=&page_size=&file=&category=` | none | The paginated leaderboard. `sort` is `score`, `last_seen` or `proven`. `category` filters by likely cause. |
| `GET /api/tests/{id}/history?limit=` | none | One test: location and permalink, last failing commit, executions with failure details, the jobs that its runs came from, `score_history` (90 days), `clean_streak` and `trend`. |
| `GET /api/jobs?repo=&include_stable=&sort=&page=&page_size=` | none | The jobs leaderboard for a repo, worst first. |
| `GET /api/jobs/summary?repo=` | none | Job summary tiles. |
| `GET /api/jobs/{id}/history?limit=` | none | One job: recent job executions, each marked `passed`, `failed`, `explained` or `skipped`, with the tests that explain them, `score_history` (90 days), `clean_streak` and `trend`. |
| `GET /api/summary?repo=&project=` | none | Dashboard tiles for tests. `category_counts` gives the number of flaky and suspect tests for each likely cause. |
| `POST /api/tests/{id}/quarantine` | none | Set or clear quarantine (body `{"quarantined": bool}`). |
| `GET /api/quarantine?repo=&project=` | `X-API-Key` | Quarantined tests for one repo and project, for the test runner. |
| `/mcp/` | Bearer token | The MCP server. |
| `GET /api/health` | none | Liveness check. |

A `project` filter needs `repo`. The interactive OpenAPI docs are at `/docs`.
Each test execution has `attempt` (0 is the first try in its run) and
`failure_category`.

## Development

```bash
cd backend
.venv/bin/python -m pytest -q    # needs Docker (it starts a temporary Postgres)
# or use an existing database:
# FLAKERADAR_TEST_DATABASE_URL=postgresql+asyncpg://... .venv/bin/python -m pytest -q
cd ../frontend
npm test                         # Vitest
npm run build                    # strict TypeScript check
```
