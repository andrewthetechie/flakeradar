# 05 — Likely-cause badge, filter and drawer column

## Tracer-Bullet Outcome
In the Tests view, each leaderboard row with a Failure category shows a small `likely: timing` badge. A **Likely cause** select next to "Sort by" filters the list (the choice lives in the URL as `?cause=timing`), and each option shows its count, for example `timing (3)`. The Test drawer shows the Test's likely cause, plus a **Cause** column in its execution table.

## User Story
As a maintainer on the dashboard, I want to filter flaky tests by likely cause and see the cause on each row, so that I can pick one kind of flake and fix those together.

## Description
Frontend only: types in `api.ts`, a new URL key in `urlState.ts`, an optional select in `LeaderboardControls.tsx`, wiring in `App.tsx`, a badge in `Leaderboard.tsx`, a stat and a column in `TestDetail.tsx`, and fixture updates.

## Context Pack
- Source decisions: `00-shared-context.md`, "Failure categories". UI text says "likely cause", never "root cause" (CONTEXT.md).
- Repo facts (API after tasks 03–04):
  - Each `TestOut` has `failure_category: "network" | "environment" | "timing" | "assertion" | "other" | null`. Each history execution has `failure_category` (same values, or `null`).
  - `GET /api/tests` accepts `category=<one of the five>`.
  - `GET /api/summary` returns `category_counts: {network, environment, timing, assertion, other}` (always all five keys): flaky + suspect Tests per category.
  - `frontend/src/api.ts` today has `TestCase`, `Execution` (with `attempt` from task 02), `Summary`, `TestQuery extends Scope { page; pageSize; sort; showStable }`, and:
    ```ts
    export function fetchTests(q: TestQuery): Promise<TestPage> {
      const params = scopeParams(q);
      params.set("page", String(q.page));
      params.set("page_size", String(q.pageSize));
      params.set("sort", q.sort);
      if (q.showStable) params.set("include_stable", "true");
      return getJson<TestPage>(withQuery("/api/tests", params));
    }
    ```
  - `frontend/src/urlState.ts` today:
    ```ts
    export interface ViewState {
      repo: string | null; project: string | null; test: number | null; job: number | null;
      page: number; sort: SortKey; showStable: boolean; view: ViewKind;
    }
    export const DEFAULT_VIEW: ViewState = {
      repo: null, project: null, test: null, job: null, page: 1, sort: "score", showStable: false, view: "tests",
    };
    // parseViewState(search): reads repo, project, test, job, page, sort, stable, view
    // serializeViewState(s): writes repo, project, view, test, job, page, sort, stable (in that order), defaults omitted
    // applyPatch(prev, patch): ... const relisted = (["repo", "project", "sort", "showStable"] as const).some(
    //     (k) => k in patch && patch[k] !== prev[k]); if (relisted && !("page" in patch)) next.page = 1;
    ```
  - `frontend/src/components/LeaderboardControls.tsx` today takes `{ sort, showStable, stableLabel?, onSort, onShowStable }` and renders a `<label className="flex items-center gap-2">Sort by <select className="control py-0.5 text-xs" …>` followed by the stable checkbox. Copy that markup for the new select.
  - `App.tsx` renders the controls like this, and calls `fetchTests({ ...scope, page: pageNumber, pageSize: PAGE_SIZE, sort, showStable })` inside `refresh`. `refresh` is a `useCallback` with deps `[kind, repo, project, pageNumber, sort, showStable]`. Summary state is `summary: Summary | null`.
    ```tsx
    <LeaderboardControls
      sort={sort}
      showStable={showStable}
      stableLabel={kind === "jobs" ? "Show stable jobs" : "Show stable tests"}
      onSort={(s) => setView({ sort: s })}
      onShowStable={(v) => setView({ showStable: v })}
    />
    ```
  - `Leaderboard.tsx`, the Flakiness cell ends with:
    ```tsx
    <span className={`mt-1 inline-block text-xs ${TIER_CLASS[t.tier]}`}>{t.tier}</span>
    ```
  - `TestDetail.tsx` has a stat row of blocks shaped like `<div><div className="text-xs text-muted">Proven flakes</div><div className="text-lg font-semibold tabular-nums …">…</div></div>`, and an execution table with headers `Status, Commit, Branch, When, Message`.
  - Fixtures to update (every object of the changed types): `App.test.tsx`, `components/Leaderboard.test.tsx`, `components/TestDrawer.test.tsx`, `components/StatTiles.test.tsx`, `urlState.test.ts` (full `ViewState` objects).
- Non-goals: backend changes; a category for Jobs; charts of categories; changing StatTiles.

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `feat/flake-insights` after this draft

