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
    <div className="leaderboard-controls">
      <label>
        Sort by{" "}
        <select value={sort} onChange={(e) => onSort(e.target.value as SortKey)}>
          {(Object.keys(SORT_LABELS) as SortKey[]).map((k) => (
            <option key={k} value={k}>{SORT_LABELS[k]}</option>
          ))}
        </select>
      </label>
      <label>
        <input
          type="checkbox"
          checked={showStable}
          onChange={(e) => onShowStable(e.target.checked)}
        />{" "}
        Show stable tests
      </label>
    </div>
  );
}
