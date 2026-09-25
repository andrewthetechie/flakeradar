# 13 — UI: queue indicator

## Tracer-Bullet Outcome
When Reports are waiting or have failed, the dashboard header shows a pill: `3 pending · 1 failed`. It turns red when anything has failed. Clicking it lists up to 20 failed Reports (`#12 · andrewthetechie/writers-app / e2e · abcdef1234`) with their error text, plus a hint on how to retry. When the queue is idle and nothing has failed, the pill is hidden. It refreshes with the rest of the dashboard every 30 s.

## User Story
As the maintainer, I want to see at a glance when CI uploads are failing to process (e.g. a runner started sending malformed XML), so that bad data doesn't go unnoticed.

## Description
Add the Report types and two fetchers to `api.ts`, a presentational `QueueIndicator` component (it gets the summary as a prop and loads the failed list when opened), and wire it into `App.tsx`'s refresh cycle and header.

There is no retry button in the UI. Retry needs the API token, and the dashboard holds no token.

## Context Pack
- Source decisions:
  - A small header indicator ("3 pending · 1 failed"); clicking lists failed Reports with their errors.
  - The Report read endpoints are open, and retry needs the token (`00-shared-context.md` → Ingest and the queue).
- API contract (task 03, already implemented):
  - `GET /api/reports/summary` → `{"pending": int, "failed": int}`.
  - `GET /api/reports?status=failed&limit=20` → a newest-first array of `{id, repo, project, commit_sha, branch, ci_run_id, status, error, counts, run_id, created_at, processed_at}`.
  - `POST /api/reports/{id}/retry` (needs `X-API-Key`).
- Repo facts (after task 12): `frontend/src/App.tsx`'s `refresh` callback runs `Promise.all([fetchRepos(), fetchSummary(scope), fetchTests({...})])` every 30 s. The header renders `<h1>`, the `.tagline`, then `<ScopePicker … />`. `frontend/src/api.ts` has the private helper `getJson<T>(url: string): Promise<T>`, which throws `Error("<url> -> <status>")` on a non-2xx response. `.scope-picker` has `margin-left: auto`, and there is a `@media (max-width: 640px)` block at the end of `styles.css` (task 12).
- Verified external contracts: none new. It uses the Vitest/Testing Library setup from task 11. `vi.fn().mockResolvedValue(...)` and `screen.findByText` (async) were verified by the tests below.
- Non-goals: a retry button; deleting Reports; showing pending Reports individually; websockets or live push.

## Delivery Strategy
- Shape: Wide refactor: Migrate (integration branch).
- Valid-state scope: Named integration branch `feat/repo-split`.

## Implementation Contract
- Expected files:
  - Create `frontend/src/components/QueueIndicator.tsx` and `frontend/src/components/QueueIndicator.test.tsx`.
  - Edit `frontend/src/api.ts` (append), `frontend/src/App.tsx` and `frontend/src/styles.css`.
