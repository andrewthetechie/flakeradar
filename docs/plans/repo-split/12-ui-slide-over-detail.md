# 12 — UI: slide-over test detail

## Tracer-Bullet Outcome
The leaderboard uses the full page width. Clicking a test (or opening a link with `?test=42`) slides a detail panel in from the right. The panel shows:
- a `repo / project` breadcrumb;
- the full test name and classname, wrapping at `.`, `:`, `/`, `_` and `-` instead of overflowing;
- the Location as a GitHub permalink (`frontend/src/app.test.ts:12`), or a note that the runner reported none;
- a Quarantine toggle;
- the score facts;
- the **latest failure's full details** (traceback + output) in a scrollable block;
- the execution strip and table, with messages that wrap instead of being cut off.

Esc, the × button or a click on the backdrop closes it (and removes `test` from the URL). Scores display floor-rounded so a `0.2975` "suspect" never shows as `0.30`. On phones, the leaderboard hides the Proof and Last status columns so test names get the width.

Verified visually in a browser at 1280×900 and 390×844 against a live backend with realistic long names (`test_reminder_outcome_helpers_shared_by_cron_and_resend_path`, `writers-app::summarize`).

## User Story
As someone investigating a flaky test, I want its full name, location, traceback and history in a roomy panel, so that nothing is cut off and I can jump straight to the code.

## Description
Replace the two-column layout (leaderboard beside a cramped detail column) with a full-width leaderboard and a slide-over `TestDrawer`. Refactor `TestDetail` into the drawer's body. Add two small helpers: `breakable()` for break opportunities in long identifiers, and `formatScore()`, which floors to 2 decimals.

This task also fixes three display bugs found while verifying the UI:
1. `toFixed(2)` rounded a score of 0.2975 up to "0.30" next to a "suspect" tier.
2. The execution SVG scaled **up** with few Executions (huge markers). It is now capped at its natural width.
3. On a 390 px phone, the test-name column collapsed to about 25 px (one letter per line).

## Context Pack
- Source decisions:
  - A full-width leaderboard with detail in a slide-over panel.
  - Long names wrap (break on `.` and `::`) instead of being cut off.
  - The URL state from task 11 supports deep links to the panel.
  - The permalink is pinned to the last failing SHA (built by the backend in task 09).
- Repo facts (after task 11):
  - `frontend/src/api.ts` has `History { test: TestCase; location: Location | null; last_failing_sha; last_failing_branch; executions: Execution[] }`, `Location { path: string; line: number | null; url: string | null }`, and `Execution { …, message: string; details: string; … }`.
  - `frontend/src/urlState.ts` provides `useViewState(): [ViewState, (patch: Partial<ViewState>) => void]`, where `ViewState.test: number | null`.
  - `frontend/src/App.tsx` currently renders `<div className="columns">` with the leaderboard section and a `<section className="panel"><h2>Test detail</h2><TestDetail history={history} /></section>`, and fetches the history in an effect keyed on `[view.test, page]`.
  - `frontend/src/components/TestDetail.tsx` currently: `export function TestDetail({ history }: { history: History | null })` renders the empty state when `null`, then a title, subtitle, facts, the `ExecutionStrip` (an SVG with `width="100%"` and `viewBox` width `max(n*18+18, 200)`), a legend, and a table whose message cell has `className="msg" title={e.message}` and is CSS-ellipsized.
  - `frontend/src/components/Leaderboard.tsx` (task 11) renders the score as `{t.flakiness_score.toFixed(2)}`.
  - `frontend/src/styles.css` has `.columns { display: grid; … }` plus a `@media (max-width: 900px)` rule for it, and `.exec-table td.msg { … max-width: 260px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }`.
- Verified external contracts: a regex split with lookbehind (`/(?<=[.:/_-])/`) is supported by the jsdom (Vitest) and evergreen-browser targets (ES2022, as in `tsconfig.json`). React renders `<wbr />` as `<wbr>` (verified by `breakable.test.tsx`).
- Non-goals: the queue indicator (13); editing tests; a focus trap in the drawer (Esc and close-button are enough for an internal tool); a new chart design.

## Delivery Strategy
- Shape: Wide refactor: Migrate (integration branch).
- Valid-state scope: Named integration branch `feat/repo-split`. `npm test` and `npm run build` must pass.

