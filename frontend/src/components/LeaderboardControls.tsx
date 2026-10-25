import { useEffect, useState } from "react";
import { FAILURE_CATEGORIES, type FailureCategory, type IssueFilter, type SortKey } from "../api";

const SORT_LABELS: Record<SortKey, string> = {
  score: "Flakiness score",
  last_seen: "Last seen",
  proven: "Proven flakes",
};

const ISSUE_LABELS: Record<IssueFilter, string> = {
  linked: "Linked",
  none: "None",
};

/** Keystrokes wait this long before the search hits the API. */
const SEARCH_DEBOUNCE_MS = 300;

/** Window of the "New in the last N days" filter, matching the API's new_flaky_7d. */
const NEW_FLAKY_DAYS = 7;

export function LeaderboardControls({
  sort,
  showStable,
  stableLabel = "Show stable tests",
  cause,
  causeCounts,
  quarantined,
  issue,
  newDays,
  pipelines,
  pipeline,
  search,
  searchPlaceholder,
  onSort,
  onShowStable,
  onCause,
  onQuarantined,
  onIssue,
  onNewDays,
  onPipeline,
  onSearch,
}: {
  sort: SortKey;
  showStable: boolean;
  stableLabel?: string;
  cause?: FailureCategory | null;
  causeCounts?: Record<FailureCategory, number> | null;
  quarantined?: boolean;
  issue?: IssueFilter | null;
  /** Both views: days window of the "new flaky" filter, or null when it is off. */
  newDays?: number | null;
  /** Jobs view: Pipeline names of the current scope, from the job summary. */
  pipelines?: string[] | null;
  pipeline?: string | null;
  search: string;
  searchPlaceholder?: string;
  onSort: (sort: SortKey) => void;
  onShowStable: (show: boolean) => void;
  onCause?: (cause: FailureCategory | null) => void;
  onQuarantined?: (quarantined: boolean) => void;
  onIssue?: (issue: IssueFilter | null) => void;
  /** Both views: turn the "new flaky" filter on with the day window, or off with null. */
  onNewDays?: (newDays: number | null) => void;
  onPipeline?: (pipeline: string | null) => void;
  onSearch: (q: string) => void;
}) {
  // Local text so typing stays instant; the URL (and a fetch) only sees the
  // term once the keystrokes have been quiet for SEARCH_DEBOUNCE_MS.
  const [text, setText] = useState(search);
  // Keep in sync with the URL for back / forward navigation.
  useEffect(() => setText(search), [search]);
  useEffect(() => {
    if (text === search) return;
    const timer = setTimeout(() => onSearch(text), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [text, search, onSearch]);

  return (
    <div className="flex flex-wrap items-center gap-4 text-xs text-text-2">
      <label className="flex items-center gap-2">
        Search
        <input
          type="search"
          className="control py-0.5 text-xs"
          value={text}
          maxLength={200}
          placeholder={searchPlaceholder}
          onChange={(e) => setText(e.target.value)}
        />
      </label>
      <label className="flex items-center gap-2">
        Sort by
        <select
          className="control py-0.5 text-xs"
          value={sort}
          onChange={(e) => onSort(e.target.value as SortKey)}
        >
          {(Object.keys(SORT_LABELS) as SortKey[]).map((k) => (
            <option key={k} value={k}>
              {SORT_LABELS[k]}
            </option>
          ))}
        </select>
      </label>
      {onCause && (
        <label className="flex items-center gap-2">
          Likely cause
          <select
            className="control py-0.5 text-xs"
            value={cause ?? ""}
            onChange={(e) => onCause((e.target.value || null) as FailureCategory | null)}
          >
            <option value="">Any</option>
            {FAILURE_CATEGORIES.map((c) => (
              <option key={c} value={c}>
                {causeCounts ? `${c} (${causeCounts[c]})` : c}
              </option>
            ))}
          </select>
        </label>
      )}
      {onPipeline && (
        <label className="flex items-center gap-2">
          Pipeline
          <select
            className="control py-0.5 text-xs"
            value={pipeline ?? ""}
            onChange={(e) => onPipeline(e.target.value || null)}
          >
            <option value="">All</option>
            {(pipelines ?? []).map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
        </label>
      )}
      {onIssue && (
        <label className="flex items-center gap-2">
          Issue
          <select
            className="control py-0.5 text-xs"
            value={issue ?? ""}
            onChange={(e) => onIssue((e.target.value || null) as IssueFilter | null)}
          >
            <option value="">Any</option>
            {(Object.keys(ISSUE_LABELS) as IssueFilter[]).map((i) => (
              <option key={i} value={i}>
                {ISSUE_LABELS[i]}
              </option>
            ))}
          </select>
        </label>
      )}
      <label className="flex cursor-pointer items-center gap-2">
        <input
          type="checkbox"
          className="size-3.5 accent-(--signal)"
          checked={showStable}
          onChange={(e) => onShowStable(e.target.checked)}
        />
        {stableLabel}
      </label>
      {onQuarantined && (
        <label className="flex cursor-pointer items-center gap-2">
          <input
            type="checkbox"
            className="size-3.5 accent-(--signal)"
            checked={quarantined ?? false}
            onChange={(e) => onQuarantined(e.target.checked)}
          />
          Quarantined only
        </label>
      )}
      {onNewDays && (
        <label className="flex cursor-pointer items-center gap-2">
          <input
            type="checkbox"
            className="size-3.5 accent-(--signal)"
            checked={newDays != null}
            onChange={(e) => onNewDays(e.target.checked ? NEW_FLAKY_DAYS : null)}
          />
          New in the last {NEW_FLAKY_DAYS} days
        </label>
      )}
    </div>
  );
}
