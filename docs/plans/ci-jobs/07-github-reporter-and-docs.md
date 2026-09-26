# 07 — GitHub reporter workflow, JUnit snippet, README and simulator

## Tracer-Bullet Outcome
A user adds one workflow file to a repo, `samples/flakeradar-jobs.yml`, copied to `.github/workflows/`, and lists their workflow names in it. After every attempt of those workflows, it pushes that attempt's Job results to FlakeRadar. The JUnit snippet sends `ci_job_id`, `ci_run_attempt`, `pipeline` and `default_branch`, so attribution works. The README documents both. The demo simulator produces Jobs, including an explained failure and an infrastructure flake, so the UI in task 08 has data to show.

## User Story
As a repo owner, I want to copy one file and one updated step and get job-level flake tracking, without giving FlakeRadar access to GitHub.

## Context Pack
- Read `00-shared-context.md`, section "External facts": `workflow_run`, the jobs-per-attempt endpoint, fork branches, `job.check_run_id` and `GITHUB_WORKFLOW_REF`.
- Current files: `samples/github-actions-snippet.yml` (the JUnit curl step), `README.md` sections "CI integration", "How scoring works", "API" and "Configuration", and `samples/simulate_ci.py` (replays 14 JUnit runs into `demo/shop`).
- The server contract is in task 03 (`PipelineReportIn`). The JUnit params come from tasks 02 and 05.

## Implementation Contract

### `samples/flakeradar-jobs.yml` (new)
Keep the behaviour below. The style can differ.
```yaml
# Reports every job of the listed workflows to FlakeRadar after each attempt.
# Copy to .github/workflows/flakeradar-jobs.yml on your DEFAULT branch
# (workflow_run only fires from there). List workflows by their `name:`
# (exact names, no globs). Needs FLAKERADAR_URL and FLAKERADAR_TOKEN secrets.
name: FlakeRadar jobs
on:
  workflow_run:
    workflows: ["CI"]        # <- your workflow names
    types: [completed]
permissions:
  actions: read
jobs:
  report:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/github-script@v7
        env:
          FLAKERADAR_URL: ${{ secrets.FLAKERADAR_URL }}
          FLAKERADAR_TOKEN: ${{ secrets.FLAKERADAR_TOKEN }}
        with:
          script: |
            const run = context.payload.workflow_run;
            const repo = context.payload.repository;
            const jobs = await github.paginate(
              github.rest.actions.listJobsForWorkflowRunAttempt,
              { owner: context.repo.owner, repo: context.repo.repo,
                run_id: run.id, attempt_number: run.run_attempt, per_page: 100 });
            const STATUS = { success: "passed", failure: "failed", timed_out: "failed",
                             cancelled: "skipped", skipped: "skipped", neutral: "skipped" };
            const results = jobs.filter(j => STATUS[j.conclusion]).map(j => ({
              ci_job_id: String(j.id), name: j.name, status: STATUS[j.conclusion],
              url: j.html_url ?? "", runner_name: j.runner_name ?? "",
              runner_labels: j.labels ?? [], started_at: j.started_at, completed_at: j.completed_at }));
            if (results.length === 0) { core.info("No reportable jobs."); return; }
            const fork = run.head_repository && run.head_repository.full_name !== repo.full_name;
            const body = {
              repo: repo.full_name, provider: "github", pipeline: run.path,
              commit_sha: run.head_sha,
              branch: fork ? `${run.head_repository.owner.login}:${run.head_branch}` : run.head_branch,
              default_branch: repo.default_branch,
              ci_run_id: String(run.id), ci_run_attempt: run.run_attempt, jobs: results };
            for (let i = 1; i <= 5; i++) {
              const resp = await fetch(`${process.env.FLAKERADAR_URL}/api/ingest/pipeline`, {
                method: "POST",
                headers: { "Content-Type": "application/json", "X-API-Key": process.env.FLAKERADAR_TOKEN },
                body: JSON.stringify(body) });
              if (resp.ok) { core.info(`Queued report ${(await resp.json()).report_id}`); return; }
              if (resp.status < 500) { core.setFailed(`${resp.status} ${await resp.text()}`); return; }
              await new Promise(r => setTimeout(r, 2000 * i));
            }
            core.setFailed("FlakeRadar unreachable after 5 attempts");
```
Before you commit, check these against the current docs, using context7 or the GitHub docs, and fix the sample if any is wrong:
- `github.rest.actions.listJobsForWorkflowRunAttempt` is the Octokit method name.
- `workflow_run.path` exists in the event payload.
- `head_branch` can be null for some events. Fall back to `""`, and let the server's 422 show up in the log, or skip. Pick one and document it.

