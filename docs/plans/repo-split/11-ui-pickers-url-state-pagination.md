# 11 — UI: pickers, URL state, paginated leaderboard

## Tracer-Bullet Outcome
The dashboard at `/` has a **Repo** picker and a **Project** picker. The Project picker is disabled until a Repo is chosen, and it lists only that Repo's Projects.

The leaderboard:
- is paginated (50 per page, "Page 2 of 3 · 51–100 of 123");
- sorts by flakiness score, last seen or proven flakes;
- hides stable tests unless "Show stable tests" is ticked;
- labels each row with `repo · project` when viewing all repos, or with `project` when viewing a whole repo;
- shows a tier (`flaky`/`suspect`/`stable`) under each score.

The whole view (repo, project, selected test, page, sort, stable toggle) lives in the URL, e.g. `/?repo=andrewthetechie%2Fwriters-app&project=backend&page=2`. Views can be bookmarked and shared, and the back button works.

Vitest + Testing Library are set up, and `npm test` runs them.

## User Story
As someone on the dashboard, I want to narrow the leaderboard to one Repo or Repo/Project and page through it, so that I only see the flaky tests I care about and can send a link to exactly that view.

## Description
This makes the frontend match the new API from tasks 08 and 09 and fixes the "All projects shows flaky tests, one project shows none" confusion:
- The old leaderboard listed every Test, including score 0, so a filtered view looked empty of flaky tests while "All" showed a few. It now shows only Flaky and Suspect Tests by default, and the empty state explains the toggle.
- Rows from different Repos are now labeled.

The detail panel stays in the right-hand column for now (task 12 replaces it with a slide-over). It only needs its types updated and one label renamed.

## Context Pack
- Source decisions:
  - Pickers: Repo, then Project (Project only after Repo).
  - Each row carries a `repo · project` label in the "All" views.
  - URL query-string state with **no router library**.
  - Server-side pagination (page size 50).
  - Sortable by score, last seen and proven flakes (not by name).
  - Default view hides stable tests; the toggle shows all.
  - Add Vitest + Testing Library now, covering the new logic only (no backfill for existing components).
- API contract (from tasks 08 and 09; the backend is already in place):
  - `GET /api/repos` → `[{"name": "andrewthetechie/writers-app", "projects": [{"name": "backend", "root": ""}]}]`.
  - `GET /api/tests?repo=&project=&include_stable=true&sort=score|last_seen|proven&page=1&page_size=50` → `{"items": TestCase[], "total", "page", "page_size"}`.
  - `GET /api/summary?repo=&project=` → adds `suspect_tests`.
  - `GET /api/tests/{id}/history?limit=60` → `{test, location, last_failing_sha, last_failing_branch, executions}`.
  - `POST /api/tests/{id}/quarantine`.
  - `/api/projects` **no longer exists**.
- Repo facts (current frontend):
  - `frontend/src/api.ts`: old types (`TestCase.project`, no `repo`/`tier`/`file`) and `fetchProjects()`.
  - `frontend/src/App.tsx`: one `<select className="project-select">`, fetching `fetchTests(project)` as an array.
  - `frontend/src/components/Leaderboard.tsx`: props `{ tests, selectedId, onSelect, onToggleQuarantine }`.
  - `frontend/src/components/StatTiles.tsx`: 5 tiles.
  - `frontend/src/components/TestDetail.tsx`: a facts block with `<div className="label">Same-SHA flips</div>`.
  - `frontend/package.json` scripts are `dev`, `build` (`tsc -b && vite build`) and `preview`.
  - `tsconfig.json` has `strict`, `noUnusedLocals` and `noUnusedParameters`, and `include: ["src"]`, so test files are type-checked by `npm run build` too.
  - The lockfile has vite 6.4.3 and typescript 5.9.3.
- Verified external contracts: vitest 5.0.1 accepts vite `^6.4.0`. `@testing-library/jest-dom@7.0.1` exports `./vitest` (verified in its `package.json` `exports`). The whole setup below was run: 14 tests passed, and `npm run build` succeeded.
- Non-goals: the slide-over detail, permalink UI and name wrapping (12); the queue indicator (13); React Router; frontend tests for untouched components.

## Delivery Strategy
- Shape: Wide refactor: Migrate (integration branch). After this task, `npm test` and `npm run build` pass again.
- Valid-state scope: Named integration branch `feat/repo-split`.

