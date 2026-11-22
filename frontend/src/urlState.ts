// The dashboard's view lives in the URL query string so any view can be
// bookmarked or pasted into an issue / agent prompt.
//   ?repo=owner/name&project=backend&test=42&view=jobs&job=3&page=2&sort=proven&stable=1
//   ?repo=owner/name&view=ci-run&ci_run=12345678&attempt=2
//   ?repo=owner/name&quarantined=1&issue=linked
//   ?repo=owner/name&view=jobs&pipeline=.github/workflows/nightly.yml&issue=none
//   ?repo=owner/name&new=7
import { useCallback, useEffect, useRef, useState } from "react";
// Type-only: `api.ts` imports LIST_FILTERS from here at runtime, so this module
// must not import a runtime value back from it (a cycle whose table init would
// read an uninitialized binding). See FAILURE_CATEGORIES below.
import type { IssueFilter, JobQuery, SortKey, TestQuery } from "./api";

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
  /** Tests view: list only Tests broken on the Default branch (stable ones). */
  broken: boolean;
  /** Both leaderboard views: list only what a person acknowledged (still live). */
  acknowledged: boolean;
  /** Both leaderboard views: filter by Issue link, or null for any. */
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
  broken: false,
  acknowledged: false,
  issue: null,
  pipeline: null,
  newDays: null,
  view: "tests",
  ci_run: null,
  attempt: null,
  q: "",
};

const SORTS: readonly SortKey[] = ["score", "last_seen", "proven", "impact"];
const ISSUE_FILTERS: readonly IssueFilter[] = ["linked", "none"];

/** The backend's failure taxonomy (mirrors backend/app/schemas.py). Lives here
 *  rather than in `api.ts` because the `cause` descriptor reads it at table
 *  init, and `api.ts` imports that table at runtime. `api.ts` re-exports both
 *  names, so importers keep spelling them as before. */
export type FailureCategory = "network" | "environment" | "timing" | "assertion" | "other";
export const FAILURE_CATEGORIES: readonly FailureCategory[] = [
  "network",
  "environment",
  "timing",
  "assertion",
  "other",
];

function positiveInt(raw: string | null): number | null {
  if (raw == null || !/^\d+$/.test(raw)) return null;
  const n = Number(raw);
  return n >= 1 ? n : null;
}

/** One endpoint's spelling of a list filter: the wire parameter name, the value
 *  to send for it (or `null` to leave the param out), and the position it takes
 *  in the query string. Spelled exactly as the hand-written `params.set` calls
 *  `api.ts` had before this table, bytes included. */
export interface WireParam<Q> {
  /** Fetch-argument name this filter enters the endpoint's query object under,
   *  when it differs from the ViewState field name (`cause` is read as
   *  `category`, `newDays` as `newWithinDays`). Absent means the field name. */
  key?: keyof Q;
  /** Wire parameter name, which differs from the URL param for some filters
   *  (`stable` goes out as `include_stable`, `cause` as `category`). */
  param: string;
  /** Position among the endpoint's filter params; the derived loop sorts on it.
   *  The two endpoints take a filter at different positions (tests sends `q`
   *  last, jobs third), so the order is per endpoint, not per filter. */
  order: number;
  /** Reads the endpoint's query object, so the mapping has no state of its own. */
  value: (q: Q) => string | null;
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
  /** Leaderboard views that offer this filter, i.e. render its control and send
   *  its wire param. The ci-run view shows no leaderboard controls, so it never
   *  appears here. The split is fixed by the backend contract (`reads.rs`:
   *  TestsQuery vs JobsQuery). URL serialization stays view-independent:
   *  `serializeViewState` emits every entry regardless of the current view. */
  views: readonly ViewKind[];
  /** Per-endpoint wire mapping. Absent for repo and project, whose params
   *  `scopeParams` (`api.ts`) owns. */
  wire?: {
    tests?: WireParam<TestQuery>;
    jobs?: WireParam<JobQuery>;
  };
}

/** Identity helper so each entry's parse/serialize is checked against its own
 *  field's value type rather than the union over every ViewState field. */
const listFilter = <F extends keyof ViewState>(
  d: ListFilterDescriptor<F>,
): ListFilterDescriptor<F> => d;

/** The parse/serialize idioms several entries share, each written once and
 *  closed over its param name by the entries that spread it. Spreading keeps
 *  the per-field typecheck: an idiom whose value shape does not match the
 *  entry's field fails to compile, exactly as a hand-written body would. */

/** `?param=name`: an optional string — null when the param is absent or empty,
 *  serialized only when set. */
