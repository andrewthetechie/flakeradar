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
  repo: null,
  project: null,
  test: null,
  page: 1,
  sort: "score",
  showStable: false,
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
