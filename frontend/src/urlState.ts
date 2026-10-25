// The dashboard's view lives in the URL query string so any view can be
// bookmarked or pasted into an issue / agent prompt.
//   ?repo=owner/name&project=backend&test=42&view=jobs&job=3&page=2&sort=proven&stable=1
//   ?repo=owner/name&view=ci-run&ci_run=12345678&attempt=2
//   ?repo=owner/name&quarantined=1&issue=linked
//   ?repo=owner/name&view=jobs&pipeline=.github/workflows/nightly.yml&issue=none
import { useCallback, useEffect, useRef, useState } from "react";
import { FAILURE_CATEGORIES, type FailureCategory, type IssueFilter, type SortKey } from "./api";

export type ViewKind = "tests" | "jobs" | "ci-run";

export interface ViewState {
  repo: string | null;
  project: string | null;
  test: number | null;
  job: number | null;
  page: number;
  sort: SortKey;
  showStable: boolean;
  cause: FailureCategory | null;
  /** Tests view: list only quarantined Tests (including stable ones). */
  quarantined: boolean;
  /** Tests view: filter by Issue link, or null for any. */
  issue: IssueFilter | null;
  /** Jobs view: exact Pipeline name to list, or null for every pipeline. */
  pipeline: string | null;
  view: ViewKind;
  /** CI run id of the ci-run view; only meaningful with `view: "ci-run"`. */
  ci_run: string | null;
  /** CI run attempt, or null for the latest one the backend knows about. */
  attempt: number | null;
  /** Leaderboard search term; "" means no search (keeps the input controlled). */
  q: string;
}

export const DEFAULT_VIEW: ViewState = {
  repo: null,
  project: null,
  test: null,
  job: null,
  page: 1,
  sort: "score",
  showStable: false,
  cause: null,
  quarantined: false,
  issue: null,
  pipeline: null,
  view: "tests",
  ci_run: null,
  attempt: null,
  q: "",
};

const SORTS: readonly SortKey[] = ["score", "last_seen", "proven"];
const ISSUE_FILTERS: readonly IssueFilter[] = ["linked", "none"];

function positiveInt(raw: string | null): number | null {
  if (raw == null || !/^\d+$/.test(raw)) return null;
  const n = Number(raw);
  return n >= 1 ? n : null;
}

export function parseViewState(search: string): ViewState {
  const p = new URLSearchParams(search);
  const repo = p.get("repo") || null;
  const sort = p.get("sort");
  const cause = p.get("cause");
  const issue = p.get("issue");
  return {
    repo,
    project: repo ? p.get("project") || null : null,
    test: positiveInt(p.get("test")),
    job: positiveInt(p.get("job")),
    page: positiveInt(p.get("page")) ?? 1,
    sort: SORTS.includes(sort as SortKey) ? (sort as SortKey) : "score",
    showStable: p.get("stable") === "1",
    cause:
      cause && FAILURE_CATEGORIES.includes(cause as FailureCategory)
        ? (cause as FailureCategory)
        : null,
    quarantined: p.get("quarantined") === "1",
    issue: ISSUE_FILTERS.includes(issue as IssueFilter) ? (issue as IssueFilter) : null,
    pipeline: p.get("pipeline") || null,
    view: p.get("view") === "jobs" ? "jobs" : p.get("view") === "ci-run" ? "ci-run" : "tests",
    ci_run: p.get("ci_run") || null,
    attempt: positiveInt(p.get("attempt")),
    q: p.get("q") ?? "",
  };
}

/** "?repo=…" (defaults omitted), or "" for the default view. */
export function serializeViewState(s: ViewState): string {
  const p = new URLSearchParams();
  if (s.repo) {
    p.set("repo", s.repo);
    if (s.project) p.set("project", s.project);
  }
  if (s.q) p.set("q", s.q);
  if (s.view === "jobs") {
    p.set("view", "jobs");
  } else if (s.view === "ci-run") {
    // The CI run parameters belong to this view only.
    p.set("view", "ci-run");
    if (s.ci_run) p.set("ci_run", s.ci_run);
    if (s.attempt != null) p.set("attempt", String(s.attempt));
  }
  if (s.test != null) p.set("test", String(s.test));
  if (s.job != null) p.set("job", String(s.job));
  if (s.page !== 1) p.set("page", String(s.page));
  if (s.sort !== "score") p.set("sort", s.sort);
  if (s.showStable) p.set("stable", "1");
  if (s.cause) p.set("cause", s.cause);
  // Unscoped by view, like cause: the params survive a view switch.
  if (s.quarantined) p.set("quarantined", "1");
  if (s.issue) p.set("issue", s.issue);
  if (s.pipeline) p.set("pipeline", s.pipeline);
  const q = p.toString();
  return q ? `?${q}` : "";
}

/** Merge a change into the view. Changing what is listed resets to page 1;
 *  changing the repo also clears the project (names overlap across repos).
 *  Switching the view resets the page and closes any drawer; `test` and `job`
 *  are mutually exclusive (setting one clears the other). */
export function applyPatch(prev: ViewState, patch: Partial<ViewState>): ViewState {
  const next = { ...prev, ...patch };
  if ("repo" in patch && patch.repo !== prev.repo && !("project" in patch)) {
    next.project = null;
  }
  // Pipeline names are repo-scoped too: a stale one would filter the new repo to nothing.
  if ("repo" in patch && patch.repo !== prev.repo && !("pipeline" in patch)) {
    next.pipeline = null;
  }
  if (!next.repo) next.project = null;

  if ("view" in patch && patch.view !== prev.view) {
    if (!("page" in patch)) next.page = 1;
    next.test = null;
    next.job = null;
    return next;
  }

  if (patch.test != null) next.job = null;
  if (patch.job != null) next.test = null;

  const relisted = (
    [
      "repo",
      "project",
      "sort",
      "showStable",
      "cause",
      "quarantined",
      "issue",
      "pipeline",
      "q",
    ] as const
  ).some((k) => k in patch && patch[k] !== prev[k]);
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