const optionalString = (param: string) => ({
  parse: (raw: string | null): string | null => raw || null,
  serialize: (p: URLSearchParams, v: string | null): void => {
    if (v) p.set(param, v);
  },
});

/** `?param=1`: a boolean flag — true only when the param is exactly "1",
 *  serialized only when true. */
const flag = (param: string) => ({
  parse: (raw: string | null): boolean => raw === "1",
  serialize: (p: URLSearchParams, v: boolean): void => {
    if (v) p.set(param, "1");
  },
});

/** `raw` itself when it is one of `values`, else null. */
const oneOf = <V extends string>(values: readonly V[], raw: string | null): V | null =>
  raw != null && values.includes(raw as V) ? (raw as V) : null;

/** `?param=one-of-values`: an enum filter — null unless the param is one of the
 *  allowed values, serialized only when set. (`sort` shares only the `oneOf`
 *  check: it defaults to "score" rather than null, so it keeps its own rules.) */
const enumFilter = <V extends string>(param: string, values: readonly V[]) => ({
  parse: (raw: string | null): V | null => oneOf(values, raw),
  serialize: (p: URLSearchParams, v: V | null): void => {
    if (v) p.set(param, v);
  },
});

/** Emission order (pinned by tests): repo, project, q | sort, stable, cause,
 *  quarantined, broken, acknowledged, issue, pipeline, new. The hand-written
 *  params (view, ci_run, attempt, test, job, page) are emitted between q and sort:
 *  serializeViewState splits the table at the sort descriptor. */
export const LIST_FILTERS: ListFilterDescriptor[] = [
  // repo and project carry no `wire`: scopeParams (api.ts) writes those params.
  listFilter({
    param: "repo",
    field: "repo",
    relists: true,
    views: ["tests", "jobs"],
    ...optionalString("repo"),
  }),
  listFilter({
    // Serialize emits this only under a repo; see serializeViewState.
    param: "project",
    field: "project",
    relists: true,
    views: ["tests"],
    ...optionalString("project"),
  }),
  listFilter({
    param: "q",
    field: "q",
    relists: true,
    views: ["tests", "jobs"],
    parse: (raw) => raw ?? "",
    serialize: (p, v) => {
      if (v) p.set("q", v);
    },
    wire: {
      // tests sends q last, jobs third — each matches the hand-written api.ts.
      tests: { param: "q", order: 7, value: (q) => q.q || null },
      jobs: { param: "q", order: 2, value: (q) => q.q || null },
    },
  }),
  listFilter({
    param: "sort",
    field: "sort",
    relists: true,
    views: ["tests", "jobs"],
    parse: (raw) => oneOf(SORTS, raw) ?? "score",
    serialize: (p, v) => {
      if (v !== "score") p.set("sort", v);
    },
    // Both endpoints always take a sort, first among the filters, as the
    // hand-written `params.set("sort", …)` did.
    wire: {
      tests: { param: "sort", order: 0, value: (q) => q.sort },
      jobs: { param: "sort", order: 0, value: (q) => q.sort },
    },
  }),
  listFilter({
    param: "stable",
    field: "showStable",
    relists: true,
    views: ["tests", "jobs"],
    ...flag("stable"),
    wire: {
      tests: {
        param: "include_stable",
        order: 1,
        value: (q) => (q.showStable ? "true" : null),
      },
      jobs: {
        param: "include_stable",
        order: 1,
        value: (q) => (q.showStable ? "true" : null),
      },
    },
  }),
  listFilter({
    param: "cause",
    field: "cause",
    relists: true,
    views: ["tests"],
    ...enumFilter("cause", FAILURE_CATEGORIES),
    wire: {
      tests: { key: "category", param: "category", order: 2, value: (q) => q.category },
    },
  }),
  // Tests-only on the wire, like cause; the URL params still survive a view switch.
  listFilter({
    param: "quarantined",
    field: "quarantined",
    relists: true,
    views: ["tests"],
    ...flag("quarantined"),
    wire: {
      tests: {
        param: "quarantined",
        order: 3,
        value: (q) => (q.quarantined ? "true" : null),
      },
    },
  }),
  listFilter({
    param: "broken",
    field: "broken",
    relists: true,
    views: ["tests"],
    ...flag("broken"),
    wire: {
      tests: { param: "broken", order: 4, value: (q) => (q.broken ? "true" : null) },
    },
  }),
  listFilter({
    param: "acknowledged",
    field: "acknowledged",
    relists: true,
    views: ["tests", "jobs"],
    ...flag("acknowledged"),
    wire: {
      tests: {
        param: "acknowledged",
        order: 8,
        value: (q) => (q.acknowledged ? "true" : null),
      },
      jobs: {
        param: "acknowledged",
        order: 6,
        value: (q) => (q.acknowledged ? "true" : null),
      },
    },
  }),
  listFilter({
    param: "issue",
    field: "issue",
    relists: true,
    views: ["tests", "jobs"],
    ...enumFilter("issue", ISSUE_FILTERS),
    wire: {
      tests: {
        param: "has_issue",
        order: 5,
        value: (q) => (q.issue == null ? null : String(q.issue === "linked")),
      },
      jobs: {
        param: "has_issue",
        order: 4,
        value: (q) => (q.issue == null ? null : String(q.issue === "linked")),
      },
    },
  }),
  listFilter({
    param: "pipeline",
    field: "pipeline",
    relists: true,
    views: ["jobs"],
    ...optionalString("pipeline"),
    wire: { jobs: { param: "pipeline", order: 3, value: (q) => q.pipeline } },
  }),
  listFilter({
    param: "new",
    field: "newDays",
    relists: true,
    views: ["tests", "jobs"],
    parse: (raw) => positiveInt(raw),
    serialize: (p, v) => {
      if (v != null) p.set("new", String(v));
    },
    wire: {
      tests: {
        key: "newWithinDays",
        param: "new_within_days",
        order: 6,
        value: (q) => (q.newWithinDays == null ? null : String(q.newWithinDays)),
      },
      jobs: {
        key: "newWithinDays",
        param: "new_within_days",
        order: 5,
        value: (q) => (q.newWithinDays == null ? null : String(q.newWithinDays)),
      },
    },
  }),
];