## Implementation Contract
- Expected files:
  - **Create:** `frontend/src/breakable.tsx`, `frontend/src/format.ts`, `frontend/src/components/TestDrawer.tsx`, and the tests `frontend/src/breakable.test.tsx`, `frontend/src/format.test.ts` and `frontend/src/components/TestDrawer.test.tsx`.
  - **Replace:** `frontend/src/components/TestDetail.tsx` and `frontend/src/App.tsx`.
  - **Edit:** `frontend/src/components/Leaderboard.tsx` and `frontend/src/styles.css`.
- `frontend/src/breakable.tsx`:
```tsx
import { Fragment, type ReactNode } from "react";

/** Long test ids (`tests.test_cron_health.TestSuccessfulTickStamps`,
 *  `writers-app::summarize`, `src/a/b.test.ts`) have no spaces, so browsers
 *  either overflow or break mid-word. Offer a line break after each
 *  separator instead: `.`, `:`, `/`, `_`, `-`. */
export function breakable(text: string): ReactNode {
  const parts = text.split(/(?<=[.:/_-])/);
  return parts.map((part, i) => (
    <Fragment key={i}>
      {part}
      {i < parts.length - 1 && <wbr />}
    </Fragment>
  ));
}
```
- `frontend/src/format.ts`:
```ts
/** Two-decimal score that never rounds UP across the flake threshold:
 *  0.2975 shows as 0.29 (suspect), not 0.30 next to a "suspect" tier.
 *  The epsilon absorbs float error (0.29 * 100 = 28.999…). */
export function formatScore(score: number): string {
  return (Math.floor(score * 100 + 1e-9) / 100).toFixed(2);
}
```
- `frontend/src/components/TestDrawer.tsx`:
```tsx
import { useEffect } from "react";
import type { History, TestCase } from "../api";
import { breakable } from "../breakable";
import { TestDetail } from "./TestDetail";

/** Slide-over panel for one Test. The leaderboard keeps the full width. */
export function TestDrawer({
  testId, history, onClose, onToggleQuarantine,
}: {
  testId: number;
  history: History | null;
  onClose: () => void;
  onToggleQuarantine: (t: TestCase) => void;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const loaded = history != null && history.test.id === testId;
  return (
    <>
      <div className="drawer-backdrop" onClick={onClose} aria-hidden />
      <aside className="drawer" role="dialog" aria-modal="true" aria-label="Test detail">
        <div className="drawer-head">
          <div className="breadcrumb">
            {loaded ? breakable(`${history.test.repo} / ${history.test.project}`) : "Loading…"}
          </div>
          <button type="button" className="drawer-close" onClick={onClose} aria-label="Close">
            ×
          </button>
        </div>
        {loaded && (
          <>
            <h2 className="detail-title">{breakable(history.test.name)}</h2>
            <div className="detail-sub">
              {breakable(history.test.classname || history.test.suite)}
              {history.test.github_issue_number != null && (
                <> · issue #{history.test.github_issue_number}</>
              )}
            </div>
            <div className="detail-location">
              {history.location == null ? (
                <span className="muted">Location not reported by the test runner</span>
              ) : history.location.url ? (
                <a href={history.location.url} target="_blank" rel="noreferrer">
                  {breakable(history.location.path)}
                  {history.location.line != null && `:${history.location.line}`}
                </a>
              ) : (
                <span>
                  {breakable(history.location.path)}
                  {history.location.line != null && `:${history.location.line}`}
                </span>
              )}
            </div>
            <button
              type="button"
              className="quarantine-btn"
              onClick={() => onToggleQuarantine(history.test)}
            >
              {history.test.quarantined ? "Un-quarantine" : "Quarantine"}
            </button>
            <TestDetail history={history} />
          </>
        )}
      </aside>
    </>
  );
}
```
- `frontend/src/components/TestDetail.tsx` (full target; now it is the drawer body and always receives a `History`):
```tsx
import { useState } from "react";
import type { Execution, History } from "../api";
import { formatScore } from "../format";
import { MarkShape, StatusMark, statusColor } from "./StatusMark";

const CELL = 18; // horizontal step per execution
const R = 5; // mark radius
const H = 46; // strip height

function fmtWhen(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleString(undefined, {
    month: "short", day: "numeric", hour: "2-digit", minute: "2-digit",
  });
}

/** Execution history strip: oldest -> newest, left -> right.
 *  Hover any mark for commit/branch/time/message. */
function ExecutionStrip({ executions }: { executions: Execution[] }) {
  const [hover, setHover] = useState<{ x: number; e: Execution } | null>(null);
  const ordered = [...executions].reverse(); // API returns newest first
  const width = Math.max(ordered.length * CELL + CELL, 200);

  return (
    <div className="strip-wrap">
      <svg
        width="100%"
        viewBox={`0 0 ${width} ${H}`}
        preserveAspectRatio="xMinYMid meet"
        role="img"
        aria-label={`Execution history, oldest to newest: ${ordered
          .map((e) => e.status)
          .join(", ")}`}
        onMouseLeave={() => setHover(null)}
        style={{ display: "block", maxWidth: width }} // shrink for long histories, never enlarge
      >
        <line x1={0} y1={H - 8} x2={width} y2={H - 8} stroke="var(--baseline)" strokeWidth={1} />
        {ordered.map((e, i) => {
          const cx = CELL / 2 + i * CELL;
          return (
            <g key={e.id}>
              {/* hit target larger than the mark */}
              <rect
                x={cx - CELL / 2} y={0} width={CELL} height={H}
                fill="transparent"
                onMouseEnter={() => setHover({ x: cx, e })}
              />
              <MarkShape
                status={e.status}
                cx={cx}
                cy={H / 2 - 4}
                r={hover?.e.id === e.id ? R + 1.5 : R}
                color={statusColor(e.status)}
              />
            </g>
          );
        })}
      </svg>
      {hover && (
        <div
          className="tooltip"
          style={{
            left: `min(${(hover.x / width) * 100}%, calc(100% - 200px))`,
            top: 0,
            transform: "translateY(-100%)",
          }}
        >
          <div className="t-status">
            <StatusMark status={hover.e.status} size={9} /> {hover.e.status}
            {hover.e.duration > 0 && ` · ${hover.e.duration.toFixed(2)}s`}
          </div>
          <div className="t-meta">
            {hover.e.commit_sha.slice(0, 10)} on {hover.e.branch} · {fmtWhen(hover.e.created_at)}
          </div>
          {hover.e.message && <div className="t-meta">{hover.e.message.slice(0, 140)}</div>}
        </div>
      )}
    </div>
  );
}

export function TestDetail({ history }: { history: History }) {
  const { executions } = history;
  const fails = executions.filter((e) => e.status === "failed" || e.status === "error").length;
  const latestFailure = executions.find((e) => e.status === "failed" || e.status === "error");
  return (
    <div>
      <div className="facts">
        <div className="fact">
          <div className="label">Flakiness score</div>
          <div className="value">{formatScore(history.test.flakiness_score)}</div>
        </div>
        <div className="fact">
          <div className="label">Proven flakes</div>
          <div className="value">{history.test.confirmed_flake_count}</div>
        </div>
        <div className="fact">
          <div className="label">Failures (window)</div>
          <div className="value">
            {fails}/{executions.length}
          </div>
        </div>
      </div>

      {latestFailure && (latestFailure.details || latestFailure.message) && (
        <details className="failure" open>
          <summary>
            Latest failure · {latestFailure.commit_sha.slice(0, 10)} on {latestFailure.branch}
          </summary>
          <pre>{latestFailure.details || latestFailure.message}</pre>
        </details>
      )}

      <ExecutionStrip executions={executions} />
      <div className="strip-legend" aria-hidden>
        <span className="item"><StatusMark status="passed" /> passed</span>
        <span className="item"><StatusMark status="failed" /> failed</span>
        <span className="item"><StatusMark status="error" /> error</span>
        <span className="item"><StatusMark status="skipped" /> skipped</span>
      </div>

      <table className="exec-table">
        <thead>
          <tr>
            <th>Status</th>
            <th>Commit</th>
            <th>Branch</th>
            <th>When</th>
            <th>Message</th>
          </tr>
        </thead>
        <tbody>
          {executions.slice(0, 15).map((e) => (
            <tr key={e.id}>
              <td>
                <span className="chip">
                  <StatusMark status={e.status} size={9} /> {e.status}
                </span>
              </td>
              <td>{e.commit_sha.slice(0, 10)}</td>
              <td>{e.branch}</td>
              <td>{fmtWhen(e.created_at)}</td>
              <td className="msg">{e.message || "—"}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
```
- `frontend/src/App.tsx` (full target):
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
import { TestDrawer } from "./components/TestDrawer";
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

  const closeDrawer = useCallback(() => setView({ test: null }), [setView]);

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
- `frontend/src/components/Leaderboard.tsx` edits:
  - Add `import { formatScore } from "../format";`.
  - Replace `{t.flakiness_score.toFixed(2)}` with `{formatScore(t.flakiness_score)}`.
  - Change the headers `<th>Proof</th>` / `<th>Last status</th>` to `<th className="col-proof">Proof</th>` / `<th className="col-status">Last status</th>`.
  - Give the matching body cells `className="col-proof"` (the cell with the `⚠ … proven` chip) and `className="col-status"` (the cell with `<StatusMark status={t.last_status} />`).
