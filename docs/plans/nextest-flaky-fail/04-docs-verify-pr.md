# 04 — ADR 0005, README, end-to-end check, open the PR

## Tracer-Bullet Outcome
ADR 0005 and the README "Retries" section describe the nextest flaky marker. A fresh Docker stack shows that the fixture from task 01 gives a Proven flake, and that migration `0009` runs on existing data. One PR is open against `main` and closes issue #8.

## User Story
As the maintainer, I want one reviewable PR with docs and evidence that nextest flaky-fail retries now count as flakes.

## Description
Documentation, verification, then the PR. Other code changes here are only fixes for problems the checks find.

## Context Pack
- Source decisions: `00-shared-context.md` (all). Issue #8 "Done when": a nextest JUnit fixture with `flaky-fail-status = "failure"` parses to attempt 0 `failed` and attempt 1 `passed` (task 01); that Test gets a Proven flake on its SHA (tasks 01 and 03); ADR 0005 or the README "Retries" section mentions the nextest marker (this task).
- Repo facts:
  - `docs/adr/0005-retry-attempts-from-junit.md` has front matter `status: proposed`, a context paragraph, `## Considered Options`, and `## Consequences` (a bullet list). Its last Consequences bullet starts `- Uploads without these elements behave as before.`
  - README `## Retries` holds a two-row table (`Playwright 1.59 or later`, `Maven Surefire / Failsafe`). Then comes a paragraph that starts `Runners that write each attempt as a separate \`<testcase>\``, then a paragraph that starts `Each failed retry counts as one failure.`
  - Ingest: `POST /api/ingest?repo=&project=&commit_sha=&branch=&ci_run_id=&ci_job_id=...` with header `X-API-Key`, `Content-Type: application/xml` and the XML as the body. It answers `202 {"report_id": N, "status": "pending"}`. `GET /api/reports/N` shows when it is processed.
  - Local stack: `cp .env.example .env`, set `FLAKERADAR_API_TOKEN` (or `FLAKERADAR_ALLOW_INSECURE=1` for a throwaway run), then `docker compose up --build`. App, API and MCP on `http://localhost:8000`. Simulator: `python samples/simulate_ci.py http://localhost:8000 <token>`.
- Non-goals: new features; editing `CONTEXT.md` (the commit that added this plan also updated its **Execution** entry; Step 1 checks it); a new ADR.

## Delivery Strategy
- Shape: Normal tracer bullet (final verification; every earlier task is already green on its own)
- Valid-state scope: `fix/nextest-flaky-fail`, and the PR's CI

## Implementation Contract
- Expected files: `docs/adr/0005-retry-attempts-from-junit.md`, `README.md`. Also any file that needs a fix found by the checks (each fix in its own commit that names the task it belongs to).
- Interfaces and names: None new.
- Verified external contracts: `gh pr create --base main --head fix/nextest-flaky-fail --title … --body-file …` (GitHub CLI).
- Behavior rules (exact text):
  - ADR 0005, context paragraph: after the sentence `We now parse these elements.` insert ` cargo-nextest writes its retries as \`<flakyFailure>\` and \`<flakyError>\` too.` (one sentence, same paragraph).
  - ADR 0005, `## Consequences`: add this bullet after the bullet that starts `- Runners that write each attempt as a duplicate`:
    ```markdown
    - cargo-nextest with `flaky-result = "fail"` writes the final try of a flaky test, which passed, as a `<failure>` with the message `test passed on attempt N/M but is configured to fail when flaky`. When a testcase has that message and at least one `flaky*` element, we store that try as a passed Execution, so the Test is still a Proven flake. The Job still fails, and the earlier failed tries explain it. Without a `flaky*` element we keep the failure, so a Run never loses its only failing Execution. Migration 0009 corrects the Executions stored before this rule (issue #8).
    ```
  - README `## Retries` table: add a third row:
    ```markdown
    | cargo-nextest | `retries = 2` in a profile in `.config/nextest.toml`, with `[profile.<name>.junit] path = "junit.xml"` (retries are written as `<flakyFailure>`) |
    ```
  - README `## Retries`: add this paragraph after the paragraph that starts `Runners that write each attempt as a separate`:
    ```markdown
    With `flaky-result = "fail"`, nextest fails a test that passed only on retry.
    It writes the passing try as a `<failure>` with the message `test passed on
    attempt N/M but is configured to fail when flaky`. FlakeRadar stores that
    try as a pass, so the test is still a proven flake and
    `/api/ci-runs/{ci_run_id}/failures` shows it with `status: "passed"`. The
    job still fails, and the failed tries explain it.
    ```