## Implementation Contract
- Expected files:
  - Install the dev dependencies (updates `package.json` and `package-lock.json`):
    `cd frontend && npm install -D vitest@^5.0.1 jsdom@^30.1.1 @testing-library/react@^16.3.3 @testing-library/dom@^10.4.0 @testing-library/jest-dom@^7.0.1 @testing-library/user-event@^14.6.7`
  - In `frontend/package.json` `scripts`, add `"test": "vitest run"`.
  - **Replace:** `frontend/vite.config.ts`, `frontend/src/api.ts`, `frontend/src/App.tsx`, `frontend/src/components/Leaderboard.tsx` and `frontend/src/components/StatTiles.tsx`.
  - **Create:** `frontend/src/test/setup.ts`, `frontend/src/urlState.ts`, `frontend/src/components/ScopePicker.tsx`, `frontend/src/components/Pagination.tsx`, `frontend/src/components/LeaderboardControls.tsx`, and the 4 test files below.
  - **Edit:** `frontend/src/components/TestDetail.tsx` (change the label `Same-SHA flips` to `Proven flakes`) and `frontend/src/styles.css` (delete the line `.project-select { margin-left: auto; padding: 4px 8px; }` and append the block below).
- `frontend/vite.config.ts`:
```ts
/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/api": "http://localhost:8000",
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
  },
});
```
- `frontend/src/test/setup.ts`:
```ts
// Vitest setup: jest-dom matchers (toBeInTheDocument, toBeDisabled, ...) and
// DOM cleanup between tests (not automatic without Vitest globals).
import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => cleanup());
```
- `frontend/src/api.ts` (the typed contract; mirrors `backend/app/schemas.py`):
```ts
// Typed contract with the FastAPI backend (mirrors backend/app/schemas.py).

export type Tier = "flaky" | "suspect" | "stable";
export type SortKey = "score" | "last_seen" | "proven";

export interface TestCase {
  id: number;
  repo: string;
  project: string;
  fingerprint: string;
  suite: string;
  classname: string;
  name: string;
  file: string | null;
  line: number | null;
  flakiness_score: number;
  tier: Tier;
  confirmed_flake_count: number;
  last_status: string;
  last_seen_at: string;
  quarantined: boolean;
  quarantined_at: string | null;
  github_issue_number: number | null;
}

export interface TestPage {
  items: TestCase[];
  total: number;
  page: number;
  page_size: number;
}

export interface ProjectInfo {
  name: string;
  root: string;
}

export interface RepoInfo {
  name: string;
  projects: ProjectInfo[];
}

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

export interface Location {
  path: string;
  line: number | null;
  url: string | null;
}

export interface History {
  test: TestCase;
  location: Location | null;
  last_failing_sha: string | null;
  last_failing_branch: string | null;
  executions: Execution[];
}

export interface Summary {
  total_tests: number;
  flaky_tests: number;
  suspect_tests: number;
  confirmed_flaky_tests: number;
  total_runs: number;
  total_executions: number;
  flake_threshold: number;
}

/** Which Tests a view covers. `project` is only meaningful with `repo`. */
export interface Scope {
  repo: string | null;
  project: string | null;
}

export interface TestQuery extends Scope {
  page: number;
  pageSize: number;
  sort: SortKey;
  showStable: boolean;
}

async function getJson<T>(url: string): Promise<T> {
  const resp = await fetch(url);
  if (!resp.ok) throw new Error(`${url} -> ${resp.status}`);
  return resp.json() as Promise<T>;
}

export function scopeParams(scope: Scope): URLSearchParams {
  const params = new URLSearchParams();
  if (scope.repo) {
    params.set("repo", scope.repo);
    if (scope.project) params.set("project", scope.project);
  }
  return params;
}

function withQuery(path: string, params: URLSearchParams): string {
  const q = params.toString();
  return q ? `${path}?${q}` : path;
}

export const fetchRepos = () => getJson<RepoInfo[]>("/api/repos");

export const fetchSummary = (scope: Scope) =>
  getJson<Summary>(withQuery("/api/summary", scopeParams(scope)));

export function fetchTests(q: TestQuery): Promise<TestPage> {
  const params = scopeParams(q);
  params.set("page", String(q.page));
  params.set("page_size", String(q.pageSize));
  params.set("sort", q.sort);
  if (q.showStable) params.set("include_stable", "true");
  return getJson<TestPage>(withQuery("/api/tests", params));
}

export const fetchHistory = (id: number) =>
  getJson<History>(`/api/tests/${id}/history?limit=60`);

export async function setQuarantine(id: number, quarantined: boolean): Promise<TestCase> {
  const resp = await fetch(`/api/tests/${id}/quarantine`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ quarantined }),
  });
  if (!resp.ok) throw new Error(`quarantine ${id} -> ${resp.status}`);
  return resp.json() as Promise<TestCase>;
}
```
- `frontend/src/urlState.ts`:
```ts
// The dashboard's view lives in the URL query string so any view can be
// bookmarked or pasted into an issue / agent prompt.
//   ?repo=owner/name&project=backend&test=42&page=2&sort=proven&stable=1
import { useCallback, useEffect, useRef, useState } from "react";
import type { SortKey } from "./api";

export interface ViewState {
  repo: string | null;
  project: string | null;
  test: number | null;
  page: number;
  sort: SortKey;
  showStable: boolean;
}

export const DEFAULT_VIEW: ViewState = {
  repo: null, project: null, test: null, page: 1, sort: "score", showStable: false,
};

const SORTS: readonly SortKey[] = ["score", "last_seen", "proven"];

function positiveInt(raw: string | null): number | null {
  if (raw == null || !/^\d+$/.test(raw)) return null;
  const n = Number(raw);
  return n >= 1 ? n : null;
}

export function parseViewState(search: string): ViewState {
  const p = new URLSearchParams(search);
  const repo = p.get("repo") || null;
  const sort = p.get("sort");
  return {
    repo,
    project: repo ? p.get("project") || null : null,
    test: positiveInt(p.get("test")),
    page: positiveInt(p.get("page")) ?? 1,
    sort: SORTS.includes(sort as SortKey) ? (sort as SortKey) : "score",
    showStable: p.get("stable") === "1",
  };
}

/** "?repo=…" (defaults omitted), or "" for the default view. */
export function serializeViewState(s: ViewState): string {
  const p = new URLSearchParams();
  if (s.repo) {
    p.set("repo", s.repo);
    if (s.project) p.set("project", s.project);
  }
  if (s.test != null) p.set("test", String(s.test));
  if (s.page !== 1) p.set("page", String(s.page));
  if (s.sort !== "score") p.set("sort", s.sort);
  if (s.showStable) p.set("stable", "1");
  const q = p.toString();
  return q ? `?${q}` : "";
}

/** Merge a change into the view. Changing what is listed resets to page 1;
 *  changing the repo also clears the project (names overlap across repos). */
export function applyPatch(prev: ViewState, patch: Partial<ViewState>): ViewState {
  const next = { ...prev, ...patch };
  if ("repo" in patch && patch.repo !== prev.repo && !("project" in patch)) {
    next.project = null;
  }
  if (!next.repo) next.project = null;
  const relisted = (["repo", "project", "sort", "showStable"] as const).some(
    (k) => k in patch && patch[k] !== prev[k],
  );
  if (relisted && !("page" in patch)) next.page = 1;
  return next;
}

export function useViewState(): [ViewState, (patch: Partial<ViewState>) => void] {
  const [view, setView] = useState<ViewState>(() => parseViewState(window.location.search));
  // Latest view for update(): keeps the pushState side effect out of setState.
  const viewRef = useRef(view);

  useEffect(() => {
    const onPop = () => {
      viewRef.current = parseViewState(window.location.search);
      setView(viewRef.current);
    };
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  const update = useCallback((patch: Partial<ViewState>) => {
    const next = applyPatch(viewRef.current, patch);
    viewRef.current = next;
    const search = serializeViewState(next);
    if (search !== window.location.search) {
      window.history.pushState(null, "", search || window.location.pathname);
    }
    setView(next);
  }, []);

  return [view, update];
}
```
- `frontend/src/components/ScopePicker.tsx`:
```tsx
import type { RepoInfo, Scope } from "../api";

/** Repo picker, then Project picker (enabled once a Repo is chosen:
 *  project names like "backend" repeat across repos). */
export function ScopePicker({
  repos, scope, onChange,
}: {
  repos: RepoInfo[];
  scope: Scope;
  onChange: (scope: Scope) => void;
}) {
  const projects = repos.find((r) => r.name === scope.repo)?.projects ?? [];
  return (
    <div className="scope-picker">
      <select
        aria-label="Repo"
        value={scope.repo ?? ""}
        onChange={(e) => onChange({ repo: e.target.value || null, project: null })}
      >
        <option value="">All repos</option>
        {repos.map((r) => <option key={r.name} value={r.name}>{r.name}</option>)}
      </select>
      <select
        aria-label="Project"
        value={scope.project ?? ""}
        disabled={!scope.repo}
        onChange={(e) => onChange({ repo: scope.repo, project: e.target.value || null })}
      >
        <option value="">All projects</option>
        {projects.map((p) => <option key={p.name} value={p.name}>{p.name}</option>)}
      </select>
    </div>
  );
}
```
- `frontend/src/components/Pagination.tsx`:
```tsx
export function Pagination({
  page, pageSize, total, onPage,
}: {
  page: number;
  pageSize: number;
  total: number;
  onPage: (page: number) => void;
}) {
  const pages = Math.max(1, Math.ceil(total / pageSize));
  const first = total === 0 ? 0 : (page - 1) * pageSize + 1;
  const last = Math.min(page * pageSize, total);
  return (
    <nav className="pagination" aria-label="Leaderboard pages">
      <button type="button" onClick={() => onPage(page - 1)} disabled={page <= 1}>
        ‹ Prev
      </button>
      <span className="pagination-status">
        Page {page} of {pages} · {first}–{last} of {total}
      </span>
      <button type="button" onClick={() => onPage(page + 1)} disabled={page >= pages}>
        Next ›
      </button>
    </nav>
  );
}
```
- `frontend/src/components/LeaderboardControls.tsx`:
```tsx
import type { SortKey } from "../api";

const SORT_LABELS: Record<SortKey, string> = {
  score: "Flakiness score",
  last_seen: "Last seen",
  proven: "Proven flakes",
};

export function LeaderboardControls({
  sort, showStable, onSort, onShowStable,
}: {
  sort: SortKey;
  showStable: boolean;
  onSort: (sort: SortKey) => void;
  onShowStable: (show: boolean) => void;
}) {
  return (
    <div className="leaderboard-controls">
      <label>
        Sort by{" "}
        <select value={sort} onChange={(e) => onSort(e.target.value as SortKey)}>
          {(Object.keys(SORT_LABELS) as SortKey[]).map((k) => (
            <option key={k} value={k}>{SORT_LABELS[k]}</option>
          ))}
        </select>
      </label>
      <label>
        <input
          type="checkbox"
          checked={showStable}
          onChange={(e) => onShowStable(e.target.checked)}
        />{" "}
        Show stable tests
      </label>
    </div>
  );
}
```
- `frontend/src/components/Leaderboard.tsx`:
```tsx
import type { Scope, TestCase } from "../api";
import { StatusMark } from "./StatusMark";

// Sequential blue: darker = worse, so severity reads as magnitude.
function scoreColor(score: number): string {
  if (score >= 0.75) return "var(--seq-650)";
  if (score >= 0.5) return "var(--seq-550)";
  if (score >= 0.25) return "var(--seq-400)";
  return "var(--seq-250)";
}

/** Where a row lives, shown only when the view spans more than that. */
export function scopeLabel(t: TestCase, scope: Scope): string | null {
  if (!scope.repo) return `${t.repo} · ${t.project}`;
  if (!scope.project) return t.project;
  return null;
}

export function Leaderboard({
  tests, scope, showStable, selectedId, onSelect, onToggleQuarantine,
}: {
  tests: TestCase[];
  scope: Scope;
  showStable: boolean;
  selectedId: number | null;
  onSelect: (id: number) => void;
  onToggleQuarantine: (t: TestCase) => void;
}) {
  if (tests.length === 0) {
    return showStable ? (
      <div className="empty">
        No test data yet. POST a JUnit XML report to <code>/api/ingest</code> — see
        the README for the one-line CI snippet.
      </div>
    ) : (
      <div className="empty">
        No flaky or suspect tests in this view. Tick “Show stable tests” to list every test.
      </div>
    );
  }
  return (
    <table className="leaderboard">
      <thead>
        <tr>
          <th>Test</th>
          <th>Flakiness</th>
          <th>Proof</th>
          <th>Last status</th>
          <th>Quarantine</th>
        </tr>
      </thead>
      <tbody>
        {tests.map((t) => {
          const label = scopeLabel(t, scope);
          return (
            <tr
              key={t.id}
              className={t.id === selectedId ? "selected" : ""}
              onClick={() => onSelect(t.id)}
            >
              <td>
                {label && <div className="test-scope">{label}</div>}
                <div className="test-name">
                  {t.quarantined && (
                    <span className="badge-quarantined" title="Quarantined — runner may skip">
                      ⏻ quarantined
                    </span>
                  )}
                  {t.name}
                </div>
                <div className="test-class">{t.classname || t.suite}</div>
              </td>
              <td>
                <div className="meter">
                  <div className="track">
                    <div
                      className="fill"
                      style={{
                        width: `${Math.max(t.flakiness_score * 100, t.flakiness_score > 0 ? 4 : 0)}%`,
                        background: scoreColor(t.flakiness_score),
                      }}
                    />
                  </div>
                  <span className="num">{t.flakiness_score.toFixed(2)}</span>
                </div>
                <span className={`tier tier-${t.tier}`}>{t.tier}</span>
              </td>
              <td>
                {t.confirmed_flake_count > 0 ? (
                  <span className="chip" title="Failed and passed on the same commit">
                    ⚠ {t.confirmed_flake_count}× proven
                  </span>
                ) : (
                  <span className="chip">—</span>
                )}
              </td>
              <td>
                <span className="chip">
                  <StatusMark status={t.last_status} /> {t.last_status}
                  {t.github_issue_number != null && (
                    <span className="badge-issue">#{t.github_issue_number}</span>
                  )}
                </span>
              </td>
              <td>
                <button
                  className="quarantine-btn"
                  onClick={(e) => { e.stopPropagation(); onToggleQuarantine(t); }}
                >
                  {t.quarantined ? "Un-quarantine" : "Quarantine"}
                </button>
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
```
- `frontend/src/components/StatTiles.tsx`:
```tsx
import type { Summary } from "../api";

export function StatTiles({ summary }: { summary: Summary }) {
  const tiles = [
    { label: "Tracked tests", value: summary.total_tests, alert: false },
    {
      label: `Flaky (score ≥ ${summary.flake_threshold})`,
      value: summary.flaky_tests,
      alert: summary.flaky_tests > 0,
    },
    { label: "Suspect (score > 0)", value: summary.suspect_tests, alert: false },
    {
      label: "Proven flaky (same-commit flip)",
      value: summary.confirmed_flaky_tests,
      alert: summary.confirmed_flaky_tests > 0,
    },
    { label: "CI runs ingested", value: summary.total_runs, alert: false },
    { label: "Executions recorded", value: summary.total_executions, alert: false },
  ];
  return (
    <div className="tiles">
      {tiles.map((t) => (
        <div className="tile" key={t.label}>
          <div className="label">{t.label}</div>
          <div className={t.alert ? "value alert" : "value"}>{t.value}</div>
        </div>
      ))}
    </div>
  );
}
```
- `frontend/src/App.tsx`:
```tsx
import { useCallback, useEffect, useState } from "react";
import {
  fetchHistory, fetchRepos, fetchSummary, fetchTests, setQuarantine,
  type History, type RepoInfo, type Summary, type TestPage, type TestCase,
} from "./api";
import { Leaderboard } from "./components/Leaderboard";
import { LeaderboardControls } from "./components/LeaderboardControls";
import { Pagination } from "./components/Pagination";
import { ScopePicker } from "./components/ScopePicker";
import { StatTiles } from "./components/StatTiles";
import { TestDetail } from "./components/TestDetail";
import { useViewState } from "./urlState";

const REFRESH_MS = 30_000;
const PAGE_SIZE = 50;

export default function App() {
  const [view, setView] = useViewState();
  const [repos, setRepos] = useState<RepoInfo[]>([]);
  const [summary, setSummary] = useState<Summary | null>(null);
  const [page, setPage] = useState<TestPage | null>(null);
  const [history, setHistory] = useState<History | null>(null);
  const [error, setError] = useState<string | null>(null);

  const { repo, project, sort, showStable } = view;
  const pageNumber = view.page;

  const refresh = useCallback(async () => {
    const scope = { repo, project };
    try {
      const [r, s, t] = await Promise.all([
        fetchRepos(),
        fetchSummary(scope),
        fetchTests({ ...scope, page: pageNumber, pageSize: PAGE_SIZE, sort, showStable }),
      ]);
      setRepos(r);
      setSummary(s);
      setPage(t);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [repo, project, pageNumber, sort, showStable]);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), REFRESH_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  useEffect(() => {
    if (view.test == null) {
      setHistory(null);
      return;
    }
    let cancelled = false;
    fetchHistory(view.test)
      .then((h) => { if (!cancelled) setHistory(h); })
      .catch((e) => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; };
  }, [view.test, page]); // re-fetch when the leaderboard refreshes

  const onToggleQuarantine = useCallback(async (t: TestCase) => {
    try {
      await setQuarantine(t.id, !t.quarantined);
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [refresh]);

  return (
    <div className="app">
      <header className="app-header">
        <h1>FlakeRadar</h1>
        <span className="tagline">flaky-test detection for your CI</span>
        <ScopePicker
          repos={repos}
          scope={{ repo, project }}
          onChange={(scope) => setView(scope)}
        />
      </header>

      {error && (
        <div className="error-banner">
          Could not reach the FlakeRadar API ({error}). Is the backend running on port 8000?
        </div>
      )}

      {summary && <StatTiles summary={summary} />}

      <div className="columns">
        <section className="panel">
          <div className="panel-head">
            <h2>Flakiness leaderboard</h2>
            <LeaderboardControls
              sort={sort}
              showStable={showStable}
              onSort={(s) => setView({ sort: s })}
              onShowStable={(v) => setView({ showStable: v })}
            />
          </div>
          <Leaderboard
            tests={page?.items ?? []}
            scope={{ repo, project }}
            showStable={showStable}
            selectedId={view.test}
            onSelect={(id) => setView({ test: id })}
            onToggleQuarantine={onToggleQuarantine}
          />
          {page && page.total > 0 && (
            <Pagination
              page={page.page}
              pageSize={page.page_size}
              total={page.total}
              onPage={(p) => setView({ page: p })}
            />
          )}
        </section>
        <section className="panel">
          <h2>Test detail</h2>
          <TestDetail history={history} />
        </section>
      </div>

      <footer className="app-footer">
        Ingest from CI:{" "}
        <code>
          curl -X POST "$URL/api/ingest?repo=$OWNER/$REPO&amp;project=backend&amp;commit_sha=$SHA&amp;branch=$BRANCH"
          -H "X-API-Key: $TOKEN" --data-binary @junit.xml
        </code>
      </footer>
    </div>
  );
}
```
- Append to `frontend/src/styles.css`:
```css
/* ---- scope + leaderboard controls (task 11) ---- */
.scope-picker { margin-left: auto; display: flex; gap: 8px; flex-wrap: wrap; }
.scope-picker select { padding: 4px 8px; max-width: 280px; }
.panel-head { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; flex-wrap: wrap; margin-bottom: 8px; }
.panel-head h2 { margin: 0; }
.leaderboard-controls { display: flex; gap: 14px; flex-wrap: wrap; font-size: 12px; color: var(--text-secondary); }
.test-scope { font-size: 11px; color: var(--accent); overflow-wrap: anywhere; }
.tier { display: inline-block; margin-top: 2px; font-size: 11px; color: var(--text-secondary); }
.tier-flaky { color: var(--status-critical); font-weight: 600; }
.pagination { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-top: 12px; font-size: 12px; color: var(--text-secondary); }
.pagination button { cursor: pointer; }
.pagination button:disabled { cursor: default; opacity: 0.5; }
```
- Behavior rules:
  - URL keys: `repo`, `project`, `test`, `page`, `sort`, `stable=1`. Defaults are omitted. Invalid values fall back to defaults. `project` without `repo` is ignored.
  - `applyPatch`:
    - changing `repo` clears `project` (unless the patch sets it) and resets `page` to 1;
    - changing `project`, `sort` or `showStable` resets `page` to 1;
    - selecting a test or setting the page keeps everything else.
  - Every view change is a `history.pushState`, and `popstate` restores the view.
  - Pagination is shown only when `total > 0`.
