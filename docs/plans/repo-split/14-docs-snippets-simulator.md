# 14 — Docs, CI snippets and demo simulator

## Tracer-Bullet Outcome
A new user can follow the README from zero to a working instance: `docker compose up` (with Postgres), a CI step that uploads with `repo`/`project`/`root` and retries, reporter flags that make runners emit file/line, the MCP registration command for agents, and a correct API and configuration reference. `python samples/simulate_ci.py` seeds a demo Repo with two Projects against the new queued API and waits until everything is processed.

Verified: the new simulator ran against a live 2-worker server. It uploaded 29 Reports, 0 failed. The result was `test_checkout_total_rounding` flaky with 1 proven flake, plus a suspect frontend test with a Location. The CONTRIBUTING migration commands (`alembic upgrade head`, `alembic revision --autogenerate`) ran against Postgres, and autogenerate produced an empty migration.

## User Story
As someone setting up or contributing to this fork, I want docs that match the code so that my CI integration and agent setup work on the first try.

## Description
Replace `README.md`, `CONTRIBUTING.md`, `samples/github-actions-snippet.yml` and `samples/simulate_ci.py` with the verified versions below. The old docs describe SQLite, the `project=`-as-repository model, `/api/projects`, synchronous `200` ingest with counts, and `FLAKERADAR_GITHUB_REPO`. All of that is gone.

## Context Pack
- Source decisions: everything in `00-shared-context.md` → Decisions. The documented behavior must match tasks 01–13 exactly:
  - endpoints and auth;
  - `202` ingest;
  - reporter flags (verified: pytest `-o junit_family=xunit1` with 0-based lines; Vitest `addFileAttribute`; jest-junit `JEST_JUNIT_ADD_FILE_ATTRIBUTE`);
  - the `claude mcp add --transport http … --header "Authorization: Bearer …"` syntax (verified from `claude mcp add --help`);
  - settings names and defaults from `backend/app/config.py`.
- Repo facts: the current `README.md` sections are Who it's for, What it does, Quick start (local), Quick start (Docker), CI integration, Multi-project, Quarantine workflow, GitHub issue automation, How scoring works, API, Architecture, Development and Roadmap. It uses Windows venv paths (`.venv/Scripts/…`). `samples/simulate_ci.py` posts without `repo` and prints `resp.json()["counts"]`, which no longer exists in the `202` response.
- Non-goals: a new dashboard screenshot (`docs/dashboard.jpg` stays; the Roadmap notes it is stale); a docs site; changing `CONTEXT.md` or the ADRs; `LICENSE`, `NOTICE`, `SECURITY.md`.

## Delivery Strategy
- Shape: Wide refactor: Migrate (integration branch). Docs only.
- Valid-state scope: Named integration branch `feat/repo-split`.

## Implementation Contract
- Expected files: replace `README.md`, `CONTRIBUTING.md`, `samples/github-actions-snippet.yml` and `samples/simulate_ci.py`. Touch nothing else.
- `README.md` (full target):
````markdown
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

![FlakeRadar dashboard — flakiness leaderboard and per-test execution history](docs/dashboard.jpg)

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
    curl --fail-with-body -sS --retry 5 --retry-all-errors --retry-delay 2 \
      -X POST "$FLAKERADAR_URL/api/ingest?repo=${{ github.repository }}&project=backend&root=backend&commit_sha=${{ github.sha }}&branch=${{ github.ref_name }}&ci_run_id=${{ github.run_id }}-${{ github.run_attempt }}" \
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
| `ci_run_id` | no | include the attempt number: it turns "re-run failed jobs" into labeled flake data |

The server answers `202 {"report_id": 7, "status": "pending"}` as soon as the
report is stored; `GET /api/reports/7` shows when it has been processed. The
`--retry` flags cover the server being briefly unreachable (e.g. a redeploy).

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

