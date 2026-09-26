# 02 — Retry attempts in the drawer, README and simulator

## Tracer-Bullet Outcome
In the Test drawer, an Execution that was a retry shows `retry N` in the execution table and in the history-strip tooltip. The README tells Playwright and Maven users how to send retries, and says that the `min_failures` issue gate counts each failed retry. `samples/simulate_ci.py` sends a Playwright-style retried test, so a demo instance shows a Proven flake that came from a retry.

## User Story
As a maintainer reading a Test's history, I want to see which failures were retries inside one Run, so that I can tell "failed then passed on retry" from "failed on one push, passed on the next".

## Description
Frontend: add `attempt` to the `Execution` type and show it. Docs: a README section, two config comments, and one `.env.example` comment. Sample: one retried test in the simulator.

## Context Pack
- Source decisions: `00-shared-context.md`, "Retries". ADR 0005.
- Repo facts:
  - Task 01 added `attempt: int` to every item of `executions` in `GET /api/tests/{id}/history` (0 = first try in its Run).
  - `frontend/src/api.ts`, current type:
    ```ts
    export interface Execution {
      id: number;
      status: string;
      duration: number;
      message: string;
      details: string;
      created_at: string;
      commit_sha: string;
      branch: string;
      ci_run_id: string;
    }
    ```
  - `frontend/src/components/TestDetail.tsx` renders the tooltip header line like this:
    ```tsx
    <div className="flex items-center gap-1.5 font-semibold">
      <StatusMark status={hover.e.status} size={9} /> {hover.e.status}
      {hover.e.duration > 0 && ` · ${hover.e.duration.toFixed(2)}s`}
    </div>
    ```
    and the table Status cell like this:
    ```tsx
    <td>
      <span className="inline-flex items-center gap-1.5 text-text-2">
        <StatusMark status={e.status} size={9} /> {e.status}
      </span>
    </td>
    ```
  - Fixtures typed `Execution` (inside `History.executions`) are in `frontend/src/components/TestDrawer.test.tsx` and `frontend/src/App.test.tsx`. Search for `ci_run_id: "` to find each object.
  - README section "Test locations and failure details" ends with this paragraph (insert the new section right after it, before `## Quarantine workflow`):
    ```md
    Runners without a file attribute (Go, cargo-nextest) still work — the test is
    identified by its classname and name, and the full failure traceback (which
    usually contains `file:line`) is kept for every failure.
    ```
  - README gate block (section "Filing gate") has this line:
    ```
    FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES=0          # failures in recent window min
    ```
    `.env.example` has `FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES=0` under a comment block that starts `# Issue-filing gate:`. `backend/app/config.py` has the comment `# and failure count over the recent window.` above `github_issue_min_score`.
  - `samples/simulate_ci.py`, the frontend report is built in `main()` with:
    ```python
    report_ids.append(post_junit(client, "frontend", "web", sha, f"run-{i}", f"f-{i}", 1,
                                 case_xml("Cart > updates the badge", badge,
    ```
    `case_xml(name, status, classname=..., file=..., line=...) -> str` returns one `<testcase>` string, and `rng = random.Random(42)`.
- Non-goals: backend changes; showing retries in the Job drawer (Job executions have no retries); a retry filter.

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `feat/flake-insights` after this draft

## Implementation Contract
- Expected files: `frontend/src/api.ts`, `frontend/src/components/TestDetail.tsx`, `frontend/src/components/TestDrawer.test.tsx`, `frontend/src/App.test.tsx` (fixtures only), `README.md`, `.env.example`, `backend/app/config.py` (comment only), `samples/simulate_ci.py`.
- Interfaces and names:
  - `api.ts`: `Execution` gains `attempt: number; // 0 = first try in its Run; >0 = a retry`.
  - `TestDetail.tsx`: add
    ```tsx
    /** " · retry 2" for a retry, "" for a first try. */
    export function retryLabel(attempt: number): string {
      return attempt > 0 ? ` · retry ${attempt}` : "";
    }
    ```
    Append `{retryLabel(hover.e.attempt)}` after the duration in the tooltip header, and `{retryLabel(e.attempt)}` after `{e.status}` in the table Status cell.
  - `simulate_ci.py`: add
    ```python
    def retried_case_xml(name: str, classname: str, file: str, line: int) -> str:
        """A Playwright-style test that failed once and passed on retry (includeRetries)."""
        return (
            f'<testcase classname="{classname}" name="{name}" file="{file}" line="{line}" time="1.40">'
            '<flakyFailure message="Timed out 5000ms waiting for expect(locator).toHaveText(expected)" '
            'type="FAILURE" time="5.10"><stackTrace>Error: Timed out 5000ms waiting for '
            "expect(locator).toHaveText(expected)\n    at " + file + ":" + str(line) + "</stackTrace>"
            "</flakyFailure></testcase>"
        )
    ```
    In `main()`, add a second frontend case to every frontend report where `i % 4 == 0`: `retried_case_xml("Checkout > shows the total", "web/src/checkout.spec.ts", "src/checkout.spec.ts", 21)`. On other runs, add the same test as a plain `case_xml("Checkout > shows the total", "passed", classname="web/src/checkout.spec.ts", file="src/checkout.spec.ts", line=21)`. Update the module docstring's list of simulated tests with one line: `Checkout > shows the total -> passes, but needs a retry every 4th run (flakyFailure)`.
