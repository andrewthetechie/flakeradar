import { useCallback, useEffect, useState } from "react";
import {
  fetchFailedReports,
  setAcknowledge,
  setQuarantine,
  type History,
  type Job,
  type JobHistory,
  type JobPage,
  type JobSummary,
  type RepoInfo,
  type ReportSummary,
  type Summary,
  type TestCase,
  type TestPage,
} from "./api";
import { AcknowledgeDialog, type AcknowledgeChoice } from "./components/AcknowledgeDialog";
import { CiRunView } from "./components/CiRunView";
import { JobDrawer } from "./components/JobDrawer";
import { JobLeaderboard } from "./components/JobLeaderboard";
import { JobStatTiles } from "./components/JobStatTiles";
import { Leaderboard } from "./components/Leaderboard";
import { LeaderboardControls } from "./components/LeaderboardControls";
import { Pagination } from "./components/Pagination";
import { QueueIndicator } from "./components/QueueIndicator";
import { QuarantineDialog, type QuarantineChoice } from "./components/QuarantineDialog";
import { isAcknowledged } from "./components/resultCells";
import { ScopePicker } from "./components/ScopePicker";
import { StatTiles } from "./components/StatTiles";
import { TestDrawer } from "./components/TestDrawer";
import { ThemeToggle } from "./components/ThemeToggle";
import { ViewToggle } from "./components/ViewToggle";
import { useConfirmedDecision } from "./confirmedDecision";
import { useGuardedLoad, type GuardedLoader } from "./guardedLoad";
import { LIST_FILTERS, listFilterSlice, useViewState, type ViewState } from "./urlState";
import { loadJobHistory, loadTestHistory, loadViewBundle, type ViewBundle } from "./viewData";

const REFRESH_MS = 30_000;