- Error and security rules: None.

## Steps
1. Check `git log main..HEAD -- CONTEXT.md` and `git diff main -- CONTEXT.md`. The **Execution** entry must contain `The outcome is what that try did`. If your branch does not have it but `main` does, rebase or merge `main` into the branch. If neither has it, stop and report: the planner's CONTEXT.md change was not committed.
2. Make the ADR and README changes.
3. Run every check in the Validator and fix anything that fails.
4. **Migration on existing data:**
   1. `git switch main`, `docker compose up --build -d`, then run the simulator so `0008` data exists.
   2. Ingest the fixture **once on `main`** (the old parser), so a marker row is stored the old way. The fixture exists only on the branch, so copy it out first:
      ```bash
      git show fix/nextest-flaky-fail:backend/tests/fixtures/nextest_flaky_fail.xml > /tmp/nextest_flaky_fail.xml
      curl -sS -X POST "localhost:8000/api/ingest?repo=demo/nextest&commit_sha=777022bf&ci_run_id=38994487107&ci_job_id=J1" \
        -H "X-API-Key: <token>" -H "Content-Type: application/xml" \
        --data-binary @/tmp/nextest_flaky_fail.xml
      ```
      Wait until `GET /api/reports/<report_id>` shows `processed`. Then `curl -s "localhost:8000/api/ci-runs/38994487107/failures?repo=demo/nextest"` shows `status: "failed"`, `failed_attempts: 2`. This is the bug.
   3. `git switch fix/nextest-flaky-fail`, `docker compose up --build -d`. In `docker compose logs flakeradar`, startup migrates `0008 → 0009` without errors.
   4. The same `curl` for the CI run now shows `status: "passed"`, `failed_attempts: 1` (the migration fixed the rows). `confirmed_flake_count` can still be 0 here: the Test is rescored only on its next Report.
5. **End-to-end check of the new parser:** post the fixture again with `commit_sha=777022bf&ci_run_id=38994487108`. After it is processed:
   - `curl -s "localhost:8000/api/ci-runs/38994487108/failures?repo=demo/nextest"` → the flaky Test with `status: "passed"`, `failed_attempts: 1`, `confirmed_flake_count` ≥ 1.
   - `curl -s "localhost:8000/api/tests?repo=demo/nextest&project=default"` (or the UI) → the Test is `suspect` or `flaky`, not `stable`.
6. **Downgrade sanity** (on a throwaway DB only): `cd backend && FLAKERADAR_DATABASE_URL=… .venv/bin/alembic downgrade 0008`, then `upgrade head`. Both succeed.
7. **Open the PR:** `gh pr create --base main --head fix/nextest-flaky-fail --title "Parsing: count nextest flaky-fail retries as a pass" --body-file <file>`. The body has:
   - the problem in two sentences (nextest's flaky marker was stored as a failure, so the Test was never a Proven flake);
   - the rule (marker message plus at least one `flaky*` element → that try is `passed`) and why a lone marker stays failed;
   - what did not change, and why: attribution (attempt 0 still fails, so the Job stays explained) and the CI run endpoint (now `status: "passed"`, `failed_attempts: 1`);
   - migration notes: `0009` is data only, scores fix themselves on each Test's next Report, and past score history and report counts are not rewritten;
   - the task list (01–03) with commit ranges;
   - the manual checks from steps 4–6 and what you saw;
   - `Fixes #8`;
   - and it ends with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.

## Acceptance Criteria
- [ ] Every command in the Validator passes locally.
- [ ] The checks in steps 4–6 are done, and the results are in the PR body.
- [ ] ADR 0005 has the extra sentence and the bullet. The README has the row and the paragraph.
- [ ] The PR is open against `main`, its body contains `Fixes #8`, and its CI is green.

## Test Expectations
- No new automated tests unless a check finds a bug. A bug fix comes with a regression test in `backend/tests/test_parsing.py`, `test_processing.py`, `test_db.py` or `test_ci_runs_api.py`, whichever covers the broken part.

## Validator
```bash
cd backend && .venv/bin/python -m pytest -q
cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
```

## Dependencies
- Blocked by: 01, 02, 03.
- Blocks: None.

## Labels
`chore`, `docs`, `priority:high`