/** The ViewState fields the table owns: everything except the six fields
 *  `parseViewState` writes by hand after its loop, which are not filters. */
export type ListFilterField = Exclude<
  keyof ViewState,
  "test" | "job" | "page" | "view" | "ci_run" | "attempt"
>;

/** The filter slice a leaderboard load consumes: the table's fields, plus
 *  `view`, which picks the endpoint. Derived from the table at the type level,
 *  so a filter added to `LIST_FILTERS` lands here with no further edit. */
export type ListFilters = { view: ViewKind } & Pick<ViewState, ListFilterField>;

/** Single-entry memo for `listFilterSlice`. One entry suffices: every call in a
 *  render projects the same current view. */
let sliceCache: { source: ViewState; slice: ListFilters } | null = null;

/** Project the slice `LIST_FILTERS` owns plus `view` out of a ViewState — the
 *  one place that names the filter fields for a load, so the loader's inputs
 *  are the table rather than a hand-written list.
 *
 *  Referentially stable by design: the dashboard keys its loaders on the slice,
 *  and the fields it excludes (the drawer ids, page, CI-run fields) change the
 *  ViewState on every `setView` without changing what is listed. A call whose
 *  filter values equal the cached ones therefore answers the same slice, so
 *  opening a drawer cannot refetch the list.
 *
 *  The memo compares against the current table, so a slice cached before an
 *  entry was pushed onto `LIST_FILTERS` can lack that entry; only the live app
 *  and these tests call this function. */
export function listFilterSlice(view: ViewState): ListFilters {
  const prev = sliceCache;
  if (
    prev &&
    prev.source.view === view.view &&
    LIST_FILTERS.every((d) => prev.source[d.field] === view[d.field])
  ) {
    return prev.slice;
  }
  // The widened-record idiom of parseViewState: TypeScript cannot narrow a
  // union-keyed index write to the matching value type.
  const slice: Record<string, unknown> = { view: view.view };
  for (const d of LIST_FILTERS) slice[d.field] = view[d.field];
  sliceCache = { source: view, slice: slice as unknown as ListFilters };
  return sliceCache.slice;
}

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
  // sort..new. The split is anchored to the sort descriptor rather than a bare
  // index, so inserting or reordering entries before sort cannot silently move
  // the hand-written block.
  const HAND_WRITTEN_AT = LIST_FILTERS.findIndex((d) => d.param === "sort");
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
 *  Switching the view resets the page and closes any drawer the patch does not
 *  set itself; `test` and `job` are mutually exclusive (setting one clears the
 *  other). */
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
    // Only clear a drawer the patch did not set: a view switch may carry the
    // drawer id it should open (e.g. openJobFromTest sets view+job together).
    if (!("test" in patch)) next.test = null;
    if (!("job" in patch)) next.job = null;
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
