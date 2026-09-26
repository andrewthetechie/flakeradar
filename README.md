# FlakeRadar

![License](https://img.shields.io/badge/license-Apache--2.0-blue)
![Python](https://img.shields.io/badge/python-3.12-blue)
![Self-hosted](https://img.shields.io/badge/self--hosted-yes-green)

**Self-hosted flaky-test detection for small engineering teams — an open-source
alternative to BuildPulse and Datadog CI Visibility.**

Every time a developer clicks "re-run job" on a red CI build, evidence of a flaky
test evaporates. FlakeRadar captures that evidence: it ingests JUnit XML reports
from any CI system (pytest, Jest, Vitest, Go, JUnit — anything that emits JUnit
XML), tracks every test's outcome across runs *keyed by commit SHA*, and scores
flakiness statistically. A test that fails and then passes on the same commit is
**proven** nondeterministic — no heuristics required.

<img width="1180" height="463" alt="image" src="https://github.com/user-attachments/assets/f2f14878-1ad7-4b08-9e2b-cbe6fd5441c7" />


## Contents

- [Who it's for](#who-its-for)
- [What it does](#what-it-does)
- [Quick start (Docker)](#quick-start-docker)
- [Quick start (local development)](#quick-start-local-development)
- [CI integration](#ci-integration)
- [Repos and projects](#repos-and-projects)
- [Test locations and failure details](#test-locations-and-failure-details)
- [Quarantine workflow](#quarantine-workflow)
- [MCP server for agents](#mcp-server-for-agents)
- [GitHub issue automation](#github-issue-automation)
- [How scoring works](#how-scoring-works)
- [Configuration](#configuration)
- [API](#api)
- [Architecture](#architecture)
- [Development](#development)
- [Roadmap](#roadmap)

## Who it's for

Small and mid-size engineering teams (roughly 2–50 developers) who:

- run tests in CI (GitHub Actions, GitLab CI, Jenkins — anything that can emit
  JUnit XML) and have started to distrust red builds;
- catch themselves clicking **"re-run job"** as a reflex, without knowing which
  tests are actually unreliable;
- can't justify paid CI-analytics platforms (BuildPulse, Datadog CI Visibility)
  for a problem this size, but also can't afford the day a real regression hides
  behind "oh, that test is always flaky."

If you're a solo developer with a 30-second test suite, you don't need this yet.
If you're Google, you already built it in-house. Everyone in between: this is
the missing middle.

## What it does

- **One-line CI integration** — `curl` your `junit.xml` to `/api/ingest` after
  every test run (including failed ones). Uploads are queued and answered with
  `202` immediately, so a busy server never loses a run.
- **Flakiness scoring that knows the difference between flaky and broken** —
  a test failing 100% of the time scores 0 (it's broken); a test that *flips*
  between pass and fail scores high. Recent flips weigh more, small samples are
  damped, and a same-commit fail→pass flip floors the score at 0.6.
- **Repos and projects** — one instance serves many repositories, and each
  repository can hold several test suites (`frontend`, `backend`, `e2e`).
- **Evidence you can act on** — the test's file and line (when your runner
  reports them), a GitHub permalink at the last failing commit, and the full
  traceback plus captured output of every failure.
- **Dashboard** — repo and project pickers, a paginated leaderboard of flaky and
  suspect tests, a slide-over detail panel, and shareable URLs for every view.
- **MCP server** — AI agents can ask for the top flaky tests in a repo and get
  everything needed to find and fix one.
- **Quarantine workflow** — mark a test quarantined in the dashboard; your test
  runner asks the API which tests to skip.
- **GitHub issue automation (optional)** — when a test crosses the flakiness
  threshold, FlakeRadar files an issue in that test's own repository.

## Quick start (Docker)

```bash
cp .env.example .env   # set FLAKERADAR_API_TOKEN (and POSTGRES_PASSWORD)
docker compose up --build
# App, API and MCP on http://localhost:8000; Postgres data in a named volume
```

Compose runs two services: `db` (PostgreSQL 17) and `flakeradar` (the API, the
Report processor and the built dashboard, two uvicorn workers). Migrations run
automatically on startup.

Seed it with demo data:

```bash
python samples/simulate_ci.py http://localhost:8000 "$FLAKERADAR_API_TOKEN"
```

## Quick start (local development)

FlakeRadar needs PostgreSQL. The quickest local one:

```bash
docker run -d --name flakeradar-pg -p 5432:5432 \
  -e POSTGRES_USER=flakeradar -e POSTGRES_PASSWORD=flakeradar -e POSTGRES_DB=flakeradar \
  postgres:17-alpine

# Backend (Python 3.12)
cd backend
python3.12 -m venv .venv
.venv/bin/pip install -r requirements.txt
FLAKERADAR_ALLOW_INSECURE=1 .venv/bin/python -m uvicorn app.main:app --port 8000
# (the default FLAKERADAR_DATABASE_URL points at the container above)

# Frontend (dev mode with hot reload, proxies /api to :8000)
cd frontend
npm install
npm run dev            # http://localhost:5173
```

## CI integration

Add one step after your tests (see `samples/github-actions-snippet.yml`):

```yaml
- name: Report to FlakeRadar
  if: always()   # crucial — failed runs are the signal
  run: |
    PIPELINE="${GITHUB_WORKFLOW_REF#${GITHUB_REPOSITORY}/}"; PIPELINE="${PIPELINE%@*}"
    QUERY=$(jq -rn --arg repo "${{ github.repository }}" --arg project "backend" \
      --arg root "backend" --arg sha "${{ github.sha }}" --arg branch "${{ github.ref_name }}" \
      --arg run "${{ github.run_id }}" --arg attempt "${{ github.run_attempt }}" \
      --arg job "${{ job.check_run_id }}" --arg pipeline "$PIPELINE" \
      --arg default_branch "${{ github.event.repository.default_branch }}" \
      '"repo="+($repo|@uri)+"&project="+($project|@uri)+"&root="+($root|@uri)+"&commit_sha="+($sha|@uri)+"&branch="+($branch|@uri)+"&ci_run_id="+($run|@uri)+"&ci_run_attempt="+($attempt|@uri)+"&ci_job_id="+($job|@uri)+"&pipeline="+($pipeline|@uri)+"&default_branch="+($default_branch|@uri)')
    curl --fail-with-body -sS --retry 5 --retry-all-errors --retry-delay 2 \
      -X POST "$FLAKERADAR_URL/api/ingest?${QUERY}" \
      -H "X-API-Key: ${{ secrets.FLAKERADAR_TOKEN }}" \
      -H "Content-Type: application/xml" \
      --data-binary @junit.xml
```

| Parameter | Required | Meaning |
|---|---|---|
| `repo` | yes | `owner/name` — lowercased; `${{ github.repository }}` on GitHub Actions |
| `project` | no (`default`) | the test suite inside the repo, e.g. `frontend`, `backend`, `e2e` |
| `root` | no | the project's directory in the repo (e.g. `frontend`), used to build file paths and permalinks |
| `commit_sha` | yes | the commit under test |
| `branch` | no (`main`) | |
| `ci_run_id` | no | the workflow run id (`${{ github.run_id }}`) |
| `ci_run_attempt` | no | the run attempt number (`${{ github.run_attempt }}`); a re-run is a new attempt on the same SHA |
| `ci_job_id` | no | this Job's id (`${{ job.check_run_id }}`); links the Run to a Job execution so a failed Job can be explained by failing Tests |
| `pipeline` | no | the workflow file path, e.g. `.github/workflows/ci.yml` (derived from `GITHUB_WORKFLOW_REF`) |
| `default_branch` | no | the repo's default branch, so flips elsewhere don't count |

The server answers `202 {"report_id": 7, "status": "pending"}` as soon as the
report is stored; `GET /api/reports/7` shows when it has been processed. The
`--retry` flags cover the server being briefly unreachable (e.g. a redeploy).

## CI jobs

FlakeRadar also scores **CI jobs** for flakiness with the same rules as tests,
even when they produce no JUnit. On GitHub this is a one-file setup · copy
`samples/flakeradar-jobs.yml` to `.github/workflows/flakeradar-jobs.yml` on
your **default branch** (the `workflow_run` trigger only fires from there) and
list the workflows you want tracked by their exact `name:` (no globs). After
every attempt of those workflows it fetches that attempt's job results and
POSTs them to `POST /api/ingest/pipeline`. It needs `FLAKERADAR_URL` and
`FLAKERADAR_TOKEN` secrets, and `actions: read` permission. FlakeRadar never
calls GitHub — your workflow pushes results to it. An attempt with no branch
(for example, a workflow run started by a tag push) is skipped with a notice,
because every Job execution needs a branch.

The JUnit snippet above sends `ci_job_id`, `ci_run_attempt` and `pipeline` so
that each test Run is linked to the Job execution that produced it.

### Vocabulary

- A **Pipeline** is a named CI workflow in a Repo, identified on GitHub by its
  workflow file path (e.g. `.github/workflows/ci.yml`). It groups Jobs and is
  not scored.
- A **Job** is one job within a Pipeline, identified by its display name. Each
  matrix leg is its own Job, e.g. `test (ubuntu-latest, 3.12)`. It belongs to a
  Repo through its Pipeline, not to a Project.
- A **Job execution** is one Job's outcome (`passed`, `failed`, `skipped`) in
  one attempt at one commit. A failed attempt followed by a passing re-run on
  the same SHA is a **Proven flake**, exactly as for Tests.

### Explained vs unexplained failures

A failed **Test** frequently explains a failed **Job** — the test broke, so the
job failed. Counting those against the Job would just double-count a flaky
test. So a failed Job execution is **explained** when a JUnit report with the
same `ci_job_id` (same Repo) contains a failing or errored Test. Explained
failures are scored as `skipped`; only **unexplained** failures (setup, network,
runners, a crash before the tests run) move a Job's score.

To make attribution work, **every JUnit report must send `ci_job_id`** — this
is why the snippet sends `ci_job_id=${{ job.check_run_id }}`. Without it, that
Job's failures are all unexplained and can look flaky even when a test broke.

### Other providers

Any CI can POST the same JSON contract to `POST /api/ingest/pipeline`:

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

The `status` is already normalized to `passed | failed | skipped` by your
reporter; FlakeRadar decides nothing about provider conclusions. As a *sketch*
for GitLab, map `CI_JOB_ID`, `CI_JOB_NAME`, `CI_PIPELINE_ID`, `CI_DEFAULT_BRANCH`
and `CI_COMMIT_SHA` onto the fields above (a GitLab example is out of scope and
not built or tested).

## Repos and projects

A **repo** is a repository (`owner/name`); a **project** is a test suite inside
it. `andrewthetechie/writers-app` might have `frontend`, `backend` and `e2e`
projects, and another repo can have projects with the same names — they never
merge. A repo with a single suite can omit `project` (it becomes `default`).
Repos and projects are created on their first upload.

In the dashboard, pick a repo, then (optionally) one of its projects. In the
"all repos" view every row is labeled `repo · project`.

## Test locations and failure details

FlakeRadar stores each test's file and line when the JUnit report includes
them, and links to the code on GitHub at the last failing commit. Most runners
need a flag to emit them:

| Runner | Setting |
|---|---|
| pytest | `pytest --junitxml=junit.xml -o junit_family=xunit1` (reports 0-based line numbers) |
| Vitest | `reporters: [["junit", { addFileAttribute: true }]]` |
| Jest (jest-junit) | `JEST_JUNIT_ADD_FILE_ATTRIBUTE=true` |

Runners without a file attribute (Go, cargo-nextest) still work — the test is
identified by its classname and name, and the full failure traceback (which
usually contains `file:line`) is kept for every failure.

## Quarantine workflow

Mark an unreliable test **Quarantine** in the dashboard. Your test runner then
queries the quarantine list for its own repo and project before a run:

```
curl -sS "$FLAKERADAR_URL/api/quarantine?repo=$GITHUB_REPOSITORY&project=backend" \
  -H "X-API-Key: $FLAKERADAR_TOKEN"
# -> [{"suite","classname","name","fingerprint","file","line","quarantined_at"}, ...]
```

A runner matches each item on `classname` + `name` to decide what to skip.
Quarantining is a deliberate, reversible human action — FlakeRadar never
auto-skips a test on its own, and the MCP server cannot quarantine.

## MCP server for agents

FlakeRadar serves a read-only [MCP](https://modelcontextprotocol.io) server. It
uses the **streamable HTTP** transport: the endpoint answers JSON-RPC **POST**
requests — it is not a web page, so opening it in a browser returns **404**.

### Endpoint

```
https://<your-host>/mcp/
```

The trailing slash matters: the app is mounted at `/mcp`, so `/mcp/` is the
canonical endpoint. A request to `/mcp` (no slash) is *not* served and 404s.

### Authentication (required)

The MCP server is **token-gated**. Every request must send the API token as a
bearer credential:

```
Authorization: Bearer <FLAKERADAR_API_TOKEN>
```

Without a valid token the server responds **401 Unauthorized** with
`WWW-Authenticate: Bearer`. `FLAKERADAR_API_TOKEN` is the same token set in
`.env` (and the one CI systems send in the `X-API-Key` header).

### Connect an MCP client

Point your client at `/mcp/` with streamable HTTP and the bearer token. For
example, with the Claude Code CLI:

```bash
claude mcp add --transport http flakeradar https://flakeradar.example.com/mcp/ \
  --header "Authorization: Bearer $FLAKERADAR_API_TOKEN"
```

Other clients (Cursor, VS Code, custom agents) accept the same three values:
URL `https://<your-host>/mcp/`, transport **streamable HTTP**, and the bearer
token as the credential. Always HTTPS in production so the token is not sent
in the clear.

### Tools

| Tool | Returns |
|---|---|
| `list_repos` | every repo with its projects |
| `list_projects(repo)` | one repo's projects |
| `top_flaky_tests(repo, project?, limit=20, include_suspect=true, file?)` | worst tests first |
| `search_tests(repo, query, project?, limit=20)` | tests whose name, classname or file contains `query` |
| `get_test(test_id \| repo+project+name[+classname], executions_limit=20)` | location + GitHub permalink, last failing commit, latest failure message and traceback (≤4 KB), recent executions |

## GitHub issue automation

Set in `.env`:

```
FLAKERADAR_GITHUB_TOKEN=<fine-grained PAT with Issues:write on your repos>
FLAKERADAR_GITHUB_ISSUE_LABEL=flakeradar
```

When a test crosses the *filing gate*, FlakeRadar files an issue **in that test's
own repo** with everything an engineer (or an agent) needs to find and debug it:
repo/project, file/line, a permalink to the test at its last failing commit,
score (and the threshold used), proven-flake count, suite/classname, last failing
commit and branch, the last 10 executions, and a sample failure traceback.
One issue per test, labeled `FLAKERADAR_GITHUB_ISSUE_LABEL`, filed after
processing (never on the upload path), rate-limit aware. Leave the token blank
to disable the whole feature.

### Filing gate (score / proven flakes / failures)

Filing and tiering are decoupled: `FLAKE_THRESHOLD` still decides the UI/API
"flaky" tier, while filing is controlled by three independent minimums. A test
is filed only when it meets **every** configured (non-zero) minimum:

```
FLAKERADAR_GITHUB_ISSUE_MIN_SCORE=0.30          # flakiness score (0..1) min
FLAKERADAR_GITHUB_ISSUE_MIN_PROVEN_FLAKES=0     # same-commit fail+pass min
FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES=0          # failures in recent window min
```

Set any signal's minimum to `0` to skip it, so you can gate on score, proven
flakes, failures, or any combination. Defaults reproduce the historical behavior
(score ≥ 0.30 only). If issues turn out to be false positives, raise these
gates rather than loosening the tier threshold. (A token with all three at `0`
would file for every touched test — keep `MIN_SCORE` above 0.)

### Deduplication

A filed issue's number is stored on the test and blocks re-filing. FlakeRadar
checks GitHub periodically (on the leader's maintenance interval) and, once an
issue is **closed** (or deleted), clears that marker so a later Report that
re-triggers the flaky test files a fresh issue. Until the issue is closed, no
new issue is opened.

Filed issues are surfaced in the UI (a link on the leaderboard row and in the
test drawer) and via the API/MCP (`github_issue_number` + `github_issue_url` on
`/api/tests`, `/api/tests/{id}/history`, and the `get_test`/`top_flaky_tests`
MCP tools).

## How scoring works

For each test, over its last 50 executions (configurable):

1. **Flip score** — the decayed rate of pass↔fail transitions between
   consecutive runs. Alternating forever → 1.0; always-fail or always-pass → 0.
2. **Sample damping** — one flip across two runs is 100% flip rate but weak
   evidence; confidence scales in until ~7 executions are recorded.
3. **Same-SHA floor** — any commit with both a pass and a fail recorded is a
   **proven flake**: the score is floored at 0.6, rising with each additional
   proven flip.

`skipped` executions are ignored; `error` counts as failing. Tests are grouped
into tiers: **flaky** (score ≥ threshold), **suspect** (0 < score < threshold)
and **stable** (0). The leaderboard hides stable tests unless you ask for them.

The same rules and tiers apply to **CI jobs**.

### The default-branch rule (Tests and Jobs)

Flips only count on a Repo's **default branch** — a test (or job) that breaks
on a PR branch and is then fixed should not look flaky. So the **flip score** is
computed over the newest executions on the default branch only. **Proven
flakes** (a pass and a fail on the same commit) still count on every branch,
because a same-commit flip is nondeterminism on identical code. Until the
default branch is known (no report has named it), every branch counts — the
original behaviour.

## Configuration

Environment variables (prefix `FLAKERADAR_`, `.env` supported):

| Variable | Default | Meaning |
|---|---|---|
| `API_TOKEN` | `changeme` (refused unless `FLAKERADAR_ALLOW_INSECURE=1`) | CI uploads (`X-API-Key`) and MCP (`Authorization: Bearer`) |
| `DATABASE_URL` | `postgresql+asyncpg://flakeradar:flakeradar@localhost:5432/flakeradar` | PostgreSQL only; `postgresql://` URLs are accepted |
| `FLAKE_THRESHOLD` | `0.30` | score at which a test is flaky |
| `SCORE_WINDOW` / `SCORE_DECAY` | `50` / `0.85` | scoring window and recency weight |
| `GITHUB_TOKEN` | empty (disabled) | issue automation |
| `WORKER_POLL_SECONDS` | `1.0` | processor idle poll interval |
| `REPORT_RETENTION_DAYS` | `7` | processed raw reports are deleted after this (failed ones are kept) |
| `EXECUTION_RETENTION_DAYS` | `90` | older executions are deleted |
| `PRUNE_INTERVAL_SECONDS` | `3600` | how often retention runs |
| `CORS_ORIGINS` | `http://localhost:5173` | dev-server origin |

## API

| Endpoint | Auth | Purpose |
|---|---|---|
| `POST /api/ingest?repo=&project=&root=&commit_sha=&branch=&ci_run_id=&ci_run_attempt=&ci_job_id=&pipeline=&default_branch=` | `X-API-Key` | Queue a JUnit XML report (raw body or multipart `report` field, ≤20 MB) → `202` |
| `POST /api/ingest/pipeline` | `X-API-Key` | Queue a Pipeline report (JSON) with `jobs` → `202` |
| `GET /api/reports/{id}` | — | One report's status (`pending`/`processed`/`failed`), counts, error |
| `GET /api/reports?status=&limit=` | — | Recent reports, newest first |
| `GET /api/reports/summary` | — | `{pending, failed}` |
| `POST /api/reports/{id}/retry` | `X-API-Key` | Re-queue a failed report |
| `GET /api/repos` | — | Repos with their projects |
| `GET /api/tests?repo=&project=&include_stable=&sort=&page=&page_size=&file=` | — | Paginated leaderboard (`sort`: `score`, `last_seen`, `proven`) |
| `GET /api/tests/{id}/history?limit=` | — | One test: location + permalink, last failing commit, executions with failure details, and the Jobs its Runs came from |
| `GET /api/jobs?repo=&include_stable=&sort=&page=&page_size=` | — | Jobs leaderboard for a Repo (worst first) |
| `GET /api/jobs/summary?repo=` | — | Job summary tiles |
| `GET /api/jobs/{id}/history?limit=` | — | One job: recent Job executions, each tagged `passed`/`failed`/`explained`/`skipped`, with the explaining Tests |
| `GET /api/summary?repo=&project=` | — | Dashboard tiles (Tests) |
| `POST /api/tests/{id}/quarantine` | — | Toggle quarantine (body `{"quarantined": bool}`) |
| `GET /api/quarantine?repo=&project=` | `X-API-Key` | Quarantined tests for one repo + project (for the test runner) |
| `/mcp/` | Bearer token | MCP server (see above) |
| `GET /api/health` | — | Liveness |

`project` filters require `repo`. Interactive docs at `/docs` (OpenAPI).

## Architecture

```
backend/   FastAPI + async SQLAlchemy 2 + asyncpg + PostgreSQL
  app/
    routers/reports.py  ingest (validate, queue, 202) + report status
    routers/tests.py    repos, leaderboard, summary, test detail, quarantine
    queries.py          read services shared by REST and MCP
    parsing.py          JUnit parsing: identity, location, failure details
    processing.py       one Report -> one Run, batched upserts, rescoring
    worker.py           the single Report processor (Postgres advisory lock)
    retention.py        hourly pruning
    scoring.py          flip score + same-SHA proof (pure functions)
    github_integration.py  per-repo issue filing
    mcp_server.py       read-only MCP tools (fastmcp)
  migrations/     Alembic (async), applied on startup
  tests/          pytest against a real Postgres (testcontainers)
frontend/  React 18 + Vite + TypeScript, Vitest; zero runtime chart deps
samples/   CI snippet + demo-data simulator
```

Design notes:

- **Queued ingest** ([ADR 0002](docs/adr/0002-queued-ingest.md)): uploads only
  validate and store the raw report; exactly one processor, elected with a
  Postgres advisory lock across uvicorn workers, turns reports into runs in
  upload order — scoring depends on that order.
- **PostgreSQL only, fully async** ([ADR 0003](docs/adr/0003-postgres-only-async.md)).
- **Repos and projects** ([ADR 0001](docs/adr/0001-repo-project-split.md)) —
  domain terms are defined in [CONTEXT.md](CONTEXT.md).
- The read APIs and the quarantine toggle are unauthenticated by design (the
  dashboard is expected to sit on a private network). CI-facing endpoints and
  the MCP server are token-gated.
- Execution-status marks in the UI are shape-coded (circle/square/diamond/hollow)
  because pass-green vs fail-red collapses under deuteranopia — color never
  carries meaning alone.

## Development

```bash
cd backend
.venv/bin/python -m pytest -q    # needs Docker (starts a throwaway Postgres)
# or point at an existing database:
# FLAKERADAR_TEST_DATABASE_URL=postgresql+asyncpg://... .venv/bin/python -m pytest -q
cd ../frontend
npm test                         # Vitest
npm run build                    # strict TypeScript is the other frontend gate
```

## Roadmap

Delivered in v2.0: repos and projects, queued ingest on PostgreSQL, test
locations and failure details, per-repo GitHub issues, retention, the MCP
server, and a dashboard that scales. Still ahead:

- **Branch filtering** in the dashboard (data is already recorded per branch).
- **Issue lifecycle** — auto-close the GitHub issue after N consecutive stable runs.
- **Test-runner plugin** — a pytest/Vitest plugin that consumes `/api/quarantine`
  automatically.
- **Dashboard screenshot refresh** (`docs/dashboard.jpg` predates v2.0).