- `frontend/src/styles.css` edits:
  - Delete the two `.columns` rules under `/* ---- layout ---- */` (keep the comment).
  - Replace the `.exec-table td.msg` rule with `.exec-table td.msg { color: var(--text-secondary); white-space: normal; overflow-wrap: anywhere; min-width: 160px; }`.
  - Append:
```css
/* ---- slide-over test detail (task 12) ---- */
.drawer-backdrop { position: fixed; inset: 0; background: rgba(0, 0, 0, 0.25); z-index: 20; }
.drawer {
  position: fixed; top: 0; right: 0; bottom: 0; z-index: 21;
  width: min(760px, 100vw); overflow-y: auto;
  background: var(--surface-1); border-left: 1px solid var(--border);
  box-shadow: -8px 0 24px rgba(0, 0, 0, 0.18); padding: 16px 20px 32px;
}
.drawer-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px; margin-bottom: 8px; }
.breadcrumb { color: var(--accent); font-size: 12px; overflow-wrap: anywhere; }
.drawer-close { font-size: 20px; line-height: 1; background: none; border: none; color: var(--text-secondary); cursor: pointer; }
.drawer .detail-title { margin: 0 0 2px; }
.detail-location { font-size: 12px; margin-bottom: 10px; overflow-wrap: anywhere; }
.detail-location a { color: var(--accent); }
.muted { color: var(--muted); }
details.failure { margin: 12px 0; }
details.failure summary { cursor: pointer; font-size: 12px; color: var(--text-secondary); }
details.failure pre {
  margin: 6px 0 0; padding: 10px; max-height: 320px; overflow: auto;
  background: var(--page); border: 1px solid var(--border); border-radius: 6px;
  font-size: 12px; white-space: pre-wrap; overflow-wrap: anywhere;
}
.drawer .quarantine-btn { margin-bottom: 14px; }

/* ---- phones: give the test name the width (task 12) ---- */
@media (max-width: 640px) {
  .app { padding: 16px 16px 32px; }
  header.app-header { flex-wrap: wrap; }
  header.app-header .tagline { display: none; }
  .scope-picker { margin-left: 0; width: 100%; }
  .scope-picker select { flex: 1; min-width: 0; }
  .leaderboard .col-proof, .leaderboard .col-status { display: none; }
  .leaderboard td:first-child { min-width: 55vw; }
  .meter { min-width: 72px; }
}
```
- Behavior rules:
  - The drawer renders whenever `view.test != null`. It shows `Loading…` until `history.test.id === testId`, so it never shows the previous test's data.
  - The Location link opens in a new tab (`target="_blank" rel="noreferrer"`). Without a URL (the test never failed), the path shows as plain text. Without a Location, it reads `Location not reported by the test runner`.
  - The latest failure is the first `failed`/`error` Execution in the (newest-first) list. Its `details` show in a `<pre>`, falling back to `message`.
  - `formatScore` floors (with a `1e-9` epsilon), so the displayed value never crosses the threshold upward.
