import { useCallback, useEffect, useRef, useState } from "react";
import {
  fetchFailedReports,
  fetchHistory,
  fetchReportSummary,
  fetchRepos,
  fetchSummary,
  fetchTests,
  setQuarantine,
  type History,
  type RepoInfo,
  type ReportSummary,
  type Summary,
  type TestPage,
  type TestCase,
} from "./api";
import { Leaderboard } from "./components/Leaderboard";
import { LeaderboardControls } from "./components/LeaderboardControls";
import { Pagination } from "./components/Pagination";
import { QueueIndicator } from "./components/QueueIndicator";
import { ScopePicker } from "./components/ScopePicker";
import { StatTiles } from "./components/StatTiles";
import { TestDrawer } from "./components/TestDrawer";
import { ThemeToggle } from "./components/ThemeToggle";
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

  // Only the newest refresh may write state: a slow response for the old
  // scope must not overwrite the view the user has since switched to.
  const latestRefresh = useRef(0);

  const refresh = useCallback(async () => {
    const scope = { repo, project };
    const request = ++latestRefresh.current;
    try {
      const [r, s, t, q] = await Promise.all([
        fetchRepos(),
        fetchSummary(scope),
        fetchTests({ ...scope, page: pageNumber, pageSize: PAGE_SIZE, sort, showStable }),
        fetchReportSummary(),
      ]);
      if (request !== latestRefresh.current) return;
      setRepos(r);
      setSummary(s);
      setPage(t);
      setQueue(q);
      setError(null);
    } catch (e) {
      if (request !== latestRefresh.current) return;
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
      .then((h) => {
        if (!cancelled) setHistory(h);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [view.test, page]); // re-fetch when the leaderboard refreshes

  const onToggleQuarantine = useCallback(
    async (t: TestCase) => {
      try {
        await setQuarantine(t.id, !t.quarantined);
        await refresh();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    },
    [refresh],
  );

  const closeDrawer = useCallback(() => setView({ test: null }), [setView]);

  return (
    <div className="mx-auto max-w-6xl px-4 pt-5 pb-12 sm:px-6">
      <header className="mb-6 flex flex-wrap items-center gap-x-4 gap-y-3">
        <div className="flex items-center gap-2.5">
          <RadarMark />
          <h1 className="text-lg font-semibold tracking-tight">FlakeRadar</h1>
          <span className="hidden text-[13px] text-muted md:inline">
            flaky-test detection for your CI
          </span>
        </div>
        <div className="flex w-full flex-wrap items-center gap-2 sm:ml-auto sm:w-auto">
          <QueueIndicator summary={queue} loadFailed={fetchFailedReports} />
          <ScopePicker
            repos={repos}
            scope={{ repo, project }}
            onChange={(scope) => setView(scope)}
          />
          <ThemeToggle />
        </div>
      </header>

      {error && (
        <div
          role="alert"
          className="mb-4 rounded-lg border border-critical/60 bg-critical/10 px-3.5 py-2.5 text-[13px]"
        >
          Could not reach the FlakeRadar API ({error}). Is the backend running on port 8000?
        </div>
      )}

      {summary && <StatTiles summary={summary} />}

      <section className="rounded-xl border border-line bg-surface">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-line px-4 py-3">
          <h2 className="text-[15px] font-semibold">Flakiness leaderboard</h2>
          <LeaderboardControls
            sort={sort}
            showStable={showStable}
            onSort={(s) => setView({ sort: s })}
            onShowStable={(v) => setView({ showStable: v })}
          />
        </div>
        <div className="overflow-x-auto">
          <Leaderboard
            tests={page?.items ?? []}
            scope={{ repo, project }}
            showStable={showStable}
            selectedId={view.test}
            onSelect={(id) => setView({ test: id })}
            onToggleQuarantine={onToggleQuarantine}
          />
        </div>
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

      <footer className="mt-8 text-xs text-muted">
        Ingest from CI:{" "}
        <code className="rounded border border-line bg-surface px-1.5 py-0.5 font-mono [overflow-wrap:anywhere]">
          curl -X POST
          "$URL/api/ingest?repo=$OWNER/$REPO&amp;project=backend&amp;commit_sha=$SHA&amp;branch=$BRANCH"
          -H "X-API-Key: $TOKEN" --data-binary @junit.xml
        </code>
      </footer>
    </div>
  );
}

/** Same mark as the favicon: a sweep ring around a contact. */
function RadarMark() {
  return (
    <svg width="22" height="22" viewBox="0 0 16 16" aria-hidden className="text-signal">
      <circle
        cx="8"
        cy="8"
        r="6.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.25"
        opacity="0.45"
      />
      <circle
        cx="8"
        cy="8"
        r="3.75"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.25"
        opacity="0.7"
      />
      <circle cx="8" cy="8" r="1.6" fill="currentColor" />
    </svg>
  );
}
