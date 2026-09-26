# 08 — UI: Tests / Jobs toggle, Jobs leaderboard, and Job drawer

## Tracer-Bullet Outcome
The dashboard gets a **Tests / Jobs** toggle, stored in the URL as `?view=jobs`. The Jobs view shows Job stat tiles and a paginated Jobs leaderboard, using the same tier colours, score bar and sorts as Tests. Clicking a Job opens a slide-over (`?job=<id>`) that shows recent Job executions:
- outcome, where `explained` lists the failing Tests, each linking to that Test's drawer;
- short SHA, branch and attempt;
- runner name and labels;
- a link to the job page in CI.

It also shows the unexplained and explained failure counts. The Test drawer gains a "Seen in Jobs" line that links to each Job.

## User Story
As a maintainer, I want to switch from flaky tests to flaky jobs in one click, and to see at a glance whether a job's failures are tests or infrastructure.

## Context Pack
- Read `00-shared-context.md`.
- API (task 06): `GET /api/jobs`, `GET /api/jobs/summary` and `GET /api/jobs/{id}/history` return `JobPage`, `JobSummaryOut` and `JobHistoryOut`. `HistoryOut.jobs` is `TestJobLinkOut[]`.
- Frontend facts:
  - `src/urlState.ts`: `ViewState {repo, project, test, page, sort, showStable}`, `parseViewState`, `serializeViewState`, and `applyPatch`, which resets `page` when the list changes and clears `project` when `repo` changes.
  - `src/App.tsx` fetches repos, summary, tests and queue every 30 s, keeps only the newest refresh, and renders `ScopePicker`, `StatTiles`, `Leaderboard`, `Pagination` and `TestDrawer`.
  - `src/components/Leaderboard.tsx` has `scoreColor`, `TIER_CLASS` and `scopeLabel`. `TestDrawer.tsx` and `TestDetail.tsx` are the slide-over pattern (Escape closes it, scrim, `role="dialog"`).
  - Styling uses Tailwind 4 tokens that already exist (`text-signal`, `border-line`, `bg-surface`, `text-muted` …). Do not add new colours.
  - Tests use Vitest + Testing Library (`src/test/setup.ts`). There are examples in `Leaderboard.test.tsx`, `TestDrawer.test.tsx` and `urlState.test.ts`.
- Non-goals: quarantine for Jobs (there is none, so render no toggle); any new backend work.

## Implementation Contract

### URL state (`urlState.ts`)
- Add `view: "tests" | "jobs"` (default `"tests"`, serialized only as `view=jobs`) and `job: number | null` (`job=<id>`).
- In the Jobs view, `project` is ignored and hidden. Keep it in state, so that switching back restores it.
- `applyPatch`:
  - changing `view` resets `page` to 1 and clears `test` and `job`;
  - `test` and `job` are mutually exclusive, so setting one clears the other.
- Sort keys are shared (`score`, `last_seen`, `proven`).

### API client (`api.ts`)
Add the types `Job`, `JobPage`, `JobSummary`, `JobExecution`, `ExplainingTest`, `JobHistory` and `TestJobLink`, mirroring task 06's schemas exactly. Add `History.jobs: TestJobLink[]`, and the fetchers `fetchJobs(q)`, `fetchJobSummary(repo)` and `fetchJobHistory(id)`.

### Components
- `ViewToggle.tsx`: a two-button segmented control, "Tests" and "Jobs", with `aria-pressed`. Place it in the header next to `ScopePicker`. In the Jobs view, the `ScopePicker` shows the Repo select only (hide the project select).
- `JobStatTiles.tsx`: tiles for tracked jobs, flaky, suspect, proven flaky, and job executions recorded.
- `JobLeaderboard.tsx`: the columns are score (reuse the score bar and colours), tier, `pipeline · name` (show the Repo too when no Repo is selected), proven flakes, last status (reuse `StatusMark`) and last seen. Clicking a row selects the Job.
- `JobDrawer.tsx`: the slide-over. The header shows `repo · pipeline`, the Job name, and an issue link when there is one (reuse `IssueLink`). A summary line reads "N unexplained failures · M explained by tests (last K executions)". Then the executions table. Each explaining Test is a button that calls `setView({ test: id })`, which closes the Job drawer and opens the Test drawer.
- `TestDetail.tsx`: when `history.jobs` is not empty, render "Seen in Jobs:" with one link per Job, which calls `setView({ view: "jobs", job: id })`.
- `App.tsx`: branch the refresh and render on `view.view`. Keep the "newest refresh wins" guard. In the Jobs view, do not fetch Tests, and the other way round.
- The footer ingest hint: add a second line that points at `samples/flakeradar-jobs.yml`.

### Accessibility and layout
Keep the existing patterns. Dialogs have a label. The toggle can be operated with the keyboard. At 375 px width there is no horizontal page scroll (the leaderboard scrolls inside its own container, as the Test leaderboard does).

## Acceptance Criteria
- [ ] `?view=jobs&repo=acme/app&job=3` loads directly into the Jobs view with Job 3 open. Back and forward work.
- [ ] Switching the view resets the page and closes any drawer.
- [ ] The Jobs leaderboard hides stable Jobs unless "show stable" is on, and paginates.
- [ ] The Job drawer shows `explained` rows with their Tests, and clicking a Test opens that Test's drawer.
- [ ] The Test drawer shows "Seen in Jobs" and navigates to the Job.
- [ ] The Jobs view shows no project picker.

## Test Expectations
- `urlState.test.ts`: parse and serialize `view` and `job`, and the new `applyPatch` rules.
- New `JobLeaderboard.test.tsx` and `JobDrawer.test.tsx` (outcome tags, the explained Tests list, CI link, runner labels).
- `App.test.tsx`: toggling views calls the right fetchers (mock `fetch` as the existing tests do).
- `TestDrawer.test.tsx`: the "Seen in Jobs" link.

## Dependencies
- Blocked by: 06 (and 07 for demo data when checking manually).
- Blocks: 11.

## Estimate / Risk
Medium. Risk 2.

## Validator Stopping Point
```bash
cd frontend && npm run typecheck && npm run lint && npm run format:check && npm test && npm run build
```
Also run the app with the simulator data from task 07, and look at both views at desktop and phone widths.