A 4xx response fails the reporter step, and that is visible in Actions. The reporter itself is never listed in `workflows:`, so it cannot report on itself.

### `samples/github-actions-snippet.yml` and README "CI integration"
Add the new query params to the JUnit curl:
```
&ci_job_id=${{ job.check_run_id }}&ci_run_attempt=${{ github.run_attempt }}
&default_branch=${{ github.event.repository.default_branch }}
&pipeline=<derived from GITHUB_WORKFLOW_REF>
```
Derive `pipeline` in bash: `PIPELINE="${GITHUB_WORKFLOW_REF#${GITHUB_REPOSITORY}/}"; PIPELINE="${PIPELINE%@*}"`. URL-encode the query with `curl -G --data-urlencode ...` for the values that can contain `/` or spaces, and keep the body as `--data-binary @junit.xml` sent to the URL built by `-G`. **Check** that `curl -G` together with `--data-binary` does what you expect (`-G` turns `-d`/`--data-urlencode` into query params, but `--data-binary` would also be moved into the query). If it doesn't, build the query string with `jq -rn --arg ... '@uri'` instead. Test the final command against a local server.

Update the parameter table with `default_branch`, `ci_job_id`, `ci_run_attempt` and `pipeline`.

### README (other sections)
- New section "**CI jobs**" after "CI integration". Cover:
  - what a Pipeline, a Job and a Job execution are;
  - the reporter workflow;
  - explained vs unexplained failures;
  - why every job needs `ci_job_id` on its JUnit upload;
  - other providers: POST the JSON contract, with a GitLab example that maps `CI_JOB_ID`, `CI_JOB_NAME`, `CI_PIPELINE_ID` and `CI_DEFAULT_BRANCH`. The GitLab example is not built or tested, so label it as a sketch.
- "How scoring works": add the Default-branch rule, and say that it applies to Tests and Jobs.
- "API" table: add `POST /api/ingest/pipeline`, `GET /api/jobs`, `GET /api/jobs/summary`, `GET /api/jobs/{id}/history`, and the new ingest params.
- Show the JSON contract as a documented example payload.

### `samples/simulate_ci.py`
For each simulated CI run, also POST a Pipeline report for `demo/shop` with `default_branch="main"`:
- `.github/workflows/ci.yml` with Jobs `backend`, `frontend` and `lint`;
- JUnit uploads carrying the matching `ci_job_id`;
- one Job, `e2e (ubuntu-latest)`, that fails about 20% of the time with **no** failing Tests (an infrastructure flake), plus one re-run attempt on the same SHA.

The script must still wait until every Report is processed.

## Acceptance Criteria
- [ ] `samples/flakeradar-jobs.yml` is valid YAML. If `actionlint` is available, run it on the file. If not, say so in the commit message.
- [ ] The updated curl snippet was run against a local FlakeRadar, and the Run has `ci_job_id`, `ci_run_attempt` and `pipeline` set.
- [ ] `python samples/simulate_ci.py` against a local instance produces Jobs, with `e2e (ubuntu-latest)` scoring above `backend`. Check it with `GET /api/jobs?repo=demo/shop&include_stable=true`.
- [ ] The README describes every new param and endpoint. No stale `ci_run_id=${{ github.run_id }}-${{ github.run_attempt }}` advice is left in text that now has `ci_run_attempt`. Keep `ci_run_id=${{ github.run_id }}`.

## Test Expectations
There are no new pytest tests. Validation is manual, against a running instance (see "Quick start (local development)" in the README). Record the commands you ran in the commit message.

## Dependencies
- Blocked by: 03, 05 (and 06 for the check with `/api/jobs`).
- Blocks: 08 (demo data), 11.

## Estimate / Risk
Small–medium. Risk 2: nothing in CI tests the sample files. The manual check is the only safety net.

## Validator Stopping Point
```bash
cd backend && .venv/bin/python -m pytest -q
# plus the manual simulator run described above
```
