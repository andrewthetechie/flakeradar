# 11 — Integrate, verify end to end, and open the PR

## Tracer-Bullet Outcome
`feat/ci-jobs` passes every check. A fresh Docker stack, with the simulator run against it, shows the whole feature working:
- Tests scored with the Default-branch rule;
- Jobs with explained and unexplained failures;
- the UI in both views;
- the MCP tools.

The upgrade from a `0001` database with data works. One PR is open against `main`.

## User Story
As the maintainer, I want one reviewable PR that I can trust, with evidence that the migration and the whole flow work.

## Context Pack
- Read `00-shared-context.md` and skim tasks 01–10.
- Local stack: `docker compose up --build` (see the README "Quick start (Docker)"). The simulator is `python samples/simulate_ci.py http://localhost:8000 <token>`.
- CI (`.github/workflows/ci.yml`) runs ruff, pytest, typecheck, ESLint, Prettier and Vitest.

## Steps
1. Run every check (see the Validator below) and fix anything that fails. A fix that belongs to an earlier task's scope goes in its own commit, with a message that names the task.
2. **Migration on real-shaped data:**
   - check out `main` and start the stack;
   - run the simulator (the old version) so that `0001` data exists;
   - switch to `feat/ci-jobs` and rebuild;
   - confirm that startup migrates to `0002`, old Reports list with `kind: "junit"`, and Test scores are unchanged until a Default branch is reported.
   - Then run the new simulator and confirm the Default branch is set, Tests are rescored, and Jobs appear.
3. **End-to-end checks** against the running stack:
   - `GET /api/jobs?repo=demo/shop&include_stable=true`: `e2e (ubuntu-latest)` is flaky or suspect with unexplained failures, and `backend` shows `explained` executions.
   - The UI at desktop and phone widths: the Jobs view, the Job drawer, "explained by" → Test drawer, and "Seen in Jobs" → Job drawer.
   - MCP: `claude mcp add --transport http flakeradar http://localhost:8000/mcp/ --header "Authorization: Bearer <token>"`, then call `top_flaky_jobs` and `get_job`. Or use the fastmcp `Client` from a Python shell.
4. **Documentation check:** README sections, samples, and nothing stale. `CONTEXT.md` and ADR 0004 still match what was built. If the implementation had to differ from ADR 0004, **stop and ask the maintainer** rather than editing the ADR on your own.
5. **Open the PR:** `gh pr create --base main --head feat/ci-jobs`. The title is `Track flaky CI jobs from pushed Pipeline reports (ADR 0004)`. The body has:
   - a summary;
   - the task list with commit ranges;
   - the migration notes (with the downgrade caveat that it deletes Pipeline reports);
   - the manual checks you ran and what you saw;
   - known limits: GitLab and Forgejo reporters are not built; attribution needs `ci_job_id`; fork PR branches are prefixed with the fork owner.
   - It ends with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.

## Acceptance Criteria
- [ ] Every command in the Validator passes.
- [ ] The migration from real `0001` data is verified as described.
- [ ] The end-to-end checks are done, and the results are in the PR body.
- [ ] The PR is open, and CI is green on it.

## Dependencies
- Blocked by: 01–10.

## Estimate / Risk
Small–medium. Risk 2.

## Validator Stopping Point
```bash
ruff check backend && ruff format --check backend
cd backend && .venv/bin/python -m pytest -q
cd ../frontend && npm ci && npm run typecheck && npm run lint && npm run format:check && npm test && npm run build
```