- Verified external contracts: Playwright ≥ 1.59 JUnit reporter option `includeRetries` (config) or env `PLAYWRIGHT_JUNIT_INCLUDE_RETRIES=1`. It writes `<flakyFailure>`/`<flakyError>` for tests that passed on retry and `<rerunFailure>`/`<rerunError>` for tests that failed every attempt. This was verified against `packages/playwright/src/reporters/junit.ts` at v1.59.0 and later (absent at v1.58.0). Maven Surefire/Failsafe write the same elements when `rerunFailingTestsCount` is set (standard Surefire report format).
- Behavior rules:
  - README new section, exactly this heading and substance:
    ```md
    ## Retries

    When a runner retries a test inside one run, FlakeRadar stores every attempt.
    A test that fails and then passes on retry has a fail and a pass on the same
    commit, so it is a **proven flake** from its first run.

    | Runner | Setting |
    |---|---|
    | Playwright ≥ 1.59 | `reporter: [["junit", { outputFile: "junit.xml", includeRetries: true }]]` (or `PLAYWRIGHT_JUNIT_INCLUDE_RETRIES=1`) |
    | Maven Surefire / Failsafe | `-Dsurefire.rerunFailingTestsCount=2` (retries are written as `<flakyFailure>` / `<rerunFailure>`) |

    Runners that write each attempt as its own `<testcase>` with the same name
    also work. Without these settings, a retried test that finally passes looks
    like a clean pass.

    Each failed retry counts as one failure, including for
    `FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES`. With 2 retries, one run can add 3 failures.
    ```
  - README gate line becomes `FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES=0          # failures in recent window min (each failed retry counts)`.
  - `.env.example`: add the comment line `# Each failed retry attempt counts as one failure for MIN_FAILURES.` directly above `FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES=0`.
  - `config.py`: the comment becomes `# and failure count over the recent window (each failed retry attempt counts).`
- Error and security rules: None.

## Acceptance Criteria
- [ ] `retryLabel(0) === ""` and `retryLabel(2) === " · retry 2"`.
- [ ] The Test drawer table shows `failed · retry 1` for an Execution with `status: "failed", attempt: 1`, and shows `passed` alone for `attempt: 0`.
- [ ] The README has the `## Retries` section, and the gate line and `.env.example` mention retries.
- [ ] `python -m py_compile samples/simulate_ci.py` succeeds.
- [ ] Frontend checks pass.

## Test Expectations
- Framework: Vitest + Testing Library. Run: `cd frontend && npm test`.
- In `frontend/src/components/TestDrawer.test.tsx`, add `attempt: 0` to the existing fixture executions, then add one execution `{ id: 3, status: "failed", attempt: 1, … }` (copy the other fields from an existing execution; give it a unique `id` and put it first, because the list is newest first). Add the test `it("marks retries in the execution table", …)`: render the drawer with that history and assert `screen.getByText(/failed · retry 1/)` is in the document.
- Add a small unit test for `retryLabel` in the same file (import it from `./TestDetail`) with the two literals above.
- Add `attempt: 0` to every `Execution` object in `App.test.tsx`.

## Dependencies
- Blocked by: 01.
- Why blocked: 01 adds `attempt` to the history API and makes the parser read retry elements, which the README and the simulator describe.
- Blocks: 09.

## Labels
`feature`, `frontend`, `docs`, `priority:medium`

## Estimate
Small

## Risk
1 - Display, docs and a sample only.

## Validator Stopping Point
```bash
cd frontend && npm run typecheck && npm run lint && npm run format:check && npm test
cd .. && python -m py_compile samples/simulate_ci.py
cd backend && .venv/bin/python -m pytest -q
```