## Implementation Contract
- Expected files: `frontend/src/api.ts`, `frontend/src/urlState.ts`, `frontend/src/urlState.test.ts`, `frontend/src/components/LeaderboardControls.tsx`, `frontend/src/App.tsx`, `frontend/src/components/Leaderboard.tsx`, `frontend/src/components/Leaderboard.test.tsx`, `frontend/src/components/TestDetail.tsx`, `frontend/src/components/TestDrawer.test.tsx`, `frontend/src/App.test.tsx`, `frontend/src/components/StatTiles.test.tsx`.
- Interfaces and names:
  - `api.ts`:
    ```ts
    export type FailureCategory = "network" | "environment" | "timing" | "assertion" | "other";
    export const FAILURE_CATEGORIES: readonly FailureCategory[] = ["network", "environment", "timing", "assertion", "other"];
    // TestCase:   failure_category: FailureCategory | null;
    // Execution:  failure_category: FailureCategory | null;
    // Summary:    category_counts: Record<FailureCategory, number>;
    // TestQuery:  category: FailureCategory | null;
    ```
    In `fetchTests`, add `if (q.category) params.set("category", q.category);`.
  - `urlState.ts`: `ViewState` gains `cause: FailureCategory | null` (default `null`). `parseViewState` reads `p.get("cause")` and keeps it only if it is in `FAILURE_CATEGORIES`. `serializeViewState` writes `cause` **last**, after `stable`: `if (s.cause) p.set("cause", s.cause);`. `applyPatch` adds `"cause"` to the `relisted` keys, so a change resets `page` to 1. Switching `view` does **not** clear `cause`.
  - `LeaderboardControls.tsx` gains optional props:
    ```ts
    cause?: FailureCategory | null;
    causeCounts?: Record<FailureCategory, number> | null;
    onCause?: (cause: FailureCategory | null) => void;
    ```
    When `onCause` is given, render (between "Sort by" and the checkbox):
    ```tsx
    <label className="flex items-center gap-2">
      Likely cause
      <select className="control py-0.5 text-xs" value={cause ?? ""} onChange={(e) => onCause((e.target.value || null) as FailureCategory | null)}>
        <option value="">Any</option>
        {FAILURE_CATEGORIES.map((c) => (
          <option key={c} value={c}>{causeCounts ? `${c} (${causeCounts[c]})` : c}</option>
        ))}
      </select>
    </label>
    ```
  - `App.tsx`: destructure `cause` from `view`. Pass `category: cause` in the `fetchTests` call and add `cause` to `refresh`'s dependency list. Render the controls with `cause={cause}`, `causeCounts={summary?.category_counts ?? null}` and `onCause={kind === "tests" ? (c) => setView({ cause: c }) : undefined}`.
  - `Leaderboard.tsx`: after the tier span, add
    ```tsx
    {t.failure_category && (
      <span className="mt-1 ml-2 inline-block rounded border border-line px-1 text-[11px] text-text-2" title="Likely cause, from failure messages">
        likely: {t.failure_category}
      </span>
    )}
    ```
  - `TestDetail.tsx`: add a stat block `Likely cause` whose value is `history.test.failure_category ?? "—"` (same markup as the other stats, without `tabular-nums`). Add a `Cause` column after `Status` in the table: header `<th className="py-1.5 pr-3 font-medium">Cause</th>`, cell `<td className="text-text-2">{e.failure_category ?? "—"}</td>`.
- Verified external contracts: None.
- Behavior rules: in the Jobs view the select is hidden (`onCause` is undefined) and `cause` stays in the URL, unused. The `cause` filter combines with sort, stable and pagination.
- Error and security rules: an unknown `?cause=` value parses to `null` (no error).

## Acceptance Criteria
- [ ] `parseViewState("?cause=timing").cause === "timing"`, `parseViewState("?cause=nope").cause === null`, and `serializeViewState({ ...DEFAULT_VIEW, showStable: true, cause: "network" }) === "?stable=1&cause=network"`.
- [ ] `applyPatch({ ...DEFAULT_VIEW, page: 4 }, { cause: "timing" }).page === 1`.
- [ ] `fetchTests` sends `category=timing` when `category: "timing"`, and sends no `category` when it is `null`.
- [ ] A leaderboard row with `failure_category: "timing"` shows the text `likely: timing`, and a row with `null` shows no badge.
- [ ] The Test drawer shows `Likely cause` with the Test's category and a `Cause` column.
- [ ] All frontend checks pass.

## Test Expectations
- Framework: Vitest + Testing Library. Run: `cd frontend && npm test`.
- `urlState.test.ts`: add `cause: null` to the full view objects in the round-trip tests. Add `it("parses, validates and serializes cause", …)` with the three literals above, and add `expect(applyPatch(base, { cause: "timing" }).page).toBe(1)` to the page-reset test.
- `components/Leaderboard.test.tsx`: set the fixture's `failure_category: "timing"` and assert `screen.getByText("likely: timing")`. Add a second render with `failure_category: null` and assert `screen.queryByText(/likely:/)` is null.
- `components/TestDrawer.test.tsx`: add `failure_category` to the fixture Test (`"assertion"`) and executions (`null` for passed, `"assertion"` for failed). Assert `screen.getByText("Likely cause")` and that `"assertion"` appears (use `getAllByText("assertion").length > 0`).
- `App.test.tsx` and `components/StatTiles.test.tsx`: add `category_counts: { network: 0, environment: 0, timing: 0, assertion: 0, other: 0 }` to every `Summary` fixture, and `failure_category: null` to every `TestCase`/`Execution` fixture.

## Dependencies
- Blocked by: 04.
- Why blocked: 04 adds the `category` query parameter and `category_counts` (03 adds `failure_category`, and 04 depends on 03).
- Blocks: 09.

## Labels
`feature`, `frontend`, `priority:medium`

## Estimate
Medium

## Risk
2 - Touches the URL state shape; the round-trip tests guard it.

## Validator Stopping Point
```bash
cd frontend && npm run typecheck && npm run lint && npm run format:check && npm test
```
