import type { Summary } from "../api";

export interface Tile {
  label: string;
  value: number;
  alert?: boolean;
}

// Tailwind only sees literal class names, so the column count is a lookup.
const LG_COLS: Record<number, string> = {
  5: "lg:grid-cols-5",
  6: "lg:grid-cols-6",
};

/** One strip of statistics split by hairlines (gap-px over the line color), not separate cards. */
export function StatStrip({ tiles }: { tiles: Tile[] }) {
  return (
    <div
      className={`mb-6 grid grid-cols-2 gap-px overflow-hidden rounded-xl border border-line bg-line sm:grid-cols-3 ${LG_COLS[tiles.length] ?? "lg:grid-cols-6"}`}
    >
      {tiles.map((t) => (
        <div
          className="flex flex-col justify-between bg-surface px-4 py-3"
          data-testid="stat-tile"
          key={t.label}
        >
          <div className="text-xs text-text-2">{t.label}</div>
          <div
            className={`mt-1 text-[22px] font-semibold tabular-nums ${t.alert ? "text-signal" : ""}`}
          >
            {t.value.toLocaleString()}
          </div>
        </div>
      ))}
    </div>
  );
}

export function StatTiles({ summary }: { summary: Summary }) {
  return (
    <StatStrip
      tiles={[
        { label: "Tracked tests", value: summary.total_tests },
        {
          label: `Flaky (score ≥ ${summary.flake_threshold})`,
          value: summary.flaky_tests,
          alert: summary.flaky_tests > 0,
        },
        { label: "Suspect (score > 0)", value: summary.suspect_tests },
        {
          label: "Proven flaky",
          value: summary.confirmed_flaky_tests,
          alert: summary.confirmed_flaky_tests > 0,
        },
        { label: "CI runs ingested", value: summary.total_runs },
        { label: "Executions recorded", value: summary.total_executions },
      ]}
    />
  );
}