- Error and security rules: none (read-only UI; the error banner is unchanged).

## Acceptance Criteria
- [ ] `npm test` passes (4 files, 14 tests), and `npm run build` succeeds.
- [ ] `serializeViewState` of the full view gives exactly `?repo=andrewthetechie%2Fwriters-app&project=backend&test=42&page=3&sort=proven&stable=1`, and parsing it gives the same view back.
- [ ] The Project `<select>` (label "Project") is disabled with no Repo. Choosing a Repo calls `onChange({repo, project: null})`.
- [ ] Pagination renders `Page 2 of 3 · 51–100 of 123`, and Next calls `onPage(3)`.
- [ ] In the "All repos" view, a row shows `andrewthetechie/writers-app · backend`.
- [ ] Manual: with the backend running, `npm run dev`, then pick a Repo → the URL gains `?repo=…` and the leaderboard and tiles narrow. The browser Back button restores "All repos".

## Test Expectations
- Framework: Vitest 5 (jsdom environment) + @testing-library/react + user-event + jest-dom. Run with `cd frontend && npm test`.
- `frontend/src/urlState.test.ts`:
```ts
import { describe, expect, it } from "vitest";
import { DEFAULT_VIEW, applyPatch, parseViewState, serializeViewState } from "./urlState";

describe("view state in the URL", () => {
  it("round-trips a full view", () => {
    const view = {
      repo: "andrewthetechie/writers-app", project: "backend", test: 42,
      page: 3, sort: "proven" as const, showStable: true,
    };
    const search = serializeViewState(view);
    expect(search).toBe(
      "?repo=andrewthetechie%2Fwriters-app&project=backend&test=42&page=3&sort=proven&stable=1",
    );
    expect(parseViewState(search)).toEqual(view);
  });

  it("omits defaults and parses an empty query to the default view", () => {
    expect(serializeViewState(DEFAULT_VIEW)).toBe("");
    expect(parseViewState("")).toEqual(DEFAULT_VIEW);
  });

  it("ignores invalid values and a project without a repo", () => {
    expect(parseViewState("?project=backend&page=0&test=abc&sort=name&stable=yes"))
      .toEqual(DEFAULT_VIEW);
  });
});

describe("applyPatch", () => {
  const base = { ...DEFAULT_VIEW, repo: "a/b", project: "backend", page: 4, test: 7 };

  it("changing repo clears project and resets page, keeps the selected test", () => {
    expect(applyPatch(base, { repo: "c/d" })).toEqual({
      ...base, repo: "c/d", project: null, page: 1,
    });
  });

  it("sort and stable toggle reset page; selecting a test does not", () => {
    expect(applyPatch(base, { sort: "last_seen" }).page).toBe(1);
    expect(applyPatch(base, { showStable: true }).page).toBe(1);
    expect(applyPatch(base, { test: 9 }).page).toBe(4);
    expect(applyPatch(base, { page: 5 }).page).toBe(5);
  });

  it("clearing the repo clears the project", () => {
    expect(applyPatch(base, { repo: null, project: null })).toMatchObject({
      repo: null, project: null, page: 1,
    });
  });
});
```
- `frontend/src/components/ScopePicker.test.tsx`:
```tsx
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { RepoInfo } from "../api";
import { ScopePicker } from "./ScopePicker";

const repos: RepoInfo[] = [
  { name: "andrewthetechie/fantasy", projects: [{ name: "backend", root: "" }] },
  {
    name: "andrewthetechie/writers-app",
    projects: [{ name: "backend", root: "" }, { name: "e2e", root: "e2e" }],
  },
];

describe("ScopePicker", () => {
  it("disables the project picker until a repo is chosen", () => {
    render(<ScopePicker repos={repos} scope={{ repo: null, project: null }} onChange={() => {}} />);
    expect(screen.getByLabelText("Project")).toBeDisabled();
  });

  it("choosing a repo reports it with no project", async () => {
    const onChange = vi.fn();
    render(<ScopePicker repos={repos} scope={{ repo: null, project: null }} onChange={onChange} />);
    await userEvent.selectOptions(screen.getByLabelText("Repo"), "andrewthetechie/writers-app");
    expect(onChange).toHaveBeenCalledWith({ repo: "andrewthetechie/writers-app", project: null });
  });

  it("lists only the chosen repo's projects", async () => {
    const onChange = vi.fn();
    render(
      <ScopePicker
        repos={repos}
        scope={{ repo: "andrewthetechie/writers-app", project: null }}
        onChange={onChange}
      />,
    );
    const project = screen.getByLabelText("Project");
    expect(project).toBeEnabled();
    const options = Array.from(project.querySelectorAll("option")).map((o) => o.textContent);
    expect(options).toEqual(["All projects", "backend", "e2e"]);
    await userEvent.selectOptions(project, "e2e");
    expect(onChange).toHaveBeenCalledWith({ repo: "andrewthetechie/writers-app", project: "e2e" });
  });
});
```
- `frontend/src/components/Pagination.test.tsx`:
```tsx
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Pagination } from "./Pagination";

describe("Pagination", () => {
  it("shows the position and moves forward", async () => {
    const onPage = vi.fn();
    render(<Pagination page={2} pageSize={50} total={123} onPage={onPage} />);
    expect(screen.getByText("Page 2 of 3 · 51–100 of 123")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Next ›" }));
    expect(onPage).toHaveBeenCalledWith(3);
  });

  it("disables prev on the first page and next on the last", () => {
    const { rerender } = render(<Pagination page={1} pageSize={50} total={60} onPage={() => {}} />);
    expect(screen.getByRole("button", { name: "‹ Prev" })).toBeDisabled();
    rerender(<Pagination page={2} pageSize={50} total={60} onPage={() => {}} />);
    expect(screen.getByRole("button", { name: "Next ›" })).toBeDisabled();
  });
});
```
- `frontend/src/components/Leaderboard.test.tsx`:
```tsx
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { TestCase } from "../api";
import { Leaderboard, scopeLabel } from "./Leaderboard";

const test: TestCase = {
  id: 1, repo: "andrewthetechie/writers-app", project: "backend", fingerprint: "f",
  suite: "unit", classname: "tests.test_views", name: "test_login", file: null, line: null,
  flakiness_score: 0.7, tier: "flaky", confirmed_flake_count: 2, last_status: "failed",
  last_seen_at: "2026-09-25T00:00:00Z", quarantined: false, quarantined_at: null,
  github_issue_number: null,
};

describe("Leaderboard", () => {
  it("labels rows with repo · project when viewing all repos", () => {
    expect(scopeLabel(test, { repo: null, project: null }))
      .toBe("andrewthetechie/writers-app · backend");
    expect(scopeLabel(test, { repo: test.repo, project: null })).toBe("backend");
    expect(scopeLabel(test, { repo: test.repo, project: "backend" })).toBeNull();
  });

  it("renders the label, tier and proof", () => {
    render(
      <Leaderboard tests={[test]} scope={{ repo: null, project: null }} showStable={false}
        selectedId={null} onSelect={() => {}} onToggleQuarantine={() => {}} />,
    );
    expect(screen.getByText("andrewthetechie/writers-app · backend")).toBeInTheDocument();
    expect(screen.getByText("flaky")).toBeInTheDocument();
    expect(screen.getByText(/2× proven/)).toBeInTheDocument();
  });

  it("explains the empty state when stable tests are hidden", () => {
    render(
      <Leaderboard tests={[]} scope={{ repo: null, project: null }} showStable={false}
        selectedId={null} onSelect={() => {}} onToggleQuarantine={() => {}} />,
    );
    expect(screen.getByText(/No flaky or suspect tests in this view/)).toBeInTheDocument();
  });
});
```

## Dependencies
- Blocked by: 08 — Repos, leaderboard and summary read API; 09 — Test detail API (the `History` shape)
- Why blocked: the UI consumes `/api/repos`, the paged `/api/tests`, `suspect_tests` and the new `History` fields.
- Blocks: 12 (slide-over detail builds on `App.tsx` and `urlState`), 13 (queue indicator in the header)

## Labels
`feature`, `frontend`, `priority:high`

## Estimate
Large

## Risk
2 - UI only. Every new piece of logic has a unit test, and strict TypeScript guards the API contract.

## Validator Stopping Point
```bash
cd frontend && npm install && npm test && npm run build
cd ../backend && .venv/bin/python -m pytest -q   # still 87 passed
```
