# 37 — Playwright tests for the dashboard

Read `00-fixes-context.md` first. Finding: `R1`. This task adds tests and CI jobs. It changes no server behaviour and no goldens.

## Why

Nothing in CI opens a browser. `frontend/` has only vitest component tests, and tasks 09, 12, 22 and 25 each recorded the browser check as unverified. The owner's reviewer ran the check by hand at `ac5e6f3` and it passed, so these tests start from a working dashboard. They keep it working.

## Contracts

Add Playwright to `frontend/` with an exact version pin and two projects. Both run in Chromium against the built `frontend/dist`.

**1. `ui` project: mocked API.** Serve the built dashboard and answer every `/api/*` call with `page.route` from fixtures in `frontend/e2e/fixtures/`. An unmocked `/api/*` call fails the test. This project covers states a real server rarely produces:

- Empty lists: no repos, no tests, no jobs.
- A 500 on each read the page makes, and a 422 on a write.
- A pending queue in `/api/reports/summary`.
- A long test name, a null `failure_category`, a null `trend`, and a pagination boundary.

Derive fixture shapes from the real server's answers. Add one test in the `e2e` project that fetches each mocked route and compares its top-level field names with the fixture, so the mocks cannot drift from the server.

**2. `e2e` project: a mock install.** Start the real `flakeradar` binary with `frontend/dist` embedded, a throwaway `postgres:17-alpine`, and data seeded by `samples/simulate_ci.py`. It makes no outside network call. Use a Playwright `globalSetup` or a script in `frontend/e2e/`, with one command to start and stop everything.

The `e2e` project runs the reviewer's checks:

| Page | Assert |
|---|---|
| `/?repo=demo/shop` | The tiles show the seeded counts, and the leaderboard lists its rows worst first with tier and likely cause |
| Click a Test row | The URL gains `test=<id>`. The drawer shows the score, proven flakes, clean streak, the file permalink, execution history and the latest failure |
| `jobs` view | The job leaderboard renders. Clicking a row sets `job=<id>` and the drawer shows unexplained and explained counts, the runner table and history |
| `/?repo=demo/shop&view=ci-run&ci_run=run-12` | Failed tests and failed jobs render, each job marked explained or unexplained |
| Quarantine | Reason and expiry dialog, then `Confirm`, sends `POST /api/tests/{id}/quarantine` answering 200. After a reload the row is marked quarantined, and the API agrees |
| Search, sort, filters, project picker | Each changes the rows and the URL. Loading that URL directly gives the same page |
| Theme toggle, fonts | The toggle changes `data-theme`. Each font request answers 200 |

**Every test in both projects** fails on any console error or uncaught page error. Every `e2e` test also fails on any `/api/*` response of 400 or above that the test did not expect. Put this in one shared fixture so a new test cannot leave it out.

**Tests.** Select by role and name, the way the checks above read. Do not use fixed sleeps. Use Playwright's own auto-waiting. Seeded scores come from a seeded random generator only if `simulate_ci.py` is deterministic. Read it first, and if it is not, assert relations (worst first, a score at or above the threshold shows `flaky`) rather than exact values.

**CI.** Add two jobs to `.github/workflows/ci.yml`, both required for the PR:

- `Frontend UI (Playwright, mocked)` builds `dist` or reuses the `frontend-dist` artifact.
- `Dashboard e2e (Playwright, mock install)` uses the same artifact, a `postgres` service, and a binary built with that `dist`.

Pin and cache the browser install. Upload the Playwright trace and report on failure. Keep the jobs under the `changes` path filter only if a Rust or frontend change still triggers both.

**Docs.** `CONTRIBUTING.md` gives the two commands. Remove the "frontend browser check" unverified items from `HANDOFF.md` and `RE-REVIEW.md` once CI runs these jobs green.

## Done when

- [ ] `npx playwright test --project=ui` and `--project=e2e` pass locally, each from a clean start, with one documented command.
- [ ] A negative control is recorded in `STATUS.md` for each project. Break a fixture field and see `ui` fail. Answer 500 from a route and see `e2e` fail. Log a console error and see the shared fixture fail. Revert all three.
- [ ] Both jobs are green on the pushed head, and their names appear in `gh pr checks 135`.
- [ ] Vitest coverage gates are untouched, and Playwright files are excluded from them.
- [ ] Three consecutive local runs of both projects pass under CPU stress with no retries. Set Playwright `retries: 0`.
