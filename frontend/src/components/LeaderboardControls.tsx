import type { SortKey } from "../api";

const SORT_LABELS: Record<SortKey, string> = {
  score: "Flakiness score",
  last_seen: "Last seen",
  proven: "Proven flakes",
};

export function LeaderboardControls({
  sort, showStable, onSort, onShowStable,
}: {
  sort: SortKey;
  showStable: boolean;
  onSort: (sort: SortKey) => void;
  onShowStable: (show: boolean) => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-4 text-xs text-text-2">
      <label className="flex items-center gap-2">
        Sort by
        <select className="control py-0.5 text-xs" value={sort} onChange={(e) => onSort(e.target.value as SortKey)}>
          {(Object.keys(SORT_LABELS) as SortKey[]).map((k) => (
            <option key={k} value={k}>{SORT_LABELS[k]}</option>
          ))}
        </select>
      </label>
      <label className="flex cursor-pointer items-center gap-2">
        <input
          type="checkbox"
          className="size-3.5 accent-(--signal)"
          checked={showStable}
          onChange={(e) => onShowStable(e.target.checked)}
        />
        Show stable tests
      </label>
    </div>
  );
}