export default function App() {
  const [view, setView] = useViewState();
  const [repos, setRepos] = useState<RepoInfo[]>([]);
  const [summary, setSummary] = useState<Summary | null>(null);
  const [jobSummary, setJobSummary] = useState<JobSummary | null>(null);
  const [queue, setQueue] = useState<ReportSummary | null>(null);
  const [page, setPage] = useState<TestPage | null>(null);
  const [jobPage, setJobPage] = useState<JobPage | null>(null);
  // The dashboard's one error surface: the guarded loads and the confirmed
  // decisions mirror their own writes into it, so no single writer knows what
  // is showing. Every write therefore goes through.
  const [error, setError] = useState<string | null>(null);

  // The filter values a load consumes, as one slice derived from the filter
  // table (urlState.ts): no field name is listed here, so the next filter is one
  // table entry plus its control. It changes identity exactly when a filter
  // value does, which is what the load's dependencies key on.
  const filters = listFilterSlice(view);
  const { view: kind, page: pageNumber } = view;
  // Bound to a local so the `repo` check below narrows inside the closures it
  // guards: TypeScript discards a narrowed property read (`filters.repo`) in a
  // callback, and re-checking it there would add a branch no test can reach.
  const repo = filters.repo;

  // The list's whole load, guard and all: only the newest request may write
  // state, so a slow response for the old scope cannot overwrite the view the
  // user has since switched to. The hook owns the component-lifetime guard, the
  // token, the supersede check and the error mapping; this file names the loader
  // and applies the answer.
  const load = useCallback<GuardedLoader<ViewBundle>>(
    (guard, request) => loadViewBundle({ ...filters, page: pageNumber }, guard, request),
    [filters, pageNumber],
  );

  const list = useGuardedLoad<ViewBundle>(load, {
    clearErrorOnSuccess: true,
    onData: (bundle) => {
      // Every view's header data. The CI run view fetches its own run inside
      // CiRunView, so it applies nothing beyond this.
      setRepos(bundle.repos);
      setQueue(bundle.queue);
      if (bundle.kind === "tests") {
        setSummary(bundle.summary);
        setPage(bundle.page);
      } else if (bundle.kind === "jobs") {
        setJobSummary(bundle.jobSummary);
        setJobPage(bundle.jobPage);
      }
    },
    onErrorChange: setError,
  });

  // Imperative as before: the quarantine handlers await it, and its identity
  // changes exactly when the load's does — that is, when the filter slice's or
  // the page's does — so the search debounce survives the poll.
  const refresh = list.refresh;

  // Stable identity: the search box's debounce timer must not restart on the
  // 30 s polling re-renders.
  const onSearch = useCallback((text: string) => setView({ q: text }), [setView]);

  // The hook loads on mount and re-fetches whenever the scope changes, so this
  // effect is the 30 s poll alone, keyed on refresh's stable identity.
  useEffect(() => {
    const timer = setInterval(() => void refresh(), REFRESH_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  // The drawers' histories, one guarded load each at effect guard lifetime:
  // every run gets a guard of its own, cancelled when the drawer closes or the
  // scope changes, so a closed drawer's in-flight answer is dropped without
  // superseding the list's refresh. `refresh` stays in each loader's deps, as
  // it was in the old effect's: a drawer re-fetches on any scope or filter
  // change — those flow through refresh's identity, which is the filter slice's
  // or the page's — but not on an imperative refresh() call, which leaves that
  // identity alone. The slice is what keeps it that way: it is referentially
  // stable across a render whose filters are unchanged, so opening a drawer,
  // editing the CI-run fields, or a poll tick hands back the same refresh (the
  // rule is documented with `listFilterSlice` in urlState.ts).
  const testHistoryLoader = useCallback<GuardedLoader<History>>(
    // The null arm is a safety net for a null drawer id, which `enabled` below
    // already prevents; it stays so the loader is total on its own.
    async (guard, request) =>
      view.test == null ? null : loadTestHistory(view.test, guard, request),
    // pageNumber and refresh are unread here on purpose: they are the old
    // effect's re-fetch triggers, carried on the loader's identity.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [view.test, pageNumber, refresh],
  );
  const testHistoryLoad = useGuardedLoad<History>(testHistoryLoader, {
    lifetime: "effect",
    // The early returns of the effect this replaces: only the Tests view with
    // an open drawer loads, and the ci-run view loads neither history.
    enabled: kind === "tests" && view.test != null,
    // The drawer displays one Test, so it registers as that row's refresher: a
    // confirmed decision on that row re-fetches this load too.
    rowReload: view.test == null ? undefined : { kind: "tests", id: view.test },
    onErrorChange: setError,
  });

  const jobHistoryLoader = useCallback<GuardedLoader<JobHistory>>(
    // The same safety net as the Test drawer's loader above.
    async (guard, request) => (view.job == null ? null : loadJobHistory(view.job, guard, request)),
    // The same deliberate triggers as the Test drawer's loader above.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [view.job, pageNumber, refresh],
  );
  const jobHistoryLoad = useGuardedLoad<JobHistory>(jobHistoryLoader, {
    lifetime: "effect",
    enabled: kind === "jobs" && view.job != null,
    // The same as the Test drawer's: this load displays one Job.
    rowReload: view.job == null ? undefined : { kind: "jobs", id: view.job },
    onErrorChange: setError,
  });

  // Quarantining a Test: the direct path un-quarantines a quarantined row with
  // one click; the confirm path posts the reason and expiry the dialog
  // collected. The flow itself — pending, dialog split, refresh, error mapping
  // — lives in the confirmed-decision module.
  const quarantine = useConfirmedDecision<TestCase, QuarantineChoice>({
    call: (t, decided, choice) =>
      choice == null
        ? setQuarantine(t.id, decided)
        : setQuarantine(t.id, decided, {
            reason: choice.reason,
            expiresInDays: choice.expiresInDays,
          }),
    isDecided: (t) => t.quarantined,
    refresh,
    row: (t) => ({ kind: "tests", id: t.id }),
    onError: setError,
  });

  // Acknowledging a Test: the direct path clears an acknowledged row with one
  // click; the confirm path posts the reason and duration the dialog collected.
  const testAcknowledge = useConfirmedDecision<TestCase, AcknowledgeChoice>({
    call: (t, decided, choice) =>
      choice == null
        ? setAcknowledge("tests", t.id, decided)
        : setAcknowledge("tests", t.id, decided, {
            reason: choice.reason,
            days: choice.days,
          }),
    isDecided: (t) => isAcknowledged(t.acknowledged_until),
    refresh,
    row: (t) => ({ kind: "tests", id: t.id }),
    onError: setError,
  });

  // The reload rule no longer lives here: the load layer owns it. A decision
  // names the row it mutated, and the guarded-load registry refetches whatever
  // is open and showing that row — here, the Job drawer whose loader is keyed on
  // the scope alone and would otherwise keep showing the pre-mutation Job.
  const jobAcknowledge = useConfirmedDecision<Job, AcknowledgeChoice>({
    call: (j, decided, choice) =>
      choice == null
        ? setAcknowledge("jobs", j.id, decided)
        : setAcknowledge("jobs", j.id, decided, {
            reason: choice.reason,
            days: choice.days,
          }),
    isDecided: (j) => isAcknowledged(j.acknowledged_until),
    refresh,
    row: (j) => ({ kind: "jobs", id: j.id }),
    onError: setError,
  });

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
  const openCiRun = useCallback(
    (repo: string, ciRun: string, attempt: number | null) =>
      setView({ view: "ci-run", repo, ci_run: ciRun, attempt }),
    [setView],
  );

  const visiblePage = kind === "jobs" ? jobPage : page;

  // Which controls this view offers comes from the filter table: an entry names
  // the views that render it, and LeaderboardControls shows a control exactly when
  // App passes its handler. The next filter is one table entry plus its control,
  // with no conditional added here.
  const offers = (field: keyof ViewState): boolean =>
    LIST_FILTERS.some((d) => d.field === field && d.views.includes(kind));

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
            scope={{ repo, project: filters.project }}
            showProject={kind === "tests"}
            onChange={(scope) => setView(scope)}
          />
          {repo && <CiRunForm repo={repo} onOpen={(ciRun) => openCiRun(repo, ciRun, null)} />}
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
              sort={filters.sort}
              showStable={filters.showStable}
              stableLabel={kind === "jobs" ? "Show stable jobs" : "Show stable tests"}
              cause={filters.cause}
              causeCounts={summary?.category_counts ?? null}
              quarantined={filters.quarantined}
              broken={filters.broken}
              acknowledged={filters.acknowledged}
              issue={filters.issue}
              newDays={filters.newDays}
              pipelines={jobSummary?.pipelines ?? []}
              pipeline={filters.pipeline}
              search={filters.q}
              searchPlaceholder={kind === "jobs" ? "Job or pipeline" : "Name, classname or file"}
              onSort={(s) => setView({ sort: s })}
              onShowStable={(v) => setView({ showStable: v })}
              onCause={offers("cause") ? (c) => setView({ cause: c }) : undefined}
              onQuarantined={offers("quarantined") ? (v) => setView({ quarantined: v }) : undefined}
              onBroken={offers("broken") ? (v) => setView({ broken: v }) : undefined}
              // acknowledged is offered on both leaderboard views — the only views
              // that render these controls — so unlike the view-specific filters
              // above it passes its handler unconditionally.
              onAcknowledged={(v) => setView({ acknowledged: v })}
              onPipeline={offers("pipeline") ? (p) => setView({ pipeline: p }) : undefined}
              onIssue={(i) => setView({ issue: i })}
              onNewDays={(d) => setView({ newDays: d })}
              onSearch={onSearch}
            />
          </div>
          <div className="overflow-x-auto">
            {kind === "jobs" ? (
              <JobLeaderboard
                jobs={jobPage?.items ?? []}
                repo={repo}
                showStable={filters.showStable}
                search={filters.q}
                selectedId={view.job}
                onSelect={(id) => setView({ job: id })}
              />
            ) : (
              <Leaderboard
                tests={page?.items ?? []}
                scope={{ repo, project: filters.project }}
                showStable={filters.showStable}
                search={filters.q}
                selectedId={view.test}
                onSelect={(id) => setView({ test: id })}
                onToggleQuarantine={(t) => void quarantine.toggle(t)}
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
          history={testHistoryLoad.data}
          onClose={closeDrawer}
          onToggleQuarantine={(t) => void quarantine.toggle(t)}
          onToggleAcknowledge={(t) => void testAcknowledge.toggle(t)}
          onOpenJob={openJobFromTest}
          onOpenCiRun={openCiRun}
        />
      )}
      {quarantine.pending != null && (
        <QuarantineDialog
          test={quarantine.pending}
          onClose={quarantine.cancel}
          onConfirm={(choice) => void quarantine.confirm(choice)}
        />
      )}
      {testAcknowledge.pending != null && (
        <AcknowledgeDialog
          name={testAcknowledge.pending.name}
          scope={`${testAcknowledge.pending.repo} / ${testAcknowledge.pending.project}`}
          onClose={testAcknowledge.cancel}
          onConfirm={(choice) => void testAcknowledge.confirm(choice)}
        />
      )}
      {jobAcknowledge.pending != null && (
        <AcknowledgeDialog
          name={jobAcknowledge.pending.name}
          scope={`${jobAcknowledge.pending.repo} · ${jobAcknowledge.pending.pipeline}`}
          onClose={jobAcknowledge.cancel}
          onConfirm={(choice) => void jobAcknowledge.confirm(choice)}
        />
      )}
      {kind === "jobs" && view.job != null && (
        <JobDrawer
          jobId={view.job}
          history={jobHistoryLoad.data}
          onClose={closeDrawer}
          onOpenTest={openTestFromJob}
          onOpenJob={(jobId) => setView({ job: jobId })}
          onOpenCiRun={openCiRun}
          onAcknowledge={(j) => void jobAcknowledge.toggle(j)}
        />
      )}

      <footer className="mt-8 text-xs text-muted">
        Ingest from CI:{" "}
        <a
          href="https://github.com/andrewthetechie/flakeradar/blob/main/README.md#ci-integration"
          target="_blank"
          rel="noreferrer"
          className="text-link underline-offset-2 hover:underline"
        >
          see the README — CI integration
        </a>
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

/** Header entry point to the CI run view: the run id the developer reads off
 *  the CI page, for the repo the ScopePicker already selected. */
function CiRunForm({ repo, onOpen }: { repo: string; onOpen: (ciRun: string) => void }) {
  const [ciRun, setCiRun] = useState("");
  const trimmed = ciRun.trim();
  return (
    <form
      className="flex items-center gap-1"
      title={`Open a CI run of ${repo}`}
      onSubmit={(e) => {
        e.preventDefault();
        if (trimmed) onOpen(trimmed);
      }}
    >
      <label className="text-xs whitespace-nowrap text-muted" htmlFor="open-ci-run">
        Open CI run
      </label>
      <input
        id="open-ci-run"
        className="control w-24 py-0.5 text-xs"
        value={ciRun}
        placeholder="run id"
        onChange={(e) => setCiRun(e.target.value)}
      />
      <button type="submit" className="btn px-2 py-0.5 text-xs" disabled={!trimmed}>
        Open
      </button>
    </form>
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