- Append to `frontend/src/api.ts`:
```ts
// --- Report queue (task 13) ----------------------------------------------

export interface ReportInfo {
  id: number;
  repo: string;
  project: string;
  commit_sha: string;
  branch: string;
  ci_run_id: string;
  status: "pending" | "processed" | "failed";
  error: string | null;
  counts: Record<string, number> | null;
  run_id: number | null;
  created_at: string;
  processed_at: string | null;
}

export interface ReportSummary {
  pending: number;
  failed: number;
}

export const fetchReportSummary = () => getJson<ReportSummary>("/api/reports/summary");

export const fetchFailedReports = () =>
  getJson<ReportInfo[]>("/api/reports?status=failed&limit=20");
```
- `frontend/src/components/QueueIndicator.tsx`:
```tsx
import { useState } from "react";
import type { ReportInfo, ReportSummary } from "../api";

/** "3 pending · 1 failed" — the only place a broken CI upload becomes visible.
 *  Hidden while the queue is empty and nothing has failed. */
export function QueueIndicator({
  summary, loadFailed,
}: {
  summary: ReportSummary | null;
  loadFailed: () => Promise<ReportInfo[]>;
}) {
  const [open, setOpen] = useState(false);
  const [failed, setFailed] = useState<ReportInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  if (!summary || (summary.pending === 0 && summary.failed === 0)) return null;

  const toggle = async () => {
    const next = !open;
    setOpen(next);
    if (!next) return;
    try {
      setFailed(await loadFailed());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="queue">
      <button
        type="button"
        className={summary.failed > 0 ? "queue-btn alert" : "queue-btn"}
        aria-expanded={open}
        onClick={() => void toggle()}
      >
        {summary.pending} pending · {summary.failed} failed
      </button>
      {open && (
        <div className="queue-pop" role="region" aria-label="Failed reports">
          {error && <div className="muted">Could not load failed reports ({error}).</div>}
          {failed && failed.length === 0 && <div className="muted">No failed reports.</div>}
          {failed && failed.length > 0 && (
            <>
              <ul>
                {failed.map((r) => (
                  <li key={r.id}>
                    <div className="queue-item-head">
                      #{r.id} · {r.repo} / {r.project} · {r.commit_sha.slice(0, 10)}
                    </div>
                    <div className="queue-item-error">{r.error}</div>
                  </li>
                ))}
              </ul>
              <div className="muted queue-hint">
                Retry one with <code>POST /api/reports/&lt;id&gt;/retry</code> (X-API-Key).
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
```
- `frontend/src/App.tsx` — full target after this task:
```tsx
import { useCallback, useEffect, useState } from "react";
import {
  fetchFailedReports, fetchHistory, fetchReportSummary, fetchRepos, fetchSummary,
  fetchTests, setQuarantine,
  type History, type RepoInfo, type ReportSummary, type Summary, type TestPage,
  type TestCase,
} from "./api";
import { Leaderboard } from "./components/Leaderboard";
import { LeaderboardControls } from "./components/LeaderboardControls";
import { Pagination } from "./components/Pagination";
import { QueueIndicator } from "./components/QueueIndicator";
import { ScopePicker } from "./components/ScopePicker";
import { StatTiles } from "./components/StatTiles";
import { TestDrawer } from "./components/TestDrawer";
import { useViewState } from "./urlState";

const REFRESH_MS = 30_000;
const PAGE_SIZE = 50;

export default function App() {
  const [view, setView] = useViewState();
  const [repos, setRepos] = useState<RepoInfo[]>([]);
  const [summary, setSummary] = useState<Summary | null>(null);
  const [queue, setQueue] = useState<ReportSummary | null>(null);
  const [page, setPage] = useState<TestPage | null>(null);
  const [history, setHistory] = useState<History | null>(null);
  const [error, setError] = useState<string | null>(null);

  const { repo, project, sort, showStable } = view;
  const pageNumber = view.page;

  const refresh = useCallback(async () => {
    const scope = { repo, project };
    try {
      const [r, s, t, q] = await Promise.all([
        fetchRepos(),
        fetchSummary(scope),
        fetchTests({ ...scope, page: pageNumber, pageSize: PAGE_SIZE, sort, showStable }),
        fetchReportSummary(),
      ]);
      setRepos(r);
      setSummary(s);
      setPage(t);
      setQueue(q);
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

  const closeDrawer = useCallback(() => setView({ test: null }), [setView]);

  return (
    <div className="app">
      <header className="app-header">
        <h1>FlakeRadar</h1>
        <span className="tagline">flaky-test detection for your CI</span>
        <QueueIndicator summary={queue} loadFailed={fetchFailedReports} />
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

      {view.test != null && (
        <TestDrawer
          testId={view.test}
          history={history}
          onClose={closeDrawer}
          onToggleQuarantine={onToggleQuarantine}
        />
      )}

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
- `frontend/src/styles.css`: insert this block **directly above** the `/* ---- phones: give the test name the width (task 12) ---- */` comment:
```css
/* ---- report queue indicator (task 13) ---- */
.queue { position: relative; margin-left: auto; }
.queue + .scope-picker { margin-left: 0; }
.queue-btn {
  font-size: 12px; border: 1px solid var(--border); border-radius: 999px;
  background: var(--surface-1); color: var(--text-secondary); padding: 3px 10px; cursor: pointer;
}
.queue-btn.alert { color: var(--status-critical); border-color: var(--status-critical); }
.queue-pop {
  position: absolute; right: 0; top: calc(100% + 6px); z-index: 15;
  width: min(420px, calc(100vw - 32px)); max-height: 360px; overflow-y: auto;
  background: var(--surface-1); border: 1px solid var(--border); border-radius: 8px;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.18); padding: 10px 12px; font-size: 12px;
}
.queue-pop ul { list-style: none; margin: 0; padding: 0; }
.queue-pop li { padding: 6px 0; border-bottom: 1px solid var(--grid); }
.queue-item-head { font-weight: 600; overflow-wrap: anywhere; }
.queue-item-error { color: var(--status-critical); overflow-wrap: anywhere; }
.queue-hint { margin-top: 8px; }
```
- Behavior rules:
  - Hidden when `summary` is `null` or `pending === 0 && failed === 0`.
  - The `alert` class (red) when `failed > 0`.
  - `aria-expanded` reflects whether the list is open.
  - The failed list is fetched on every open, never on close.
  - A load error shows `Could not load failed reports (<message>).` inside the popover.
- Error and security rules: error text is rendered as plain text (no HTML). No token is ever stored in or sent from the browser.

## Acceptance Criteria
- [ ] `npm test` passes (8 files, 25 tests), and `npm run build` succeeds.
- [ ] With `{pending: 0, failed: 0}`, the component renders nothing.
- [ ] With `{pending: 3, failed: 1}`, a button labeled `3 pending · 1 failed` appears. Clicking it calls `loadFailed` once and shows `#12 · andrewthetechie/writers-app / e2e · abcdef1234` and the error text.
- [ ] Manual (optional, with `docker compose up`): ingest any valid report, then mark it failed with `docker compose exec db psql -U flakeradar -c "UPDATE reports SET status='failed', error='manual test' WHERE id=1"`. Within 30 s the pill shows `0 pending · 1 failed`, and clicking it shows `manual test`. (Ingest rejects invalid XML up front, so a real upload cannot easily produce a failed Report.)