- Error and security rules: render the traceback as text only (a `<pre>` with React escaping). **Never** use `dangerouslySetInnerHTML` for CI output.

## Acceptance Criteria
- [ ] `npm test` passes (7 files, 22 tests), and `npm run build` succeeds.
- [ ] `breakable("tests.test_a::run/x-y")` renders `tests.<wbr>test_<wbr>a:<wbr>:<wbr>run/<wbr>x-<wbr>y`.
- [ ] `formatScore(0.2975) === "0.29"`, `formatScore(0.3) === "0.30"`, `formatScore(0.29) === "0.29"`.
- [ ] The drawer shows the breadcrumb `andrewthetechie/writers-app / frontend`, and a link whose `href` is the permalink and whose text is `frontend/src/app.test.ts:12`. Esc and the Close button each call `onClose`.
- [ ] Manual: at 1280 px wide, the leaderboard spans the page and a long test name is fully readable in the drawer. At 390 px wide, names wrap at separators and are never one letter per line. Opening `/?test=<id>` directly opens the drawer.

## Test Expectations
- Framework: Vitest 5 + Testing Library (jsdom). Run with `cd frontend && npm test`.
- `frontend/src/breakable.test.tsx`:
```tsx
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { breakable } from "./breakable";

describe("breakable", () => {
  it("adds a break opportunity after each separator", () => {
    const { container } = render(<span>{breakable("tests.test_a::run/x-y")}</span>);
    expect(container.innerHTML).toBe(
      "<span>tests.<wbr>test_<wbr>a:<wbr>:<wbr>run/<wbr>x-<wbr>y</span>",
    );
  });

  it("leaves plain words alone", () => {
    const { container } = render(<span>{breakable("renders")}</span>);
    expect(container.innerHTML).toBe("<span>renders</span>");
  });
});
```
- `frontend/src/format.test.ts`:
```ts
import { describe, expect, it } from "vitest";
import { formatScore } from "./format";

describe("formatScore", () => {
  it("floors to two decimals so the number agrees with the tier", () => {
    expect(formatScore(0.2975)).toBe("0.29");
    expect(formatScore(0.3)).toBe("0.30");
    expect(formatScore(0.29)).toBe("0.29");
    expect(formatScore(1)).toBe("1.00");
    expect(formatScore(0)).toBe("0.00");
  });
});
```
- `frontend/src/components/TestDrawer.test.tsx`:
```tsx
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { History } from "../api";
import { TestDrawer } from "./TestDrawer";

const history: History = {
  test: {
    id: 7, repo: "andrewthetechie/writers-app", project: "frontend", fingerprint: "f",
    suite: "unit", classname: "src/app.test.ts", name: "renders", file: "src/app.test.ts",
    line: 12, flakiness_score: 0.6, tier: "flaky", confirmed_flake_count: 1,
    last_status: "passed", last_seen_at: "2026-09-25T00:00:00Z", quarantined: false,
    quarantined_at: null, github_issue_number: null,
  },
  location: {
    path: "frontend/src/app.test.ts", line: 12,
    url: "https://github.com/andrewthetechie/writers-app/blob/aaa111/frontend/src/app.test.ts#L12",
  },
  last_failing_sha: "aaa111",
  last_failing_branch: "main",
  executions: [
    { id: 2, status: "passed", duration: 0.1, message: "", details: "",
      created_at: "2026-09-25T01:00:00Z", commit_sha: "bbb222", branch: "feat", ci_run_id: "2" },
    { id: 1, status: "failed", duration: 0.1, message: "expected 3",
      details: "Traceback\n  at src/app.test.ts:14", created_at: "2026-09-25T00:00:00Z",
      commit_sha: "aaa111", branch: "main", ci_run_id: "1" },
  ],
};

function renderDrawer(overrides: Partial<History> = {}, onClose = vi.fn(), onQ = vi.fn()) {
  render(
    <TestDrawer testId={7} history={{ ...history, ...overrides }} onClose={onClose}
      onToggleQuarantine={onQ} />,
  );
  return { onClose, onQ };
}

describe("TestDrawer", () => {
  it("shows breadcrumb, permalink and the latest failure details", () => {
    renderDrawer();
    expect(screen.getByRole("dialog", { name: "Test detail" })).toBeInTheDocument();
    expect(screen.getByText(/andrewthetechie\/writers-app \/ frontend/)).toBeInTheDocument();
    const link = screen.getByRole("link");
    expect(link).toHaveAttribute("href", history.location!.url);
    expect(link).toHaveTextContent("frontend/src/app.test.ts:12");
    expect(screen.getByText(/Traceback/)).toHaveTextContent("Traceback at src/app.test.ts:14");
  });

  it("says when the runner reported no location", () => {
    renderDrawer({ location: null });
    expect(screen.getByText("Location not reported by the test runner")).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("closes on Escape and on the close button", async () => {
    const { onClose } = renderDrawer();
    fireEvent.keyDown(window, { key: "Escape" });
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("toggles quarantine for the shown test", async () => {
    const { onQ } = renderDrawer();
    await userEvent.click(screen.getByRole("button", { name: "Quarantine" }));
    expect(onQ).toHaveBeenCalledWith(history.test);
  });

  it("shows a loading state until the right test's history arrives", () => {
    render(<TestDrawer testId={99} history={history} onClose={() => {}}
      onToggleQuarantine={() => {}} />);
    expect(screen.getByText("Loading…")).toBeInTheDocument();
  });
});
```

## Dependencies
- Blocked by: 11 — UI: pickers, URL state, paginated leaderboard
- Why blocked: builds on 11's `App.tsx`, `urlState` (`test` in the URL), the `api.ts` types and the Vitest setup.
- Blocks: 13 (edits the same `App.tsx` header)

## Labels
`feature`, `frontend`, `ux`, `priority:high`

## Estimate
Medium

## Risk
2 - UI only, with unit tests, and verified visually on desktop and phone widths.

## Validator Stopping Point
```bash
cd frontend && npm test && npm run build   # expect: 22 tests passed, build ok
```
