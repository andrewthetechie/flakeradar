import { FAILURE_CATEGORIES, type FailureCategory, type SortKey } from "../api";

const SORT_LABELS: Record<SortKey, string> = {
  score: "Flakiness score",
  last_seen: "Last seen",
  proven: "Proven flakes",
};

export function LeaderboardControls({
  sort,
  showStable,
  stableLabel = "Show stable tests",
  cause,
  causeCounts,
  onSort,
  onShowStable,
  onCause,
}: {
  sort: SortKey;
  showStable: boolean;
  stableLabel?: string;
  cause?: FailureCategory | null;
  causeCounts?: Record<FailureCategory, number> | null;
  onSort: (sort: SortKey) => void;
  onShowStable: (show: boolean) => void;
  onCause?: (cause: FailureCategory | null) => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-4 text-xs text-text-2">
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
      <label className="flex cursor-pointer items-center gap-2">
        <input
          type="checkbox"
          className="size-3.5 accent-(--signal)"
          checked={showStable}
          onChange={(e) => onShowStable(e.target.checked)}
        />
        {stableLabel}
      </label>
    </div>
  );
}
