// The dashboard's view lives in the URL query string so any view can be
// bookmarked or pasted into an issue / agent prompt.
//   ?repo=owner/name&project=backend&test=42&view=jobs&job=3&page=2&sort=proven&stable=1
//   ?repo=owner/name&view=ci-run&ci_run=12345678&attempt=2
//   ?repo=owner/name&quarantined=1&issue=linked
//   ?repo=owner/name&view=jobs&pipeline=.github/workflows/nightly.yml&issue=none
//   ?repo=owner/name&new=7
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
  /** Leaderboard: list only what became flaky in the last N days, or null for any. */
  newDays: number | null;
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
  newDays: null,
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

/** One URL-backed leaderboard list filter, declared once. `parseViewState` and
 *  `serializeViewState` derive from this table (and `applyPatch`'s page-reset
 *  rule), so adding the next filter is one entry here plus its control. */
export interface ListFilterDescriptor<F extends keyof ViewState = keyof ViewState> {
  /** Query-string parameter name ("stable" backs showStable, "new" backs newDays). */
  param: string;
  /** The ViewState field the param backs; a typo here fails `npm run typecheck`. */
  field: F;
  /** Read the raw param (or null) into the field's value, defaulting exactly as today. */
  parse(raw: string | null): ViewState[F];
  /** Append the param to the query string exactly when today's code would. */
  serialize(params: URLSearchParams, value: ViewState[F]): void;
  /** Changing this filter relists the leaderboard and resets the page.
   *  `applyPatch` resets the page when a filter with this flag changes. */
  relists: boolean;
}

/** Identity helper so each entry's parse/serialize is checked against its own
 *  field's value type rather than the union over every ViewState field. */
const listFilter = <F extends keyof ViewState>(
  d: ListFilterDescriptor<F>,
): ListFilterDescriptor<F> => d;

/** Emission order (pinned by tests): repo, project, q | sort, stable, cause,
 *  quarantined, issue, pipeline, new. The hand-written params (view, ci_run,
 *  attempt, test, job, page) are emitted between q and sort. */
export const LIST_FILTERS: ListFilterDescriptor[] = [
  listFilter({
    param: "repo",
    field: "repo",
    relists: true,
    parse: (raw) => raw || null,
    serialize: (p, v) => {
      if (v) p.set("repo", v);
    },
  }),
  listFilter({
    // Serialize emits this only under a repo; see serializeViewState.
    param: "project",
    field: "project",
    relists: true,
    parse: (raw) => raw || null,
    serialize: (p, v) => {
      if (v) p.set("project", v);
    },
  }),
  listFilter({
    param: "q",
    field: "q",
    relists: true,
    parse: (raw) => raw ?? "",
    serialize: (p, v) => {
      if (v) p.set("q", v);
    },
  }),
  listFilter({
    param: "sort",
    field: "sort",
    relists: true,
    parse: (raw) => (raw != null && SORTS.includes(raw as SortKey) ? (raw as SortKey) : "score"),
    serialize: (p, v) => {
      if (v !== "score") p.set("sort", v);
    },
  }),
  listFilter({
    param: "stable",
    field: "showStable",
    relists: true,
    parse: (raw) => raw === "1",
    serialize: (p, v) => {
      if (v) p.set("stable", "1");
    },
  }),
  listFilter({
    param: "cause",
    field: "cause",
    relists: true,
    parse: (raw) =>
      raw != null && FAILURE_CATEGORIES.includes(raw as FailureCategory)
        ? (raw as FailureCategory)
        : null,
    serialize: (p, v) => {
      if (v) p.set("cause", v);
    },
  }),
  // Unscoped by view, like cause: these params survive a view switch.
  listFilter({
    param: "quarantined",
    field: "quarantined",
    relists: true,
    parse: (raw) => raw === "1",
    serialize: (p, v) => {
      if (v) p.set("quarantined", "1");
    },
  }),
  listFilter({
    param: "issue",
    field: "issue",
    relists: true,
    parse: (raw) =>
      raw != null && ISSUE_FILTERS.includes(raw as IssueFilter) ? (raw as IssueFilter) : null,
    serialize: (p, v) => {
      if (v) p.set("issue", v);
    },
  }),
  listFilter({
    param: "pipeline",
    field: "pipeline",
    relists: true,
    parse: (raw) => raw || null,
    serialize: (p, v) => {
      if (v) p.set("pipeline", v);
    },
  }),
  listFilter({
    param: "new",
    field: "newDays",
    relists: true,
    parse: (raw) => positiveInt(raw),
    serialize: (p, v) => {
      if (v != null) p.set("new", String(v));
    },
  }),
];

export function parseViewState(search: string): ViewState {
  const p = new URLSearchParams(search);
  const out: ViewState = { ...DEFAULT_VIEW };
  // One cast for the whole table: TypeScript cannot narrow a union-keyed index
  // write (`out[d.field]`) to the matching value type, so the write goes through
  // the widened record view of ViewState. `field` is still checked against
  // keyof ViewState, so a typo fails the typecheck.
  const widen: Record<keyof ViewState, unknown> = out;
  for (const d of LIST_FILTERS) widen[d.field] = d.parse(p.get(d.param));
  out.test = positiveInt(p.get("test"));
  out.job = positiveInt(p.get("job"));
  out.page = positiveInt(p.get("page")) ?? 1;
  out.view = p.get("view") === "jobs" ? "jobs" : p.get("view") === "ci-run" ? "ci-run" : "tests";
  out.ci_run = p.get("ci_run") || null;
  out.attempt = positiveInt(p.get("attempt"));
  // Cross-field rule (kept explicit per the issue): a project without a repo is dropped.
  if (!out.repo) out.project = null;
  return out;
}

/** "?repo=…" (defaults omitted), or "" for the default view. */
export function serializeViewState(s: ViewState): string {
  const p = new URLSearchParams();
  // Pinned emission order: repo, project, q, then the hand-written params, then
  // sort..new. The 3 is the table position where the hand-written block sits.
  const HAND_WRITTEN_AT = 3;
  for (const d of LIST_FILTERS.slice(0, HAND_WRITTEN_AT)) {
    // Cross-field rule (kept explicit per the issue): a project serializes only
    // under its repo.
    if (d.field === "project" && !s.repo) continue;
    d.serialize(p, s[d.field]);
  }
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
  for (const d of LIST_FILTERS.slice(HAND_WRITTEN_AT)) d.serialize(p, s[d.field]);
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

  // A changed list filter relists the leaderboard, so the page resets to 1 —
  // unless the patch brings its own page. Derived from LIST_FILTERS: a filter
  // added to the table relists without any further edit here.
  const relisted = LIST_FILTERS.some(
    (d) => d.relists && d.field in patch && patch[d.field] !== prev[d.field],
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