FlakeRadar serves a read-only [MCP](https://modelcontextprotocol.io) server at
`/mcp/` (streamable HTTP). Authenticate with the API token as a Bearer token:

```bash
claude mcp add --transport http flakeradar https://flakeradar.example.com/mcp/ \
  --header "Authorization: Bearer $FLAKERADAR_API_TOKEN"
```

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
FLAKERADAR_FLAKE_THRESHOLD=0.30
```

When a test crosses the threshold, FlakeRadar files an issue **in that test's
own repo** with the evidence: repo/project, location, score, proven flakes, the
last 10 executions and a sample failure. One issue per test, labeled
`flakeradar`, filed after processing (never on the upload path), rate-limit
aware. Leave the token blank to disable.

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
| `POST /api/ingest?repo=&project=&root=&commit_sha=&branch=&ci_run_id=` | `X-API-Key` | Queue a JUnit XML report (raw body or multipart `report` field, ≤20 MB) → `202` |
| `GET /api/reports/{id}` | — | One report's status (`pending`/`processed`/`failed`), counts, error |
| `GET /api/reports?status=&limit=` | — | Recent reports, newest first |
| `GET /api/reports/summary` | — | `{pending, failed}` |
| `POST /api/reports/{id}/retry` | `X-API-Key` | Re-queue a failed report |
| `GET /api/repos` | — | Repos with their projects |
| `GET /api/tests?repo=&project=&include_stable=&sort=&page=&page_size=&file=` | — | Paginated leaderboard (`sort`: `score`, `last_seen`, `proven`) |
| `GET /api/tests/{id}/history?limit=` | — | One test: location + permalink, last failing commit, executions with failure details |
| `GET /api/summary?repo=&project=` | — | Dashboard tiles |
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
````
- `CONTRIBUTING.md` (full target):
````markdown
# Contributing to FlakeRadar

Domain terms (Repo, Project, Report, Run, Execution, flaky/suspect/stable, …)
are defined in [CONTEXT.md](CONTEXT.md); architectural decisions live in
[docs/adr/](docs/adr/). Use the same words in code, UI text and docs.

## Dev setup

```bash
# PostgreSQL (the backend's default DATABASE_URL points here)
docker run -d --name flakeradar-pg -p 5432:5432 \
  -e POSTGRES_USER=flakeradar -e POSTGRES_PASSWORD=flakeradar -e POSTGRES_DB=flakeradar \
  postgres:17-alpine

# Backend (Python 3.12)
cd backend
python3.12 -m venv .venv
.venv/bin/pip install -r requirements.txt
FLAKERADAR_ALLOW_INSECURE=1 .venv/bin/python -m uvicorn app.main:app --port 8000

# Frontend
cd frontend
npm install
npm run dev
```

## Test gate

All must pass before a PR is merged:

```bash
cd backend && .venv/bin/python -m pytest -q   # needs Docker: testcontainers starts Postgres
cd frontend && npm test                        # Vitest + Testing Library
cd frontend && npm run build                   # strict TypeScript build
```

Set `FLAKERADAR_TEST_DATABASE_URL=postgresql+asyncpg://…` to run the backend
tests against an existing (throwaway!) database instead — its tables are
truncated between tests.

## Database migrations

Schema changes go through Alembic. After editing `backend/app/models.py`, with
a Postgres reachable at `FLAKERADAR_DATABASE_URL`:

```bash
cd backend
.venv/bin/python -m alembic upgrade head
.venv/bin/python -m alembic revision --autogenerate -m "describe change"
# review the generated file
```

Migrations run automatically on app startup (serialized across uvicorn
workers by an advisory lock).
````
- `samples/github-actions-snippet.yml` (full target):
````yaml
# Add this step to your GitHub Actions workflow AFTER the test step.
# Requires: FLAKERADAR_URL (e.g. https://flakeradar.example.com) as a variable
#           or secret, and FLAKERADAR_TOKEN (your FLAKERADAR_API_TOKEN) as a secret.
# Works with any runner that produces a JUnit XML report, e.g.:
#   pytest --junitxml=junit.xml -o junit_family=xunit1   (Python; xunit1 adds file/line)
#   vitest --reporter=junit --outputFile=junit.xml        (JS/TS; set addFileAttribute)
#   go test ... | go-junit-report > junit.xml              (Go)
# One step per project: set `project` (and `root`, its directory) for each suite.

- name: Report test results to FlakeRadar
  if: always()   # crucial — you want the FAILED runs recorded too
  run: |
    curl --fail-with-body -sS --retry 5 --retry-all-errors --retry-delay 2 -X POST \
      "${{ secrets.FLAKERADAR_URL }}/api/ingest?repo=${{ github.repository }}&project=backend&root=backend&commit_sha=${{ github.sha }}&branch=${{ github.ref_name }}&ci_run_id=${{ github.run_id }}-${{ github.run_attempt }}" \
      -H "X-API-Key: ${{ secrets.FLAKERADAR_TOKEN }}" \
      -H "Content-Type: application/xml" \
      --data-binary @junit.xml
````
- `samples/simulate_ci.py` (full target, verified against a live server):
````python
"""Replay a realistic CI history into a running FlakeRadar instance.

Usage:  python samples/simulate_ci.py [base_url] [token]
Defaults: http://localhost:8000  /  changeme

Simulates 14 CI runs of one Repo with two Projects:
- demo/shop : backend
    test_checkout_total_rounding  -> genuinely flaky (random failures + one
                                     same-SHA retry that flips fail->pass)
    test_payment_gateway_timeout  -> mildly flaky (occasional failure)
    test_schema_migration_v42     -> broken: fails EVERY run (scores 0)
    5 stable tests                -> always pass
- demo/shop : frontend
    Cart > updates the badge      -> flaky, reports its file (Location demo)
Uploads are queued (202); the script waits until every Report is processed.
"""
import random
import sys
import time

import httpx

BASE = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8000"
TOKEN = sys.argv[2] if len(sys.argv) > 2 else "changeme"
REPO = "demo/shop"

rng = random.Random(42)

STABLE = [
    "test_login_with_valid_credentials",
    "test_signup_sends_welcome_email",
    "test_cart_add_and_remove",
    "test_product_search_pagination",
    "test_invoice_pdf_render",
]


def case_xml(name: str, status: str, classname: str = "tests.e2e.test_shop",
             file: str = "tests/e2e/test_shop.py", line: int = 88) -> str:
    body = ""
    if status == "failed":
        body = (
            '<failure message="AssertionError: expected 104.85, got 104.84">'
            f"Traceback (most recent call last):\n  File \"{file}\", line {line}, in {name}\n"
            "    assert total == Decimal('104.85')\n"
            "AssertionError: expected 104.85, got 104.84</failure>"
        )
    return (f'<testcase classname="{classname}" name="{name}" file="{file}" line="{line}" '
            f'time="{rng.uniform(0.1, 2.5):.2f}">{body}</testcase>')


def report(suite: str, cases: list[str]) -> bytes:
    return (f'<testsuites><testsuite name="{suite}" tests="{len(cases)}">'
            f'{"".join(cases)}</testsuite></testsuites>').encode()


def post(client: httpx.Client, project: str, root: str, sha: str, run_id: str,
         body: bytes) -> int:
    resp = client.post(
        f"{BASE}/api/ingest",
        params={"repo": REPO, "project": project, "root": root, "commit_sha": sha,
                "branch": "main", "ci_run_id": run_id},
        content=body,
        headers={"X-API-Key": TOKEN, "Content-Type": "application/xml"},
    )
    resp.raise_for_status()
    return resp.json()["report_id"]


def backend_cases(checkout: str, gateway: str) -> list[str]:
    cases = [case_xml(n, "passed") for n in STABLE]
    cases.append(case_xml("test_checkout_total_rounding", checkout))
    cases.append(case_xml("test_payment_gateway_timeout", gateway))
    cases.append(case_xml("test_schema_migration_v42", "failed"))
    return cases


def main():
    report_ids: list[int] = []
    with httpx.Client(timeout=10) as client:
        for i in range(14):
            sha = f"{rng.getrandbits(160):040x}"
            checkout = "failed" if rng.random() < 0.35 else "passed"
            if i == 6:
                checkout = "failed"  # guarantee the same-SHA retry demo below
            gateway = "failed" if rng.random() < 0.15 else "passed"
            report_ids.append(post(client, "backend", "", sha, f"run-{i}",
                                   report("backend", backend_cases(checkout, gateway))))
            badge = "failed" if rng.random() < 0.3 else "passed"
            report_ids.append(post(client, "frontend", "web", sha, f"run-{i}", report("frontend", [
                case_xml("Cart > updates the badge", badge, classname="src/Cart.test.tsx",
                         file="src/Cart.test.tsx", line=21),
            ])))

            # The habit FlakeRadar exploits: a failed run gets re-run on the
            # same commit. Replay run 6's failure as a same-SHA retry that passes.
            if i == 6:
                report_ids.append(post(client, "backend", "", sha, f"run-{i}-retry",
                                       report("backend", backend_cases("passed", gateway))))

        deadline = time.monotonic() + 60
        while True:
            summary = client.get(f"{BASE}/api/reports/summary").json()
            if summary["pending"] == 0:
                break
            if time.monotonic() > deadline:
                sys.exit(f"timed out waiting for {summary['pending']} pending reports")
            time.sleep(0.5)
    print(f"uploaded {len(report_ids)} reports; failed to process: {summary['failed']}")
    print(f"open {BASE}/?repo={REPO}")


if __name__ == "__main__":
    main()
````
- Behavior rules: before committing, cross-check every endpoint, parameter, env var and default in the README against the code. Where they disagree, the **code** from tasks 01–13 wins. Fix the README and note the discrepancy in the commit message.
- Error and security rules: example tokens are placeholders (`$FLAKERADAR_API_TOKEN`, `${{ secrets.FLAKERADAR_TOKEN }}`). Never write a real token.

## Acceptance Criteria
- [ ] README has no remaining mention of SQLite, `/api/projects`, `FLAKERADAR_GITHUB_REPO` or `.venv/Scripts`.
- [ ] Every row of the README API table corresponds to a route in `backend/app/routers/*.py` or the `/mcp` mount.
- [ ] Every row of the README Configuration table corresponds to a field in `backend/app/config.py`, with the same default.
- [ ] `python samples/simulate_ci.py http://localhost:8000 <token>` against a running instance prints `uploaded 29 reports; failed to process: 0`.

## Test Expectations
- No automated tests (docs). Verify with these commands:
  - `grep -nE "sqlite|/api/projects|FLAKERADAR_GITHUB_REPO|\.venv/Scripts" README.md CONTRIBUTING.md samples/*` → no output.
  - With `docker compose up --build` running and `.env` holding the token: `python samples/simulate_ci.py http://localhost:8000 "$FLAKERADAR_API_TOKEN"` → `uploaded 29 reports; failed to process: 0`, then `open http://localhost:8000/?repo=demo/shop`.

## Dependencies
- Blocked by: 03 (ingest/report API), 05 (worker), 06 (retention settings), 07 (GitHub settings), 10 (MCP)
- Why blocked: the docs describe the final behavior of those tasks, and the simulator needs the queued ingest and a running processor.
- Blocks: 15

## Labels
`docs`, `priority:medium`

## Estimate
Small

## Risk
1 - Docs only. The main risk is drift, which the cross-check rule covers.

## Validator Stopping Point
```bash
grep -nE "sqlite|/api/projects|FLAKERADAR_GITHUB_REPO|\.venv/Scripts" README.md CONTRIBUTING.md samples/* ; test $? -eq 1 && echo docs-clean
cd backend && .venv/bin/python -m pytest -q   # unchanged: 87 passed
```
