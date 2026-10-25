import { useCallback, useEffect, useRef, useState } from "react";
import {
  fetchFailedReports,
  fetchHistory,
  fetchJobHistory,
  fetchJobSummary,
  fetchJobs,
  fetchReportSummary,
  fetchRepos,
  fetchSummary,
  fetchTests,
  setQuarantine,
  type History,
  type JobHistory,
  type JobPage,
  type JobSummary,
  type RepoInfo,
  type ReportSummary,
  type Summary,
  type TestCase,
  type TestPage,
} from "./api";
import { CiRunView } from "./components/CiRunView";
import { JobDrawer } from "./components/JobDrawer";
import { JobLeaderboard } from "./components/JobLeaderboard";
import { JobStatTiles } from "./components/JobStatTiles";
import { Leaderboard } from "./components/Leaderboard";
import { LeaderboardControls } from "./components/LeaderboardControls";
import { Pagination } from "./components/Pagination";
import { QueueIndicator } from "./components/QueueIndicator";
import { ScopePicker } from "./components/ScopePicker";
import { StatTiles } from "./components/StatTiles";
import { TestDrawer } from "./components/TestDrawer";
import { ThemeToggle } from "./components/ThemeToggle";
import { ViewToggle } from "./components/ViewToggle";
import { useViewState } from "./urlState";

const REFRESH_MS = 30_000;
const PAGE_SIZE = 50;

