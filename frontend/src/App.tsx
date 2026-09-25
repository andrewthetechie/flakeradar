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