## Test Expectations
- Framework: Vitest 5 + Testing Library + user-event (jsdom). Run with `cd frontend && npm test`.
- `frontend/src/components/QueueIndicator.test.tsx`:
```tsx
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ReportInfo } from "../api";
import { QueueIndicator } from "./QueueIndicator";

const failedReport: ReportInfo = {
  id: 12, repo: "andrewthetechie/writers-app", project: "e2e", commit_sha: "abcdef1234567",
  branch: "main", ci_run_id: "7-1", status: "failed",
  error: "ParseError: Not a valid JUnit XML report: syntax error",
  counts: null, run_id: null, created_at: "2026-09-25T00:00:00Z", processed_at: null,
};

describe("QueueIndicator", () => {
  it("renders nothing when the queue is idle", () => {
    const { container } = render(
      <QueueIndicator summary={{ pending: 0, failed: 0 }} loadFailed={vi.fn()} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("shows counts and lists failed reports on click", async () => {
    const loadFailed = vi.fn().mockResolvedValue([failedReport]);
    render(<QueueIndicator summary={{ pending: 3, failed: 1 }} loadFailed={loadFailed} />);
    const button = screen.getByRole("button", { name: "3 pending · 1 failed" });
    await userEvent.click(button);
    expect(loadFailed).toHaveBeenCalledOnce();
    expect(await screen.findByText("#12 · andrewthetechie/writers-app / e2e · abcdef1234"))
      .toBeInTheDocument();
    expect(screen.getByText(/ParseError: Not a valid JUnit XML report/)).toBeInTheDocument();
    expect(button).toHaveAttribute("aria-expanded", "true");
  });

  it("says so when there are no failed reports", async () => {
    render(<QueueIndicator summary={{ pending: 2, failed: 0 }}
      loadFailed={vi.fn().mockResolvedValue([])} />);
    await userEvent.click(screen.getByRole("button"));
    expect(await screen.findByText("No failed reports.")).toBeInTheDocument();
  });
});
```

## Dependencies
- Blocked by: 03 — Queued ingest and the Report API; 12 — UI: slide-over test detail
- Why blocked: 03 provides the endpoints. 12 is the last task to change `App.tsx` and `styles.css`, which this task edits.
- Blocks: 15

## Labels
`feature`, `frontend`, `observability`, `priority:medium`

## Estimate
Small

## Risk
1 - An isolated read-only UI element.

## Validator Stopping Point
```bash
cd frontend && npm test && npm run build   # expect: 25 tests passed, build ok
```