export default function App() {
  const [view, setView] = useViewState();
  const [repos, setRepos] = useState<RepoInfo[]>([]);
  const [summary, setSummary] = useState<Summary | null>(null);
  const [jobSummary, setJobSummary] = useState<JobSummary | null>(null);
  const [queue, setQueue] = useState<ReportSummary | null>(null);
  const [page, setPage] = useState<TestPage | null>(null);
  const [jobPage, setJobPage] = useState<JobPage | null>(null);
  const [history, setHistory] = useState<History | null>(null);
  const [jobHistory, setJobHistory] = useState<JobHistory | null>(null);
  const [error, setError] = useState<string | null>(null);

  const { repo, project, sort, showStable, cause, q, view: kind } = view;
  const pageNumber = view.page;

  // Only the newest refresh may write state: a slow response for the old
  // scope must not overwrite the view the user has since switched to.
  const latestRefresh = useRef(0);

  const refresh = useCallback(async () => {
    const request = ++latestRefresh.current;
    try {
      const queueQ = fetchReportSummary();
      if (kind === "ci-run") {
        // CiRunView fetches the CI run itself; the header needs repos + queue.
        const [r, queue] = await Promise.all([fetchRepos(), queueQ]);
        if (request !== latestRefresh.current) return;
        setRepos(r);
        setQueue(queue);
        setError(null);
      } else if (kind === "jobs") {
        const [r, s, jobs, queue] = await Promise.all([
          fetchRepos(),
          fetchJobSummary(repo),
          fetchJobs({ repo, page: pageNumber, pageSize: PAGE_SIZE, sort, showStable, q }),
          queueQ,
        ]);
        if (request !== latestRefresh.current) return;
        setRepos(r);
        setJobSummary(s);
        setJobPage(jobs);
        setQueue(queue);
        setError(null);
      } else {
        const scope = { repo, project };
        const [r, s, t, queue] = await Promise.all([
          fetchRepos(),
          fetchSummary(scope),
          fetchTests({
            ...scope,
            page: pageNumber,
            pageSize: PAGE_SIZE,
            sort,
            showStable,
            category: cause,
            q,
          }),
          queueQ,
        ]);
        if (request !== latestRefresh.current) return;
        setRepos(r);
        setSummary(s);
        setPage(t);
        setQueue(queue);
        setError(null);
      }
    } catch (e) {
      if (request !== latestRefresh.current) return;
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [kind, repo, project, pageNumber, sort, showStable, cause, q]);

  // Stable identity: the search box's debounce timer must not restart on the
  // 30 s polling re-renders.
  const onSearch = useCallback((text: string) => setView({ q: text }), [setView]);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), REFRESH_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  useEffect(() => {
    if (kind === "ci-run") {
      // The CI run view has no drawers and no leaderboard histories.
      setHistory(null);
      setJobHistory(null);
      return;
    }
    if (kind === "jobs") {
      if (view.job == null) {
        setJobHistory(null);
        return;
      }
      let cancelled = false;
      fetchJobHistory(view.job)
        .then((h) => {
          if (!cancelled) setJobHistory(h);
        })
        .catch((e) => {
          if (!cancelled) setError(String(e));
        });
      return () => {
        cancelled = true;
      };
    }
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
  }, [kind, view.job, view.test, pageNumber, refresh]); // re-fetch when a drawer's list refreshes

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

  const closeDrawer = useCallback(() => {
    if (kind === "jobs") setView({ job: null });
    else setView({ test: null });
  }, [kind, setView]);

  const openJobFromTest = useCallback(
    (jobId: number) => setView({ view: "jobs", job: jobId }),
    [setView],
  );
  const openTestFromJob = useCallback(
    (testId: number) => setView({ view: "tests", test: testId }),
    [setView],
  );

  const visiblePage = kind === "jobs" ? jobPage : page;

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
          <ViewToggle view={kind} onChange={(v) => setView({ view: v })} />
          <ScopePicker
            repos={repos}
            scope={{ repo, project }}
            showProject={kind === "tests"}
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

      {kind === "tests" && summary && <StatTiles summary={summary} />}
      {kind === "jobs" && jobSummary && <JobStatTiles summary={jobSummary} />}

      {kind === "ci-run" ? (
        repo && view.ci_run ? (
          <CiRunView
            repo={repo}
            ciRun={view.ci_run}
            attempt={view.attempt}
            onAttempt={(a) => setView({ attempt: a })}
            onOpenTest={openTestFromJob}
            onOpenJob={openJobFromTest}
          />
        ) : (
          <section className="rounded-xl border border-line bg-surface px-4 py-10 text-center text-text-2">
            No CI run selected. Open one from a Test or Job execution, or add{" "}
            <code className="font-mono text-text">
              ?view=ci-run&amp;repo=owner/name&amp;ci_run=12345678
            </code>{" "}
            to the URL.
          </section>
        )
      ) : (
        <section className="rounded-xl border border-line bg-surface">
          <div className="flex flex-wrap items-center justify-between gap-3 border-b border-line px-4 py-3">
            <h2 className="text-[15px] font-semibold">Flakiness leaderboard</h2>
            <LeaderboardControls
              sort={sort}
              showStable={showStable}
              stableLabel={kind === "jobs" ? "Show stable jobs" : "Show stable tests"}
              cause={cause}
              causeCounts={summary?.category_counts ?? null}
              search={q}
              searchPlaceholder={kind === "jobs" ? "Job or pipeline" : "Name, classname or file"}
              onSort={(s) => setView({ sort: s })}
              onShowStable={(v) => setView({ showStable: v })}
              onCause={kind === "tests" ? (c) => setView({ cause: c }) : undefined}
              onSearch={onSearch}
            />
          </div>
          <div className="overflow-x-auto">
            {kind === "jobs" ? (
              <JobLeaderboard
                jobs={jobPage?.items ?? []}
                repo={repo}
                showStable={showStable}
                search={q}
                selectedId={view.job}
                onSelect={(id) => setView({ job: id })}
              />
            ) : (
              <Leaderboard
                tests={page?.items ?? []}
                scope={{ repo, project }}
                showStable={showStable}
                search={q}
                selectedId={view.test}
                onSelect={(id) => setView({ test: id })}
                onToggleQuarantine={onToggleQuarantine}
              />
            )}
          </div>
          {visiblePage && visiblePage.total > 0 && (
            <Pagination
              page={visiblePage.page}
              pageSize={visiblePage.page_size}
              total={visiblePage.total}
              onPage={(p) => setView({ page: p })}
            />
          )}
        </section>
      )}

      {kind === "tests" && view.test != null && (
        <TestDrawer
          testId={view.test}
          history={history}
          onClose={closeDrawer}
          onToggleQuarantine={onToggleQuarantine}
          onOpenJob={openJobFromTest}
        />
      )}
      {kind === "jobs" && view.job != null && (
        <JobDrawer
          jobId={view.job}
          history={jobHistory}
          onClose={closeDrawer}
          onOpenTest={openTestFromJob}
          onOpenJob={(jobId) => setView({ job: jobId })}
        />
      )}

      <footer className="mt-8 text-xs text-muted">
        Ingest from CI:{" "}
        <code className="rounded border border-line bg-surface px-1.5 py-0.5 font-mono [overflow-wrap:anywhere]">
          curl -X POST
          "$URL/api/ingest?repo=$OWNER/$REPO&amp;project=backend&amp;commit_sha=$SHA&amp;branch=$BRANCH"
          -H "X-API-Key: $TOKEN" --data-binary @junit.xml
        </code>
        <br />
        Job-level flakiness: add{" "}
        <code className="rounded border border-line bg-surface px-1.5 py-0.5 font-mono [overflow-wrap:anywhere]">
          samples/flakeradar-jobs.yml
        </code>{" "}
        to your repo — see the README.
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
